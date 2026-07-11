//! Headless render spike: prove the real composite pipeline without a window.
//! Renders the user shader over a synthetic video frame into an offscreen target
//! and reads back the center pixel. Covers: wgpu device w/ BC feature, GLSL
//! compile -> pipeline, RGBA upload, BC1 upload + Metal block decode, and a
//! live-reload-style shader swap. Run: `cargo run --bin spike_render`.

use vidiotic::commands::{ChainSlot, SlotRef};
use vidiotic::render::{Globals, Renderer};
use vidiotic::shader::ShaderLang;
use vidiotic::video::frame::{DecodedFrame, PixelData};
use vidiotic::video::hap::HapTextureFormat;

const W: u32 = 64;
const H: u32 = 64;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn init_gpu() -> Gpu {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("no adapter");
    assert!(
        adapter
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
        "adapter lacks BC texture compression"
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("spike-device"),
        required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .expect("no device");
    Gpu { device, queue }
}

/// Render the current renderer state into an offscreen RGBA8 target and read the
/// center pixel back to the CPU.
fn render_center_pixel(gpu: &Gpu, renderer: &mut Renderer) -> [u8; 4] {
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let bytes_per_row = (W * 4).div_ceil(256) * 256;
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (bytes_per_row * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    renderer.render(&gpu.device, &mut encoder, &view, W, H);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    rx.recv().unwrap().expect("map");
    let data = slice.get_mapped_range();

    let cx = W / 2;
    let cy = H / 2;
    let off = (cy * bytes_per_row + cx * 4) as usize;
    let px = [data[off], data[off + 1], data[off + 2], data[off + 3]];
    drop(data);
    readback.unmap();
    px
}

fn close(a: u8, b: u8, tol: u8) -> bool {
    a.abs_diff(b) <= tol
}

fn main() {
    env_logger::init();
    let gpu = init_gpu();
    let mut renderer = Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
    let g = Globals::default();
    renderer.update_globals(&gpu.queue, &g);

    let mut ok = true;

    // 1. RGBA solid red frame + built-in passthrough -> center pixel red.
    let red: Vec<u8> = (0..(W * H))
        .flat_map(|_| [220u8, 20, 20, 255])
        .collect();
    let frame = DecodedFrame {
        pixels: PixelData::Rgba {
            data: red,
            stride: W * 4,
        },
        w: W,
        h: H,
        pts_sec: 0.0,
    };
    renderer.upload_frame(&gpu.device, &gpu.queue, &frame);
    let p = render_center_pixel(&gpu, &mut renderer);
    let pass1 = close(p[0], 220, 6) && close(p[1], 20, 6) && close(p[2], 20, 6);
    println!("[{}] RGBA passthrough: center={p:?} (want ~[220,20,20,255])", if pass1 { " OK " } else { "FAIL" });
    ok &= pass1;

    // 2. Live-reload style swap: install a shader that samples video but tints green.
    let tint = "void main() { vec4 v = video(fragTexCoord); FragColor = vec4(0.0, v.r, 0.0, 1.0); }";
    renderer.set_shader(&gpu.device, tint, ShaderLang::Glsl);
    assert!(renderer.shader_error().is_none(), "tint shader should compile");
    let p = render_center_pixel(&gpu, &mut renderer);
    // green channel should carry the red input (~220), red/blue ~0
    let pass2 = close(p[0], 0, 6) && p[1] > 180 && close(p[2], 0, 6);
    println!("[{}] shader swap (tint): center={p:?} (want green~=input red)", if pass2 { " OK " } else { "FAIL" });
    ok &= pass2;

    // 3. Broken shader -> keep last-good (green tint), error recorded.
    renderer.set_shader(&gpu.device, "void main() { FragColor = nope(); }", ShaderLang::Glsl);
    let has_err = renderer.shader_error().is_some();
    let p = render_center_pixel(&gpu, &mut renderer);
    let pass3 = has_err && p[1] > 180;
    println!("[{}] broken shader kept last-good: err={} center={p:?}", if pass3 { " OK " } else { "FAIL" }, has_err);
    ok &= pass3;

    // 4. BC1 solid-red frame -> Metal decodes the block. Reset to passthrough first.
    renderer.set_shader(&gpu.device, "void main(){ FragColor = video(fragTexCoord); }", ShaderLang::Glsl);
    // BC1 block for solid red: color0 = RGB565 red (0xF800), color1 = 0, all indices 0.
    let mut bc1 = Vec::new();
    let blocks = (W / 4) * (H / 4);
    for _ in 0..blocks {
        bc1.extend_from_slice(&0xF800u16.to_le_bytes()); // color0 = red
        bc1.extend_from_slice(&0x0000u16.to_le_bytes()); // color1 = black
        bc1.extend_from_slice(&0u32.to_le_bytes()); // indices all -> color0
    }
    let frame = DecodedFrame {
        pixels: PixelData::Bc {
            format: HapTextureFormat::Bc1,
            data: bc1,
            alpha: None,
            video_mode: 0,
        },
        w: W,
        h: H,
        pts_sec: 0.0,
    };
    renderer.upload_frame(&gpu.device, &gpu.queue, &frame);
    let p = render_center_pixel(&gpu, &mut renderer);
    // BC1 565 red decodes to ~ (255, 0, 0)
    let pass4 = p[0] > 230 && close(p[1], 0, 8) && close(p[2], 0, 8);
    println!("[{}] BC1 upload + GPU decode: center={p:?} (want ~[255,0,0])", if pass4 { " OK " } else { "FAIL" });
    ok &= pass4;

    // 5. Effect chain: a seed pass primes prev() with the decoded source, then
    // one Live stage reads prev() and tints green. Re-upload the RGBA red frame
    // (test 4 left a BC1 frame), set a prev()-based live shader, run [Live].
    let red: Vec<u8> = (0..(W * H)).flat_map(|_| [220u8, 20, 20, 255]).collect();
    let frame = DecodedFrame {
        pixels: PixelData::Rgba { data: red, stride: W * 4 },
        w: W,
        h: H,
        pts_sec: 0.0,
    };
    renderer.upload_frame(&gpu.device, &gpu.queue, &frame);
    let prev_tint = "void main() { FragColor = vec4(0.0, prev(fragTexCoord).r, 0.0, 1.0); }";
    renderer.set_shader(&gpu.device, prev_tint, ShaderLang::Glsl);
    assert!(renderer.shader_error().is_none(), "prev() tint should compile");
    renderer.set_active_chain(vec![ChainSlot::new(SlotRef::Live)]);
    let p = render_center_pixel(&gpu, &mut renderer);
    // seed prev()==source (red 220) -> green channel ~220, red/blue ~0
    let pass5 = close(p[0], 0, 6) && p[1] > 180 && close(p[2], 0, 6);
    println!("[{}] chain seed+prev(): center={p:?} (want green~=source red)", if pass5 { " OK " } else { "FAIL" });
    ok &= pass5;
    renderer.set_active_chain(Vec::new());

    println!("\n{}", if ok { "SPIKE PASS" } else { "SPIKE FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}
