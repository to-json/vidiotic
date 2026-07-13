/*{
    "DESCRIPTION": "Datamosh-style smear — the frame persists and drifts in blocks; only a trickle of the live image bleeds back in. Refresh high = subtle motion smear, low = full melt.",
    "CREDIT": "vidiotic",
    "ISFVSN": "2.0",
    "CATEGORIES": ["Feedback", "Glitch"],
    "INPUTS": [
        { "NAME": "refresh", "TYPE": "float", "MIN": 0.01, "MAX": 1.0,  "DEFAULT": 0.08, "LABEL": "Refresh" },
        { "NAME": "warp",    "TYPE": "float", "MIN": 0.0,  "MAX": 30.0, "DEFAULT": 8.0,  "LABEL": "Block drift px" },
        { "NAME": "cell",    "TYPE": "float", "MIN": 4.0,  "MAX": 64.0, "DEFAULT": 24.0, "LABEL": "Block size px" },
        { "NAME": "reroll",  "TYPE": "float", "MIN": 0.25, "MAX": 8.0,  "DEFAULT": 2.0,  "LABEL": "Reroll rate Hz" }
    ],
    "PASSES": [
        { "TARGET": "mosh", "PERSISTENT": true },
        {}
    ]
}*/

vec2 hash22(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.xx + p3.yz) * p3.zy);
}

void main() {
    vec2 uv = isf_FragNormCoord;
    if (PASSINDEX == 0) {
        // each block picks a drift direction, rerolled `reroll` times a second
        vec2 id = floor(uv * RENDERSIZE / cell);
        vec2 h = hash22(id + floor(TIME * reroll) * 61.7);
        vec2 disp = (h - 0.5) * 2.0 * warp / RENDERSIZE;

        vec4 acc = IMG_NORM_PIXEL(mosh, uv + disp);
        vec4 cur = IMG_THIS_NORM_PIXEL(inputImage);
        // bootstrap: the persistent buffer starts black; seed it with the live
        // frame instead of fading up from nothing
        if (dot(acc.rgb, vec3(1.0)) < 0.001) acc = cur;

        gl_FragColor = mix(acc, cur, clamp(refresh, 0.0, 1.0));
    } else {
        gl_FragColor = IMG_THIS_NORM_PIXEL(mosh);
    }
}
