//! OSM road network around the Tokyo site (Overpass, cached forever).
//! Produces ground ribbons, street-lamp positions, and the polylines the
//! car simulation drives on.

use super::{DataMsg, LightGpu, cache_path, http_get, plateau};
use std::sync::mpsc::Sender;

const QUERY: &str = concat!(
    "[out:json][timeout:60];",
    "way[\"highway\"~\"^(motorway|motorway_link|trunk|trunk_link|primary|secondary|",
    "tertiary|unclassified|residential)$\"]",
    "(35.664,139.745,35.698,139.787);out geom;"
);

pub struct RoadPath {
    /// Local meters (x east, z south).
    pub pts: Vec<[f32; 2]>,
    pub width: f32,
    pub class: u8, // 0 motorway .. 5 minor
}

pub struct RoadNet {
    pub paths: Vec<RoadPath>,
    pub ribbon_verts: Vec<[f32; 4]>, // x, y, z, class
    pub ribbon_indices: Vec<u32>,
    pub lamps: Vec<LightGpu>,
}

fn class_of(hw: &str) -> (u8, f32) {
    match hw {
        "motorway" => (0, 15.0),
        "motorway_link" | "trunk_link" => (1, 7.5),
        "trunk" => (1, 13.0),
        "primary" => (2, 11.0),
        "secondary" => (3, 9.0),
        "tertiary" => (4, 7.0),
        _ => (5, 5.5),
    }
}

fn to_local(lon: f64, lat: f64) -> [f32; 2] {
    let m_lon = 111_320.0 * plateau::SITE_LAT.to_radians().cos();
    let m_lat = 111_132.0;
    [
        ((lon - plateau::SITE_LON) * m_lon) as f32,
        ((plateau::SITE_LAT - lat) * m_lat) as f32,
    ]
}

pub fn load_roads() -> anyhow::Result<RoadNet> {
    let cache = cache_path("osm_roads.json");
    let bytes = match std::fs::read(&cache) {
        Ok(b) if !b.is_empty() => b,
        _ => {
            let url = format!(
                "https://overpass-api.de/api/interpreter?data={}",
                urlencode(QUERY)
            );
            let b = http_get(&url, 90)?;
            std::fs::write(&cache, &b).ok();
            b
        }
    };
    let v: serde_json::Value = serde_json::from_slice(&bytes)?;
    let empty = Vec::new();
    let mut paths = Vec::new();
    for el in v["elements"].as_array().unwrap_or(&empty) {
        let tags = &el["tags"];
        // Tunnels are invisible from above; skip them.
        if tags["tunnel"].as_str().is_some() {
            continue;
        }
        let hw = tags["highway"].as_str().unwrap_or("");
        let (class, width) = class_of(hw);
        let pts: Vec<[f32; 2]> = el["geometry"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|g| {
                let (Some(lat), Some(lon)) = (g["lat"].as_f64(), g["lon"].as_f64()) else {
                    return None;
                };
                Some(to_local(lon, lat))
            })
            .collect();
        if pts.len() >= 2 {
            paths.push(RoadPath { pts, width, class });
        }
    }
    anyhow::ensure!(!paths.is_empty(), "no roads parsed");

    // Ribbons: one quad per segment, extended by half a width at each end so
    // bends stay closed. Small per-way height offsets avoid z-fighting.
    let mut verts: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut lamps: Vec<LightGpu> = Vec::new();
    let mut hash_state = 0x9E37u32;
    let mut hashf = || {
        hash_state = hash_state.wrapping_mul(747796405).wrapping_add(2891336453);
        (hash_state >> 9) as f32 / 8_388_607.0
    };
    for (wi, road) in paths.iter().enumerate() {
        let y = 0.06 + (wi % 9) as f32 * 0.012;
        let hw = road.width * 0.5;
        let c = road.class as f32;
        for seg in road.pts.windows(2) {
            let (a, b) = (glam::Vec2::from(seg[0]), glam::Vec2::from(seg[1]));
            let d = (b - a).normalize_or_zero();
            if d == glam::Vec2::ZERO {
                continue;
            }
            let (a, b) = (a - d * hw, b + d * hw);
            let p = glam::Vec2::new(-d.y, d.x) * hw;
            let base = verts.len() as u32;
            verts.push([a.x - p.x, y, a.y - p.y, c]);
            verts.push([a.x + p.x, y, a.y + p.y, c]);
            verts.push([b.x - p.x, y, b.y - p.y, c]);
            verts.push([b.x + p.x, y, b.y + p.y, c]);
            indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
        }
        // Street lamps along everything bigger than residential.
        if road.class <= 4 {
            let mut acc = 13.0f32;
            let mut side = if wi % 2 == 0 { 1.0f32 } else { -1.0 };
            for seg in road.pts.windows(2) {
                let (a, b) = (glam::Vec2::from(seg[0]), glam::Vec2::from(seg[1]));
                let len = a.distance(b);
                let d = (b - a).normalize_or_zero();
                let perp = glam::Vec2::new(-d.y, d.x);
                let mut s = acc;
                while s < len {
                    let pos = a + d * s + perp * side * (hw + 1.6);
                    lamps.push(LightGpu {
                        pos: [pos.x, 5.6, pos.y, 0.0],
                        aux: [hashf(), 0.0, 0.0, 1.0],
                    });
                    side = -side;
                    s += 26.0;
                }
                acc = s - len;
            }
        }
    }
    log::info!(
        "roads: {} ways, {} ribbon tris, {} lamps",
        paths.len(),
        indices.len() / 3,
        lamps.len()
    );
    Ok(RoadNet {
        paths,
        ribbon_verts: verts,
        ribbon_indices: indices,
        lamps,
    })
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn run(tx: Sender<DataMsg>) {
    match load_roads() {
        Ok(net) => {
            let _ = tx.send(DataMsg::Roads {
                paths: net.paths.into_iter().map(|p| (p.pts, p.class)).collect(),
                ribbon_verts: net.ribbon_verts,
                ribbon_indices: net.ribbon_indices,
                lamps: net.lamps,
            });
        }
        Err(e) => {
            log::warn!("roads load failed: {e:#}");
            let _ = tx.send(DataMsg::Note(format!("road network failed: {e}")));
        }
    }
}
