// Block diagram: relief surface, side walls and floor, all built from grid indices.

struct Uniforms {
    view_proj: mat4x4<f32>,
    params: vec4<f32>, // exaggeration, base depth (m), cell (m), line width (px)
    grid: vec4<f32>,   // points east, points north, min height, max height (m)
    sun: vec4<f32>,    // direction towards the sun; w: 1 with the buildings' shadows
    opacity: array<vec4<f32>, 3>, // per layer slot (SLOTS), bottom first
    sun_view_proj: mat4x4<f32>, // the block seen from the sun, for the buildings' shadows
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
@group(0) @binding(11) var layer8: texture_2d<f32>;
// Where the sun reaches the surface (shadow.rs), and its spacing in grid points.
@group(0) @binding(12) var shadows: texture_2d<f32>;
const SHADOW_STEP: f32 = 2.0;
// Share of the sky each point sees (shadow.rs), on the shadows' grid.
@group(0) @binding(13) var sky_seen: texture_2d<f32>;
// The buildings seen from the sun (their depth), for their shadows.
@group(0) @binding(14) var sun_depth: texture_depth_2d;
@group(0) @binding(15) var sun_compare: sampler_comparison;

// Vertex kinds, see view3d.rs.
const SURFACE: u32 = 0u;
const WALL_TOP: u32 = 1u;
const WALL_BOTTOM: u32 = 2u;
const FLOOR: u32 = 3u;
// Light grey of the buildings, the base's walls and floor, and the bare relief.
const GREY: vec3<f32> = vec3(0.88);

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
    // On the walls: the relief's profile above (height, as `world.y`), the distance to the
    // wall's nearer end (m), and 1; zero elsewhere. For the block diagram's lines.
    @location(6) edge: vec4<f32>,
};

fn height(i: i32, j: i32) -> f32 {
    let n = vec2(i32(u.grid.x), i32(u.grid.y));
    return textureLoad(heights, clamp(vec2(i, j), vec2(0), n - 1), 0).r;
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
            // Grey under the layers, as the map is plain under a see-through photo.
            out.color = GREY;
            out.surface = 1.0;
            out.shaded = 1.0;
        }
        case FLOOR: {
            p.y = -u.params.y;
            out.normal = vec3(0.0, -1.0, 0.0);
            out.color = GREY;
        }
        default: {
            // Along the wall, from its west or south end.
            let west_east = kind_side.y < 2u;
            let along = select(f32(i), f32(j), west_east) * cell;
            let length = (select(u.grid.x, u.grid.y, west_east) - 1.0) * cell;
            out.edge = vec4(p.y, min(along, length - along), 0.0, 1.0);
            if kind_side.x == WALL_BOTTOM { p.y = -u.params.y; }
            let sides = array(vec3(-1.0, 0.0, 0.0), vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, -1.0));
            out.normal = sides[kind_side.y];
            out.color = GREY;
        }
    }
    out.pos = u.view_proj * vec4(p, 1.0);
    out.world = p;
    return out;
}

// A building corner's position, see buildings.rs: true heights over the exaggerated ground.
fn building_corner(grid: vec2<f32>, ground: f32, roof: f32) -> vec3<f32> {
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
    return vec3((grid.x - half.x) * u.params.z, y, (half.y - grid.y) * u.params.z);
}

// The buildings seen from the sun, depth only: what their shadows are read from.
@vertex
fn vs_building_sun(
    @location(0) grid: vec2<f32>,
    @location(1) ground: f32,
    @location(2) roof: f32,
) -> @builtin(position) vec4<f32> {
    return u.sun_view_proj * vec4(building_corner(grid, ground, roof), 1.0);
}

@vertex
fn vs_building(
    @location(0) grid: vec2<f32>,
    @location(1) ground: f32,
    @location(2) roof: f32,
    @location(3) normal: vec2<f32>,
) -> Out {
    let p = building_corner(grid, ground, roof);
    var out: Out;
    out.pos = u.view_proj * vec4(p, 1.0);
    out.world = p;
    out.uv = vec2(grid.x, u.grid.y - 1.0 - grid.y) / (u.grid.xy - 1.0);
    // Roofs face up, walls face out (north being -z); all plain light grey, like the base.
    let is_roof = all(normal == vec2(0.0));
    out.normal = select(vec3(normal.x, 0.0, -normal.y), vec3(0.0, 1.0, 0.0), is_roof);
    out.color = GREY;
    out.surface = 0.0;
    out.shaded = 1.0;
    return out;
}

// Lays one layer (premultiplied alpha) over the colour below, as the map does.
fn over(below: vec3<f32>, layer: vec4<f32>, opacity: f32) -> vec3<f32> {
    return below * (1.0 - layer.a * opacity) + layer.rgb * opacity;
}

// Share of the sun the buildings leave at `p`, `n` its normal: 0 in their shadow. Read a
// little off the surface, so that a building does not shade itself.
fn clear_of_buildings(p: vec3<f32>, n: vec3<f32>) -> f32 {
    if u.sun.w == 0.0 {
        return 1.0;
    }
    let s = u.sun_view_proj * vec4(p + n * 0.5, 1.0);
    let uv = vec2(0.5 + 0.5 * s.x, 0.5 - 0.5 * s.y);
    return textureSampleCompareLevel(sun_depth, sun_compare, uv, s.z);
}

