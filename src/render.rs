//! Output render pipeline: a single fullscreen pass where the user fragment
//! shader is the compositor. It reads the video texture through the preamble's
//! `video()` helper and audio/beat state through the `Globals` uniform block.
//!
//! Compile failures are caught by `shader.rs` (naga parse+validate) *before* a
//! pipeline is built, so a bad live-reload keeps the last-good pipeline.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::analysis::{AUDIO_TEX_LEN, AUDIO_TEX_W};
use crate::commands::{ChainSlot, ShaderId, ShaderPoolView, SlotRef};
use crate::isf::{self, IsfBuiltins, IsfInput, IsfPass, IsfTarget, IsfUbo, IsfValue};
use crate::shader::{self, ShaderError, ShaderLang};

/// Format of ISF intermediate pass targets. Always float so `FLOAT`/feedback
/// passes keep precision; the final (untargeted) pass writes `color_format`.
const ISF_MID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Minimum alignment for a non-dynamic uniform buffer binding offset (wgpu's
/// default `min_uniform_buffer_offset_alignment`), used to pack one parameter
/// block per ISF pass into a single buffer.
const ISF_UBO_ALIGN: u64 = 256;
use crate::video::frame::{DecodedFrame, PixelData};
use crate::video::hap::HapTextureFormat;

/// std140-exact mirror of the preamble's `Globals` block. See shaders/preamble.frag.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Globals {
    pub resolution: [f32; 2], //  0
    pub mouse: [f32; 2],      //  8
    pub time: f32,            // 16
    pub lvl: f32,             // 20
    pub beat: f32,            // 24
    pub bar_phase: f32,       // 28
    pub phrase_phase: f32,    // 32
    pub bpm: f32,             // 36
    pub video_mode: i32,      // 40
    pub _pad0: f32,           // 44
    pub freqs: [[f32; 4]; 6], // 48..144
}

const _: () = assert!(std::mem::size_of::<Globals>() == 144);

impl Default for Globals {
    fn default() -> Self {
        Self {
            resolution: [1.0, 1.0],
            mouse: [0.0, 0.0],
            time: 0.0,
            lvl: 0.0,
            beat: 0.0,
            bar_phase: 0.0,
            phrase_phase: 0.0,
            bpm: 120.0,
            video_mode: 0,
            _pad0: 0.0,
            freqs: [[0.0; 4]; 6],
        }
    }
}

impl Globals {
    /// Pack 21 FFT bands into the 6-vec4 array.
    pub fn set_bands(&mut self, bands: &[f32; 21]) {
        for (i, &b) in bands.iter().enumerate() {
            self.freqs[i / 4][i % 4] = b;
        }
    }
}

fn wgpu_format(f: HapTextureFormat) -> wgpu::TextureFormat {
    // All non-sRGB: the whole pipeline works in gamma space, matching the
    // non-sRGB surface format gfx.rs picks and what Shadertoy-style user
    // shaders expect.
    match f {
        HapTextureFormat::Bc1 => wgpu::TextureFormat::Bc1RgbaUnorm,
        HapTextureFormat::Bc3 | HapTextureFormat::Bc3YCoCg => wgpu::TextureFormat::Bc3RgbaUnorm,
        HapTextureFormat::Bc4 => wgpu::TextureFormat::Bc4RUnorm,
        HapTextureFormat::Bc7 => wgpu::TextureFormat::Bc7RgbaUnorm,
    }
}

/// The current clip's GPU texture(s) and their bind group. `alpha` is `Some`
/// only for a real alpha plane (`HapM`); the bind group keeps the views alive.
struct VideoTexture {
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    has_alpha: bool,
    main: wgpu::Texture,
    alpha: Option<wgpu::Texture>,
    bind_group: wgpu::BindGroup,
}

/// A shader in the pool: a frozen compile a cue's chain can render with.
/// `builtin` entries are the bundled effects (addressable by stable name and
/// persistable); non-builtin entries are livecoded pins (runtime-only).
struct PooledShader {
    id: ShaderId,
    name: Arc<str>,
    builtin: bool,
    pipeline: wgpu::RenderPipeline,
    /// Present for ISF pool entries: the input schema + the `set=3` parameter
    /// buffer/bind group the pipeline expects.
    isf: Option<IsfEntry>,
}

