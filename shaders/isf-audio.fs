/*{
    "DESCRIPTION": "Audio-reactive scope: brightens the image by the FFT under each column and overlays the waveform.",
    "CREDIT": "vidiotic",
    "ISFVSN": "2.0",
    "CATEGORIES": ["Audio"],
    "INPUTS": [
        { "NAME": "gain", "TYPE": "float", "MIN": 0.0, "MAX": 4.0, "DEFAULT": 1.5, "LABEL": "Reactivity" },
        { "NAME": "line", "TYPE": "color", "DEFAULT": [0.2, 1.0, 0.6, 1.0], "LABEL": "Scope colour" },
        { "NAME": "spectrum", "TYPE": "audioFFT" },
        { "NAME": "wave", "TYPE": "audio" }
    ]
}*/

void main() {
    vec2 uv = isf_FragNormCoord;
    vec4 src = IMG_THIS_NORM_PIXEL(inputImage);

    // FFT magnitude under this column drives a brightness lift.
    float fft = IMG_NORM_PIXEL(spectrum, vec2(uv.x, 0.0)).r;
    vec3 col = src.rgb * (1.0 + fft * gain);

    // Overlay the waveform as a thin horizontal scope line (0..1, centred 0.5).
    float w = IMG_NORM_PIXEL(wave, vec2(uv.x, 0.0)).r;
    float d = abs(uv.y - w);
    col = mix(col, line.rgb, smoothstep(0.02, 0.0, d));

    gl_FragColor = vec4(col, src.a);
}
