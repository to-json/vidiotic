// Demo compositor: the video with audio-reactive brightness, a downbeat flash,
// and a spectrum ribbon along the bottom. Shows how a user shader composes the
// clip via video() plus the audio/beat uniforms. Live-edit this file while the
// app runs.

void main() {
    vec2 uv = fragTexCoord;

    // gentle bass-driven zoom toward center
    float bass = log(1.0 + fftBand(1)) * 0.02;
    vec2 cuv = (uv - 0.5) / (1.0 + bass) + 0.5;

    vec3 col = video(cuv).rgb;

    // overall loudness lifts brightness a touch
    col *= 1.0 + log(1.0 + lvl) * 0.12;

    // flash on the downbeat (bar_phase runs 0..1 across each bar)
    float flash = pow(max(0.0, 1.0 - bar_phase), 6.0) * 0.35;
    col += flash;

    // spectrum ribbon along the bottom edge
    int band = int(clamp(uv.x * 21.0, 0.0, 20.0));
    float mag = log(1.0 + fftBand(band)) / 8.0;
    if (uv.y < mag * 0.14) {
        col = mix(col, vec3(0.15, 0.55, 0.95), 0.7);
    }

    FragColor = vec4(col, 1.0);
}
