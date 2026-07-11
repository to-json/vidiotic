/*{
    "DESCRIPTION": "Hue rotate + gain + optional invert — an example ISF filter for vidiotic.",
    "CREDIT": "vidiotic",
    "ISFVSN": "2.0",
    "CATEGORIES": ["Color Effect"],
    "INPUTS": [
        { "NAME": "gain",   "TYPE": "float", "MIN": 0.0, "MAX": 2.0, "DEFAULT": 1.0, "LABEL": "Gain" },
        { "NAME": "hue",    "TYPE": "float", "MIN": 0.0, "MAX": 1.0, "DEFAULT": 0.0, "LABEL": "Hue rotate" },
        { "NAME": "invert", "TYPE": "bool",  "DEFAULT": false, "LABEL": "Invert" },
        { "NAME": "tint",   "TYPE": "color", "DEFAULT": [1.0, 1.0, 1.0, 1.0], "LABEL": "Tint" }
    ]
}*/

// Rotate an RGB color around the value axis by `amount` turns (0..1).
vec3 hueRotate(vec3 c, float amount) {
    const vec3 k = vec3(0.57735); // 1/sqrt(3), the luma-ish rotation axis
    float ca = cos(amount * 6.2831853);
    float sa = sin(amount * 6.2831853);
    return c * ca + cross(k, c) * sa + k * dot(k, c) * (1.0 - ca);
}

void main() {
    vec4 src = IMG_THIS_NORM_PIXEL(inputImage);
    vec3 rgb = hueRotate(src.rgb, hue) * gain * tint.rgb;
    if (invert) rgb = 1.0 - rgb;
    gl_FragColor = vec4(clamp(rgb, 0.0, 1.0), src.a);
}
