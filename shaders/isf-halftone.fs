/*{
    "DESCRIPTION": "Halftone dot screen — mono newsprint or RGB press screens at classic offset angles.",
    "CREDIT": "vidiotic",
    "ISFVSN": "2.0",
    "CATEGORIES": ["Stylize"],
    "INPUTS": [
        { "NAME": "scale",   "TYPE": "float", "MIN": 20.0, "MAX": 200.0, "DEFAULT": 80.0,  "LABEL": "Dots across" },
        { "NAME": "angle",   "TYPE": "float", "MIN": 0.0,  "MAX": 1.0,   "DEFAULT": 0.125, "LABEL": "Screen angle" },
        { "NAME": "sharp",   "TYPE": "float", "MIN": 0.0,  "MAX": 1.0,   "DEFAULT": 0.8,   "LABEL": "Dot sharpness" },
        { "NAME": "colored", "TYPE": "bool",  "DEFAULT": true, "LABEL": "RGB screens" }
    ]
}*/

// One rotated dot screen: rotate into grid space, sample the source at the
// cell center (via the inverse rotation), and size the dot by the channel
// value under `mask`. Returns dot coverage 0..1.
float screenDot(vec2 uv, float ang, vec3 mask, float aspect, float soft) {
    float c = cos(ang);
    float s = sin(ang);
    vec2 p = vec2(uv.x * aspect, uv.y);
    vec2 g = vec2(c * p.x - s * p.y, s * p.x + c * p.y) * scale;
    vec2 id = floor(g) + 0.5;
    vec2 q = vec2(c * id.x + s * id.y, -s * id.x + c * id.y) / scale;
    vec2 suv = vec2(q.x / aspect, q.y);
    float v = dot(IMG_NORM_PIXEL(inputImage, suv).rgb, mask);
    float r = sqrt(clamp(v, 0.0, 1.0)) * 0.75;
    return 1.0 - smoothstep(r - soft, r, length(fract(g) - 0.5));
}

void main() {
    vec2 uv = isf_FragNormCoord;
    float aspect = RENDERSIZE.x / RENDERSIZE.y;
    float soft = mix(0.35, 0.03, sharp);
    float base = angle * 6.2831853;

    if (colored) {
        // classic press offsets: 15deg / 75deg / 0deg between the screens
        float r = screenDot(uv, base + 0.2618, vec3(1.0, 0.0, 0.0), aspect, soft);
        float g = screenDot(uv, base + 1.3090, vec3(0.0, 1.0, 0.0), aspect, soft);
        float b = screenDot(uv, base,          vec3(0.0, 0.0, 1.0), aspect, soft);
        gl_FragColor = vec4(r, g, b, 1.0);
    } else {
        float v = screenDot(uv, base, vec3(0.299, 0.587, 0.114), aspect, soft);
        gl_FragColor = vec4(vec3(v), 1.0);
    }
}
