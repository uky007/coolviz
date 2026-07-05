// Satellites: instanced billboard glow points, positions extrapolated on GPU.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    sun_dir: vec4<f32>,
    viewport: vec4<f32>,
    params: vec4<f32>,  // y = seconds since sat state epoch
    layers: vec4<f32>,
};

struct Sat {
    pos: vec4<f32>, // xyz world (earth radii), w = kind (0 LEO,1 MEO,2 GEO,3 HEO,4 ISS)
    vel: vec4<f32>, // xyz world per second, w = brightness
};

@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var<storage, read> sats: array<Sat>;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) alpha: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VSOut {
    let sid = vi / 6u;
    let corner_idx = vi % 6u;
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    let corner = corners[corner_idx];
    let s = sats[sid];
    let wp = s.pos.xyz + s.vel.xyz * G.params.y;
    var clip = G.view_proj * vec4(wp, 1.0);

    let kind = u32(s.pos.w + 0.5);
    var size = 2.1;
    var color = vec3(0.45, 0.85, 1.0);
    var brightness = 0.85;
    if kind == 1u { size = 1.9; color = vec3(0.62, 0.78, 1.0); brightness = 0.6; }
    if kind == 2u { size = 2.5; color = vec3(1.0, 0.84, 0.58); brightness = 0.95; }
    if kind == 3u { size = 2.0; color = vec3(0.78, 0.66, 1.0); brightness = 0.7; }
    if kind == 4u { size = 6.5; color = vec3(1.0, 1.0, 1.0); brightness = 2.2; }

    let res_scale = G.params.w;
    let size_px = size * res_scale;
    clip.x += corner.x * size_px * 2.0 * G.viewport.z * clip.w;
    clip.y += corner.y * size_px * 2.0 * G.viewport.w * clip.w;

    var out: VSOut;
    out.pos = clip;
    out.uv = corner;
    out.color = color;
    out.alpha = brightness * s.vel.w * G.layers.y;
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let r2 = dot(in.uv, in.uv);
    let g = exp(-r2 * 4.2);
    return vec4(in.color * g * in.alpha, 1.0);
}
