//! GSI elevation tiles (dem5a) resampled onto the Tokyo flood-sim grid.
//! 出典: 国土地理院 標高タイル.

use std::sync::mpsc::Sender;

use super::{DataMsg, cache_path, http_get, plateau};

pub const GRID_N: u32 = 512;
pub const CELL_M: f32 = 6.5; // ~3.3 km square around Tokyo Station

const Z: u32 = 15;

fn tile_xy(lon: f64, lat: f64) -> (f64, f64) {
    let n = f64::from(1u32 << Z);
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n;
    (x, y)
}

struct Mosaic {
    px0: f64, // global pixel coords of mosaic origin
    py0: f64,
    w: usize,
    h: usize,
    data: Vec<f32>,
}

fn decode_elev(p: [u8; 3]) -> Option<f32> {
    let x = (p[0] as i64) * 65536 + (p[1] as i64) * 256 + p[2] as i64;
    if x == 8_388_608 {
        return None; // NA (sea / water)
    }
    let x = if x > 8_388_608 { x - 16_777_216 } else { x };
    Some(x as f32 * 0.01)
}

pub fn load_terrain() -> anyhow::Result<(Vec<f32>, [f32; 2])> {
    // Grid extent in world meters (x east, z south from the site).
    let half = GRID_N as f32 * CELL_M * 0.5;
    let origin = [-half, -half];

    let m_lon = 111_320.0 * plateau::SITE_LAT.to_radians().cos();
    let m_lat = 111_132.0;
    let lon0 = plateau::SITE_LON - half as f64 / m_lon;
    let lon1 = plateau::SITE_LON + half as f64 / m_lon;
    let lat0 = plateau::SITE_LAT + half as f64 / m_lat; // north
    let lat1 = plateau::SITE_LAT - half as f64 / m_lat; // south

    let (fx0, fy0) = tile_xy(lon0, lat0);
    let (fx1, fy1) = tile_xy(lon1, lat1);
    let (tx0, ty0) = (fx0.floor() as i64, fy0.floor() as i64);
    let (tx1, ty1) = (fx1.floor() as i64, fy1.floor() as i64);

    let w = ((tx1 - tx0 + 1) * 256) as usize;
    let h = ((ty1 - ty0 + 1) * 256) as usize;
    let mut mosaic = Mosaic {
        px0: tx0 as f64 * 256.0,
        py0: ty0 as f64 * 256.0,
        w,
        h,
        data: vec![f32::NAN; w * h],
    };

    let dir = cache_path("gsi");
    std::fs::create_dir_all(&dir).ok();
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let name = format!("dem5a_{Z}_{tx}_{ty}.png");
            let path = dir.join(&name);
            let bytes = match std::fs::read(&path) {
                Ok(b) if !b.is_empty() => b,
                _ => {
                    let url =
                        format!("https://cyberjapandata.gsi.go.jp/xyz/dem5a_png/{Z}/{tx}/{ty}.png");
                    match http_get(&url, 30) {
                        Ok(b) => {
                            std::fs::write(&path, &b).ok();
                            b
                        }
                        Err(e) => {
                            log::warn!("gsi tile {tx}/{ty}: {e}");
                            continue;
                        }
                    }
                }
            };
            let Ok(img) = image::load_from_memory(&bytes) else {
                continue;
            };
            let img = img.to_rgb8();
            let ox = ((tx - tx0) * 256) as usize;
            let oy = ((ty - ty0) * 256) as usize;
            for (px, py, p) in img.enumerate_pixels() {
                if let Some(e) = decode_elev([p[0], p[1], p[2]]) {
                    mosaic.data[(oy + py as usize) * w + ox + px as usize] = e;
                }
            }
        }
    }

    // Resample onto the sim grid (nearest is plenty at 5 m source data).
    let mut grid = vec![0.0f32; (GRID_N * GRID_N) as usize];
    for gy in 0..GRID_N {
        for gx in 0..GRID_N {
            let wx = origin[0] as f64 + (gx as f64 + 0.5) * CELL_M as f64;
            let wz = origin[1] as f64 + (gy as f64 + 0.5) * CELL_M as f64;
            let lon = plateau::SITE_LON + wx / m_lon;
            let lat = plateau::SITE_LAT - wz / m_lat;
            let (px, py) = tile_xy(lon, lat);
            let mx = ((px * 256.0 - mosaic.px0) as usize).min(mosaic.w - 1);
            let my = ((py * 256.0 - mosaic.py0) as usize).min(mosaic.h - 1);
            let e = mosaic.data[my * mosaic.w + mx];
            // NA cells are the palace moats / rivers: treat as low water land.
            grid[(gy * GRID_N + gx) as usize] = if e.is_nan() { 1.2 } else { e };
        }
    }
    log::info!(
        "terrain: {}x{} grid, {:.1} m cells (GSI dem5a)",
        GRID_N,
        GRID_N,
        CELL_M
    );
    Ok((grid, origin))
}

pub fn run(tx: Sender<DataMsg>) {
    match load_terrain() {
        Ok((heights, origin)) => {
            let _ = tx.send(DataMsg::Terrain {
                n: GRID_N,
                cell: CELL_M,
                origin,
                heights,
            });
        }
        Err(e) => {
            log::warn!("terrain load failed: {e:#}");
            let _ = tx.send(DataMsg::Note(format!("terrain failed: {e}")));
        }
    }
}
