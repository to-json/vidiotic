#version 450
// ---- vidiotic preamble (auto-injected, user source appended below) ----
layout(location = 0) in vec2 fragTexCoord;   // (0,0)=bottom-left (GL convention)
layout(location = 0) out vec4 FragColor;

layout(set = 0, binding = 0) uniform Globals {
    vec2  resolution;    // std140 offset   0 : output size px
    vec2  mouse;         // offset   8 : normalized 0..1
    float time;          // offset  16 : seconds since start
    float lvl;           // offset  20
    float beat;          // offset  24 : continuous beats
    float bar_phase;     // offset  28 : 0..1
    float phrase_phase;  // offset  32 : 0..1
    float bpm;           // offset  36
    int   videoMode;     // offset  40 : 0=rgba 1=ycocg 2=ycocg+alpha 3=alpha-only
    float _pad0;         // offset  44
    vec4  uFreqs[6];     // offset  48, stride 16 : 21 bands packed; [21..23]=0
};                       // std140 size 144

layout(set = 1, binding = 0) uniform texture2D videoTex;
layout(set = 1, binding = 1) uniform sampler   videoSmp;
layout(set = 1, binding = 2) uniform texture2D alphaTex;   // 1x1 white R8 dummy unless HapM
layout(set = 1, binding = 3) uniform texture2D audioTex;   // Shadertoy audio: 512x2 R8
layout(set = 1, binding = 4) uniform sampler   audioSmp;   // clamp + linear

float fftBand(int i) { return uFreqs[i >> 2][i & 3]; }

// Shadertoy audio convention: a 512x2 texture on iChannel0. Row 0 (y=0.25) is
// the FFT spectrum (linear frequency, 0..1); row 1 (y=0.75) is the waveform
// (0..1, centered on 0.5). `texture(iChannel0, vec2(x, 0.25)).x` works directly;
// fftAt()/waveAt() are shorthands.
#define iChannel0 sampler2D(audioTex, audioSmp)
float fftAt(float x)  { return texture(sampler2D(audioTex, audioSmp), vec2(x, 0.25)).x; }
float waveAt(float x) { return texture(sampler2D(audioTex, audioSmp), vec2(x, 0.75)).x; }

vec4 video(vec2 uv) {
    vec2 st = vec2(uv.x, 1.0 - uv.y);                 // video rows stored top-down
    vec4 c  = texture(sampler2D(videoTex, videoSmp), st);
    if (videoMode == 0) return c;
    if (videoMode == 3) return vec4(1.0, 1.0, 1.0, c.r);
    // scaled YCoCg-DXT5 unswizzle (van Waveren & Castano): DXT5 holds (Co, Cg, scale, Y)
    float scale = (c.z * (255.0 / 8.0)) + 1.0;
    float Co = (c.x - 0.501960784) / scale;
    float Cg = (c.y - 0.501960784) / scale;
    float Y  = c.w;
    vec3 rgb = vec3(Y + Co - Cg, Y + Cg, Y - Co - Cg);
    float a  = (videoMode == 2) ? texture(sampler2D(alphaTex, videoSmp), st).r : 1.0;
    return vec4(clamp(rgb, 0.0, 1.0), a);
}

#define mousePos    mouse
#define iTime       time
#define iResolution vec3(resolution, 1.0)
// ---- end preamble ----
