// OKINAWA SEA v2: procedural reef lagoon with displaced swell.
// Radially inbound waves shoal and break on the reef crest and beach,
// interference-pattern caustics, sun-glitter lane, Beer-Lambert water color.
// Local frame: meters, x east, y up, z south. Everything is analytic.

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

struct SimMap {
    a: vec4<f32>, // x,y = grid origin (world xz), z = cell, w = N
    b: vec4<f32>, // x = enabled
};

@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var<uniform> SM: SimMap;
@group(0) @binding(2) var sim_tex: texture_2d<f32>;
@group(0) @binding(3) var sim_samp: sampler;

const SUN: vec3<f32> = vec3(-0.42, 0.72, -0.55);

/// Sim-domain blend weight (0 outside, 1 well inside).
fn sim_w(xz: vec2<f32>) -> f32 {
    if SM.b.x < 0.5 {
        return 0.0;
    }
    let ext = SM.a.z * SM.a.w;
    let uv = (xz - SM.a.xy) / ext;
    let m = min(min(uv.x, uv.y), 1.0 - max(uv.x, uv.y));
    return smoothstep(0.01, 0.10, m);
}

/// (surface elevation, depth, speed) from the simulation.
fn sim_surface(xz: vec2<f32>) -> vec3<f32> {
    let ext = SM.a.z * SM.a.w;
    let uv = (xz - SM.a.xy) / ext;
    let s = textureSampleLevel(sim_tex, sim_samp, uv, 0.0);
    return vec3(s.r + s.g, s.g, s.b);
}
const ISLAND: vec2<f32> = vec2(-330.0, -400.0);
const REEF_R: f32 = 940.0;

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

// Irregular shoreline radius.
fn shore_r(xz: vec2<f32>) -> f32 {
    return length(xz - ISLAND) + (fbm(xz * 0.004) - 0.5) * 70.0;
}

fn coral_mask(xz: vec2<f32>) -> f32 {
    let n = vnoise(xz * 0.05 + 31.7) * 0.65 + vnoise(xz * 0.15 + 7.1) * 0.35;
    return smoothstep(0.58, 0.66, n);
}

/// Seafloor / island height (no waves), meters relative to sea level.
fn terrain(xz: vec2<f32>) -> f32 {
    let rr = shore_r(xz);
    var h = 30.0 * smoothstep(430.0, 60.0, rr) + (430.0 - rr) * 0.045
        + 4.0 * (fbm(xz * 0.02) - 0.5) * smoothstep(400.0, 200.0, rr);
    h = min(h, 34.0);
    let lagoon = -1.0 - 2.4 * smoothstep(430.0, 900.0, rr);
    h = min(h, max(lagoon, (430.0 - rr) * 0.045));
    let coral = coral_mask(xz) * smoothstep(500.0, 590.0, rr) * smoothstep(980.0, 880.0, rr);
    h += coral * (1.4 + 0.6 * vnoise(xz * 0.3));
    h += 2.4 * exp(-pow((rr - REEF_R) / 55.0, 2.0));
    h -= 44.0 * smoothstep(955.0, 1250.0, rr);
    h += 0.05 * sin(xz.x * 0.8 + xz.y * 0.55) * smoothstep(0.0, -0.5, h);
    return h;
}

fn terrain_normal(xz: vec2<f32>) -> vec3<f32> {
    let e = 1.2;
    let dx = terrain(xz + vec2(e, 0.0)) - terrain(xz - vec2(e, 0.0));
    let dz = terrain(xz + vec2(0.0, e)) - terrain(xz - vec2(0.0, e));
    return normalize(vec3(-dx, 2.0 * e, -dz));
}

/// Cheap analytic depth (no fbm) for wave shoaling.
fn depth_apx(rr: f32) -> f32 {
    var d = 1.0 + 2.4 * smoothstep(430.0, 900.0, rr); // lagoon
    d = max(d - 2.4 * exp(-pow((rr - REEF_R) / 55.0, 2.0)), 0.15); // reef crest
    d += 44.0 * smoothstep(955.0, 1250.0, rr); // drop-off
    d = min(d, max((rr - 430.0) * 0.045, 0.02)); // beach approach
    return max(d, 0.02);
}

const SWELL_K: f32 = 0.17; // ~37 m wavelength
const SWELL_W: f32 = 1.15;

fn swell_phase(rr: f32, t: f32) -> f32 {
    return rr * SWELL_K + t * SWELL_W; // crests travel toward the island
}

