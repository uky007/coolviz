// Shallow-water simulation (virtual-pipe model) shared by TOKYO FLOOD and
// the Okinawa lagoon. Two compute passes per substep (flux, then height),
// plus a blit into an rgba16float texture for the render passes:
// r = terrain, g = water depth, b = |velocity|.

struct SweParams {
    a: vec4<f32>, // x dt, y cell size (m), z N, w gravity
    b: vec4<f32>, // x rain rate (m/s at level 1.0), y source mode, z sim time, w flux damping
    c: vec4<f32>, // splash: x,y grid coords, z radius (cells), w amount (m)
    d: vec4<f32>, // x sea level, y swell amp (m), z swell omega, w flood inflow (m/s)
    e: vec4<f32>, // rain-uv affine: uv = e.xy + grid_uv * e.zw
};

@group(0) @binding(0) var<uniform> P: SweParams;
@group(0) @binding(1) var<storage, read> terr: array<f32>;
@group(0) @binding(2) var<storage, read> h_in: array<f32>;
@group(0) @binding(3) var<storage, read> flux_in: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> h_out: array<f32>;
@group(0) @binding(5) var<storage, read_write> flux_out: array<vec4<f32>>;
@group(0) @binding(6) var rain_tex: texture_2d<f32>;
@group(0) @binding(7) var rain_samp: sampler;
@group(0) @binding(8) var out_tex: texture_storage_2d<rgba16float, write>;

fn n_of() -> u32 {
    return u32(P.a.z);
}

fn idx_of(x: u32, y: u32) -> u32 {
    return y * n_of() + x;
}

fn head(x: u32, y: u32) -> f32 {
    let i = idx_of(x, y);
    return terr[i] + h_in[i];
}

// Outflow fluxes to +x, -x, +y, -y.
@compute @workgroup_size(16, 16)
fn cs_flux(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = n_of();
    if gid.x >= n || gid.y >= n {
        return;
    }
    let i = idx_of(gid.x, gid.y);
    let dt = P.a.x;
    let cell = P.a.y;
    let g = P.a.w;
    let h_here = h_in[i];
    let big_h = terr[i] + h_here;

    // Out-of-domain neighbours look like dry land at the local terrain
    // height, so water can drain freely off the open edges.
    var hn = vec4(terr[i]);
    if gid.x + 1u < n {
        hn.x = head(gid.x + 1u, gid.y);
    }
    if gid.x >= 1u {
        hn.y = head(gid.x - 1u, gid.y);
    }
    if gid.y + 1u < n {
        hn.z = head(gid.x, gid.y + 1u);
    }
    if gid.y >= 1u {
        hn.w = head(gid.x, gid.y - 1u);
    }

    let k = dt * g * cell; // pipe coefficient
    var f = flux_in[i] * P.b.w;
    f = max(f + k * (vec4(big_h) - hn), vec4(0.0));

    // Never drain more water than the cell holds.
    let total = (f.x + f.y + f.z + f.w) * dt;
    let avail = h_here * cell * cell;
    if total > 1e-9 {
        f *= min(1.0, avail / total);
    }
    flux_out[i] = f;
}

@compute @workgroup_size(16, 16)
fn cs_height(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = n_of();
    if gid.x >= n || gid.y >= n {
        return;
    }
    let i = idx_of(gid.x, gid.y);
    let dt = P.a.x;
    let cell = P.a.y;

    let out = flux_out[i];
    var inflow = 0.0;
    if gid.x + 1u < n {
        inflow += flux_out[idx_of(gid.x + 1u, gid.y)].y;
    }
    if gid.x >= 1u {
        inflow += flux_out[idx_of(gid.x - 1u, gid.y)].x;
    }
    if gid.y + 1u < n {
        inflow += flux_out[idx_of(gid.x, gid.y + 1u)].w;
    }
    if gid.y >= 1u {
        inflow += flux_out[idx_of(gid.x, gid.y - 1u)].z;
    }
    let outflow = out.x + out.y + out.z + out.w;

    var h = h_in[i] + dt * (inflow - outflow) / (cell * cell);

    // Rain source, modulated by the nowcast texture.
    if P.b.x > 0.0 {
        let guv = vec2<f32>(f32(gid.x), f32(gid.y)) / P.a.z;
        let ruv = P.e.xy + guv * P.e.zw;
        if ruv.x >= 0.0 && ruv.x <= 1.0 && ruv.y >= 0.0 && ruv.y <= 1.0 {
            let lvl = textureSampleLevel(rain_tex, rain_samp, ruv, 0.0).r;
            h += P.b.x * lvl * dt;
        }
    }

    let mode = u32(P.b.y + 0.5);
    // River-flood scenario: the DEM marks waterways (Nihonbashi/Kanda
    // rivers, palace moats) as voids we filled at 1.2 m; buildings are
    // stamped far above that, so this band is exactly the channels. Their
    // stage rises with sim time (capped), spilling into the streets.
    if mode == 1u && terr[i] > 1.0 && terr[i] < 1.45 {
        let stage = P.d.x + min(P.d.w * P.b.z, 4.0);
        h = max(h, stage - terr[i]);
    }
    // Ocean swell: relax the deep border band toward an inward-travelling
    // circular wave centred on the island so all four edges emit coherently.
    // Mode 2 repurposes P.e: xy = island centre (grid cells), z = k (rad/cell).
    if mode == 2u {
        let b = terr[i];
        let border = min(min(gid.x, gid.y), min(n - 1u - gid.x, n - 1u - gid.y));
        if b < -8.0 && border < 10u {
            let dp = vec2<f32>(f32(gid.x), f32(gid.y)) + 0.5 - P.e.xy;
            let phase = P.d.z * P.b.z + length(dp) * P.e.z;
            let h_goal = max(P.d.x + P.d.y * sin(phase) - b, 0.0);
            let w = mix(0.55, 0.04, f32(border) / 10.0);
            h = mix(h, h_goal, w);
        }
    }

    // Splash (mouse interaction).
    if P.c.w != 0.0 {
        let dp = vec2<f32>(f32(gid.x), f32(gid.y)) - P.c.xy;
        let r2 = P.c.z * P.c.z;
        h += P.c.w * exp(-dot(dp, dp) / r2);
    }

    h_out[i] = max(h, 0.0);
}

@compute @workgroup_size(16, 16)
fn cs_blit(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = n_of();
    if gid.x >= n || gid.y >= n {
        return;
    }
    let i = idx_of(gid.x, gid.y);
    let h = h_out[i];
    let f = flux_out[i];
    // Mean speed from net pipe flux.
    let q = vec2(f.x - f.y, f.z - f.w) / max(P.a.y * max(h, 0.12), 1e-4);
    let vel = length(q);
    textureStore(
        out_tex,
        vec2<i32>(i32(gid.x), i32(gid.y)),
        vec4(terr[i], h, vel, 1.0),
    );
}
