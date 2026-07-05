// Wind particle simulation (compute). Particles live in lon/lat degrees.

struct Particle {
    cur: vec2<f32>,   // lon, lat (deg)
    prev: vec2<f32>,
    age: f32,
    life: f32,
    speed: f32,       // |wind| m/s at last sample
    _pad: f32,
};

struct SimParams {
    v0: vec4<f32>, // x = dt (s), y = warp, z = count (as f32), w = seed (as f32)
};

@group(0) @binding(0) var<uniform> P: SimParams;
@group(0) @binding(1) var<storage, read_write> particles: array<Particle>;
@group(0) @binding(2) var wind_tex: texture_2d<f32>;
@group(0) @binding(3) var wind_samp: sampler;

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

fn rand3(id: u32, seed: u32) -> vec3<f32> {
    let h = pcg3d(vec3<u32>(id, seed, 747796405u));
    return vec3<f32>(h) * (1.0 / 4294967295.0);
}

@compute @workgroup_size(256)
fn cs_update(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    let count = u32(P.v0.z);
    if i >= count {
        return;
    }
    var p = particles[i];
    let dt = P.v0.x;
    let warp = P.v0.y;
    let seed = u32(P.v0.w);

    p.prev = p.cur;

    // Sample wind (grid: lon 0..360 east, rows from +90N to -90S).
    let dims = vec2<f32>(textureDimensions(wind_tex));
    let u = fract(p.cur.x / 360.0);
    let v = ((90.0 - p.cur.y) / 180.0 * (dims.y - 1.0) + 0.5) / dims.y;
    let w = textureSampleLevel(wind_tex, wind_samp, vec2(u, v), 0.0).xy;

    let k = dt * warp / 111320.0; // meters -> degrees
    let coslat = max(cos(radians(p.cur.y)), 0.03);
    p.cur.x += w.x * k / coslat;
    p.cur.y += w.y * k;
    p.speed = length(w);
    p.age += dt;

    // Wrap / clamp.
    if p.cur.x > 180.0 { p.cur.x -= 360.0; p.prev = p.cur; }
    if p.cur.x < -180.0 { p.cur.x += 360.0; p.prev = p.cur; }

    if p.age > p.life || abs(p.cur.y) > 88.0 {
        let r = rand3(i, seed);
        let lon = r.x * 360.0 - 180.0;
        // Area-weighted latitude so poles don't clump.
        let lat = degrees(asin(clamp(r.y * 2.0 - 1.0, -0.999, 0.999)));
        p.cur = vec2(lon, clamp(lat, -84.0, 84.0));
        p.prev = p.cur;
        p.age = 0.0;
        p.life = 4.0 + 9.0 * r.z;
        p.speed = 0.0;
    }

    particles[i] = p;
}
