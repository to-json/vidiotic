//! Output render pipeline: a single fullscreen pass where the user fragment
//! shader is the compositor. It reads the video texture through the preamble's
//! `video()` helper and audio/beat state through the `Globals` uniform block.
//!
//! Compile failures are caught by `shader.rs` (naga parse+validate) *before* a
//! pipeline is built, so a bad live-reload keeps the last-good pipeline.

use std::borrow::Cow;

use crate::analysis::{AUDIO_TEX_LEN, AUDIO_TEX_W};
use crate::commands::ShaderId;
use crate::shader::{self, ShaderError, ShaderLang};
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
        Globals {
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
/// only for a real alpha plane (HapM); the bind group keeps the views alive.
struct VideoTexture {
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    has_alpha: bool,
    main: wgpu::Texture,
    alpha: Option<wgpu::Texture>,
    bind_group: wgpu::BindGroup,
}

/// A shader pinned into the pool: a frozen compile a cue can render with.
struct PooledShader {
    id: ShaderId,
    name: String,
    pipeline: wgpu::RenderPipeline,
}

/// Owns the composite pass: the compiled user shader (plus pinned pool shaders
/// and the built-in passthrough), the video texture(s), and the uniform state.
pub struct Renderer {
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    bgl_video: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    vs_glsl: wgpu::ShaderModule, // paired with GLSL fragment shaders
    vs_wgsl: wgpu::ShaderModule, // paired with WGSL fragment shaders
    sampler: wgpu::Sampler,
    dummy_alpha_view: wgpu::TextureView,
    // Shadertoy audio texture (512x2 R8): row 0 FFT, row 1 waveform. Persistent,
    // rewritten every frame, and bound into every video bind group.
    audio_tex: wgpu::Texture,
    audio_view: wgpu::TextureView,
    audio_sampler: wgpu::Sampler,
    color_format: wgpu::TextureFormat,

    video: Option<VideoTexture>,
    pipeline: Option<wgpu::RenderPipeline>,
    passthrough: wgpu::RenderPipeline,
    shader_error: Option<String>,
    // Source of the current last-good live compile, so it can be re-compiled into
    // a frozen pool entry on `capture_current`.
    last_good: Option<(String, ShaderLang)>,
    // Pinned shaders and the id of the one currently overriding the live shader
    // (set by the app from the playing cue). Falls back to the live shader when
    // the id no longer resolves.
    pool: Vec<PooledShader>,
    next_pool_id: ShaderId,
    active_override: Option<ShaderId>,
}

impl Renderer {
    /// Build the fixed GPU state (layouts, samplers, audio texture, built-in
    /// passthrough pipeline) targeting `color_format`.
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("output-layout"),
            bind_group_layouts: &[Some(&bgl_globals), Some(&bgl_video)],
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

        Renderer {
            globals_buf,
            globals_bg,
            bgl_video,
            pipeline_layout,
            vs_glsl,
            vs_wgsl,
            sampler,
            dummy_alpha_view,
            audio_tex,
            audio_view,
            audio_sampler,
            color_format,
            video: None,
            pipeline: None,
            passthrough,
            shader_error: None,
            last_good: None,
            pool: Vec::new(),
            next_pool_id: 1,
            active_override: None,
        }
    }

    /// The last live-shader compile error, if the most recent `set_shader` failed.
    pub fn shader_error(&self) -> Option<&str> {
        self.shader_error.as_deref()
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
                self.shader_error = Some(e.to_string());
                log::warn!("shader compile failed (keeping last-good): {e}");
            }
        }
    }

    /// Pin the current last-good live shader into the pool as a frozen compile.
    /// Returns the new id, or `None` if there is no compiled shader to pin.
    pub fn capture_current(&mut self, device: &wgpu::Device, name: String) -> Option<ShaderId> {
        let (src, lang) = self.last_good.clone()?;
        match self.compile(device, &src, lang) {
            Ok(pipeline) => {
                let id = self.next_pool_id;
                self.next_pool_id += 1;
                self.pool.push(PooledShader { id, name, pipeline });
                Some(id)
            }
            Err(e) => {
                // last_good compiled once, so this is unexpected; don't pin a broken entry.
                log::warn!("pin shader failed: {e}");
                None
            }
        }
    }

    /// Drop a pinned shader from the pool, clearing the override if it was active.
    pub fn remove_pool_shader(&mut self, id: ShaderId) {
        self.pool.retain(|p| p.id != id);
        if self.active_override == Some(id) {
            self.active_override = None;
        }
    }

    /// Select which pinned shader overrides the live one this frame (`None` = live).
    pub fn set_active_shader(&mut self, id: Option<ShaderId>) {
        self.active_override = id;
    }

    /// (id, name) of each pinned shader, in pin order.
    pub fn pool_view(&self) -> Vec<(ShaderId, String)> {
        self.pool.iter().map(|p| (p.id, p.name.clone())).collect()
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
        let v = self.video.as_ref().unwrap();

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

    /// Draw the composite into `view`. Uses the active user pipeline, or the
    /// built-in passthrough if none has been set yet.
    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        // A playing cue's pinned override wins, else the live shader, else passthrough.
        let pipeline = self
            .active_override
            .and_then(|id| self.pool.iter().find(|p| p.id == id))
            .map(|p| &p.pipeline)
            .or(self.pipeline.as_ref())
            .unwrap_or(&self.passthrough);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("output-pass"),
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.globals_bg, &[]);
        if let Some(v) = &self.video {
            pass.set_bind_group(1, &v.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        // No video yet → nothing to composite; the clear (black) stands.
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
