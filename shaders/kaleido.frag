// kaleido: mirror the clip into a rotating kaleidoscope. Segment count steps up
// over each phrase, rotation rides the beat, bass pulses the zoom, and the
// downbeat pops a bright bloom. The sampler wraps, so folded coords tile cleanly.

#define PI 3.14159265

float band(int i) { return log(1.0 + fftBand(i)) / 8.0; }

void main() {
    vec2 c = fragTexCoord - 0.5;
    c.x *= resolution.x / resolution.y; // keep circles round

    float bass = (band(0) + band(1) + band(2)) / 3.0;

    // 6..16 mirror segments, stepping up through the phrase
    float seg = 6.0 + floor(phrase_phase * 6.0) * 2.0;

    float a = atan(c.y, c.x);
    float r = length(c);

    // spin with musical time
    a += beat * 0.25 + bass * 2.0;

    // kaleidoscope fold
    float k = 2.0 * PI / seg;
    a = abs(mod(a, k) - k * 0.5);

    // bass breathes the zoom
    r /= (1.0 + bass * 0.6);

    vec2 kuv = vec2(cos(a), sin(a)) * r + 0.5;
    vec3 col = prev(kuv).rgb;

    // per-beat brighten + downbeat ring flash
    col *= 1.0 + pow(max(0.0, 1.0 - fract(beat)), 4.0) * 0.5;
    col += pow(max(0.0, 1.0 - bar_phase), 8.0) * 0.4;

    FragColor = vec4(col, 1.0);
}
