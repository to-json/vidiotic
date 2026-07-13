// pixellate: quantize the image to a block grid. The grid refines through the
// phrase (coarse -> fine), bass crushes it coarser, and the downbeat pops a
// brief full-res flash so the crush reads as rhythm rather than a filter.

float band(int i) { return log(1.0 + fftBand(i)) / 8.0; }

void main() {
    float bass = (band(0) + band(1) + band(2)) / 3.0;

    // 12..96 blocks across, refining through the phrase, crushed by bass
    float blocks = mix(12.0, 96.0, phrase_phase) / (1.0 + bass * 2.0);
    vec2 grid = vec2(blocks * resolution.x / resolution.y, blocks);

    // downbeat: snap back to full resolution for a flash
    float pop = pow(max(0.0, 1.0 - bar_phase), 6.0);

    vec2 quv = (floor(fragTexCoord * grid) + 0.5) / grid;
    vec3 col = prev(mix(quv, fragTexCoord, pop)).rgb;

    // per-beat brightness tick so still footage keeps pulsing
    col *= 1.0 + pow(max(0.0, 1.0 - fract(beat)), 5.0) * 0.25;

    FragColor = vec4(col, 1.0);
}
