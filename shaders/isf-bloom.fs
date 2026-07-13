/*{
    "DESCRIPTION": "Bloom — bright areas glow. Threshold picks what blooms, radius sets the spread, intensity the strength.",
    "CREDIT": "vidiotic",
    "ISFVSN": "2.0",
    "CATEGORIES": ["Glow"],
    "INPUTS": [
        { "NAME": "threshold", "TYPE": "float", "MIN": 0.0, "MAX": 1.0, "DEFAULT": 0.55, "LABEL": "Threshold" },
        { "NAME": "radius",    "TYPE": "float", "MIN": 0.5, "MAX": 4.0, "DEFAULT": 1.5,  "LABEL": "Radius" },
        { "NAME": "intensity", "TYPE": "float", "MIN": 0.0, "MAX": 3.0, "DEFAULT": 1.2,  "LABEL": "Intensity" }
    ],
    "PASSES": [
        { "TARGET": "bright", "WIDTH": "$WIDTH/2", "HEIGHT": "$HEIGHT/2" },
        { "TARGET": "blurA",  "WIDTH": "$WIDTH/2", "HEIGHT": "$HEIGHT/2" },
        { "TARGET": "blurB",  "WIDTH": "$WIDTH/2", "HEIGHT": "$HEIGHT/2" },
        {}
    ]
}*/

// 9-tap separable gaussian, run horizontally then vertically at half res.
// RENDERSIZE is per-pass, so the tap spacing below is in half-res pixels.

void main() {
    vec2 uv = isf_FragNormCoord;
    if (PASSINDEX == 0) {
        // bright-pass: keep what clears the threshold, softly
        vec3 c = IMG_THIS_NORM_PIXEL(inputImage).rgb;
        float l = dot(c, vec3(0.299, 0.587, 0.114));
        gl_FragColor = vec4(c * smoothstep(threshold, threshold + 0.2, l), 1.0);
    } else if (PASSINDEX == 1) {
        float px = radius / RENDERSIZE.x;
        vec3 acc = IMG_NORM_PIXEL(bright, uv).rgb * 0.227027;
        acc += IMG_NORM_PIXEL(bright, uv + vec2(px * 1.0, 0.0)).rgb * 0.1945946;
        acc += IMG_NORM_PIXEL(bright, uv - vec2(px * 1.0, 0.0)).rgb * 0.1945946;
        acc += IMG_NORM_PIXEL(bright, uv + vec2(px * 2.0, 0.0)).rgb * 0.1216216;
        acc += IMG_NORM_PIXEL(bright, uv - vec2(px * 2.0, 0.0)).rgb * 0.1216216;
        acc += IMG_NORM_PIXEL(bright, uv + vec2(px * 3.0, 0.0)).rgb * 0.054054;
        acc += IMG_NORM_PIXEL(bright, uv - vec2(px * 3.0, 0.0)).rgb * 0.054054;
        acc += IMG_NORM_PIXEL(bright, uv + vec2(px * 4.0, 0.0)).rgb * 0.016216;
        acc += IMG_NORM_PIXEL(bright, uv - vec2(px * 4.0, 0.0)).rgb * 0.016216;
        gl_FragColor = vec4(acc, 1.0);
    } else if (PASSINDEX == 2) {
        float py = radius / RENDERSIZE.y;
        vec3 acc = IMG_NORM_PIXEL(blurA, uv).rgb * 0.227027;
        acc += IMG_NORM_PIXEL(blurA, uv + vec2(0.0, py * 1.0)).rgb * 0.1945946;
        acc += IMG_NORM_PIXEL(blurA, uv - vec2(0.0, py * 1.0)).rgb * 0.1945946;
        acc += IMG_NORM_PIXEL(blurA, uv + vec2(0.0, py * 2.0)).rgb * 0.1216216;
        acc += IMG_NORM_PIXEL(blurA, uv - vec2(0.0, py * 2.0)).rgb * 0.1216216;
        acc += IMG_NORM_PIXEL(blurA, uv + vec2(0.0, py * 3.0)).rgb * 0.054054;
        acc += IMG_NORM_PIXEL(blurA, uv - vec2(0.0, py * 3.0)).rgb * 0.054054;
        acc += IMG_NORM_PIXEL(blurA, uv + vec2(0.0, py * 4.0)).rgb * 0.016216;
        acc += IMG_NORM_PIXEL(blurA, uv - vec2(0.0, py * 4.0)).rgb * 0.016216;
        gl_FragColor = vec4(acc, 1.0);
    } else {
        // composite: source + blurred brights
        vec3 src = IMG_THIS_NORM_PIXEL(inputImage).rgb;
        vec3 glow = IMG_NORM_PIXEL(blurB, uv).rgb;
        gl_FragColor = vec4(src + glow * intensity, 1.0);
    }
}
