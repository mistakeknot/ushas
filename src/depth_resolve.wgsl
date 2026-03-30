// Depth resolve shader: copies the content region from a full-resolution
// Depth32Float prepass texture to a content-sized Depth32Float texture.
//
// Uses a fullscreen triangle (3 vertices, no vertex buffer) with fragment
// depth output. Nearest-neighbor sampling (textureLoad) preserves exact
// depth values — no interpolation artifacts.

@group(0) @binding(0) var src_depth: texture_depth_2d;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    // Oversized fullscreen triangle — rasterizer clips to viewport.
    let x = f32(i32(vi & 1u)) * 4.0 - 1.0;
    let y = f32(i32(vi >> 1u)) * 4.0 - 1.0;
    return VertexOutput(vec4<f32>(x, y, 0.0, 1.0));
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @builtin(frag_depth) f32 {
    // textureLoad at integer coords = nearest-neighbor (no sampler).
    // pos.xy maps 1:1 because content is in the top-left corner of the source.
    let coord = vec2<i32>(pos.xy);
    return textureLoad(src_depth, coord, 0);
}
