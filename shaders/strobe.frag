// strobe: beat-gated flashes over a slightly dimmed image, so the hits land
// hard. The phrase builds an arc: quarter-note white flashes in the first
// half, eighth-notes in the third quarter, invert flashes for the finale.
// Loudness widens the flash window.

void main() {
    vec3 col = prev(fragTexCoord).rgb;

    // flash window as a fraction of the note: tight when quiet, wide when loud
    float win = 0.10 + log(1.0 + lvl) * 0.10;

    float quarter = step(fract(beat), win);
    float eighth  = step(fract(beat * 2.0), win);

    float p = phrase_phase;
    float gate = p < 0.5 ? quarter : eighth;
    vec3 hit = p > 0.75 ? 1.0 - col : vec3(1.0);

    col = mix(col * 0.92, hit, gate);

    FragColor = vec4(col, 1.0);
}
