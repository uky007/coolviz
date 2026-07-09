// TOKYO STORM: storm sky, wet ground, PLATEAU LOD1 buildings.
// Local frame: meters, x = east, y = up, z = south.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,   // xyz camera (m), w = time (s)
    sun_dir: vec4<f32>,
    viewport: vec4<f32>,
    params: vec4<f32>,    // z = exposure, w = res_scale
    layers: vec4<f32>,
    layers2: vec4<f32>,
};

struct RainMap {
    a: vec4<f32>, // x site_lon, y site_lat, z deg/m east, w deg/m south
    b: vec4<f32>, // x lon_w, y lat_n, z 1/(lon_e-lon_w), w 1/(lat_n-lat_s)
    c: vec4<f32>, // x demo flag, y live max level (0..1), z wind (m/s), w unused
};

@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var<uniform> RM: RainMap;
@group(0) @binding(2) var rain_tex: texture_2d<f32>;
@group(0) @binding(3) var rain_samp: sampler;

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2(123.34, 456.21));
    q += dot(q, q + 45.32);
    return fract(q.x * q.y);
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash21(i), hash21(i + vec2(1.0, 0.0)), u.x),
        mix(hash21(i + vec2(0.0, 1.0)), hash21(i + vec2(1.0, 1.0)), u.x),
        u.y,
    );
}

fn demo_rain(xz: vec2<f32>, t: f32) -> f32 {
    let c = vec2(-400.0, -200.0);
    let p = xz - c;
    let ang = t * 0.018;
    let rot = mat2x2<f32>(cos(ang), -sin(ang), sin(ang), cos(ang));
    var q = rot * p * 0.0014 + vec2(t * 0.006, 0.0);
    var v = 0.0;
    var a = 0.55;
    for (var i = 0; i < 4; i++) {
        v += a * vnoise(q * 3.0);
        q = mat2x2<f32>(1.6, 1.2, -1.2, 1.6) * q + vec2(0.31, -0.17);
        a *= 0.55;
    }
    let band = exp(-abs(dot(p, normalize(vec2(0.6, 0.8))) + 300.0 * sin(t * 0.012)) * 0.0014);
    return clamp(v * 1.15 + band * 0.65 - 0.55, 0.0, 1.0);
}

fn rain_level(xz: vec2<f32>, t: f32) -> f32 {
    if RM.c.x > 0.5 {
        return demo_rain(xz, t);
    }
    let lon = RM.a.x + xz.x * RM.a.z;
    let lat = RM.a.y - xz.y * RM.a.w;
    let u = (lon - RM.b.x) * RM.b.z;
    let v = (RM.b.y - lat) * RM.b.w;
    if u < 0.0 || u > 1.0 || v < 0.0 || v > 1.0 {
        return 0.0;
    }
    return textureSampleLevel(rain_tex, rain_samp, vec2(u, v), 0.0).r;
}

fn fog_mix(col: vec3<f32>, dist: f32, rain: f32) -> vec3<f32> {
    let fog_col = vec3(0.010, 0.015, 0.026);
    let f = 1.0 - exp(-dist * (0.00009 + rain * 0.00011));
    return mix(col, fog_col, clamp(f, 0.0, 1.0));
}

// ---------------- sky ----------------

struct FullOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vi: u32) -> FullOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -3.0), vec2(3.0, 1.0), vec2(-1.0, 1.0));
    let xy = p[vi];
    // z = w so ndc depth lands at 1.0 (far plane).
    return FullOut(vec4(xy, 0.99999, 1.0), xy);
}

@fragment
fn fs_sky(in: FullOut) -> @location(0) vec4<f32> {
    let far = G.inv_view_proj * vec4(in.ndc, 0.6, 1.0);
    let rd = normalize(far.xyz / far.w - G.cam_pos.xyz);
    let t = G.cam_pos.w;

    let up = clamp(rd.y, 0.0, 1.0);
    var col = mix(vec3(0.030, 0.042, 0.062), vec3(0.004, 0.006, 0.012), pow(up, 0.55));

    // Rolling storm deck.
    if rd.y > 0.005 {
        let p = rd.xz / (rd.y + 0.12);
        var q = p * 1.7 + vec2(t * 0.008, t * 0.003);
        var v = 0.0;
        var a = 0.5;
        for (var i = 0; i < 4; i++) {
            v += a * vnoise(q);
            q = q * 2.1 + vec2(5.2, 1.3);
            a *= 0.5;
        }
        let deck = smoothstep(0.35, 0.85, v);
        col = mix(col, vec3(0.008, 0.011, 0.019), deck * 0.92);
        // Faint underlit cloud rims from the city.
        col += vec3(0.030, 0.026, 0.020) * pow(1.0 - up, 3.0) * (0.4 + 0.6 * v);
    } else {
        col = vec3(0.010, 0.014, 0.022);
    }
    // Cold city glow at the horizon.
    col += vec3(0.045, 0.050, 0.058) * pow(1.0 - abs(rd.y), 6.0) * 0.5;
    return vec4(col, 1.0);
}

