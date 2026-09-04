#import bevy_pbr::forward_io::VertexOutput

struct LoadParams {
    iterations: u32,
    padding: vec3<u32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: LoadParams;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var acc = in.uv * vec2<f32>(1.17, 0.83) + vec2<f32>(0.23, 0.61);
    // Runtime uniform trip count and data-dependent output retain fragment work.
    // No claim is made that every seed avoids numerical fixed points.
    for (var i = 0u; i < params.iterations; i = i + 1u) {
        acc = acc * 1.00001 + vec2<f32>(sin(acc.y), cos(acc.x)) * 0.017;
    }
    let grid = f32((u32(in.uv.x * 32.0) + u32(in.uv.y * 24.0)) % 2u);
    let detail = 0.03 * sin(acc.x + acc.y);
    return vec4<f32>(vec3<f32>(0.05, 0.10, 0.14) + grid * 0.08 + detail, 1.0);
}
