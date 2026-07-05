// City light sprites: street lamps, car head/tail lights, aviation beacons.
// Instance = pos.xyz + kind in pos.w; aux.x = hash, aux.w = brightness.
// kind: 0 sodium lamp, 1 headlight, 2 taillight, 3 rooftop beacon.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,   // w = time
    sun_dir: vec4<f32>,
    viewport: vec4<f32>,
    params: vec4<f32>,    // w = res_scale
    layers: vec4<f32>,
    layers2: vec4<f32>,
};

struct Light {
    pos: vec4<f32>,
    aux: vec4<f32>,
};

@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var<storage, read> lights: array<Light>;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) alpha: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VSOut {
    let lid = vi / 6u;
    let corner_idx = vi % 6u;
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    let corner = corners[corner_idx];
    let l = lights[lid];
    let t = G.cam_pos.w;
    var clip = G.view_proj * vec4(l.pos.xyz, 1.0);

    let kind = u32(l.pos.w + 0.5);
    var size = 2.0;
    var color = vec3(1.0, 0.72, 0.35); // sodium
    var brightness = 1.5;
    if kind == 1u {
        size = 1.8;
        color = vec3(0.95, 0.97, 1.0);
        brightness = 2.1;
    }
    if kind == 2u {
        size = 1.5;
        color = vec3(1.0, 0.16, 0.10);
        brightness = 1.3;
    }
    if kind == 3u {
        size = 2.4;
        color = vec3(1.0, 0.10, 0.08);
        // Slow aviation blink.
        brightness = 1.6 * (0.15 + 0.85 * smoothstep(0.35, 0.65, 0.5 + 0.5 * sin(t * 1.9 + l.aux.x * 40.0)));
    }

    let dist = length(l.pos.xyz - G.cam_pos.xyz);
    let atten = clamp(2.4 - dist / 1400.0, 0.15, 1.0);

    let size_px = size * G.params.w;
    clip.x += corner.x * size_px * 2.0 * G.viewport.z * clip.w;
    clip.y += corner.y * size_px * 2.0 * G.viewport.w * clip.w;

    var out: VSOut;
    out.pos = clip;
    out.uv = corner;
    out.color = color;
    out.alpha = brightness * l.aux.w * atten;
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let r2 = dot(in.uv, in.uv);
    let g = exp(-r2 * 4.0);
    return vec4(in.color * g * in.alpha, 1.0);
}
