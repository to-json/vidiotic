// glitch-vhs: datamosh meets worn tape. Bass hits tear rows sideways and split
// the RGB channels; a bright tracking bar rolls up the frame; sparse noise
// flecks spit on the beat. Subtle when quiet, violent on a heavy drop.

float band(int i) { return log(1.0 + fftBand(i)) / 8.0; }
float hash(float x) { return fract(sin(x * 127.1) * 43758.5453); }

void main() {
    vec2 uv = fragTexCoord;
    float bass = (band(0) + band(1) + band(2)) / 3.0;

    // block glitch: rows jump horizontally, gated harder as bass rises
    float t = floor(time * 12.0);
    float row = floor(uv.y * 24.0);
    float g = hash(row + t);
    float shift = step(0.85 - bass * 0.5, g) * (g - 0.5) * (0.02 + bass * 0.15);
    vec2 suv = vec2(fract(uv.x + shift), uv.y);

    // RGB split widens with bass
    float s = 0.005 + bass * 0.03;
    vec3 col;
    col.r = video(vec2(fract(suv.x + s), suv.y)).r;
    col.g = video(suv).g;
    col.b = video(vec2(fract(suv.x - s), suv.y)).b;

    // rolling VHS tracking bar
    float track = smoothstep(0.0, 0.03, abs(fract(uv.y - time * 0.2) - 0.5));
    col *= 0.82 + 0.18 * track;

    // fine scanlines
    col *= 0.9 + 0.1 * sin(uv.y * resolution.y * 3.14159);

    // noise flecks that spit on the beat
    float n = hash(dot(floor(uv * vec2(120.0, 90.0)), vec2(1.0, 57.0)) + t);
    col += step(0.995, n) * pow(max(0.0, 1.0 - fract(beat)), 3.0);

    FragColor = vec4(col, 1.0);
}
