// Rain particles for TOKYO STORM: compute fall + streak rendering.
// Spawn density follows the JMA nowcast texture (or the demo storm field).

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,   // w = time
    sun_dir: vec4<f32>,
    viewport: vec4<f32>,
    params: vec4<f32>,
    layers: vec4<f32>,
    layers2: vec4<f32>,
};

struct RainMap {
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>, // x demo, z wind
};

struct RainSim {
    v0: vec4<f32>, // x dt, y count, z seed, w spawn boost
};

struct Drop {
    pos: vec3<f32>,
    speed: f32,
};

@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var<uniform> RM: RainMap;
@group(0) @binding(2) var rain_tex: texture_2d<f32>;
@group(0) @binding(3) var rain_samp: sampler;
@group(0) @binding(4) var<uniform> S: RainSim;
@group(0) @binding(5) var<storage, read_write> drops: array<Drop>;
// Read-only view of the same buffer for the vertex stage.
@group(0) @binding(6) var<storage, read> drops_r: array<Drop>;

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

fn pcg3d(vin: vec3<u32>) -> vec3<u32> {
    var v = vin * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3<u32>(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}

const SPAWN_HALF: f32 = 2600.0;

@compute @workgroup_size(256)
fn cs_rain(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= u32(S.v0.y) {
        return;
    }
    var d = drops[i];
    let dt = S.v0.x;
    let t = G.cam_pos.w;

    d.pos.y -= d.speed * dt;
    d.pos.x += RM.c.z * dt;

    if d.pos.y < -20.0 {
        let h = vec3<f32>(pcg3d(vec3<u32>(i, u32(S.v0.z), 1123u))) * (1.0 / 4294967295.0);
        let xz = (h.xy * 2.0 - 1.0) * SPAWN_HALF;
        let lvl = rain_level(xz, t);
        if h.z < lvl * S.v0.w + 0.002 {
            d.pos = vec3(xz.x, 500.0 + fract(h.z * 57.3) * 400.0, xz.y);
            d.speed = 90.0 + 70.0 * fract(h.x * 91.7);
        } else {
            // Stay parked below ground; retry with fresh randoms next frame.
            d.pos = vec3(xz.x, -30.0, xz.y);
            d.speed = 480.0 * max(dt, 0.004) + 60.0;
        }
    }
    drops[i] = d;
}

// ---------------- streaks ----------------

struct RainOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) alpha: f32,
};

@vertex
fn vs_rain(@builtin(vertex_index) vi: u32) -> RainOut {
    let did = vi / 2u;
    let sel = vi & 1u;
    let d = drops_r[did];
    if d.pos.y < -5.0 {
        return RainOut(vec4(0.0, 0.0, 2.0, 1.0), 0.0); // clipped away
    }
    var p = d.pos;
    if sel == 1u {
        p += vec3(-RM.c.z * 0.06, d.speed * 0.075, 0.0);
    }
    p.y = max(p.y, 0.0);
    let dist = length(d.pos - G.cam_pos.xyz);
    let a = clamp(1.6 - dist / 1800.0, 0.05, 1.0);
    return RainOut(G.view_proj * vec4(p, 1.0), a);
}

@fragment
fn fs_rain(in: RainOut) -> @location(0) vec4<f32> {
    return vec4(vec3(0.45, 0.62, 0.80) * 0.042 * in.alpha, 1.0);
}
