// Block diagram: relief surface, side walls and floor, all built from grid indices.

struct Uniforms {
    view_proj: mat4x4<f32>,
    params: vec4<f32>, // exaggeration, base depth (m), cell (m), unused
    grid: vec4<f32>,   // points east, points north, min height, max height (m)
    sun: vec4<f32>,    // direction towards the sun
    opacity: array<vec4<f32>, 2>, // per layer slot (SLOTS), bottom first
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
@group(0) @binding(9) var layer6: texture_2d<f32>;
@group(0) @binding(10) var layer7: texture_2d<f32>;
// Where the sun reaches the surface (shadow.rs), and its spacing in grid points.
@group(0) @binding(11) var shadows: texture_2d<f32>;
const SHADOW_STEP: f32 = 2.0;

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
    // Position in metres, x east, y up, z south: for the shadows.
    @location(4) world: vec3<f32>,
    // 1 where the relief's shadows fall (surface, buildings); 0 on the block's walls
    // and floor, whose sunny sides face away from the relief.
    @location(5) shaded: f32,
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
            out.shaded = 1.0;
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
    out.world = p;
    return out;
}

// Building corners, see buildings.rs: true heights over the exaggerated ground.
@vertex
fn vs_building(
    @location(0) grid: vec2<f32>,
    @location(1) ground: f32,
    @location(2) roof: f32,
    @location(3) normal: vec2<f32>,
) -> Out {
    let exag = u.params.x;
    let half = (u.grid.xy - 1.0) * 0.5;
    var y = (ground - u.grid.z) * exag + roof;
    if roof == 0.0 {
        // A wall's foot goes down to the lowest terrain point around it.
        let i = i32(floor(grid.x));
        let j = i32(floor(grid.y));
        let t = min(min(height(i, j), height(i + 1, j)), min(height(i, j + 1), height(i + 1, j + 1)));
        y = (min(t, ground) - u.grid.z) * exag;
    }
    let p = vec3((grid.x - half.x) * u.params.z, y, (half.y - grid.y) * u.params.z);

    var out: Out;
    out.pos = u.view_proj * vec4(p, 1.0);
    out.world = p;
    out.uv = vec2(grid.x, u.grid.y - 1.0 - grid.y) / (u.grid.xy - 1.0);
    // Roofs face up, walls face out (north being -z); all plain light grey.
    let is_roof = all(normal == vec2(0.0));
    out.normal = select(vec3(normal.x, 0.0, -normal.y), vec3(0.0, 1.0, 0.0), is_roof);
    out.color = vec3(0.88);
    out.surface = 0.0;
    out.shaded = 1.0;
    return out;
}

// Lays one layer (premultiplied alpha) over the colour below, as the map does.
fn over(below: vec3<f32>, layer: vec4<f32>, opacity: f32) -> vec3<f32> {
    return below * (1.0 - layer.a * opacity) + layer.rgb * opacity;
}

// Share of direct sunlight at `p`: the relief's shadows, cast every SHADOW_STEP points.
fn sunlit(p: vec3<f32>) -> f32 {
    let half = (u.grid.xy - 1.0) * 0.5;
    let g = vec2(p.x / u.params.z + half.x, half.y - p.z / u.params.z);
    let size = vec2<f32>(textureDimensions(shadows));
    let uv = (g / SHADOW_STEP + 0.5) / size;
    // Four samples around, blurring the grid's steps away.
    let d = 0.6 / size;
    return 0.25 * (textureSampleLevel(shadows, samp, uv + vec2(-d.x, -d.y), 0.0).r
        + textureSampleLevel(shadows, samp, uv + vec2(d.x, -d.y), 0.0).r
        + textureSampleLevel(shadows, samp, uv + vec2(-d.x, d.y), 0.0).r
        + textureSampleLevel(shadows, samp, uv + vec2(d.x, d.y), 0.0).r);
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
    c = over(c, textureSample(layer6, samp, in.uv), u.opacity[1].z);
    c = over(c, textureSample(layer7, samp, in.uv), u.opacity[1].w);
    let color = mix(in.color, c, in.surface);

    // The 3D light shades the layers; none of them carries baked shading.
    let direct = max(dot(normalize(in.normal), u.sun.xyz), 0.0) * mix(1.0, sunlit(in.world), in.shaded);
    let light = 0.35 + 0.65 * direct;
    return vec4(color * light, 1.0);
}
