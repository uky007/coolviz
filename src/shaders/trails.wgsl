// Trail buffer: fade previous frame (bindings 0-2), then draw particle
// segments additively (bindings 3-4). Bindings are disjoint so both entry
// points can live in one module.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    sun_dir: vec4<f32>,
    viewport: vec4<f32>,
    params: vec4<f32>,
    layers: vec4<f32>,
};

struct Particle {
    cur: vec2<f32>,
    prev: vec2<f32>,
    age: f32,
    life: f32,
    speed: f32,
    _pad: f32,
};

struct FadeParams {
    v0: vec4<f32>, // x = fade factor
};

@group(0) @binding(0) var prev_tex: texture_2d<f32>;
@group(0) @binding(1) var prev_samp: sampler;
@group(0) @binding(2) var<uniform> F: FadeParams;
@group(0) @binding(3) var<uniform> G: Globals;
@group(0) @binding(4) var<storage, read> particles: array<Particle>;

struct FullOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FullOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -3.0), vec2(3.0, 1.0), vec2(-1.0, 1.0));
    let xy = p[vi];
    return FullOut(vec4(xy, 0.0, 1.0), vec2(xy.x * 0.5 + 0.5, 0.5 - xy.y * 0.5));
}

@fragment
fn fs_fade(in: FullOut) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(prev_tex, prev_samp, in.uv, 0.0).rgb;
    let faded = max(c * F.v0.x - vec3(0.0004), vec3(0.0));
    return vec4(faded, 1.0);
}

struct SegOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) speed: f32,
};

fn to_sphere(ll: vec2<f32>) -> vec3<f32> {
    let lat = radians(ll.y);
    let lon = radians(ll.x);
    return vec3(cos(lat) * cos(lon), sin(lat), -cos(lat) * sin(lon)) * 1.0028;
}

@vertex
fn vs_seg(@builtin(vertex_index) vi: u32) -> SegOut {
    let pid = vi / 2u;
    let sel = vi & 1u;
    let p = particles[pid];
    var ll = p.prev;
    if sel == 1u {
        ll = p.cur;
    }
    // Collapse wrap-around segments (date line jumps).
    if abs(p.cur.x - p.prev.x) > 90.0 {
        ll = p.cur;
    }
    let clip = G.view_proj * vec4(to_sphere(ll), 1.0);
    return SegOut(clip, p.speed);
}

@fragment
fn fs_seg(in: SegOut) -> @location(0) vec4<f32> {
    let s = clamp(in.speed / 26.0, 0.0, 1.0);
    let slow = vec3(0.07, 0.13, 0.19);
    let mid = vec3(0.12, 0.55, 0.90);
    let fast = vec3(0.92, 0.99, 1.0);
    let c = mix(slow, mix(mid, fast, smoothstep(0.45, 1.0, s)), 0.2 + 0.8 * s);
    return vec4(c * 0.17 * G.layers.x, 1.0);
}
