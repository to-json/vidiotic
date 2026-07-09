//! Decode spike: open a clip, pull a handful of frames, report the path
//! (HAP or software), dimensions, and a center pixel so the RGBA fallback is
//! visibly working. Run: `cargo run --bin spike_decode -- <clip>`

use vidiotic::video::decoder;
use vidiotic::video::frame::PixelData;

fn main() {
    env_logger::init();
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "../throw-shade/beltram.mkv".to_string());

    let handle = match decoder::spawn(path.into(), 0.0, None, 1.0) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("spawn failed: {e:#}");
            std::process::exit(1);
        }
    };

    let mut got = 0;
    for _ in 0..5 {
        match handle.frames.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(f) => {
                got += 1;
                let kind = match &f.pixels {
                    PixelData::Bc {
                        format, video_mode, ..
                    } => format!("BC {:?} (mode {video_mode})", format),
                    PixelData::Rgba { data, stride } => {
                        // center pixel
                        let cx = f.w / 2;
                        let cy = f.h / 2;
                        let off = (cy * stride + cx * 4) as usize;
                        let px = data.get(off..off + 4).unwrap_or(&[0, 0, 0, 0]);
                        format!("RGBA stride={stride} center={px:?}")
                    }
                };
                println!(
                    "frame {got}: {}x{} pts={:.3}s  {kind}",
                    f.w, f.h, f.pts_sec
                );
            }
            Err(e) => {
                eprintln!("recv: {e}");
                break;
            }
        }
    }

    println!(
        "\n{}",
        if got >= 5 {
            "DECODE SPIKE PASS"
        } else {
            "DECODE SPIKE FAIL"
        }
    );
    drop(handle);
    std::process::exit(if got >= 5 { 0 } else { 1 });
}
