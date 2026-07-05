// Post: composite (scene + trails), bloom down/up chain, tonemap.
// One shared binding set: 0 = Globals, 1/2 = input textures, 3 = sampler.
// composite: tex_a = scene HDR, tex_b = trails
// down/up:   tex_a = source mip
// tonemap:   tex_a = composite, tex_b = bloom

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    sun_dir: vec4<f32>,
    viewport: vec4<f32>,
    params: vec4<f32>, // x trail_gain, z exposure
    layers: vec4<f32>,
    layers2: vec4<f32>, // z = bloom threshold
};

@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var tex_a: texture_2d<f32>;
@group(0) @binding(2) var tex_b: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

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
fn fs_composite(in: FullOut) -> @location(0) vec4<f32> {
    let hdr = textureSampleLevel(tex_a, samp, in.uv, 0.0).rgb;
    let trail = textureSampleLevel(tex_b, samp, in.uv, 0.0).rgb;
    return vec4(hdr + trail * G.params.x, 1.0);
}

// First bloom downsample: apply the soft threshold from G.layers2.z.
@fragment
fn fs_down_first(in: FullOut) -> @location(0) vec4<f32> {
    let ts = 1.0 / vec2<f32>(textureDimensions(tex_a));
    var c = vec3(0.0);
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(-0.75, -0.75) * ts, 0.0).rgb;
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(0.75, -0.75) * ts, 0.0).rgb;
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(-0.75, 0.75) * ts, 0.0).rgb;
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(0.75, 0.75) * ts, 0.0).rgb;
    c *= 0.25;
    let thr = G.layers2.z;
    c = max(c - vec3(thr), vec3(0.0));
    return vec4(c, 1.0);
}

@fragment
fn fs_down(in: FullOut) -> @location(0) vec4<f32> {
    let ts = 1.0 / vec2<f32>(textureDimensions(tex_a));
    var c = vec3(0.0);
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(-0.75, -0.75) * ts, 0.0).rgb;
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(0.75, -0.75) * ts, 0.0).rgb;
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(-0.75, 0.75) * ts, 0.0).rgb;
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(0.75, 0.75) * ts, 0.0).rgb;
    return vec4(c * 0.25, 1.0);
}

@fragment
fn fs_up(in: FullOut) -> @location(0) vec4<f32> {
    let ts = 1.0 / vec2<f32>(textureDimensions(tex_a));
    var c = vec3(0.0);
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(-1.0, 0.0) * ts, 0.0).rgb * 0.2;
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(1.0, 0.0) * ts, 0.0).rgb * 0.2;
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(0.0, -1.0) * ts, 0.0).rgb * 0.2;
    c += textureSampleLevel(tex_a, samp, in.uv + vec2(0.0, 1.0) * ts, 0.0).rgb * 0.2;
    c += textureSampleLevel(tex_a, samp, in.uv, 0.0).rgb * 0.2;
    return vec4(c * 0.72, 1.0);
}

@fragment
fn fs_tonemap(in: FullOut) -> @location(0) vec4<f32> {
    let comp = textureSampleLevel(tex_a, samp, in.uv, 0.0).rgb;
    let bloom = textureSampleLevel(tex_b, samp, in.uv, 0.0).rgb;
    var c = comp + bloom * 0.65;

    let e = G.params.z;
    c = vec3(1.0) - exp(-c * e);

    // Gentle vignette.
    let d = (in.uv - 0.5) * vec2(G.viewport.x * G.viewport.w, 1.0);
    c *= 1.0 - 0.30 * smoothstep(0.45, 1.05, length(d));

    // Slight cool cast in the shadows for the mission-control mood.
    c = pow(max(c, vec3(0.0)), vec3(1.02, 1.0, 0.985));

    return vec4(c, 1.0);
}