/// The extra GPU state an ISF pool entry carries beyond a plain pipeline: the
/// input schema, the pass/target metadata, the `set=3` layout, the per-pass
/// parameter buffer, and the intermediate-format pipeline variant for targeted
/// passes. The untargeted final pass uses the entry's `PooledShader::pipeline`
/// (which writes `color_format`).
struct IsfEntry {
    inputs: Vec<IsfInput>,
    ubo: IsfUbo,
    passes: Vec<IsfPass>,
    targets: Vec<IsfTarget>,
    /// 256-aligned stride of one parameter block in `params_buf` (one per pass).
    aligned: u64,
    params_buf: wgpu::Buffer,
    bgl: wgpu::BindGroupLayout,
    /// Pipeline writing [`ISF_MID_FORMAT`]; `None` when the shader has no targets.
    pipeline_mid: Option<wgpu::RenderPipeline>,
}

/// Per-instance render targets for an ISF effect's named passes, double-buffered
/// for feedback and kept across frames. Sized to the effect's stage output;
/// reallocated when that size changes.
struct IsfTargets {
    base: (u32, u32),
    /// Which buffer index the current frame writes (toggled each frame). The
    /// previous frame's content is in `1 - parity`.
    parity: usize,
    /// One entry per `IsfEntry::targets`, in the same order.
    bufs: Vec<TargetTex>,
}

/// A double-buffered ISF target texture (`[write/current, previous]` selected by
/// parity). The textures are kept alive alongside their views.
struct TargetTex {
    _tex: [wgpu::Texture; 2],
    view: [wgpu::TextureView; 2],
}

/// A resolved chain stage for the draw phase: a plain single-pass pipeline, or an
/// ISF effect (by pool id) rendered via [`Renderer::draw_isf`].
enum Stage<'a> {
    Plain(&'a wgpu::RenderPipeline),
    Isf(ShaderId),
}

/// The bundled effect shaders loaded into the pool at startup, addressable by
/// the stable name written into `.viproj`. Each is a fragment shader that reads
/// its input via `prev()`; every entry is covered by the
/// `bundled_frag_shaders_compile` test.
const BUILTIN_EFFECTS: &[(&str, &str)] = &[
    ("kaleido", include_str!("../shaders/kaleido.frag")),
    ("glitch-vhs", include_str!("../shaders/glitch-vhs.frag")),
    ("chroma-punch", include_str!("../shaders/chroma-punch.frag")),
    ("spectrum-warp", include_str!("../shaders/spectrum-warp.frag")),
];

/// Two persistent offscreen color buffers ping-ponged between chain stages,
/// sized to the current output. Kept across frames (never transient) so a future
/// feedback effect can sample last frame's result.
struct PingPong {
    w: u32,
    h: u32,
    views: [wgpu::TextureView; 2],
}

/// Owns the composite pass: the compiled user shader (plus pinned pool shaders
/// and the built-in passthrough), the video texture(s), and the uniform state.
pub struct Renderer {
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    bgl_video: wgpu::BindGroupLayout,
    bgl_input: wgpu::BindGroupLayout,
    bgl_globals: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    vs_glsl: wgpu::ShaderModule, // paired with GLSL fragment shaders
    vs_wgsl: wgpu::ShaderModule, // paired with WGSL fragment shaders
    sampler: wgpu::Sampler,
    dummy_alpha_view: wgpu::TextureView,
    // 1x1 black RGBA bound as the set=2 input when no real previous-stage output
    // exists (the empty-chain fast path and the seed pass, which read `video()`).
    dummy_input_view: wgpu::TextureView,
    // Ping-pong intermediates, allocated on first multi-stage render and when the
    // output size changes.
    ping: Option<PingPong>,
    // Shadertoy audio texture (512x2 R8): row 0 FFT, row 1 waveform. Persistent,
    // rewritten every frame, and bound into every video bind group.
    audio_tex: wgpu::Texture,
    audio_view: wgpu::TextureView,
    audio_sampler: wgpu::Sampler,
    color_format: wgpu::TextureFormat,

    video: Option<VideoTexture>,
    pipeline: Option<wgpu::RenderPipeline>,
    passthrough: wgpu::RenderPipeline,
    shader_error: Option<Arc<str>>,
    // Source of the current last-good live compile, so it can be re-compiled into
    // a frozen pool entry on `capture_current`.
    last_good: Option<(String, ShaderLang)>,
    // Built-in effects (loaded at startup) plus livecoded pins. Chain slots
    // resolve against this by name (built-ins) or id (pins).
    pool: Vec<PooledShader>,
    next_pool_id: ShaderId,
    // The playing cue's effect chain (set by the app). Empty = the live shader.
    active_chain: Vec<ChainSlot>,
    // Monotonic frame counter, exposed to ISF shaders as FRAMEINDEX.
    frame_index: u32,
    // Per-ISF-entry named-pass render targets (keyed by pool id), kept across
    // frames for feedback.
    isf_targets: HashMap<ShaderId, IsfTargets>,
}