/// Displaced water surface height; detail fades with view distance
/// so the radial swell doesn't alias into rings.
fn wave_h(xz: vec2<f32>, t: f32, dist: f32) -> f32 {
    let rr = length(xz - ISLAND);
    let d = depth_apx(rr);
    let lod = 1.0 / (1.0 + dist * 0.0016);
    // Shoaling: swell grows over the reef and near the beach, dies on dry land.
    let amp = 0.16 * clamp(1.0 / (0.45 + d * 0.35), 0.7, 2.6)
        * smoothstep(-0.15, 0.8, d)
        * lod;
    let ph = swell_phase(rr, t);
    var h = amp * (sin(ph) + 0.42 * sin(2.0 * ph + 1.3)); // sharpened crests
    h += 0.045 * sin(dot(xz, vec2(0.055, 0.078)) + t * 0.8) * lod; // cross swell
    h += (vnoise(xz * 0.30 + vec2(t * 0.20, -t * 0.13)) - 0.5) * 0.10 * lod; // chop
    return h;
}

/// Water surface height: simulation inside its domain, procedural outside.
fn surf_h(xz: vec2<f32>, t: f32, dist: f32) -> f32 {
    let w = sim_w(xz);
    var h_proc = 0.0;
    if w < 0.999 {
        h_proc = wave_h(xz, t, dist);
    }
    if w > 0.001 {
        let s = sim_surface(xz);
        // Fine chop on top of the coarse sim surface.
        let lod = 1.0 / (1.0 + dist * 0.004);
        let chop = (vnoise(xz * 0.55 + vec2(t * 0.35, -t * 0.22)) - 0.5) * 0.05 * lod;
        return mix(h_proc, s.x + chop, w);
    }
    return h_proc;
}

fn water_normal(xz: vec2<f32>, t: f32, dist: f32) -> vec3<f32> {
    let e = max(0.55, SM.a.z * sim_w(xz));
    let dx = surf_h(xz + vec2(e, 0.0), t, dist) - surf_h(xz - vec2(e, 0.0), t, dist);
    let dz = surf_h(xz + vec2(0.0, e), t, dist) - surf_h(xz - vec2(0.0, e), t, dist);
    var n = vec3(-dx, 2.0 * e, -dz);
    // Capillary shimmer, fading with distance to avoid moire.
    let cap = 0.35 / (1.0 + dist * 0.02);
    n.x += (vnoise(xz * 1.9 + vec2(t * 1.1, t * 0.4)) - 0.5) * cap;
    n.z += (vnoise(xz * 1.9 + vec2(-t * 0.6, t * 0.9) + 13.1) - 0.5) * cap;
    return normalize(n);
}

/// Interfering-wavefront caustic filaments.
fn caustics(p: vec2<f32>, t: f32) -> f32 {
    let a = sin(p.x * 1.6 + sin(p.y * 1.25 + t * 1.15) * 1.8 + t * 0.62);
    let b = sin(p.y * 1.8 + sin(p.x * 1.05 - t * 0.95) * 1.9 - t * 0.85);
    let c = sin((p.x + p.y) * 1.15 + sin((p.x - p.y) * 1.45 + t * 0.75) * 1.6);
    let v = abs(a + b + c);
    return pow(clamp(1.0 - v * 0.42, 0.0, 1.0), 3.0);
}

fn sky(rd: vec3<f32>, t: f32) -> vec3<f32> {
    let sun = normalize(SUN);
    let up = clamp(rd.y, 0.0, 1.0);
    var col = mix(vec3(0.55, 0.74, 0.94), vec3(0.075, 0.34, 0.88), pow(up, 0.55));
    // Thin bright haze right at the horizon.
    col += vec3(0.30, 0.32, 0.33) * exp(-abs(rd.y) * 14.0) * 0.55;
    let sd = clamp(dot(rd, sun), 0.0, 1.0);
    col += vec3(1.0, 0.93, 0.78) * (pow(sd, 1100.0) * 14.0 + pow(sd, 10.0) * 0.14);

    if rd.y > 0.015 {
        let p = rd.xz / (rd.y + 0.16);
        let drift = vec2(t * 0.005, t * 0.002);
        let d1 = fbm(p * 0.9 + drift);
        let d2 = fbm(p * 0.9 + drift + vec2(0.05, -0.13));
        let body = smoothstep(0.56, 0.72, d1);
        // Flat-bottomed look: tops brighter where the second sample thins.
        let top = smoothstep(0.0, 0.25, d1 - d2 + 0.06);
        let cloud_col = mix(vec3(0.72, 0.74, 0.80), vec3(1.06, 1.06, 1.04), top);
        col = mix(col, cloud_col, body * smoothstep(0.0, 0.12, rd.y) * 0.96);
    }
    return col;
}

