// audio-scope: a Shadertoy-style audio visualizer driven by the 512x2 audio
// texture on iChannel0. The bottom half draws the FFT spectrum as glowing bars
// (row 0, y=0.25); the top half draws the waveform as a scope line (row 1,
// y=0.75). The clip shows through, tinted and shoved by the sound.
//
// This is the reference for Shadertoy audio compat: `texture(iChannel0, uv)`
// works exactly as on shadertoy.com, plus fftAt()/waveAt() shorthands.

void main() {
    vec2 uv = fragTexCoord;
    vec3 col = video(uv).rgb * 0.4;

    // FFT spectrum (linear frequency) — bars rising from the bottom.
    float fft = texture(iChannel0, vec2(uv.x, 0.25)).x;   // == fftAt(uv.x)
    float bar = smoothstep(fft, fft - 0.03, uv.y);        // 1 below the bar top
    vec3 spec = 0.5 + 0.5 * cos(6.2831 * (uv.x + vec3(0.0, 0.33, 0.67)));
    col += spec * bar * 0.9;

    // Waveform scope — a bright line around the vertical center.
    float wave = waveAt(uv.x);                            // 0..1, silence = 0.5
    float d = abs(uv.y - wave);
    col += vec3(0.9, 0.95, 1.0) * smoothstep(0.02, 0.0, d);

    // Overall level pushes brightness; downbeat kicks.
    col *= 1.0 + lvl * 0.5;
    col *= 1.0 + pow(max(0.0, 1.0 - fract(beat)), 5.0) * 0.3;

    FragColor = vec4(col, 1.0);
}
