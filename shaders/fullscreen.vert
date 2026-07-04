#version 450
// Fixed fullscreen-triangle vertex shader, GLSL twin of fullscreen.wgsl. Paired
// with GLSL fragment shaders so the location-0 varying's interpolation/sampling
// (perspective, unspecified) matches naga's GLSL convention on both stages.
layout(location = 0) out vec2 fragTexCoord;
void main() {
    // vertex 0->(0,0) 1->(2,0) 2->(0,2): an oversized triangle covering the view.
    vec2 p = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    fragTexCoord = p;                     // 0..1 across the visible region
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
