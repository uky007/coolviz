// OKINAWA SEA: fully procedural physically-inspired reef lagoon.
// Raymarched terrain (island / lagoon / reef crest / drop-off) under a water
// plane with depth absorption (emerald -> cobalt), caustics, breaking foam,
// tropical cumulus. Local frame: meters, x east, y up, z south.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,   // w = time
    sun_dir: vec4<f32>,
    viewport: vec4<f32>,
    params: vec4<f32>,    // z exposure, w res_scale
    layers: vec4<f32>,
    layers2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> G: Globals;

const SUN: vec3<f32> = vec3(-0.42, 0.72, -0.55);
const ISLAND: vec2<f32> = vec2(-330.0, -400.0);

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

fn fbm(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 4; i++) {
        v += a * vnoise(q);
        q = mat2x2<f32>(1.6, 1.2, -1.2, 1.6) * q;
        a *= 0.5;
    }
    return v;
}

fn coral_mask(xz: vec2<f32>) -> f32 {
    return smoothstep(0.56, 0.72, vnoise(xz * 0.055 + 31.7));
}

/// Seafloor / island height in meters (0 = mean sea level).
fn terrain(xz: vec2<f32>) -> f32 {
    let r = length(xz - ISLAND);
    let wob = (fbm(xz * 0.004) - 0.5) * 90.0; // irregular coastline
    let rr = r + wob;

    // Island body and beach.
    var h = 30.0 * smoothstep(430.0, 60.0, rr) + (430.0 - rr) * 0.045
        + 4.0 * (fbm(xz * 0.02) - 0.5) * smoothstep(400.0, 200.0, rr);
    h = min(h, 34.0);
    // Lagoon floor: gently deepening sand.
    let lagoon = -1.1 - 2.2 * smoothstep(430.0, 900.0, rr);
    h = min(h, max(lagoon, (430.0 - rr) * 0.045));
    // Coral heads rise from the lagoon.
    let coral = coral_mask(xz) * smoothstep(520.0, 620.0, rr) * smoothstep(980.0, 860.0, rr);
    h += coral * 1.7;
    // Reef crest, then the drop-off.
    h += 2.6 * exp(-pow((rr - 940.0) / 55.0, 2.0));
    h -= 42.0 * smoothstep(960.0, 1250.0, rr);
    // Sand ripples.
    h += 0.06 * sin(xz.x * 0.9 + xz.y * 0.6) * smoothstep(0.0, -0.5, h);
    return h;
}

fn terrain_normal(xz: vec2<f32>) -> vec3<f32> {
    let e = 1.4;
    let dx = terrain(xz + vec2(e, 0.0)) - terrain(xz - vec2(e, 0.0));
    let dz = terrain(xz + vec2(0.0, e)) - terrain(xz - vec2(0.0, e));
    return normalize(vec3(-dx, 2.0 * e, -dz));
}

fn sky(rd: vec3<f32>, t: f32) -> vec3<f32> {
    let sun = normalize(SUN);
    let up = clamp(rd.y, 0.0, 1.0);
    var col = mix(vec3(0.55, 0.74, 0.95), vec3(0.11, 0.40, 0.92), pow(up, 0.60));
    let sd = clamp(dot(rd, sun), 0.0, 1.0);
    col += vec3(1.0, 0.92, 0.75) * (pow(sd, 900.0) * 12.0 + pow(sd, 8.0) * 0.16);

    if rd.y > 0.02 {
        let p = rd.xz / (rd.y + 0.18) * 1.1 + vec2(t * 0.004, t * 0.0016);
        var v = 0.0;
        var a = 0.5;
        var q = p;
        for (var i = 0; i < 5; i++) {
            v += a * vnoise(q);
            q = q * 2.04 + vec2(3.1, 7.7);
            a *= 0.52;
        }
        let cl = smoothstep(0.55, 0.78, v);
        let shade = mix(0.66, 0.95, smoothstep(0.78, 0.95, v));
        col = mix(col, vec3(0.92, 0.93, 0.95) * shade, cl * smoothstep(0.0, 0.15, rd.y));
    }
    return col;
}

