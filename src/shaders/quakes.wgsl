// Earthquakes: pulsing rings, the one "hot" accent color in the scene.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,   // w = app time (s)
    sun_dir: vec4<f32>,
    viewport: vec4<f32>,
    params: vec4<f32>,    // w = res_scale
    layers: vec4<f32>,
};

struct Quake {
    pos: vec4<f32>,  // xyz world, w = magnitude
    info: vec4<f32>, // x = event time (app-relative s, usually negative), y = hash
};

@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var<storage, read> quakes: array<Quake>;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) mag: f32,
    @location(2) age_h: f32,
    @location(3) hash: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VSOut {
    let qid = vi / 6u;
    let corner_idx = vi % 6u;
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    let corner = corners[corner_idx];
    let q = quakes[qid];
    var clip = G.view_proj * vec4(q.pos.xyz, 1.0);

    let mag = q.pos.w;
    let zoom_scale = clamp(3.3 / max(length(G.cam_pos.xyz), 0.001), 0.30, 1.25);
    let size_px = clamp((7.0 + (mag - 2.5) * 10.0), 5.0, 80.0) * G.params.w * zoom_scale;
    clip.x += corner.x * size_px * 2.0 * G.viewport.z * clip.w;
    clip.y += corner.y * size_px * 2.0 * G.viewport.w * clip.w;

    let age_h = (G.cam_pos.w - q.info.x) / 3600.0;

    var out: VSOut;
    out.pos = clip;
    out.uv = corner;
    out.mag = mag;
    out.age_h = age_h;
    out.hash = q.info.y;
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let r = length(in.uv);
    if r > 1.0 {
        discard;
    }
    let t = G.cam_pos.w;

    // Static ring.
    let ring = exp(-pow((r - 0.52) / 0.10, 2.0));
    // Expanding pulse.
    let ph = fract(t * 0.35 + in.hash);
    let pulse = exp(-pow((r - ph * 0.92) / 0.06, 2.0)) * (1.0 - ph) * (1.0 - ph);
    // Core dot.
    let core = exp(-r * r * 30.0) * 0.7;

    let age_fade = exp(-max(in.age_h, 0.0) / 14.0) * 0.92 + 0.08;
    let intensity = (0.35 + (in.mag - 2.5) * 0.16) * age_fade;

    let hot = vec3(1.0, 0.46, 0.14);
    let a = (ring * 0.55 + pulse * 1.1 + core) * intensity * G.layers.z;
    return vec4(hot * a, 1.0);
}
