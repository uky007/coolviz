// Fullscreen pass: starfield background + analytically raytraced Earth + atmosphere.
// Writes frag_depth so later passes (lines, sprites, particles) are occluded correctly.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,   // xyz = camera, w = app time (s)
    sun_dir: vec4<f32>,   // xyz = sun direction (world)
    viewport: vec4<f32>,  // w, h, 1/w, 1/h  (physical px)
    params: vec4<f32>,    // x trail_gain, y sat_dt, z exposure, w res_scale
    layers: vec4<f32>,    // x wind, y sats, z quakes, w coast
    layers2: vec4<f32>,   // x clouds, y cloud-texture-loaded
};

@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var land_tex: texture_2d<f32>;
@group(0) @binding(2) var land_samp: sampler;
@group(0) @binding(3) var cloud_tex: texture_2d<f32>;
@group(0) @binding(4) var cloud_samp: sampler;

struct VSOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VSOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -3.0), vec2(3.0, 1.0), vec2(-1.0, 1.0));
    let xy = p[vi];
    return VSOut(vec4(xy, 0.0, 1.0), xy);
}

fn hash13(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.1031);
    q += dot(q, q.zyx + 31.32);
    return fract((q.x + q.y) * q.z);
}

fn hash33(p: vec3<f32>) -> vec3<f32> {
    var q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yxz + 33.33);
    return fract((q.xxy + q.yxx) * q.zyx);
}

fn star_layer(rd: vec3<f32>, scale: f32, threshold: f32, gain: f32) -> vec3<f32> {
    let p = rd * scale;
    let id = floor(p);
    let f = fract(p) - 0.5;
    let h = hash33(id);
    if h.x < threshold {
        return vec3(0.0);
    }
    // Star offset within its 3D cell.
    let dd = length(f - (h.yzx - 0.5) * 0.66);
    let core = exp(-dd * dd * 380.0);
    let tw = 0.75 + 0.25 * sin(G.cam_pos.w * (0.7 + h.z * 2.0) + h.y * 40.0);
    // Color temperature: bluish-white to warm.
    let temp = mix(vec3(0.72, 0.82, 1.0), vec3(1.0, 0.86, 0.70), h.z * h.z);
    return temp * core * gain * tw * ((h.x - threshold) / (1.0 - threshold));
}

fn stars(rd: vec3<f32>) -> vec3<f32> {
    var c = vec3(0.0);
    c += star_layer(rd, 42.0, 0.985, 0.55);
    c += star_layer(rd, 97.0, 0.991, 0.30);
    // Faint galactic band, tilted, smoothly mottled.
    let mw_n = normalize(vec3(0.12, 0.48, 0.87));
    let band = exp(-abs(dot(rd, mw_n)) * 6.5);
    let s1 = sin(rd.x * 9.1 + rd.z * 6.3) * sin(rd.y * 7.7 - rd.z * 4.9);
    let mottling = 0.8 + 0.25 * s1;
    c += vec3(0.34, 0.42, 0.60) * band * 0.020 * mottling;
    return c;
}

struct FSOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_main(in: VSOut) -> FSOut {
    let far = G.inv_view_proj * vec4(in.ndc, 0.6, 1.0);
    let ro = G.cam_pos.xyz;
    let rd = normalize(far.xyz / far.w - ro);
    let sun = G.sun_dir.xyz;

    var col = stars(rd);
    var depth = 1.0;

    let b = dot(ro, rd);
    let c0 = dot(ro, ro) - 1.0;
    let h2 = b * b - c0;

