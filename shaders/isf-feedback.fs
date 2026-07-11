/*{
    "DESCRIPTION": "Feedback trails — the image smears over time with optional horizontal drift.",
    "CREDIT": "vidiotic",
    "ISFVSN": "2.0",
    "CATEGORIES": ["Feedback"],
    "INPUTS": [
        { "NAME": "decay", "TYPE": "float", "MIN": 0.0, "MAX": 0.99, "DEFAULT": 0.85, "LABEL": "Trail decay" },
        { "NAME": "drift", "TYPE": "float", "MIN": -0.02, "MAX": 0.02, "DEFAULT": 0.0, "LABEL": "Drift" }
    ],
    "PASSES": [
        { "TARGET": "feedbackBuffer", "PERSISTENT": true },
        {}
    ]
}*/

void main() {
    if (PASSINDEX == 0) {
        // Accumulate: brightest of the current frame and the decayed, drifted
        // previous frame. PERSISTENT keeps feedbackBuffer across frames.
        vec2 uv = isf_FragNormCoord + vec2(drift, 0.0);
        vec4 prev = IMG_NORM_PIXEL(feedbackBuffer, uv) * decay;
        vec4 cur = IMG_THIS_NORM_PIXEL(inputImage);
        gl_FragColor = max(cur, prev);
    } else {
        // Present the accumulated buffer.
        gl_FragColor = IMG_THIS_NORM_PIXEL(feedbackBuffer);
    }
}
