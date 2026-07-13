// Demo compositor: the video with audio-reactive brightness, a downbeat flash,
// and a spectrum ribbon along the bottom. Shows how a user shader composes the
// clip via video() plus the audio/beat uniforms. Live-edit this file while the
// app runs.

// lvl 

//    "time",
//    "lvl",
//    "mousePos",
//    "mouse",
//    "resolution",
//    "beat",
//    "bpm",
//    "bar_phase",
//    "phrase_phase",
//    "freqs1",
//    "iTime",
//    "iResolution",

void main() {
    vec2 uv = fragTexCoord;

    // gentle bass-driven zoom toward center
    float bass = log(1.0 + fftBand(1)) * 0.08;
    float lowmid = log(1.0 + fftBand(6)) * 0.08;
    float mid = log(1.0 + fftBand(12)) * 0.08;
    float himid = log(1.0 + fftBand(16)) * 0.08;
    float treb = log(1.0 + fftBand(20)) * 0.08;
    vec2 cuv = (uv - 0.5) / (1.0 + bass) + 0.5;
    float wiggly = tan(cos(beat));
    float jiggly = cos(tan(beat));

    cuv.x = ((0.8 + (2 * treb)) * cuv.x) + 0.1;
    cuv.y = ((0.8 + (2 * mid)) * cuv.y) + 0.1 ;

    cuv.x = ((0.8 + (0.07 * wiggly)) * cuv.x) + 0.1;
    cuv.y = ((0.8 + (0.07 * jiggly)) * cuv.y) + 0.1 ;

    vec3 col = video(cuv).rgb;
    col.r = ((0.8 + (bass*9)) * col.r) -0.1;
    col.g = ((0.8 + (mid*9)) * col.g) -0.1;
    col.b = ((0.8 + (treb*9)) * col.b) -0.1;

    col *= 1.0 + log(1.0 + lvl) * 0.12;

    // flash on the downbeat (bar_phase runs 0..1 across each bar)
    float flash = pow(max(0.0, 1.0 - bar_phase), 6.0) * 0.35;
    col += flash;

    // spectrum ribbon along the bottom edge
//    int band = int(clamp(uv.x * 21.0, 0.0, 20.0));
//    float mag = log(1.0 + fftBand(band)) / 8.0;
//    if (uv.y < mag * 0.84) {
//        col = mix(col, vec3(0.15, 0.55, 0.95), 0.7);
//    }

    FragColor = vec4(col, 1.0);
}
