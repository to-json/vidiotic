/*{
    "DESCRIPTION": "Mirror — reflect one half of the image onto the other, or fold all four quadrants. Axis slides the fold line.",
    "CREDIT": "vidiotic",
    "ISFVSN": "2.0",
    "CATEGORIES": ["Geometry"],
    "INPUTS": [
        { "NAME": "mode", "TYPE": "long", "VALUES": [0, 1, 2, 3, 4],
          "LABELS": ["Left onto right", "Right onto left", "Top onto bottom", "Bottom onto top", "Quad"],
          "DEFAULT": 0, "LABEL": "Mode" },
        { "NAME": "axis", "TYPE": "float", "MIN": 0.1, "MAX": 0.9, "DEFAULT": 0.5, "LABEL": "Axis" }
    ]
}*/

void main() {
    vec2 uv = isf_FragNormCoord;
    vec2 m = uv;

    if (mode == 0) {
        if (uv.x > axis) m.x = 2.0 * axis - uv.x;
    } else if (mode == 1) {
        if (uv.x < axis) m.x = 2.0 * axis - uv.x;
    } else if (mode == 2) {
        // isf_FragNormCoord y=1 is the top
        if (uv.y < axis) m.y = 2.0 * axis - uv.y;
    } else if (mode == 3) {
        if (uv.y > axis) m.y = 2.0 * axis - uv.y;
    } else {
        // fold every quadrant onto the one below-left of the axis point
        m = vec2(axis) - abs(uv - vec2(axis));
    }

    gl_FragColor = IMG_NORM_PIXEL(inputImage, clamp(m, 0.0, 1.0));
}
