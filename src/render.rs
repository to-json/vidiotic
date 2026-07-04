//! Output render pipeline: a single fullscreen pass where the user fragment
//! shader is the compositor. It reads the video texture through the preamble's
//! `video()` helper and audio/beat state through the `Globals` uniform block.
//!
//! Compile failures are caught by `shader.rs` (naga parse+validate) *before* a
//! pipeline is built, so a bad live-reload keeps the last-good pipeline.

use std::borrow::Cow;

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
    // Gamma-space pipeline: all non-sRGB (see plan reconciliation #1).
    match f {
        HapTextureFormat::Bc1 => wgpu::TextureFormat::Bc1RgbaUnorm,
        HapTextureFormat::Bc3 | HapTextureFormat::Bc3YCoCg => wgpu::TextureFormat::Bc3RgbaUnorm,
        HapTextureFormat::Bc4 => wgpu::TextureFormat::Bc4RUnorm,
        HapTextureFormat::Bc7 => wgpu::TextureFormat::Bc7RgbaUnorm,
    }
}

struct VideoTexture {
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    has_alpha: bool,
    _main: wgpu::Texture,
    _main_view: wgpu::TextureView,
    _alpha: Option<wgpu::Texture>,
    _alpha_view: Option<wgpu::TextureView>, // Some for a real alpha plane (HapM)
    bind_group: wgpu::BindGroup,
}

pub struct Renderer {
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    bgl_video: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    vs_glsl: wgpu::ShaderModule, // paired with GLSL fragment shaders
    vs_wgsl: wgpu::ShaderModule, // paired with WGSL fragment shaders
    sampler: wgpu::Sampler,
    dummy_alpha_view: wgpu::TextureView,
    color_format: wgpu::TextureFormat,

    video: Option<VideoTexture>,
    pipeline: Option<wgpu::RenderPipeline>,
    passthrough: wgpu::RenderPipeline,
    shader_error: Option<String>,
}

impl Renderer {
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
            color_format,
            video: None,
            pipeline: None,
            passthrough,
            shader_error: None,
        }
    }

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
            }
            Err(e) => {
                self.shader_error = Some(e.to_string());
                log::warn!("shader compile failed (keeping last-good): {e}");
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
                upload_bc(queue, &v._main, *format, v.w, v.h, data);
                if let (Some(a), Some(atex)) = (alpha, &v._alpha) {
                    upload_bc(queue, atex, HapTextureFormat::Bc4, v.w, v.h, a);
                }
            }
            PixelData::Rgba { data, stride } => {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &v._main,
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
            ],
        });

        VideoTexture {
            format,
            w: pw,
            h: ph,
            has_alpha,
            _main: main,
            _main_view: main_view,
            _alpha: alpha_tex,
            _alpha_view: alpha_view,
            bind_group,
        }
    }

    pub fn update_globals(&self, queue: &wgpu::Queue, g: &Globals) {
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(g));
    }

    /// Draw the composite into `view`. Uses the active user pipeline, or the
    /// built-in passthrough if none has been set yet.
    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        let pipeline = self.pipeline.as_ref().unwrap_or(&self.passthrough);
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

    pub fn has_video(&self) -> bool {
        self.video.is_some()
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
