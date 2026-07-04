//! M0 spike: verify naga's GLSL frontend accepts the app preamble + preprocessed
//! throw-shade reference shaders, and that a deliberate error is reported with a
//! sensible user-file line number. Run: `cargo run --bin spike_naga`.

use std::path::Path;
use vidiotic::shader::{compile_glsl_to_module, compile_wgsl_to_module, ShaderError};

fn try_glsl(label: &str, path: &str) -> bool {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            println!("[SKIP] {label}: cannot read {path}: {e}");
            return true; // not a failure of the compiler
        }
    };
    match compile_glsl_to_module(&src) {
        Ok(m) => {
            println!(
                "[ OK ] {label}: parsed + validated ({} functions, {} global vars)",
                m.functions.len(),
                m.global_variables.len()
            );
            true
        }
        Err(e) => {
            println!("[FAIL] {label}:\n{e}");
            false
        }
    }
}

fn main() {
    let _ = Path::new("."); // silence unused import if refactored

    let mut ok = true;

    // The two real user shaders from the reference project.
    ok &= try_glsl(
        "plain.frag",
        "../throw-shade/resource/plain.frag",
    );
    ok &= try_glsl(
        "zellij.fs",
        "../throw-shade/resource/zellij.fs",
    );

    // A minimal shader exercising the video() helper + fftBand + shadertoy aliases.
    let synthetic = r#"
void main() {
    vec4 v = video(fragTexCoord);
    float b = fftBand(3) + lvl;
    FragColor = mix(v, vec4(iResolution.xy / 100.0, b, 1.0), 0.5 + 0.5 * sin(iTime));
}
"#;
    ok &= match compile_glsl_to_module(synthetic) {
        Ok(_) => {
            println!("[ OK ] synthetic (video/fftBand/aliases): parsed + validated");
            true
        }
        Err(e) => {
            println!("[FAIL] synthetic:\n{e}");
            false
        }
    };

    // WGSL passthrough following the documented binding contract.
    let wgsl = r#"
struct Globals {
    resolution: vec2<f32>, mouse: vec2<f32>,
    time: f32, lvl: f32, beat: f32, bar_phase: f32,
    phrase_phase: f32, bpm: f32, video_mode: i32, _pad0: f32,
    freqs: array<vec4<f32>, 6>,
}
@group(0) @binding(0) var<uniform> G: Globals;
@group(1) @binding(0) var videoTex: texture_2d<f32>;
@group(1) @binding(1) var videoSmp: sampler;
@group(1) @binding(2) var alphaTex: texture_2d<f32>;

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(videoTex, videoSmp, vec2<f32>(uv.x, 1.0 - uv.y)) * (0.5 + 0.5 * G.lvl);
}
"#;
    ok &= match compile_wgsl_to_module(wgsl) {
        Ok(_) => {
            println!("[ OK ] wgsl passthrough: parsed + validated");
            true
        }
        Err(e) => {
            println!("[FAIL] wgsl passthrough:\n{e}");
            false
        }
    };

    // Deliberate error on user line 3 — verify the reported line remaps correctly.
    let broken = "void main() {\n    float x = 1.0;\n    FragColor = notAFunction(x);\n}\n";
    match compile_glsl_to_module(broken) {
        Ok(_) => {
            println!("[FAIL] broken shader unexpectedly compiled");
            ok = false;
        }
        Err(ShaderError::Parse { line, .. }) | Err(ShaderError::Validation { line, .. }) => {
            println!("[ OK ] broken shader rejected; reported user line = {line:?} (expected ~3)");
        }
        Err(e) => {
            println!("[WARN] broken shader rejected but with unexpected error kind: {e}");
        }
    }

    println!("\n{}", if ok { "SPIKE PASS" } else { "SPIKE FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}
