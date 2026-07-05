// tunnel: wrap the clip onto the walls of an infinite tunnel. Depth scrolls with
// the beat, bass widens the mouth, and treble scatters bright rings rushing past.
// The wrapping sampler tiles the clip around the tunnel and along its length.

#define PI 3.14159265

float band(int i) { return log(1.0 + fftBand(i)) / 8.0; }

void main() {
    vec2 c = fragTexCoord - 0.5;
    c.x *= resolution.x / resolution.y;

    float bass = (band(0) + band(1) + band(2)) / 3.0;
    float treble = (band(14) + band(16) + band(18)) / 3.0;

    float r = length(c);
    float a = atan(c.y, c.x);

    // classic tunnel: radius -> depth (rushes forward), angle -> across
    float depth = 0.30 / (r + 0.05) + beat * 0.5 + bass * 0.5;
    vec2 tuv = vec2(a / PI * 1.5 + time * 0.05, depth);
    vec3 col = video(fract(tuv)).rgb;

    // dark core for depth; bass opens the throat
    col *= smoothstep(0.0, 0.15 + bass * 0.2, r);

    // treble sparkle rings racing down the tunnel
    col += treble * pow(max(0.0, 1.0 - fract(depth)), 6.0) * vec3(0.6, 0.8, 1.0);

    // downbeat pulse
    col += pow(max(0.0, 1.0 - bar_phase), 6.0) * 0.3;

    FragColor = vec4(col, 1.0);
}