    if h2 > 0.0 && -b - sqrt(h2) > 0.0 {
        let t = -b - sqrt(h2);
        let pos = ro + rd * t;
        let n = pos;
        let lat = degrees(asin(clamp(n.y, -1.0, 1.0)));
        let lon = degrees(atan2(-n.z, n.x));
        let u = fract((lon + 180.0) / 360.0);
        let v = clamp((90.0 - lat) / 180.0, 0.0, 1.0);
        let land = textureSampleLevel(land_tex, land_samp, vec2(u, v), 0.0).r;

        let day = smoothstep(-0.10, 0.20, dot(n, sun));

        let ocean = vec3(0.009, 0.036, 0.072);
        let land_c = vec3(0.058, 0.092, 0.128);
        var base = mix(ocean, land_c, land);

        // Graticule every 15 deg, very faint.
        let glat = abs(fract(lat / 15.0 + 0.5) - 0.5) * 15.0;
        let glon = abs(fract(lon / 15.0 + 0.5) - 0.5) * 15.0;
        let lat_scale = max(cos(radians(lat)), 0.05);
        let gl = max(
            1.0 - smoothstep(0.0, 0.09, glat),
            1.0 - smoothstep(0.0, 0.09 / lat_scale, glon),
        );
        base += vec3(0.20, 0.55, 0.80) * gl * 0.028;

        // Day/night shading.
        var lit = base * mix(0.20, 1.0, day);

        // Ocean sun glint — tight specular star, bloom supplies the halo.
        let hv = normalize(sun - rd);
        let spec = pow(max(dot(n, hv), 0.0), 2600.0) * (1.0 - land) * day;
        lit += vec3(1.0, 0.88, 0.72) * spec * 0.11;

        // Fresnel rim (inner atmosphere), narrow so close-ups stay clean.
        let fres = pow(1.0 - max(dot(n, -rd), 0.0), 4.6);
        lit += vec3(0.22, 0.55, 1.0) * fres * (0.11 + 0.28 * day);

        // Soft terminator tint.
        let term = exp(-abs(dot(n, sun)) * 9.0);
        lit += vec3(0.30, 0.35, 0.55) * term * 0.05;

        // Himawari-9 live clouds: reproject the geostationary full-disk image
        // (satellite at 140.7 deg E, 35,786 km altitude) onto the sphere.
        if G.layers2.x > 0.001 && G.layers2.y > 0.5 {
            let sat_pos = 6.61857 * vec3(cos(radians(140.7)), 0.0, -sin(radians(140.7)));
            let to_p = normalize(pos - sat_pos);
            // Only the hemisphere facing the satellite.
            if dot(n, -to_p) > 0.02 {
                let fwd = normalize(-sat_pos);
                let east = normalize(cross(fwd, vec3(0.0, 1.0, 0.0)));
                let north_b = cross(east, fwd);
                let z = dot(to_p, fwd);
                let edge = 0.15285; // tan(asin(1 / 6.61857)), disk fills the frame
                let su = dot(to_p, east) / (z * edge);
                let sv = dot(to_p, north_b) / (z * edge);
                if z > 0.0 && abs(su) < 1.0 && abs(sv) < 1.0 {
                    let cuv = vec2(0.5 + 0.5 * su, 0.5 - 0.5 * sv);
                    let cl = textureSampleLevel(cloud_tex, cloud_samp, cuv, 0.0).rgb;
                    let lum = dot(cl, vec3(0.299, 0.587, 0.114));
                    // Bright pixels are cloud/snow; dark ocean/land stay out.
                    let cmask = smoothstep(0.16, 0.60, lum) * G.layers2.x;
                    let cloud_c = vec3(0.80, 0.89, 1.0) * (0.30 + 0.85 * lum);
                    let vis = cmask * (0.10 + 0.90 * day);
                    lit = mix(lit, cloud_c, clamp(vis, 0.0, 0.88));
                }
            }
        }

        col = lit;

        let clip = G.view_proj * vec4(pos, 1.0);
        depth = clip.z / clip.w;
    } else if b < 0.0 {
        // Outer atmosphere halo around the limb.
        let closest = sqrt(max(dot(ro, ro) - b * b, 0.0));
        let d = closest - 1.0;
        if d < 0.6 {
            let sunside = 0.55 + 0.45 * dot(normalize(ro + rd * -b), sun);
            col += vec3(0.25, 0.58, 1.0) * exp(-max(d, 0.0) * 17.0) * 0.34 * sunside;
        }
    }

    return FSOut(vec4(col, 1.0), depth);
}
