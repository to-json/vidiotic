// wave-warp: sine-displace the UVs like the image is under water. The wave
// count steps up through the phrase, amplitude breathes with the low end, the
// wave crawls at beat rate, and the wave direction turns an eighth-turn on
// every beat so the warp never settles into a fixed ripple.

float band(int i) { return log(1.0 + fftBand(i)) / 8.0; }

void main() {
    float bass = (band(0) + band(1) + band(2)) / 3.0;

    float freq = 4.0 + floor(phrase_phase * 4.0) * 3.0; // 4..13 waves across
    float amp = 0.004 + bass * 0.05;

    float ang = floor(beat) * 0.7853982; // eighth-turn per beat
    vec2 dir = vec2(cos(ang), sin(ang));

    vec2 uv = fragTexCoord;
    float phase = dot(uv, dir) * freq * 6.2831853 + beat * 3.1415927;
    uv += vec2(-dir.y, dir.x) * sin(phase) * amp;

    vec3 col = prev(uv).rgb;
    col *= 1.0 + pow(max(0.0, 1.0 - fract(beat)), 4.0) * 0.2;

    FragColor = vec4(col, 1.0);
}
