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

#[cfg(test)]
mod tests {
    #[test]
    fn loads_coastline_strips_on_the_sphere() {
        let (verts, idx) = super::load().expect("coastline loads");
        assert!(verts.len() > 50_000, "only {} vertices", verts.len());
        assert!(idx.contains(&u32::MAX), "no primitive-restart separators");
        for v in verts.iter().step_by(997) {
            let r = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            assert!(
                (r - super::COAST_RADIUS).abs() < 1e-3,
                "vertex off sphere: {r}"
            );
        }
    }
}