impl Renderer {
    /// Build the fixed GPU state (layouts, samplers, audio texture, built-in
    /// passthrough pipeline) targeting `color_format`.
    ///
    /// # Panics
    /// Panics if a built-in shader (the GLSL fullscreen vertex shader or the
    /// passthrough fragment shader) fails to compile — a compile-time invariant.
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 144,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bgl_globals = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(144),
                },
                count: None,
            }],
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals-bg"),
            layout: &bgl_globals,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl_video = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video-bgl"),
            entries: &[
                tex_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(2),
                tex_entry(3), // audioTex
                wgpu::BindGroupLayoutEntry {
                    binding: 4, // audioSmp
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // set=2: per-pass inputs. binding 0/1 are the chain-input texture+sampler
        // (the previous stage's output). Higher bindings are reserved for future
        // per-stage params and a feedback texture.
        let bgl_input = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("input-bgl"),
            entries: &[
                tex_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("output-layout"),
            bind_group_layouts: &[Some(&bgl_globals), Some(&bgl_video), Some(&bgl_input)],
            immediate_size: 0,
        });

        let vs_wgsl = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fullscreen-vs-wgsl"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../shaders/fullscreen.wgsl"
            ))),
        });
        let vs_glsl = {
            let module = shader::compile_glsl_vertex_module(include_str!("../shaders/fullscreen.vert"))
                .expect("built-in GLSL vertex shader must compile");
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("fullscreen-vs-glsl"),
                source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
            })
        };

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // 1x1 white R8 dummy for alphaTex when the clip has no separate alpha plane.
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("alpha-dummy"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let dummy_alpha_view = dummy.create_view(&wgpu::TextureViewDescriptor::default());

        // 1x1 black RGBA for the set=2 input when there is no previous stage.
        let dummy_input = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("input-dummy"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let dummy_input_view = dummy_input.create_view(&wgpu::TextureViewDescriptor::default());

        // Persistent 512x2 R8 audio texture, rewritten each frame from the
        // analysis thread's packed spectrum + waveform rows.
        let audio_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("audio-tex"),
            size: wgpu::Extent3d {
                width: AUDIO_TEX_W as u32,
                height: 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let audio_view = audio_tex.create_view(&wgpu::TextureViewDescriptor::default());
        // Clamp on x so the top FFT bin doesn't wrap under linear filtering;
        // Shadertoy audio channels are clamp+linear.
        let audio_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("audio-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // Built-in passthrough so the app always renders (startup / compile failure).
        let passthrough = {
            let module = shader::compile_glsl_to_module(
                "void main() { FragColor = video(fragTexCoord); }",
            )
            .expect("built-in passthrough must compile");
            let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("passthrough-fs"),
                source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
            });
            build_pipeline(device, &pipeline_layout, &vs_glsl, &fs, color_format)
        };

        // Load the bundled effects into the pool, addressable by stable name.
        // A build-time invariant (the `bundled_frag_shaders_compile` test) keeps
        // these compiling; if one somehow fails here we log and skip it rather
        // than take the app down.
        let mut pool = Vec::new();
        let mut next_pool_id: ShaderId = 1;
        for (name, src) in BUILTIN_EFFECTS {
            match shader::compile_glsl_to_module(src) {
                Ok(module) => {
                    let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("builtin-fs"),
                        source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
                    });
                    let pipeline =
                        build_pipeline(device, &pipeline_layout, &vs_glsl, &fs, color_format);
                    pool.push(PooledShader {
                        id: next_pool_id,
                        name: (*name).into(),
                        builtin: true,
                        pipeline,
                        isf: None,
                    });
                    next_pool_id += 1;
                }
                Err(e) => log::error!("built-in effect {name} failed to compile: {e}"),
            }
        }

        Self {
            globals_buf,
            globals_bg,
            bgl_video,
            bgl_input,
            bgl_globals,
            pipeline_layout,
            vs_glsl,
            vs_wgsl,
            sampler,
            dummy_alpha_view,
            dummy_input_view,
            ping: None,
            audio_tex,
            audio_view,
            audio_sampler,
            color_format,
            video: None,
            pipeline: None,
            passthrough,
            shader_error: None,
            last_good: None,
            pool,
            next_pool_id,
            active_chain: Vec::new(),
            frame_index: 0,
            isf_targets: HashMap::new(),
        }
    }

    /// The last live-shader compile error, if the most recent `set_shader` failed.
    pub fn shader_error(&self) -> Option<&Arc<str>> {
        self.shader_error.as_ref()
    }

    /// Compile a user shader source. On success installs it as the active
    /// pipeline and clears the error; on failure keeps the last-good pipeline
    /// and records the error string for the UI.
    pub fn set_shader(&mut self, device: &wgpu::Device, src: &str, lang: ShaderLang) {
        match self.compile(device, src, lang) {
            Ok(p) => {
                self.pipeline = Some(p);
                self.shader_error = None;
                self.last_good = Some((src.to_string(), lang));
            }
            Err(e) => {
                self.shader_error = Some(e.to_string().into());
                log::warn!("shader compile failed (keeping last-good): {e}");
            }
        }
    }

    /// Pin the current last-good live shader into the pool as a frozen compile.
    /// Returns the new id, or `None` if there is no compiled shader to pin.
    pub fn capture_current(
        &mut self,
        device: &wgpu::Device,
        name: impl Into<Arc<str>>,
    ) -> Option<ShaderId> {
        let (src, lang) = self.last_good.clone()?;
        match self.compile(device, &src, lang) {
            Ok(pipeline) => {
                let id = self.next_pool_id;
                self.next_pool_id += 1;
                self.pool.push(PooledShader {
                    id,
                    name: name.into(),
                    builtin: false,
                    pipeline,
                    isf: None,
                });
                Some(id)
            }
            Err(e) => {
                // last_good compiled once, so this is unexpected; don't pin a broken entry.
                log::warn!("pin shader failed: {e}");
                None
            }
        }
    }

    /// Drop a pinned shader from the pool, removing any chain slots that
    /// reference it (built-ins are never removed this way).
    pub fn remove_pool_shader(&mut self, id: ShaderId) {
        self.pool.retain(|p| p.id != id || p.builtin);
        self.active_chain
            .retain(|slot| slot.shader != SlotRef::Pinned(id));
    }

    /// Set the effect chain rendered this frame (empty = the live shader).
    pub fn set_active_chain(&mut self, chain: Vec<ChainSlot>) {
        self.active_chain = chain;
    }

    /// Each pool shader as a `ShaderPoolView`, built-ins first (load order). ISF
    /// entries carry their input schema for the parameter editor.
    pub fn pool_view(&self) -> Vec<ShaderPoolView> {
        self.pool
            .iter()
            .map(|p| ShaderPoolView {
                id: p.id,
                name: p.name.clone(),
                builtin: p.builtin,
                inputs: p.isf.as_ref().map(|e| e.inputs.clone()).unwrap_or_default(),
            })
            .collect()
    }

    /// Compile an ISF shader source into the pool under `name` (its path key),
    /// returning the new pool id — or an existing entry's id if already loaded.
    ///
    /// # Errors
    /// Returns [`ShaderError`] if the source is not ISF or fails to compile.
    pub fn load_isf(
        &mut self,
        device: &wgpu::Device,
        name: Arc<str>,
        src: &str,
    ) -> Result<ShaderId, ShaderError> {
        if let Some(p) = self.pool.iter().find(|p| p.isf.is_some() && p.name == name) {
            return Ok(p.id);
        }
        let prog = isf::transpile(src).ok_or_else(|| ShaderError::Parse {
            msg: "not an ISF shader (no ISF header found)".into(),
            line: None,
        })?;
        let module = shader::compile_isf_program(&prog)?;
        let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("isf-fs"),
            source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
        });

        // set=3 layout: the parameter UBO at binding 0; when the shader has named
        // targets, a shared sampler at binding 1 and one target texture at 2.. .
        let ubo_size = prog.ubo.size() as u64;
        let mut bgl_entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: std::num::NonZeroU64::new(ubo_size),
            },
            count: None,
        }];
        if !prog.targets.is_empty() {
            bgl_entries.push(wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
            for i in 0..prog.targets.len() {
                bgl_entries.push(wgpu::BindGroupLayoutEntry {
                    binding: (i + 2) as u32,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                });
            }
        }
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("isf-set3-bgl"),
            entries: &bgl_entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("isf-layout"),
            bind_group_layouts: &[
                Some(&self.bgl_globals),
                Some(&self.bgl_video),
                Some(&self.bgl_input),
                Some(&bgl),
            ],
            immediate_size: 0,
        });
        // The final (untargeted) pass writes color_format; targeted passes write
        // the float intermediate format.
        let pipeline_out =
            build_pipeline(device, &layout, &self.vs_glsl, &fs, self.color_format);
        let pipeline_mid = (!prog.targets.is_empty())
            .then(|| build_pipeline(device, &layout, &self.vs_glsl, &fs, ISF_MID_FORMAT));

        let aligned = round_up_u64(ubo_size, ISF_UBO_ALIGN);
        let n_pass = prog.passes.len().max(1) as u64;
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("isf-params"),
            size: aligned * n_pass,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let id = self.next_pool_id;
        self.next_pool_id += 1;
        self.pool.push(PooledShader {
            id,
            name,
            builtin: false,
            pipeline: pipeline_out,
            isf: Some(IsfEntry {
                inputs: prog.inputs,
                ubo: prog.ubo,
                passes: prog.passes,
                targets: prog.targets,
                aligned,
                params_buf,
                bgl,
                pipeline_mid,
            }),
        });
        Ok(id)
    }

    /// Pack and upload every ISF slot's per-pass parameter blocks (one block per
    /// pass, differing in `PASSINDEX`/`RENDERSIZE`) from the slot's overrides,
    /// falling back to schema defaults. Call once per frame from [`Renderer::render`]
    /// after ISF targets are sized.
    ///
    /// Limitation: parameters live on the pool entry, so if the same ISF appears
    /// more than once in one chain, the last instance's values win for all.
    fn pack_isf_params(&self, queue: &wgpu::Queue, width: u32, height: u32) {
        for slot in &self.active_chain {
            let SlotRef::Isf(name) = &slot.shader else { continue };
            let Some(entry) = self
                .pool
                .iter()
                .find(|p| p.isf.is_some() && p.name.as_ref() == name.as_ref())
                .and_then(|p| p.isf.as_ref())
            else {
                continue;
            };
            // Non-image inputs in schema order — aligned 1:1 with the UBO offsets.
            let values: Vec<IsfValue> = entry
                .inputs
                .iter()
                .filter_map(|input| {
                    let default = input.kind.default_value()?; // None => image input, skip
                    Some(slot.param(&input.name).cloned().unwrap_or(default))
                })
                .collect();
            let n_pass = entry.passes.len().max(1);
            for i in 0..n_pass {
                // RENDERSIZE = this pass's target size (the stage size for the
                // final untargeted pass).
                let (rw, rh) = entry
                    .passes
                    .get(i)
                    .and_then(|p| p.target.as_ref())
                    .and_then(|tname| entry.targets.iter().find(|t| &t.name == tname))
                    .map(|t| {
                        (
                            isf::eval_size(&t.width, width, height, width),
                            isf::eval_size(&t.height, width, height, height),
                        )
                    })
                    .unwrap_or((width, height));
                let builtins = IsfBuiltins {
                    frame_index: self.frame_index as i32,
                    pass_index: i as i32,
                    render_size: [rw as f32, rh as f32],
                    ..Default::default()
                };
                let buf = entry.ubo.pack(&values, &builtins);
                queue.write_buffer(&entry.params_buf, i as u64 * entry.aligned, &buf);
            }
        }
    }

    fn compile(
        &self,
        device: &wgpu::Device,
        src: &str,
        lang: ShaderLang,
    ) -> Result<wgpu::RenderPipeline, ShaderError> {
        let (module, vs) = match lang {
            ShaderLang::Glsl => (shader::compile_glsl_to_module(src)?, &self.vs_glsl),
            ShaderLang::Wgsl => (shader::compile_wgsl_to_module(src)?, &self.vs_wgsl),
        };
        let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("user-fs"),
            source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
        });
        Ok(build_pipeline(
            device,
            &self.pipeline_layout,
            vs,
            &fs,
            self.color_format,
        ))
    }

    /// Upload a decoded frame, recreating the GPU texture(s) if format/size changed.
    ///
    /// # Panics
    /// Panics if the video texture is absent after the create-if-changed step
    /// above — an internal invariant that always holds.
    pub fn upload_frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &DecodedFrame) {
        let (format, has_alpha) = match &frame.pixels {
            PixelData::Bc { format, alpha, .. } => (wgpu_format(*format), alpha.is_some()),
            PixelData::Rgba { .. } => (wgpu::TextureFormat::Rgba8Unorm, false),
        };

        let needs_new = match &self.video {
            Some(v) => v.format != format || v.w != frame.w || v.h != frame.h || v.has_alpha != has_alpha,
            None => true,
        };
        if needs_new {
            self.video = Some(self.create_video_texture(device, format, frame.w, frame.h, has_alpha));
        }
        let v = self.video.as_ref().expect("video texture created above when missing or mismatched");

        match &frame.pixels {
            PixelData::Bc {
                format,
                data,
                alpha,
                ..
            } => {
                upload_bc(queue, &v.main, *format, v.w, v.h, data);
                if let (Some(a), Some(atex)) = (alpha, &v.alpha) {
                    upload_bc(queue, atex, HapTextureFormat::Bc4, v.w, v.h, a);
                }
            }
            PixelData::Rgba { data, stride } => {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &v.main,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(*stride),
                        rows_per_image: Some(v.h),
                    },
                    wgpu::Extent3d {
                        width: v.w,
                        height: v.h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }

    fn create_video_texture(
        &self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        w: u32,
        h: u32,
        has_alpha: bool,
    ) -> VideoTexture {
        // BC textures must be block(4x4)-aligned; RGBA needs no padding.
        let block_aligned = format != wgpu::TextureFormat::Rgba8Unorm;
        let (pw, ph) = if block_aligned {
            ((w + 3) & !3, (h + 3) & !3)
        } else {
            (w, h)
        };
        let size = wgpu::Extent3d {
            width: pw,
            height: ph,
            depth_or_array_layers: 1,
        };
        let main = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video-main"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let main_view = main.create_view(&wgpu::TextureViewDescriptor::default());

        let (alpha_tex, alpha_view) = if has_alpha {
            let a = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("video-alpha"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bc4RUnorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let v = a.create_view(&wgpu::TextureViewDescriptor::default());
            (Some(a), Some(v))
        } else {
            (None, None)
        };

        // alphaTex is always bound; use the real plane or the shared 1x1 dummy.
        let alpha_binding = alpha_view.as_ref().unwrap_or(&self.dummy_alpha_view);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video-bg"),
            layout: &self.bgl_video,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&main_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(alpha_binding),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.audio_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.audio_sampler),
                },
            ],
        });

        VideoTexture {
            format,
            w: pw,
            h: ph,
            has_alpha,
            main,
            alpha: alpha_tex,
            bind_group,
        }
    }

    /// Write the per-frame `Globals` uniform block.
    pub fn update_globals(&self, queue: &wgpu::Queue, g: &Globals) {
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(g));
    }

    /// Upload the packed 512x2 audio texture (row 0 FFT, row 1 waveform).
    pub fn upload_audio(&self, queue: &wgpu::Queue, bytes: &[u8; AUDIO_TEX_LEN]) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.audio_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(AUDIO_TEX_W as u32),
                rows_per_image: Some(2),
            },
            wgpu::Extent3d {
                width: AUDIO_TEX_W as u32,
                height: 2,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Draw the composite into `view` at `width`x`height`.
    ///
    /// With no effect chain this is a single pass — the live shader (or the
    /// built-in passthrough) straight to `view`, exactly as before. With a chain,
    /// a seed pass primes buffer 0 with the decoded source (so `prev()` in the
    /// first stage == the source), then each stage reads the previous stage's
    /// output via the set=2 input and ping-pongs, the last stage targeting `view`.
    ///
    /// # Panics
    /// Panics only on internal invariants that always hold: the video texture is
    /// present past the early-out, and the ping-pong buffers are allocated
    /// whenever the resolved chain is non-empty.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        // No video yet → nothing to composite; clear to black and return.
        if self.video.is_none() {
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("output-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            return;
        }

        self.frame_index = self.frame_index.wrapping_add(1);

        // --- mutable pre-pass: size ISF targets + toggle feedback parity ---
        let isf_ids: Vec<ShaderId> = self
            .active_chain
            .iter()
            .filter_map(|slot| match &slot.shader {
                SlotRef::Isf(name) => self
                    .pool
                    .iter()
                    .find(|p| p.isf.is_some() && p.name.as_ref() == name.as_ref())
                    .map(|p| p.id),
                _ => None,
            })
            .collect();
        for id in isf_ids {
            self.ensure_isf_targets(device, id, width, height);
        }
        if !self.active_chain.is_empty() {
            self.ensure_ping(device, width, height);
        }
        // Params depend on the target sizes just computed, so pack after.
        self.pack_isf_params(queue, width, height);

        // --- immutable draw phase ---
        let v = self.video.as_ref().expect("video present (checked above)");
        // Resolve the chain to stages, skipping slots that no longer exist.
        let stages: Vec<Stage> = self
            .active_chain
            .iter()
            .filter_map(|slot| self.resolve_stage(&slot.shader))
            .collect();

        // Empty (or fully-unresolved) chain: single-pass fast path — the live
        // shader (or passthrough) straight to `view`, input = dummy.
        if stages.is_empty() {
            let pipeline = self.pipeline.as_ref().unwrap_or(&self.passthrough);
            let input_bg = self.make_input_bg(device, &self.dummy_input_view);
            self.draw_pass(encoder, pipeline, &v.bind_group, &input_bg, view);
            return;
        }

        let ping = self.ping.as_ref().expect("ping allocated for non-empty chain");
        // Pre-build the set=2 input bind groups: the seed reads the dummy (it
        // uses `video()`), each effect reads the buffer the prior stage wrote.
        let seed_input = self.make_input_bg(device, &self.dummy_input_view);
        let input_bgs: Vec<wgpu::BindGroup> = (0..stages.len())
            .map(|i| self.make_input_bg(device, &ping.views[i % 2]))
            .collect();

        // Seed: decoded source → buffer 0.
        self.draw_pass(encoder, &self.passthrough, &v.bind_group, &seed_input, &ping.views[0]);
        for (i, stage) in stages.iter().enumerate() {
            let target = if i + 1 == stages.len() {
                view
            } else {
                &ping.views[(i + 1) % 2]
            };
            match stage {
                Stage::Plain(pipeline) => {
                    self.draw_pass(encoder, pipeline, &v.bind_group, &input_bgs[i], target);
                }
                Stage::Isf(id) => {
                    self.draw_isf(encoder, device, *id, &v.bind_group, &input_bgs[i], target);
                }
            }
        }
    }

    /// Resolve a chain slot to a draw-phase stage, or `None` if it no longer
    /// exists (removed pin, absent built-in, or unloaded ISF).
    fn resolve_stage(&self, slot: &SlotRef) -> Option<Stage<'_>> {
        match slot {
            SlotRef::Live => Some(Stage::Plain(
                self.pipeline.as_ref().unwrap_or(&self.passthrough),
            )),
            SlotRef::Builtin(name) => self
                .pool
                .iter()
                .find(|p| p.builtin && p.name.as_ref() == name.as_ref())
                .map(|p| Stage::Plain(&p.pipeline)),
            SlotRef::Pinned(id) => self
                .pool
                .iter()
                .find(|p| p.id == *id)
                .map(|p| Stage::Plain(&p.pipeline)),
            SlotRef::Isf(name) => self
                .pool
                .iter()
                .find(|p| p.name.as_ref() == name.as_ref() && p.isf.is_some())
                .map(|p| Stage::Isf(p.id)),
        }
    }

    /// One fullscreen pass: `pipeline` reading globals (set 0), video (set 1) and
    /// the given input (set 2), writing into `target`.
    fn draw_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        video_bg: &wgpu::BindGroup,
        input_bg: &wgpu::BindGroup,
        target: &wgpu::TextureView,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chain-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.globals_bg, &[]);
        pass.set_bind_group(1, video_bg, &[]);
        pass.set_bind_group(2, input_bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Render one ISF effect stage: run each of its passes in order, targeted
    /// passes into the effect's named buffers (feedback-aware) and the final
    /// untargeted pass into `stage_out`. `input_bg` (set 2) is `inputImage` for
    /// every pass.
    fn draw_isf(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        id: ShaderId,
        video_bg: &wgpu::BindGroup,
        input_bg: &wgpu::BindGroup,
        stage_out: &wgpu::TextureView,
    ) {
        let Some(p) = self.pool.iter().find(|p| p.id == id) else { return };
        let Some(entry) = p.isf.as_ref() else { return };
        let targets = self.isf_targets.get(&id);
        let parity = targets.map_or(0, |t| t.parity);
        let n_pass = entry.passes.len().max(1);

        for i in 0..n_pass {
            let named = entry.passes.get(i).and_then(|pass| pass.target.as_ref());
            // Write target + pipeline variant: named targets use the float mid
            // pipeline; the untargeted pass writes stage_out with the out pipeline.
            let (target_view, pipeline) = match named {
                Some(tname) => {
                    let idx = entry.targets.iter().position(|t| &t.name == tname);
                    match (idx, targets, entry.pipeline_mid.as_ref()) {
                        (Some(idx), Some(tg), Some(mid)) => (&tg.bufs[idx].view[parity], mid),
                        _ => continue,
                    }
                }
                None => (stage_out, &p.pipeline),
            };

            let set3 = self.build_isf_set3(device, entry, targets, i, parity);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("isf-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.globals_bg, &[]);
            pass.set_bind_group(1, video_bg, &[]);
            pass.set_bind_group(2, input_bg, &[]);
            pass.set_bind_group(3, &set3, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Build the `set=3` bind group for ISF pass `pass_i`: the parameter block at
    /// this pass's offset, plus each named target's read view. A target reads its
    /// previous-frame buffer until the pass that writes it (so a feedback pass
    /// reads last frame while writing this one), and the current buffer after.
    fn build_isf_set3(
        &self,
        device: &wgpu::Device,
        entry: &IsfEntry,
        targets: Option<&IsfTargets>,
        pass_i: usize,
        parity: usize,
    ) -> wgpu::BindGroup {
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &entry.params_buf,
                offset: pass_i as u64 * entry.aligned,
                size: std::num::NonZeroU64::new(entry.ubo.size() as u64),
            }),
        }];
        if !entry.targets.is_empty() {
            entries.push(wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            });
            for (j, t) in entry.targets.iter().enumerate() {
                let read_parity = if pass_i > t.writer_pass { parity } else { 1 - parity };
                let view = targets
                    .map(|tg| &tg.bufs[j].view[read_parity])
                    .unwrap_or(&self.dummy_input_view);
                entries.push(wgpu::BindGroupEntry {
                    binding: (j + 2) as u32,
                    resource: wgpu::BindingResource::TextureView(view),
                });
            }
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("isf-set3-bg"),
            layout: &entry.bgl,
            entries: &entries,
        })
    }

    /// (Re)allocate an ISF effect's named-pass targets when its stage size
    /// changes, and toggle its feedback parity for this frame.
    fn ensure_isf_targets(&mut self, device: &wgpu::Device, id: ShaderId, w: u32, h: u32) {
        let sizes: Vec<(u32, u32)> = {
            let Some(entry) = self.pool.iter().find(|p| p.id == id).and_then(|p| p.isf.as_ref())
            else {
                return;
            };
            if entry.targets.is_empty() {
                return;
            }
            entry
                .targets
                .iter()
                .map(|t| {
                    (
                        isf::eval_size(&t.width, w, h, w),
                        isf::eval_size(&t.height, w, h, h),
                    )
                })
                .collect()
        };
        let need_alloc = self.isf_targets.get(&id).is_none_or(|t| t.base != (w, h));
        if need_alloc {
            let bufs = sizes
                .iter()
                .map(|(tw, th)| make_target_tex(device, *tw, *th))
                .collect();
            self.isf_targets.insert(id, IsfTargets { base: (w, h), parity: 0, bufs });
        }
        if let Some(t) = self.isf_targets.get_mut(&id) {
            t.parity ^= 1;
        }
    }

    /// Build a set=2 input bind group pointing at `input`.
    fn make_input_bg(&self, device: &wgpu::Device, input: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("input-bg"),
            layout: &self.bgl_input,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(input),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// (Re)allocate the ping-pong intermediates when the output size changes.
    fn ensure_ping(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        if self.ping.as_ref().is_some_and(|p| p.w == w && p.h == h) {
            return;
        }
        let make = |label| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.color_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            tex.create_view(&wgpu::TextureViewDescriptor::default())
        };
        self.ping = Some(PingPong {
            w,
            h,
            views: [make("chain-buf-0"), make("chain-buf-1")],
        });
    }
}

