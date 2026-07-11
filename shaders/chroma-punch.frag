// chroma-punch: bass-driven zoom punch, chromatic aberration that blooms with
// low end, a downbeat strobe, a per-beat shake, scanlines and a vignette.
// Reads clean and slightly kinetic in silence (beat effects still run); goes
// aggressive with a loud track. Live-edit while the app runs.

float band(int i) { return log(1.0 + fftBand(i)) / 8.0; }

void main() {
    vec2 uv = fragTexCoord;
    vec2 c = uv - 0.5;

    float bass = (band(0) + band(1) + band(2)) / 3.0;
    float energy = log(1.0 + lvl) * 0.15;

    // zoom toward center: a snap on the downbeat plus a steady bass push
    float punch = pow(max(0.0, 1.0 - bar_phase), 3.0) * 0.06 + bass * 0.10;
    vec2 z = c / (1.0 + punch);

    // sharp shake on each beat onset
    float shake = pow(max(0.0, 1.0 - fract(beat)), 8.0);
    z += vec2(sin(time * 90.0), cos(time * 77.0)) * shake * 0.004;
    vec2 suv = z + 0.5;

    // chromatic aberration: stronger with bass and toward the edges
    float ca = (0.004 + bass * 0.022) * (0.4 + length(c));
    vec2 dir = normalize(c + 1e-5);
    vec3 col;
    col.r = prev(suv + dir * ca).r;
    col.g = prev(suv).g;
    col.b = prev(suv - dir * ca).b;

    col *= 1.0 + energy;

    // downbeat flash
    col += pow(max(0.0, 1.0 - bar_phase), 6.0) * 0.5;

    // scanlines + vignette
    col *= 0.94 + 0.06 * sin(uv.y * resolution.y * 3.14159);
    col *= smoothstep(1.15, 0.35, length(c));

    FragColor = vec4(col, 1.0);
}