// ---------------- ground ----------------

struct GroundOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_ground(@builtin(vertex_index) vi: u32) -> GroundOut {
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    let ext = 20000.0;
    let w = vec3(corners[vi].x * ext, 0.0, corners[vi].y * ext);
    return GroundOut(G.view_proj * vec4(w, 1.0), w);
}

@fragment
fn fs_ground(in: GroundOut) -> @location(0) vec4<f32> {
    let t = G.cam_pos.w;
    let rain = rain_level(in.world.xz, t);
    var col = vec3(0.012, 0.015, 0.020);

    // Very faint survey grid + block-scale tonal variation.
    let g = abs(fract(in.world.xz / 100.0 + 0.5) - 0.5) * 100.0;
    let line = 1.0 - smoothstep(0.0, 1.6, min(g.x, g.y));
    col += vec3(0.05, 0.14, 0.20) * line * 0.05;
    col *= 0.75 + 0.5 * vnoise(in.world.xz * 0.004 + 11.0);

    // Wet sheen: shimmering reflection of the city, scaled by live rain.
    let sparkle = vnoise(in.world.xz * 0.9 + vec2(t * 2.0, -t * 1.7))
        * vnoise(in.world.xz * 0.23 + vec2(-t * 0.4, t * 0.3));
    col += vec3(0.10, 0.16, 0.22) * rain * (0.12 + 0.5 * sparkle) * 0.6;
    col += vec3(0.30, 0.20, 0.10) * rain * pow(sparkle, 3.0) * 0.35; // sodium-lamp glints

    let dist = length(in.world - G.cam_pos.xyz);
    return vec4(fog_mix(col, dist, rain), 1.0);
}

// ---------------- flood water (shallow-water sim surface) ----------------

struct SimMap {
    a: vec4<f32>, // x,y origin, z cell, w N
    b: vec4<f32>,
};

@group(0) @binding(4) var<uniform> WSM: SimMap;
@group(0) @binding(5) var water_tex: texture_2d<f32>;

const WGRID: u32 = 512u;

struct WaterOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) hv: vec2<f32>, // depth, speed
};

@vertex
fn vs_water(@builtin(vertex_index) vi: u32) -> WaterOut {
    let gx = vi % WGRID;
    let gy = vi / WGRID;
    let uv = vec2(f32(gx), f32(gy)) / f32(WGRID - 1u);
    let ext = WSM.a.z * WSM.a.w;
    let xz = WSM.a.xy + uv * ext;
    // Exact texel read: filtering would blend water heights across the
    // building-stamped terrain cliffs and raise phantom water towers.
    let n_tex = i32(WSM.a.w);
    let tc = vec2<i32>(
        min(i32(gx), n_tex - 1),
        min(i32(gy), n_tex - 1),
    );
    let s = textureLoad(water_tex, tc, 0);
    var y = s.r + s.g + 0.05;
    if s.g < 0.008 {
        y = -80.0; // dry: sink the vertex out of sight
    }
    var out: WaterOut;
    out.world = vec3(xz.x, y, xz.y);
    out.pos = G.view_proj * vec4(out.world, 1.0);
    out.hv = vec2(s.g, s.b);
    return out;
}

@fragment
fn fs_water(in: WaterOut) -> @location(0) vec4<f32> {
    let t = G.cam_pos.w;
    // Muddy flood water: sediment brown peaks around 1.5 m, then deep
    // water swallows the light and goes dark.
    var col = mix(
        vec3(0.016, 0.021, 0.028),
        vec3(0.058, 0.045, 0.031),
        clamp(in.hv.x * 0.8, 0.0, 1.0),
    );
    col *= 1.0 / (1.0 + max(in.hv.x - 1.8, 0.0) * 0.9);
    let dist = length(in.world - G.cam_pos.xyz);
    let lod = 1.0 / (1.0 + dist * 0.0022);
    // City lights smeared on the moving surface (fade far off: unresolved
    // noise otherwise averages into a milky sheen).
    let streak = vnoise(in.world.xz * 0.5 + vec2(t * 1.3, -t * 1.0))
        * vnoise(in.world.xz * 0.13 + vec2(-t * 0.25, t * 0.2));
    col += vec3(0.34, 0.25, 0.12) * pow(streak, 2.0) * 0.55 * lod;
    col += vec3(0.10, 0.14, 0.20) * streak * 0.22 * lod;
    // Patchy turbulent foam where deep water runs fast.
    let fmask = vnoise(in.world.xz * 0.9 + vec2(t * 0.5, -t * 0.3));
    let foam = smoothstep(2.0, 3.8, in.hv.y) * smoothstep(0.06, 0.30, in.hv.x)
        * smoothstep(0.45, 0.75, fmask);
    col = mix(col, vec3(0.33, 0.34, 0.33), clamp(foam, 0.0, 0.30) * lod);

    // Water is a grazing-angle mirror of the dark sky: keep fog weak so
    // distant sheets stay dark glass instead of washing out.
    return vec4(fog_mix(col, dist, 0.22), 1.0);
}