fn seabed_albedo(xz: vec2<f32>) -> vec3<f32> {
    var alb = vec3(1.00, 0.95, 0.80); // bright coral sand
    alb *= 0.93 + 0.07 * sin(xz.x * 0.8 + xz.y * 0.55);
    alb *= 0.95 + 0.05 * vnoise(xz * 0.9);
    let cm = coral_mask(xz);
    let coral_col = mix(
        vec3(0.13, 0.26, 0.20),
        vec3(0.32, 0.21, 0.24),
        vnoise(xz * 0.35),
    );
    alb = mix(alb, coral_col, cm * 0.9);
    alb = mix(
        alb,
        vec3(0.16, 0.34, 0.24),
        smoothstep(0.78, 0.92, vnoise(xz * 0.11 + 71.0)) * 0.45,
    );
    return alb;
}

fn march_terrain(ro: vec3<f32>, rd: vec3<f32>, t_max: f32) -> f32 {
    var t = 1.0;
    var th = 0.05;
    for (var i = 0; i < 90; i++) {
        let p = ro + rd * t;
        let dh = p.y - terrain(p.xz);
        if dh < th {
            return t;
        }
        t += max(dh * 0.5, 0.3);
        th = 0.05 + t * 0.0022;
        if t > t_max {
            break;
        }
    }
    return -1.0;
}

