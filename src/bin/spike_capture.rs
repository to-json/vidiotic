//! Camera capture spike: enumerate devices, open one through ffmpeg's
//! avfoundation demuxer, decode timed frames for a few seconds, and measure
//! open/teardown latency. Exercises the Stream 1 exit criteria of
//! docs/camera-source-plan.md.
//!
//! Usage: `spike_capture [device-index] [seconds]` — defaults to device 0 for
//! 3 seconds. Run with no capture args after plugging/unplugging devices to
//! re-check enumeration.
//!
//! `spike_capture milestone [device-index]` runs the Stream 2 go/no-go: a
//! `CaptureService` feeding a live tap and a 1.5 s delayed tap, both rendered
//! through the real `Renderer` offscreen path, asserting the delayed tap's
//! frames trail the live edge by the requested delay.

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    macos::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("spike_capture is macOS-only");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::time::{Duration, Instant};

    use ffmpeg_next as ff;
    use vidiotic::video::capture;

    pub fn run() -> anyhow::Result<()> {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .init();
        let mut args = std::env::args().skip(1);
        let first = args.next();
        if first.as_deref() == Some("milestone") {
            return milestone(args.next().and_then(|a| a.parse().ok()));
        }
        let pick: Option<usize> = first.and_then(|a| a.parse().ok());
        let secs: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(3.0);

        // -- Permission --------------------------------------------------
        let auth = capture::authorization();
        println!("camera TCC status: {auth:?}");
        if auth == capture::Authorization::NotDetermined {
            println!("requesting camera access (system prompt should appear)...");
            let (tx, rx) = std::sync::mpsc::channel();
            capture::request_access(move |granted| {
                let _ = tx.send(granted);
            });
            match rx.recv_timeout(Duration::from_secs(60)) {
                Ok(granted) => println!("access request answered: granted={granted}"),
                Err(_) => println!("access request unanswered after 60s; proceeding anyway"),
            }
        }

        // -- Enumeration --------------------------------------------------
        let t = Instant::now();
        let devices = capture::enumerate();
        println!(
            "\nenumerated {} device(s) in {:?}:",
            devices.len(),
            t.elapsed()
        );
        for d in &devices {
            println!(
                "[{}] {:?} uid={} model={} type={}{}",
                d.index,
                d.name,
                d.uid,
                d.model_id,
                d.device_type,
                if d.muxed { " (muxed)" } else { "" },
            );
            for f in &d.formats {
                println!(
                    "      {}x{} '{}' {:.2}-{:.2} fps",
                    f.width,
                    f.height,
                    f.fourcc_str(),
                    f.min_fps,
                    f.max_fps
                );
            }
        }
        let Some(dev) = pick
            .and_then(|i| devices.iter().find(|d| d.index == i))
            .or_else(|| devices.first())
        else {
            println!("\nno capture devices found; nothing to open");
            return Ok(());
        };

        // -- Open ----------------------------------------------------------
        let fmt = capture::pick_format(&dev.formats, 1080)
            .ok_or_else(|| anyhow::anyhow!("device {} reports no formats", dev.name))?;
        let pixfmt = fmt.ffmpeg_pixel_format();
        println!(
            "\nopening [{}] {:?} at {}x{} '{}' {:.2} fps (requesting pixel_format={})",
            dev.index,
            dev.name,
            fmt.width,
            fmt.height,
            fmt.fourcc_str(),
            fmt.max_fps,
            pixfmt.unwrap_or("<default>")
        );
        let t = Instant::now();
        let mut ictx =
            capture::open_by_index(dev.index, (fmt.width, fmt.height), fmt.max_fps, pixfmt)?;
        println!("open took {:?}", t.elapsed());

        let (vid_idx, params, time_base) = {
            let st = ictx
                .streams()
                .best(ff::media::Type::Video)
                .ok_or_else(|| anyhow::anyhow!("no video stream on capture input"))?;
            (st.index(), st.parameters(), st.time_base())
        };
        let tb = f64::from(time_base.numerator()) / f64::from(time_base.denominator());
        let mut decoder = ff::codec::context::Context::from_parameters(params.clone())?
            .decoder()
            .video()?;
        println!(
            "stream: codec={:?} pixfmt={:?} {}x{} tb={}/{}",
            params.id(),
            decoder.format(),
            decoder.width(),
            decoder.height(),
            time_base.numerator(),
            time_base.denominator()
        );

        // -- Timed frame pull ----------------------------------------------
        let mut decoded = ff::frame::Video::empty();
        let mut frames = 0u32;
        let mut first: Option<(Instant, f64)> = None;
        let mut last_pts = 0.0f64;
        let deadline = Instant::now() + Duration::from_secs_f64(secs);
        let t_first_frame = Instant::now();
        for (stream, packet) in ictx.packets() {
            if stream.index() != vid_idx {
                continue;
            }
            decoder.send_packet(&packet)?;
            while decoder.receive_frame(&mut decoded).is_ok() {
                let pts = decoded.pts().unwrap_or(0) as f64 * tb;
                if frames == 0 {
                    println!("first frame after {:?}", t_first_frame.elapsed());
                    first = Some((Instant::now(), pts));
                }
                frames += 1;
                last_pts = pts;
                if frames <= 5 {
                    let (wall0, pts0) = first.unwrap();
                    println!(
                        "frame {frames}: pts={:.4}s (Δpts={:.4}s, Δwall={:.4}s)",
                        pts,
                        pts - pts0,
                        wall0.elapsed().as_secs_f64()
                    );
                }
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        if let Some((_, pts0)) = first {
            let span = last_pts - pts0;
            println!(
                "pulled {frames} frames over {span:.2}s of pts ({:.2} fps effective)",
                if span > 0.0 { f64::from(frames - 1) / span } else { 0.0 }
            );
        } else {
            println!("no frames decoded (permission denied yields silence/black here)");
        }

        // -- Teardown -------------------------------------------------------
        let t = Instant::now();
        drop(ictx);
        println!("teardown (avformat_close_input incl. session stop): {:?}", t.elapsed());

        // -- Double-open behavior (informational, 1.2) -----------------------
        println!("\ndouble-open check: opening the same device twice...");
        let a = capture::open_by_index(dev.index, (fmt.width, fmt.height), fmt.max_fps, pixfmt);
        let b = capture::open_by_index(dev.index, (fmt.width, fmt.height), fmt.max_fps, pixfmt);
        println!("first={} second={}", ok_err(&a), ok_err(&b));
        let t = Instant::now();
        drop(a);
        drop(b);
        println!("double teardown: {:?}", t.elapsed());
        Ok(())
    }

    fn ok_err<T>(r: &anyhow::Result<T>) -> String {
        match r {
            Ok(_) => "ok".into(),
            Err(e) => format!("err({e:#})"),
        }
    }

    const DELAY: f64 = 1.5;
    const RUN_FOR: Duration = Duration::from_secs(5);

    /// Stream 2 go/no-go: CaptureService + live tap + delayed tap through the
    /// real offscreen render path. Passes when the delayed tap's frames trail
    /// the live edge by the requested delay (within a couple frame periods)
    /// and both taps render through `Renderer` without error.
    fn milestone(pick: Option<usize>) -> anyhow::Result<()> {
        use vidiotic::render::{Globals, Renderer};
        use vidiotic::video::capture::{CaptureService, ServiceStatus};

        let devices = capture::enumerate();
        let dev = pick
            .and_then(|i| devices.iter().find(|d| d.index == i))
            .or_else(|| devices.first())
            .ok_or_else(|| anyhow::anyhow!("no capture devices found"))?;
        println!("milestone: service on [{}] {:?} (uid={})", dev.index, dev.name, dev.uid);

        let service = CaptureService::start(dev.uid.clone());
        let t0 = Instant::now();
        loop {
            match service.status() {
                ServiceStatus::Running { width, height, fps } => {
                    println!("service running: {width}x{height} @ {fps:.2} fps");
                    break;
                }
                ServiceStatus::Failed(e) => anyhow::bail!("service failed: {e}"),
                ServiceStatus::Starting if t0.elapsed() > Duration::from_secs(10) => {
                    anyhow::bail!("service did not start within 10s")
                }
                ServiceStatus::Starting => std::thread::sleep(Duration::from_millis(50)),
            }
        }

        // Offscreen GPU + the real renderer (mirrors spike_render's harness).
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("spike-capture-device"),
                required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            }))?;
        let mut renderer = Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        renderer.update_globals(&queue, &Globals::default());

        let mut live = service.tap();
        let mut delayed = service.tap();
        delayed.delay_eff = DELAY;

        let (mut live_frames, mut delayed_frames, mut renders) = (0u32, 0u32, 0u32);
        let mut live_edge_pts = 0.0f64;
        let mut worst_lag_err = 0.0f64;
        let mut lag_samples = 0u32;
        let deadline = Instant::now() + RUN_FOR;
        while Instant::now() < deadline {
            let now = Instant::now();
            if let Some(f) = live.poll(now) {
                live_frames += 1;
                live_edge_pts = f.pts_sec;
                render_offscreen(&device, &queue, &mut renderer, &f);
                renders += 1;
            }
            if let Some(f) = delayed.poll(now) {
                delayed_frames += 1;
                // Only judge the lag once the ring holds a full DELAY of
                // history; before that the tap clamps to the oldest frame.
                if live_edge_pts > DELAY + 0.2 {
                    let lag = live_edge_pts - f.pts_sec;
                    worst_lag_err = f64::max(worst_lag_err, (lag - DELAY).abs());
                    lag_samples += 1;
                }
                render_offscreen(&device, &queue, &mut renderer, &f);
                renders += 1;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(service); // detached teardown; process exit races it, which is fine

        println!(
            "live={live_frames} delayed={delayed_frames} renders={renders} \
             lag_samples={lag_samples} worst_lag_err={worst_lag_err:.3}s"
        );
        let ok = live_frames > 60
            && delayed_frames > 30
            && lag_samples > 10
            && worst_lag_err < 0.1;
        println!("{}", if ok { "MILESTONE PASS" } else { "MILESTONE FAIL" });
        std::process::exit(i32::from(!ok));
    }

    /// Upload a frame and run the real render pass into a throwaway offscreen
    /// target — the point is exercising the upload/render path, not the pixels.
    fn render_offscreen(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut vidiotic::render::Renderer,
        frame: &vidiotic::video::frame::DecodedFrame,
    ) {
        renderer.upload_frame(device, queue, frame);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        renderer.render(device, queue, &mut encoder, &view, 64, 64);
        queue.submit([encoder.finish()]);
    }
}
