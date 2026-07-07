//! wgpu setup: one shared Device/Queue driving two window surfaces — the
//! fullscreen output (video+shader) and the control window (egui).

use std::sync::Arc;

use winit::window::Window;

/// One window and its configured swapchain surface.
pub struct WindowSurface {
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

impl WindowSurface {
    fn configure(
        device: &wgpu::Device,
        adapter: &wgpu::Adapter,
        window: Arc<Window>,
        surface: wgpu::Surface<'static>,
        present_mode: wgpu::PresentMode,
    ) -> Self {
        // Gamma-space pipeline everywhere: prefer a non-sRGB surface format.
        let caps = surface.get_capabilities(adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        surface.configure(device, &config);
        Self {
            window,
            surface,
            config,
        }
    }

    /// Reconfigure the surface for a new window size (zero sizes are ignored).
    pub fn resize(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        if w > 0 && h > 0 {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(device, &self.config);
        }
    }

    /// Get the next drawable, reconfiguring on Outdated/Lost. `None` means
    /// skip this frame (no drawable, or the surface was just rebuilt).
    pub fn acquire(&self, device: &wgpu::Device) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                Some(t)
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(device, &self.config);
                None
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => None,
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("surface validation error");
                None
            }
        }
    }
}

/// The shared GPU context: one Device/Queue driving both window surfaces.
pub struct Graphics {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub output: WindowSurface,
    pub control: WindowSurface,
}

impl Graphics {
    /// Pick an adapter (must support BC textures, required for HAP), create the
    /// device, and configure both surfaces — output on Fifo (vsync paces the
    /// render loop), control on `AutoVsync`.
    ///
    /// # Errors
    /// Returns an error if surface creation fails, no BC-capable adapter is
    /// available, or the device request fails.
    pub fn new(output_win: Arc<Window>, control_win: Arc<Window>) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::default();
        let out_surface = instance.create_surface(output_win.clone())?;
        let ctl_surface = instance.create_surface(control_win.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&out_surface),
            force_fallback_adapter: false,
        }))?;
        anyhow::ensure!(
            adapter
                .features()
                .contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
            "GPU lacks BC texture compression (required for HAP clips)"
        );
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("vidiotic-device"),
                required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            }))?;

        let output = WindowSurface::configure(
            &device,
            &adapter,
            output_win,
            out_surface,
            wgpu::PresentMode::Fifo,
        );
        let control = WindowSurface::configure(
            &device,
            &adapter,
            control_win,
            ctl_surface,
            wgpu::PresentMode::AutoVsync,
        );
        Ok(Self {
            device,
            queue,
            output,
            control,
        })
    }
}
