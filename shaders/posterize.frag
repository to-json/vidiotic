// posterize: crush colors to a few levels and swing the palette. The level
// count falls as the low end gets loud (8 down to ~2.5), the hue wheel turns
// once per phrase, and each beat kicks the hue a half-step that eases out.

float band(int i) { return log(1.0 + fftBand(i)) / 8.0; }

// Rotate an RGB color around the value axis by `amount` turns (0..1).
vec3 hueRotate(vec3 c, float amount) {
    const vec3 k = vec3(0.57735); // 1/sqrt(3), the luma-ish rotation axis
    float ca = cos(amount * 6.2831853);
    float sa = sin(amount * 6.2831853);
    return c * ca + cross(k, c) * sa + k * dot(k, c) * (1.0 - ca);
}

void main() {
    vec3 col = prev(fragTexCoord).rgb;
    float bass = (band(0) + band(1) + band(2)) / 3.0;

    float levels = mix(8.0, 2.5, clamp(bass * 3.0, 0.0, 1.0));
    col = floor(col * levels + 0.5) / levels;

    float kick = pow(max(0.0, 1.0 - fract(beat)), 3.0) * 0.08;
    col = hueRotate(col, phrase_phase + kick);

    FragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