// Position of `p` on the shadows' grid, 0..1.
fn on_mask(p: vec3<f32>) -> vec2<f32> {
    let half = (u.grid.xy - 1.0) * 0.5;
    let g = vec2(p.x / u.params.z + half.x, half.y - p.z / u.params.z);
    return (g / SHADOW_STEP + 0.5) / vec2<f32>(textureDimensions(shadows));
}

// Share of direct sunlight at `p`: the relief's shadows, cast every SHADOW_STEP points.
fn sunlit(p: vec3<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(shadows));
    let uv = on_mask(p);
    // Four samples around, blurring the grid's steps away.
    let d = 0.6 / size;
    return 0.25 * (textureSampleLevel(shadows, samp, uv + vec2(-d.x, -d.y), 0.0).r
        + textureSampleLevel(shadows, samp, uv + vec2(d.x, -d.y), 0.0).r
        + textureSampleLevel(shadows, samp, uv + vec2(-d.x, d.y), 0.0).r
        + textureSampleLevel(shadows, samp, uv + vec2(d.x, d.y), 0.0).r);
}

// Ink of the block diagram's lines on the walls: under the relief's profile, along the
// bottom, and at the corners, `u.params.w` pixels wide whatever the zoom.
fn ink(in: Out) -> f32 {
    // Distances from the profile, the bottom and the nearer end, in metres, then pixels.
    let d = vec3(in.edge.x - in.world.y, in.world.y + u.params.y, in.edge.y);
    let px = d / max(fwidth(d), vec3(1e-6));
    let nearest = min(px.x, min(px.y, px.z));
    return (1.0 - smoothstep(u.params.w - 1.0, u.params.w, nearest)) * in.edge.w;
}

// Share of the light coming from the whole sky rather than the sun (linear).
const AMBIENT: f32 = 0.15;
// That light from above, bluish, and bounced up from the ground, warm.
const SKY: vec3<f32> = vec3(0.80, 0.88, 1.0);
const BOUNCE: vec3<f32> = vec3(0.95, 0.88, 0.78);
// Light the relief sends back where it hides the sky, as a share of the sky's.
const RELIEF_GLOW: f32 = 0.3;

// The sun's colour (linear): orange on the horizon, white from 30° up, as the sky's glow in
// view3d.rs `paint_sky`.
fn sunlight() -> vec3<f32> {
    let day = clamp(degrees(asin(u.sun.y)) / 30.0, 0.0, 1.0);
    return mix(vec3(1.0, 0.6, 0.32), vec3(1.0, 0.97, 0.93), day);
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    // Every slot is sampled (empty ones are transparent), keeping control flow uniform.
    let line = ink(in);
    // The walls darken a little towards the bottom, as on a drawn block.
    let down = clamp((in.edge.x - in.world.y) / (in.edge.x + u.params.y), 0.0, 1.0) * in.edge.w;
    var c = in.color;
    c = over(c, textureSample(layer0, samp, in.uv), u.opacity[0].x);
    c = over(c, textureSample(layer1, samp, in.uv), u.opacity[0].y);
    c = over(c, textureSample(layer2, samp, in.uv), u.opacity[0].z);
    c = over(c, textureSample(layer3, samp, in.uv), u.opacity[0].w);
    c = over(c, textureSample(layer4, samp, in.uv), u.opacity[1].x);
    c = over(c, textureSample(layer5, samp, in.uv), u.opacity[1].y);
    c = over(c, textureSample(layer6, samp, in.uv), u.opacity[1].z);
    c = over(c, textureSample(layer7, samp, in.uv), u.opacity[1].w);
    c = over(c, textureSample(layer8, samp, in.uv), u.opacity[2].x);
    let color = mix(in.color, c, in.surface);

    // The 3D light shades the layers; none of them carries baked shading. It is summed on
    // linear colours, then coded back for display, so that shade stays clear.
    let n = normalize(in.normal);
    let shade = sunlit(in.world) * clear_of_buildings(in.world, n);
    let direct = max(dot(n, u.sun.xyz), 0.0) * mix(1.0, shade, in.shaded);
    // The sky's light, from what of it each point sees; where the relief hides it, the
    // relief's own light, dimmer and warm. The sun's light is left alone.
    let seen = mix(1.0, textureSampleLevel(sky_seen, samp, on_mask(in.world), 0.0).r, in.shaded);
    let skylight = mix(BOUNCE, SKY, 0.5 + 0.5 * n.y);
    let ambient = AMBIENT * mix(RELIEF_GLOW * BOUNCE, skylight, seen);
    let light = ambient + (1.0 - AMBIENT) * sunlight() * direct;
    let lit = pow(pow(color, vec3(2.2)) * light, vec3(1.0 / 2.2)) * (1.0 - 0.2 * down);
    // Light ink: a faint line, as in pencil.
    return vec4(mix(lit, vec3(0.2), 0.15 * line), 1.0);
}