fn wave_normal(xz: vec2<f32>, t: f32, depth_hint: f32, dist: f32) -> vec3<f32> {
    // Swell steepens over the shallows; detail fades with distance (anti-moire).
    let fade = 1.0 / (1.0 + dist * 0.014);
    let amp = (0.028 + 0.10 * exp(-max(depth_hint, 0.0) * 0.35)) * fade;
    var n = vec3(0.0, 1.0, 0.0);
    let d1 = normalize(vec2(0.8, 0.6));
    let d2 = normalize(vec2(-0.5, 0.8));
    let d3 = normalize(vec2(0.2, -0.9));
    n.x -= amp * 5.2 * cos(dot(xz, d1) * 0.34 - t * 1.7) * d1.x * 0.34;
    n.z -= amp * 5.2 * cos(dot(xz, d1) * 0.34 - t * 1.7) * d1.y * 0.34;
    n.x -= amp * 3.4 * cos(dot(xz, d2) * 0.83 - t * 2.6) * d2.x * 0.83;
    n.z -= amp * 3.4 * cos(dot(xz, d2) * 0.83 - t * 2.6) * d2.y * 0.83;
    n.x -= amp * 1.6 * cos(dot(xz, d3) * 2.9 - t * 3.9) * d3.x * 0.35;
    n.z -= amp * 1.6 * cos(dot(xz, d3) * 2.9 - t * 3.9) * d3.y * 0.35;
    // Fine capillary shimmer.
    let cap = 0.05 * fade;
    n.x += (vnoise(xz * 1.4 + t * 0.9) - 0.5) * cap;
    n.z += (vnoise(xz * 1.4 - t * 0.7 + 13.1) - 0.5) * cap;
    return normalize(n);
}

fn seabed_albedo(xz: vec2<f32>) -> vec3<f32> {
    var alb = vec3(0.93, 0.88, 0.74); // coral sand
    alb *= 0.92 + 0.08 * sin(xz.x * 0.9 + xz.y * 0.6);
    let cm = coral_mask(xz);
    let coral_col = mix(vec3(0.16, 0.30, 0.26), vec3(0.38, 0.26, 0.30), vnoise(xz * 0.4));
    alb = mix(alb, coral_col, cm * 0.85);
    // Seagrass flecks.
    alb = mix(alb, vec3(0.20, 0.38, 0.28), smoothstep(0.75, 0.9, vnoise(xz * 0.13 + 71.0)) * 0.5);
    return alb;
}

fn march_terrain(ro: vec3<f32>, rd: vec3<f32>, t_max: f32) -> f32 {
    var t = 1.0;
    var th = 0.05;
    for (var i = 0; i < 80; i++) {
        let p = ro + rd * t;
        let dh = p.y - terrain(p.xz);
        if dh < th {
            return t;
        }
        t += max(dh * 0.55, 0.35);
        th = 0.05 + t * 0.002;
        if t > t_max {
            break;
        }
    }
    return -1.0;
}

fn shade_land(p: vec3<f32>, rd: vec3<f32>, t: f32) -> vec3<f32> {
    let n = terrain_normal(p.xz);
    let sun = normalize(SUN);
    var alb = vec3(0.84, 0.78, 0.63); // dry coral sand
    let veg = smoothstep(1.5, 4.5, p.y) * smoothstep(0.32, 0.55, vnoise(p.xz * 0.05 + 5.0));
    alb = mix(alb, vec3(0.09, 0.30, 0.13) * (0.8 + 0.4 * vnoise(p.xz * 0.3)), veg);
    // Wet sand at the waterline.
    alb *= 1.0 - 0.35 * smoothstep(0.8, 0.05, p.y);
    let dif = clamp(dot(n, sun), 0.0, 1.0);
    let amb = 0.35 + 0.2 * n.y;
    return alb * (dif * vec3(1.0, 0.96, 0.88) * 0.95 + amb * vec3(0.5, 0.65, 0.85));
}

struct FullOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> FullOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -3.0), vec2(3.0, 1.0), vec2(-1.0, 1.0));
    let xy = p[vi];
    return FullOut(vec4(xy, 0.0, 1.0), xy);
}

