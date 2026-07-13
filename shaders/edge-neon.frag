// edge-neon: Sobel outlines glowing over a dimmed base image. The line color
// cycles the hue wheel once per phrase, loudness drives the glow gain, bass
// thickens the line, and the downbeat flashes the outline white.

float band(int i) { return log(1.0 + fftBand(i)) / 8.0; }
float luma(vec2 uv) { return dot(prev(uv).rgb, vec3(0.299, 0.587, 0.114)); }

void main() {
    float bass = (band(0) + band(1) + band(2)) / 3.0;
    vec2 uv = fragTexCoord;
    vec2 px = (1.0 + bass * 2.0) / resolution;

    float tl = luma(uv + px * vec2(-1.0,  1.0));
    float tt = luma(uv + px * vec2( 0.0,  1.0));
    float tr = luma(uv + px * vec2( 1.0,  1.0));
    float ll = luma(uv + px * vec2(-1.0,  0.0));
    float rr = luma(uv + px * vec2( 1.0,  0.0));
    float bl = luma(uv + px * vec2(-1.0, -1.0));
    float bb = luma(uv + px * vec2( 0.0, -1.0));
    float br = luma(uv + px * vec2( 1.0, -1.0));

    float gx = (tr + 2.0 * rr + br) - (tl + 2.0 * ll + bl);
    float gy = (tl + 2.0 * tt + tr) - (bl + 2.0 * bb + br);
    float edge = clamp(length(vec2(gx, gy)) * (1.5 + log(1.0 + lvl) * 2.0), 0.0, 1.0);

    // neon tint cycles once per phrase; the downbeat flashes it white
    vec3 tint = 0.5 + 0.5 * cos(6.2831853 * (phrase_phase + vec3(0.0, 0.33, 0.67)));
    tint = mix(tint, vec3(1.0), pow(max(0.0, 1.0 - bar_phase), 6.0));

    vec3 col = prev(uv).rgb * 0.15 + tint * edge;

    FragColor = vec4(col, 1.0);
}
