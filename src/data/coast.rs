//! Natural Earth 50m coastline -> line-strip vertex/index buffers
//! (u32 indices with 0xFFFFFFFF primitive-restart separators).

use crate::astro::latlon_to_world;

use super::{asset_path, read_gz};

const COAST_RADIUS: f32 = 1.0018;

fn push_line(coords: &[serde_json::Value], verts: &mut Vec<[f32; 3]>, idx: &mut Vec<u32>) {
    if coords.len() < 2 {
        return;
    }
    let start = verts.len();
    for c in coords {
        let (Some(lon), Some(lat)) = (c[0].as_f64(), c[1].as_f64()) else {
            continue;
        };
        let p = latlon_to_world(lat as f32, lon as f32) * COAST_RADIUS;
        verts.push([p.x, p.y, p.z]);
    }
    if verts.len() - start < 2 {
        verts.truncate(start);
        return;
    }
    for i in start..verts.len() {
        idx.push(i as u32);
    }
    idx.push(u32::MAX);
}

pub fn load() -> anyhow::Result<(Vec<[f32; 3]>, Vec<u32>)> {
    let bytes = read_gz(&asset_path("ne_50m_coastline.geojson.gz"))?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)?;
    let mut verts = Vec::new();
    let mut idx = Vec::new();
    let empty = Vec::new();
    for f in v["features"].as_array().unwrap_or(&empty) {
        let geom = &f["geometry"];
        match geom["type"].as_str().unwrap_or("") {
            "LineString" => {
                if let Some(c) = geom["coordinates"].as_array() {
                    push_line(c, &mut verts, &mut idx);
                }
            }
            "MultiLineString" => {
                if let Some(lines) = geom["coordinates"].as_array() {
                    for l in lines {
                        if let Some(c) = l.as_array() {
                            push_line(c, &mut verts, &mut idx);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    anyhow::ensure!(!verts.is_empty(), "no coastline geometry parsed");
    Ok((verts, idx))
}
