//! Micro-benchmark: single-threaded vs banded BC1 compression.
//! `cargo test --test bc1_micro --release -- --nocapture --ignored`

use std::time::Instant;

use texpresso::{Format, Params};

#[test]
#[ignore = "manual timing harness"]
fn bc1_micro() {
    let (w, h) = (1920usize, 1080usize);
    // Smooth-ish gradient with noise: closer to video content than pure noise.
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [
                (x / 8) as u8,
                (y / 8) as u8,
                ((x ^ y) & 0xff) as u8,
                255,
            ]
        })
        .collect();
    let mut out = vec![0u8; Format::Bc1.compressed_size(w, h)];

    for (name, alg) in [
        ("ClusterFit", texpresso::Algorithm::ClusterFit),
        ("RangeFit", texpresso::Algorithm::RangeFit),
    ] {
        let params = Params { algorithm: alg, ..Params::default() };
        let t = Instant::now();
        for _ in 0..5 {
            Format::Bc1.compress(&rgba, w, h, params, &mut out);
        }
        println!("{name}: {:.1} ms/frame", t.elapsed().as_secs_f64() * 200.0);
    }
}