@fragment
fn fs_main(in: FullOut) -> @location(0) vec4<f32> {
    let far = G.inv_view_proj * vec4(in.ndc, 0.6, 1.0);
    let ro = G.cam_pos.xyz;
    let rd = normalize(far.xyz / far.w - ro);
    let t = G.cam_pos.w;
    let sun = normalize(SUN);

    var col = sky(rd, t);

    // Distance to the water plane (y = 0).
    var t_water = 1e9;
    if rd.y < -1e-4 {
        t_water = -ro.y / rd.y;
    }
    let t_land = march_terrain(ro, rd, min(t_water, 6000.0));

    if t_land > 0.0 {
        // Dry land before the water line.
        let p = ro + rd * t_land;
        col = shade_land(p, rd, t);
        col = mix(col, sky(rd, t), smoothstep(1800.0, 5200.0, t_land) * 0.7);
    } else if t_water < 6000.0 {
        let sp = ro + rd * t_water; // surface point
        let depth_here = -terrain(sp.xz);
        let n = wave_normal(sp.xz, t, depth_here, t_water);

        // Refracted ray into the water.
        let refr = refract(rd, n, 0.752);
        var wcol = vec3(0.0, 0.10, 0.12);
        var wlen = 30.0;
        if length(refr) > 0.0 {
            var tt = 0.4;
            var hit = sp + refr * tt;
            for (var i = 0; i < 28; i++) {
                hit = sp + refr * tt;
                let dh = hit.y - terrain(hit.xz);
                if dh < 0.05 {
                    break;
                }
                tt += max(dh * 0.7, 0.25);
                if tt > 120.0 {
                    break;
                }
            }
            wlen = tt;
            let depth = max(-hit.y, 0.02);
            var alb = seabed_albedo(hit.xz);
            // Caustics dance on the shallow floor.
            let ca = pow(
                vnoise(hit.xz * 0.55 + vec2(t * 0.35, -t * 0.22)) *
                vnoise(hit.xz * 0.47 - vec2(t * 0.27, t * 0.31)),
                2.0,
            ) * 2.6 * exp(-depth * 0.30);
            alb *= 0.85 + ca;
            // Beer-Lambert absorption: red dies first -> emerald shallows.
            let absorb = vec3(0.30, 0.058, 0.034);
            let tr = exp(-wlen * absorb * 0.62);
            let scatter = vec3(0.008, 0.155, 0.150) * (1.0 - exp(-wlen * 0.115));
            wcol = alb * tr * (0.50 + 0.55 * clamp(dot(vec3(0.0, 1.0, 0.0), sun), 0.0, 1.0)) + scatter;
        }

        // Fresnel blend with the sky reflection.
        let f0 = 0.021;
        let fres = f0 + (1.0 - f0) * pow(1.0 - clamp(dot(-rd, n), 0.0, 1.0), 5.0);
        let rdir = reflect(rd, n);
        var rcol = sky(vec3(rdir.x, abs(rdir.y) * 0.9 + 0.02, rdir.z), t);
        col = mix(wcol, rcol, clamp(fres, 0.0, 1.0));

        // Sun glitter.
        col += vec3(1.0, 0.95, 0.85) * pow(clamp(dot(rdir, sun), 0.0, 1.0), 750.0) * 2.0;

        // Breaking foam on the reef crest and the beach lap.
        let rr = length(sp.xz - ISLAND) + (fbm(sp.xz * 0.004) - 0.5) * 90.0;
        let crest = exp(-pow((rr - 940.0) / 46.0, 2.0));
        let surge = 0.5 + 0.5 * sin(rr * 0.16 - t * 1.9 + vnoise(sp.xz * 0.05) * 3.0);
        var foam = crest * smoothstep(0.35, 0.9, surge) * 0.9;
        foam += smoothstep(0.55, 0.1, depth_here) *
            (0.35 + 0.4 * sin(rr * 0.7 - t * 2.4)) * 0.45;
        foam *= smoothstep(0.0, 0.35, depth_here + 0.3);
        let foam_tex = 0.75 + 0.25 * vnoise(sp.xz * 1.1 + t * 0.5);
        col = mix(col, vec3(0.98, 1.0, 1.0) * foam_tex, clamp(foam, 0.0, 0.85));

        // Aerial perspective over long water distances.
        col = mix(col, sky(rd, t) * 0.98, smoothstep(1200.0, 5200.0, t_water) * 0.5);
    }

    // Scene gain: keep mid-tones below the bloom range so only the sun,
    // glitter and foam actually glow.
    return vec4(col * 0.56, 1.0);
}
