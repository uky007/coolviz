//! Rasterize Natural Earth 110m land polygons into an equirectangular
//! grayscale mask (even-odd scanline fill). Used by the globe shader to tint
//! land vs ocean — no texture assets needed.

use super::{asset_path, read_gz};

pub fn build(w: u32, h: u32) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let bytes = read_gz(&asset_path("ne_110m_land.geojson.gz"))?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)?;

    // Collect all ring edges (lon0, lat0, lon1, lat1).
    let mut edges: Vec<[f64; 4]> = Vec::new();
    let mut add_ring = |ring: &Vec<serde_json::Value>| {
        for pair in ring.windows(2) {
            let (Some(x0), Some(y0)) = (pair[0][0].as_f64(), pair[0][1].as_f64()) else {
                continue;
            };
            let (Some(x1), Some(y1)) = (pair[1][0].as_f64(), pair[1][1].as_f64()) else {
                continue;
            };
            if y0 != y1 {
                edges.push([x0, y0, x1, y1]);
            }
        }
    };

    let empty = Vec::new();
    for f in v["features"].as_array().unwrap_or(&empty) {
        let geom = &f["geometry"];
        match geom["type"].as_str().unwrap_or("") {
            "Polygon" => {
                if let Some(rings) = geom["coordinates"].as_array() {
                    for r in rings {
                        if let Some(r) = r.as_array() {
                            add_ring(r);
                        }
                    }
                }
            }
            "MultiPolygon" => {
                if let Some(polys) = geom["coordinates"].as_array() {
                    for p in polys {
                        for r in p.as_array().unwrap_or(&empty) {
                            if let Some(r) = r.as_array() {
                                add_ring(r);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    anyhow::ensure!(!edges.is_empty(), "no land polygons parsed");

    let mut mask = vec![0u8; (w * h) as usize];
    let mut xs: Vec<f64> = Vec::with_capacity(64);
    for y in 0..h {
        let lat = 90.0 - (y as f64 + 0.5) * 180.0 / h as f64;
        xs.clear();
        for e in &edges {
            let (y0, y1) = (e[1], e[3]);
            if (y0 > lat) != (y1 > lat) {
                xs.push(e[0] + (lat - y0) * (e[2] - e[0]) / (y1 - y0));
            }
        }
        xs.sort_by(f64::total_cmp);
        let row = (y * w) as usize;
        for pair in xs.chunks_exact(2) {
            let a = (((pair[0] + 180.0) / 360.0) * w as f64).round() as i64;
            let b = (((pair[1] + 180.0) / 360.0) * w as f64).round() as i64;
            for x in a.clamp(0, w as i64)..b.clamp(0, w as i64) {
                mask[row + x as usize] = 255;
            }
        }
    }

    // The 110m Antarctica ring stops at the -90 edge; make sure the pole row
    // south of -89 is solid land to avoid a pinhole.
    let south_row = ((90.0 + 89.0) / 180.0 * h as f64) as u32;
    for y in south_row..h {
        let row = (y * w) as usize;
        // Only fill if the row already contains substantial land (Antarctica).
        let filled = mask[row..row + w as usize].iter().filter(|&&m| m > 0).count();
        if filled > (w as usize) / 4 {
            for x in 0..w as usize {
                mask[row + x] = 255;
            }
        }
    }

    Ok((w, h, mask))
}
