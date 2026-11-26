// Block diagram: relief surface, side walls and floor, all built from grid indices.

struct Uniforms {
    view_proj: mat4x4<f32>,
    params: vec4<f32>, // exaggeration, base depth (m), cell (m), unused
    grid: vec4<f32>,   // points east, points north, min height, max height (m)
    sun: vec4<f32>,    // direction towards the sun
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var heights: texture_2d<f32>;

// Vertex kinds, see view3d.rs.
const SURFACE: u32 = 0u;
const WALL_TOP: u32 = 1u;
const WALL_BOTTOM: u32 = 2u;
const FLOOR: u32 = 3u;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
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
    switch kind_side.x {
        case SURFACE: {
            let dx = (height(i + 1, j) - height(i - 1, j)) * exag / (2.0 * cell);
            let dz = (height(i, j - 1) - height(i, j + 1)) * exag / (2.0 * cell);
            out.normal = normalize(vec3(-dx, 1.0, -dz));
            out.color = tint(h);
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

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let light = 0.35 + 0.65 * max(dot(normalize(in.normal), u.sun.xyz), 0.0);
    return vec4(in.color * light, 1.0);
}