fn round_up_u64(v: u64, align: u64) -> u64 {
    v.div_ceil(align) * align
}

/// A double-buffered [`ISF_MID_FORMAT`] render target for one ISF pass buffer.
fn make_target_tex(device: &wgpu::Device, w: u32, h: u32) -> TargetTex {
    let mk = |label| {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ISF_MID_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        (tex, view)
    };
    let (t0, v0) = mk("isf-target-0");
    let (t1, v1) = mk("isf-target-1");
    TargetTex {
        _tex: [t0, t1],
        view: [v0, v1],
    }
}

fn build_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    vs: &wgpu::ShaderModule,
    fs: &wgpu::ShaderModule,
    color_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("output-pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: vs,
            entry_point: None, // single entry point per module ("main" for GLSL, "vs_main" for WGSL)
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: fs,
            entry_point: None,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn upload_bc(
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    format: HapTextureFormat,
    pw: u32,
    ph: u32,
    data: &[u8],
) {
    let block_bytes = format.block_bytes();
    let blocks_x = pw / 4;
    let blocks_y = ph / 4;
    debug_assert_eq!(
        data.len() as u32,
        blocks_x * blocks_y * block_bytes,
        "BC payload size mismatch"
    );
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(blocks_x * block_bytes),
            rows_per_image: Some(blocks_y),
        },
        wgpu::Extent3d {
            width: pw,
            height: ph,
            depth_or_array_layers: 1,
        },
    );
}
