// Motion vector resolve shader: copies the content region from a full-resolution
// RG16Float prepass texture to a content-sized RG16Float color attachment.
//
// Uses a fullscreen triangle (3 vertices, no vertex buffer).
// Nearest-neighbor sampling (textureLoad) preserves exact motion values.

@group(0) @binding(0) var src_motion: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    // Oversized fullscreen triangle — rasterizer clips to viewport.
    let x = f32(i32(vi & 1u)) * 4.0 - 1.0;
    let y = f32(i32(vi >> 1u)) * 4.0 - 1.0;
    return VertexOutput(vec4<f32>(x, y, 0.0, 1.0));
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(pos.xy);
    return textureLoad(src_motion, coord, 0);
}