fn shade_land(p: vec3<f32>, t: f32) -> vec3<f32> {
    let n = terrain_normal(p.xz);
    let sun = normalize(SUN);
    // Warm dry sand with fine grain; damp band at the waterline.
    var alb = vec3(0.97, 0.89, 0.70);
    alb *= 0.92 + 0.16 * vnoise(p.xz * 1.3);
    alb *= 0.97 + 0.03 * vnoise(p.xz * 7.0);
    let veg = smoothstep(2.2, 5.5, p.y)
        * smoothstep(0.34, 0.56, vnoise(p.xz * 0.03 + 5.0));
    let veg_col = mix(vec3(0.05, 0.20, 0.08), vec3(0.11, 0.34, 0.12), vnoise(p.xz * 0.6));
    alb = mix(alb, veg_col, veg);
    let wet = smoothstep(0.9, 0.1, p.y);
    alb *= 1.0 - 0.38 * wet;
    let dif = clamp(dot(n, sun), 0.0, 1.0);
    let amb = 0.30 + 0.22 * n.y;
    var col = alb * (dif * vec3(1.05, 1.00, 0.90) + amb * vec3(0.42, 0.58, 0.85));
    // Glistening wet sand.
    col += vec3(0.5) * wet * pow(clamp(dot(reflect(-sun, n), vec3(0.0, 1.0, 0.0)), 0.0, 1.0), 24.0) * 0.15;
    return col;
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

    // Displaced water-surface intersection (relaxed Newton on the plane hit).
    var t_water = 1e9;
    if rd.y < -1e-4 {
        var tw = -ro.y / rd.y;
        for (var i = 0; i < 5; i++) {
            let p = ro + rd * tw;
            tw += (surf_h(p.xz, t, tw) - p.y) / rd.y;
        }
        t_water = max(tw, 0.0);
    }
    let t_land = march_terrain(ro, rd, min(t_water + 30.0, 6000.0));

    if t_land > 0.0 && t_land < t_water {
        let p = ro + rd * t_land;
        col = shade_land(p, t);
        col = mix(col, sky(rd, t), smoothstep(1800.0, 5200.0, t_land) * 0.65);
    } else if t_water < 6000.0 {
        let sp = ro + rd * t_water;
        let rr = length(sp.xz - ISLAND);
        let depth_here = max(-terrain(sp.xz), 0.0);
        let n = water_normal(sp.xz, t, t_water);

        // ---- refracted bottom color ----
        let refr = refract(rd, n, 0.752);
        var wcol = vec3(0.0, 0.09, 0.13);
        if length(refr) > 0.0 {
            var tt = 0.4;
            var hit = sp;
            for (var i = 0; i < 26; i++) {
                hit = sp + refr * tt;
                let dh = hit.y - terrain(hit.xz);
                if dh < 0.06 {
                    break;
                }
                tt += max(dh * 0.7, 0.25);
                if tt > 140.0 {
                    break;
                }
            }
            let wlen = tt;
            let depth = max(-hit.y, 0.03);
            var alb = seabed_albedo(hit.xz);
            let patchy = 0.45 + 0.9 * vnoise(hit.xz * 0.028 + vec2(t * 0.015, 0.0));
            let ca = caustics(hit.xz * 0.5, t) * 2.2 * exp(-depth * 0.24) * patchy;
            alb *= 0.84 + ca;
            // Absorption: red first -> fluorescent emerald over sand.
            let tr = exp(-wlen * vec3(0.215, 0.032, 0.016));
            let scatter_col = mix(
                vec3(0.03, 0.62, 0.56),
                vec3(0.0, 0.145, 0.38),
                1.0 - exp(-depth * 0.11),
            );
            let scatter = scatter_col * (1.0 - exp(-wlen * 0.14));
            wcol = alb * tr * 0.98 + scatter;
        }

        // ---- reflection + fresnel ----
        let f0 = 0.021;
        let fres = f0 + (1.0 - f0) * pow(1.0 - clamp(dot(-rd, n), 0.0, 1.0), 5.0);
        let rdir = reflect(rd, n);
        let rcol = sky(vec3(rdir.x, abs(rdir.y) * 0.92 + 0.02, rdir.z), t);
        col = mix(wcol, rcol, clamp(fres, 0.0, 1.0));

        // ---- sun glitter lane (near-field only) ----
        let sun_align = clamp(dot(rdir, sun), 0.0, 1.0);
        let near = smoothstep(2400.0, 600.0, t_water);
        let sparkle = step(0.845, vnoise(sp.xz * 4.2 + vec2(t * 1.9, -t * 1.4)));
        col += vec3(1.0, 0.97, 0.90)
            * (pow(sun_align, 950.0) * 2.4 + pow(sun_align, 55.0) * sparkle * 1.15 * near);

        // ---- breaking foam ----
        // Simulation-driven foam where the sim is active.
        let simw = sim_w(sp.xz);
        if simw > 0.01 {
            let s = sim_surface(sp.xz);
            let froude = s.z / sqrt(9.81 * max(s.y, 0.05));
            let sim_foam = smoothstep(0.55, 1.2, froude)
                * (0.55 + 0.45 * vnoise(sp.xz * 0.9 + t * 0.4));
            col = mix(
                col,
                vec3(0.97, 1.0, 1.0),
                clamp(sim_foam * simw * 0.85, 0.0, 0.85),
            );
        }
        let ph = swell_phase(rr, t);
        let crest = 0.5 + 0.5 * sin(ph + 0.7);
        let shallow = exp(-depth_here * 0.85);
        // White arcs that ride the crests over the reef and up the beach.
        var foam = smoothstep(0.52, 0.94, crest) * smoothstep(0.30, 0.95, shallow * 1.5);
        // Permanent boil on the reef crest ring.
        foam += exp(-pow((rr - REEF_R) / 42.0, 2.0))
            * (0.45 + 0.55 * vnoise(sp.xz * 0.35 + vec2(t * 0.5, 0.0)));
        // Trailing wash behind each crest.
        foam += smoothstep(0.52, 0.94, 0.5 + 0.5 * sin(ph - 0.9)) * shallow * 0.35;
        let mottle = vnoise(sp.xz * 1.05 + vec2(t * 0.32, -t * 0.21)) * 0.6
            + vnoise(sp.xz * 3.1 + vec2(-t * 0.18, t * 0.26)) * 0.4;
        foam *= smoothstep(0.22, 0.7, mottle + foam * 0.35);
        foam *= smoothstep(-0.05, 0.3, depth_here + 0.15); // none on dry sand
        col = mix(col, vec3(0.99, 1.0, 1.01) * (0.85 + 0.15 * mottle), clamp(foam, 0.0, 0.92));

        // Aerial perspective.
        col = mix(col, sky(rd, t) * 0.98, smoothstep(1300.0, 5200.0, t_water) * 0.35);
    }

    // Keep mid-tones under the bloom threshold; sun, glitter and foam glow.
    return vec4(col * 0.56, 1.0);
}