// ---------------- roads ----------------

struct RoadOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) klass: f32,
};

@vertex
fn vs_road(@location(0) p: vec4<f32>) -> RoadOut {
    return RoadOut(G.view_proj * vec4(p.xyz, 1.0), p.xyz, p.w);
}

@fragment
fn fs_road(in: RoadOut) -> @location(0) vec4<f32> {
    let t = G.cam_pos.w;
    let rain = rain_level(in.world.xz, t);

    // Asphalt, faintly warmed by street lighting.
    var col = vec3(0.020, 0.021, 0.024);
    col += vec3(0.055, 0.042, 0.024) * (1.0 - in.klass * 0.12);

    // Wet road: streaky reflections of the sodium lights and windows.
    let streak = vnoise(in.world.xz * vec2(0.5, 0.5) + vec2(t * 1.4, -t * 1.1))
        * vnoise(in.world.xz * 0.16 + vec2(-t * 0.3, t * 0.22));
    col += vec3(0.42, 0.30, 0.14) * rain * pow(streak, 2.0) * 0.85;
    col += vec3(0.12, 0.17, 0.24) * rain * streak * 0.4;

    let dist = length(in.world - G.cam_pos.xyz);
    return vec4(fog_mix(col, dist, rain), 1.0);
}

// ---------------- buildings (PLATEAU photo textures) ----------------

@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

struct BldgOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) hash: f32,
};

@vertex
fn vs_bldg(
    @location(0) p: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) hash: f32,
) -> BldgOut {
    return BldgOut(G.view_proj * vec4(p, 1.0), p, uv, hash);
}

@fragment
fn fs_bldg(in: BldgOut) -> @location(0) vec4<f32> {
    let t = G.cam_pos.w;
    var n = normalize(cross(dpdx(in.world), dpdy(in.world)));
    let view = normalize(G.cam_pos.xyz - in.world);
    n = faceForward(-n, -view, n);

    let rain = rain_level(in.world.xz, t);

    // Facade photo, graded into a rainy dusk: slightly desaturated, cooled.
    let tex = textureSample(atlas_tex, atlas_samp, in.uv).rgb;
    let lum = dot(tex, vec3(0.30, 0.59, 0.11));
    // Contrast curve keeps highlights while crushing mids into the dusk.
    let base = pow(
        mix(tex, vec3(lum), 0.22) * vec3(0.72, 0.82, 1.02),
        vec3(1.4),
    );

    var col = base
        * (0.060
            + 0.150 * clamp(n.y, 0.0, 1.0)
            + 0.080 * clamp(dot(n, normalize(vec3(-0.45, 0.35, -0.60))), 0.0, 1.0)
            + 0.040 * clamp(in.world.y / 150.0, 0.0, 1.0));

    // Lit windows glow through the dusk facade.
    if abs(n.y) < 0.35 {
        let perp = normalize(vec2(-n.z, n.x));
        let u = dot(in.world.xz, perp) / 3.4;
        let v = in.world.y / 3.6;
        let cell = floor(vec2(u, v));
        let f = fract(vec2(u, v));
        let inside = step(0.18, f.x) * step(f.x, 0.86) * step(0.25, f.y) * step(f.y, 0.80);
        // Offices light up by floor: some floors work late, most are dark.
        let floor_lit = step(0.58, hash21(vec2(cell.y * 3.1, in.hash * 173.0)));
        let on = floor_lit
            * step(0.35, hash21(cell + vec2(in.hash * 251.0, in.hash * 97.0)));
        let flicker = 0.85 + 0.15 * sin(t * 1.3 + hash21(cell) * 40.0);
        let warm = mix(vec3(1.0, 0.72, 0.40), vec3(0.75, 0.85, 1.0), step(0.8, hash21(cell + 7.7)));
        // Windows brighten where the photo already has glazing (darker areas).
        col += warm * inside * on * flicker * (0.55 + 0.8 * (1.0 - lum));
    }

    // Cyan rim against the fog.
    let fres = pow(1.0 - clamp(dot(n, view), 0.0, 1.0), 4.0);
    col += vec3(0.10, 0.30, 0.42) * fres * 0.12;

    // Wet facades pick up a faint sheen.
    col += vec3(0.03, 0.05, 0.07) * rain * pow(1.0 - abs(n.y), 2.0) * 0.5;

    let dist = length(in.world - G.cam_pos.xyz);
    return vec4(fog_mix(col, dist, rain), 1.0);
}
