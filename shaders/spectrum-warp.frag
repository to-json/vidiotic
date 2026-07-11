// spectrum-warp: the frequency spectrum ripples the image. Each horizontal band
// of the frame is driven by its own FFT bin — louder bins shove the pixels
// sideways and stain them with a rainbow keyed to the frequency axis. A VU
// spectrum climbs the bottom edge and the whole frame kicks on every beat.

float band(int i) { return log(1.0 + fftBand(i)) / 8.0; }

void main() {
    vec2 uv = fragTexCoord;

    // the band at this height displaces the row horizontally
    int b = int(clamp(uv.y * 21.0, 0.0, 20.0));
    float m = band(b);
    float warp = sin(uv.y * 40.0 + time * 3.0) * m * 0.08;
    vec3 col = prev(vec2(uv.x + warp, uv.y)).rgb;

    // stain hot rows with a rainbow across the frequency axis
    vec3 tint = 0.5 + 0.5 * cos(6.2831 * (uv.y + vec3(0.0, 0.33, 0.67)));
    col = mix(col, col * tint * 1.6, clamp(m * 1.5, 0.0, 0.8));

    // VU spectrum along the bottom
    float mag = band(int(clamp(uv.x * 21.0, 0.0, 20.0)));
    if (uv.y < mag * 0.2) {
        col += vec3(0.10, 0.40, 0.80) * 0.6;
    }

    // beat kick
    col *= 1.0 + pow(max(0.0, 1.0 - fract(beat)), 5.0) * 0.4;

    FragColor = vec4(col, 1.0);
}
