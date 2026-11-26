// Block diagram: relief surface, side walls and floor, all built from grid indices.

struct Uniforms {
    view_proj: mat4x4<f32>,
    params: vec4<f32>, // exaggeration, base depth (m), cell (m), unused
    grid: vec4<f32>,   // points east, points north, min height, max height (m)
    sun: vec4<f32>,    // direction towards the sun
    opacity: array<vec4<f32>, 2>, // per layer slot, bottom first
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var heights: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
// Layer images draped on the surface, bottom first (SLOTS in view3d.rs).
@group(0) @binding(3) var layer0: texture_2d<f32>;
@group(0) @binding(4) var layer1: texture_2d<f32>;
@group(0) @binding(5) var layer2: texture_2d<f32>;
@group(0) @binding(6) var layer3: texture_2d<f32>;
@group(0) @binding(7) var layer4: texture_2d<f32>;
@group(0) @binding(8) var layer5: texture_2d<f32>;

// Vertex kinds, see view3d.rs.
const SURFACE: u32 = 0u;
const WALL_TOP: u32 = 1u;
const WALL_BOTTOM: u32 = 2u;
const FLOOR: u32 = 3u;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
    // Position in the layer images: 0..1 west to east, north to south.
    @location(2) uv: vec2<f32>,
    // 1 on the surface, which takes the layers; 0 on walls and floor.
    @location(3) surface: f32,
};

fn height(i: i32, j: i32) -> f32 {
    let n = vec2(i32(u.grid.x), i32(u.grid.y));
    return textureLoad(heights, clamp(vec2(i, j), vec2(0), n - 1), 0).r;
}

// Hypsometric tint by absolute altitude.
fn tint(h: f32) -> vec3<f32> {
    let low = vec3(0.45, 0.58, 0.35);
    let mid = vec3(0.72, 0.66, 0.48);
    let high = vec3(0.55, 0.50, 0.46);
    let snow = vec3(0.95, 0.95, 0.97);
    if h < 1000.0 { return mix(low, mid, h / 1000.0); }
    if h < 2200.0 { return mix(mid, high, (h - 1000.0) / 1200.0); }
    return mix(high, snow, clamp((h - 2200.0) / 800.0, 0.0, 1.0));
}

@vertex
fn vs_main(@location(0) grid: vec2<u32>, @location(1) kind_side: vec2<u32>) -> Out {
    let exag = u.params.x;
    let cell = u.params.z;
    let half = (u.grid.xy - 1.0) * 0.5;
    let i = i32(grid.x);
    let j = i32(grid.y);
    let h = height(i, j);

    // x east, y up, z south; heights relative to the block's lowest point.
    var p = vec3((f32(i) - half.x) * cell, (h - u.grid.z) * exag, (half.y - f32(j)) * cell);
    var out: Out;
    out.uv = vec2(f32(i), u.grid.y - 1.0 - f32(j)) / (u.grid.xy - 1.0);
    switch kind_side.x {
        case SURFACE: {
            let dx = (height(i + 1, j) - height(i - 1, j)) * exag / (2.0 * cell);
            let dz = (height(i, j - 1) - height(i, j + 1)) * exag / (2.0 * cell);
            out.normal = normalize(vec3(-dx, 1.0, -dz));
            out.color = tint(h);
            out.surface = 1.0;
        }
        case FLOOR: {
            p.y = -u.params.y;
            out.normal = vec3(0.0, -1.0, 0.0);
            out.color = vec3(0.3, 0.27, 0.24);
        }
        default: {
            if kind_side.x == WALL_BOTTOM { p.y = -u.params.y; }
            let sides = array(vec3(-1.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, -1.0));
            out.normal = sides[kind_side.y];
            // Ground darkens with depth.
            out.color = mix(vec3(0.62, 0.48, 0.34), vec3(0.40, 0.30, 0.22), clamp(-p.y / u.params.y, 0.0, 1.0));
        }
    }
    out.pos = u.view_proj * vec4(p, 1.0);
    return out;
}

// Lays one layer over the colour below, as the map does.
fn over(below: vec3<f32>, layer: vec4<f32>, opacity: f32) -> vec3<f32> {
    return mix(below, layer.rgb, layer.a * opacity);
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    // Every slot is sampled (empty ones are transparent), keeping control flow uniform.
    var c = in.color;
    c = over(c, textureSample(layer0, samp, in.uv), u.opacity[0].x);
    c = over(c, textureSample(layer1, samp, in.uv), u.opacity[0].y);
    c = over(c, textureSample(layer2, samp, in.uv), u.opacity[0].z);
    c = over(c, textureSample(layer3, samp, in.uv), u.opacity[0].w);
    c = over(c, textureSample(layer4, samp, in.uv), u.opacity[1].x);
    c = over(c, textureSample(layer5, samp, in.uv), u.opacity[1].y);
    let color = mix(in.color, c, in.surface);

    // The 3D light shades the layers; none of them carries baked shading.
    let light = 0.35 + 0.65 * max(dot(normalize(in.normal), u.sun.xyz), 0.0);
    return vec4(color * light, 1.0);
}
