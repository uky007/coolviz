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
    let fog_col = vec3(0.016, 0.024, 0.038);
    let f = 1.0 - exp(-dist * (0.00010 + rain * 0.00016));
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

    // Faint 100 m survey grid — mission-control floor.
    let g = abs(fract(in.world.xz / 100.0 + 0.5) - 0.5) * 100.0;
    let line = 1.0 - smoothstep(0.0, 1.6, min(g.x, g.y));
    col += vec3(0.05, 0.14, 0.20) * line * 0.16;

    // Wet sheen: shimmering reflection of the city, scaled by live rain.
    let sparkle = vnoise(in.world.xz * 0.9 + vec2(t * 2.0, -t * 1.7))
        * vnoise(in.world.xz * 0.23 + vec2(-t * 0.4, t * 0.3));
    col += vec3(0.10, 0.16, 0.22) * rain * (0.12 + 0.5 * sparkle) * 0.6;
    col += vec3(0.30, 0.20, 0.10) * rain * pow(sparkle, 3.0) * 0.35; // sodium-lamp glints

    let dist = length(in.world - G.cam_pos.xyz);
    return vec4(fog_mix(col, dist, rain), 1.0);
}

// ---------------- buildings ----------------

struct BldgOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) hash: f32,
};

@vertex
fn vs_bldg(@location(0) p: vec4<f32>) -> BldgOut {
    return BldgOut(G.view_proj * vec4(p.xyz, 1.0), p.xyz, p.w);
}

@fragment
fn fs_bldg(in: BldgOut) -> @location(0) vec4<f32> {
    let t = G.cam_pos.w;
    var n = normalize(cross(dpdx(in.world), dpdy(in.world)));
    let view = normalize(G.cam_pos.xyz - in.world);
    n = faceForward(-n, -view, n);

    let rain = rain_level(in.world.xz, t);

    // Dark monolith body with height gradient.
    var col = vec3(0.013, 0.017, 0.025) * (0.5 + 0.5 * clamp(in.world.y / 120.0, 0.0, 1.0));
    // Cool key light from the storm deck.
    col += vec3(0.030, 0.045, 0.062) * clamp(n.y, 0.0, 1.0) * 0.45;

    // Windows on walls.
    if abs(n.y) < 0.35 {
        let perp = normalize(vec2(-n.z, n.x));
        let u = dot(in.world.xz, perp) / 3.4;
        let v = in.world.y / 3.6;
        let cell = floor(vec2(u, v));
        let f = fract(vec2(u, v));
        let inside = step(0.18, f.x) * step(f.x, 0.86) * step(0.25, f.y) * step(f.y, 0.80);
        let on = step(0.80, hash21(cell + vec2(in.hash * 251.0, in.hash * 97.0)));
        let flicker = 0.85 + 0.15 * sin(t * 1.3 + hash21(cell) * 40.0);
        let warm = mix(vec3(1.0, 0.72, 0.40), vec3(0.75, 0.85, 1.0), step(0.8, hash21(cell + 7.7)));
        col += warm * inside * on * flicker * 1.05;
    }

    // Cyan rim against the fog.
    let fres = pow(1.0 - clamp(dot(n, view), 0.0, 1.0), 4.0);
    col += vec3(0.10, 0.30, 0.42) * fres * 0.14;

    // Wet facades pick up a faint sheen.
    col += vec3(0.03, 0.05, 0.07) * rain * pow(1.0 - abs(n.y), 2.0) * 0.5;

    let dist = length(in.world - G.cam_pos.xyz);
    return vec4(fog_mix(col, dist, rain), 1.0);
}
