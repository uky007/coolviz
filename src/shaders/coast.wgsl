// Coastline polylines on the sphere, additive glow lines.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    sun_dir: vec4<f32>,
    viewport: vec4<f32>,
    params: vec4<f32>,
    layers: vec4<f32>,
};

@group(0) @binding(0) var<uniform> G: Globals;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) fade: f32,
};

@vertex
fn vs_main(@location(0) p: vec3<f32>) -> VSOut {
    let n = normalize(p);
    let view = normalize(G.cam_pos.xyz - p);
    // Fade lines near the limb so the silhouette stays clean.
    let facing = clamp(dot(n, view), 0.0, 1.0);
    let fade = smoothstep(0.03, 0.22, facing);
    return VSOut(G.view_proj * vec4(p, 1.0), fade);
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let c = vec3(0.30, 0.78, 1.0) * 0.52 * in.fade * G.layers.w;
    return vec4(c, 1.0);
}
