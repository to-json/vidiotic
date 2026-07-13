// tile: replicate the image into a grid that doubles through the phrase
// (1 -> 2 -> 4 -> 8 tiles). Alternate rows scroll with the beat (bass sets the
// pace), and each beat pulses a small zoom inside every cell. The first phrase
// quarter is a single tile, so the effect builds from passthrough.

float band(int i) { return log(1.0 + fftBand(i)) / 8.0; }

void main() {
    float bass = (band(0) + band(1) + band(2)) / 3.0;

    float n = exp2(floor(phrase_phase * 4.0)); // 1, 2, 4, 8 tiles
    vec2 uv = fragTexCoord * n;

    // alternate rows crawl in opposite directions
    float row = floor(uv.y);
    uv.x += (mod(row, 2.0) * 2.0 - 1.0) * beat * 0.05 * (0.3 + bass);

    // per-beat inset zoom pulse inside each cell
    vec2 cell = fract(uv);
    float pulse = pow(max(0.0, 1.0 - fract(beat)), 3.0) * 0.15;
    cell = (cell - 0.5) * (1.0 - pulse) + 0.5;

    vec3 col = prev(cell).rgb;

    // downbeat flash
    col += pow(max(0.0, 1.0 - bar_phase), 8.0) * 0.3;

    FragColor = vec4(col, 1.0);
}
