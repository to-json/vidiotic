//! Camera capture spike: enumerate devices, open one through ffmpeg's
//! avfoundation demuxer, decode timed frames for a few seconds, and measure
//! open/teardown latency. Exercises the Stream 1 exit criteria of
//! docs/camera-source-plan.md.
//!
//! Usage: `spike_capture [device-index] [seconds]` — defaults to device 0 for
//! 3 seconds. Run with no capture args after plugging/unplugging devices to
//! re-check enumeration.

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
        let pick: Option<usize> = args.next().and_then(|a| a.parse().ok());
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
}
