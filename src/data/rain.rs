//! JMA 高解像度降水ナウキャスト tiles for the Tokyo local scene.
//! Unofficial endpoint (the one jma.go.jp itself uses); fetched politely
//! every 5 minutes, 3x3 slippy tiles at z=12 around the site.

use std::sync::mpsc::Sender;

use anyhow::Context;

use super::{DataMsg, http_get};

const Z: u32 = 12;
const N: u32 = 3; // NxN tiles
pub const TEX_SIZE: u32 = 256 * N;

/// JMA nowcast palette -> intensity level 0..=8.
fn level_of(rgba: [u8; 4]) -> u8 {
    if rgba[3] < 40 {
        return 0;
    }
    match (rgba[0], rgba[1], rgba[2]) {
        (242, 242, 255) => 1,
        (160, 210, 255) => 2,
        (33, 140, 255) => 3,
        (0, 65, 255) => 4,
        (250, 245, 0) => 5,
        (255, 153, 0) => 6,
        (255, 40, 0) => 7,
        (180, 0, 104) => 8,
        _ => 1, // unknown but present: treat as light rain
    }
}

fn tile_xy(lon: f64, lat: f64) -> (u32, u32) {
    let n = f64::from(1u32 << Z);
    let x = ((lon + 180.0) / 360.0 * n) as u32;
    let lat_r = lat.to_radians();
    let y = ((1.0 - lat_r.tan().asinh() / std::f64::consts::PI) / 2.0 * n) as u32;
    (x, y)
}

fn tile_bounds(x: u32, y: u32) -> (f64, f64, f64, f64) {
    // (lon_w, lat_n, lon_e, lat_s)
    let n = f64::from(1u32 << Z);
    let lon = |x: f64| x / n * 360.0 - 180.0;
    let lat = |y: f64| {
        (std::f64::consts::PI * (1.0 - 2.0 * y / n))
            .sinh()
            .atan()
            .to_degrees()
    };
    (
        lon(x as f64),
        lat(y as f64),
        lon((x + 1) as f64),
        lat((y + 1) as f64),
    )
}

pub struct RainGrid {
    pub size: u32,
    pub levels: Vec<u8>,
    /// Geographic bounds of the composite: lon_w, lat_n, lon_e, lat_s.
    pub bounds: [f64; 4],
    pub label: String,
    pub max_level: u8,
}

fn latest_basetime() -> anyhow::Result<String> {
    let bytes = http_get(
        "https://www.jma.go.jp/bosai/jmatile/data/nowc/targetTimes_N1.json",
        30,
    )?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)?;
    let bt = v[0]["basetime"].as_str().context("no basetime")?;
    Ok(bt.to_string())
}

pub fn fetch_once(site_lon: f64, site_lat: f64) -> anyhow::Result<RainGrid> {
    let bt = latest_basetime()?;
    let (cx, cy) = tile_xy(site_lon, site_lat);
    let (x0, y0) = (cx - N / 2, cy - N / 2);
    let mut levels = vec![0u8; (TEX_SIZE * TEX_SIZE) as usize];
    let mut max_level = 0u8;
    for ty in 0..N {
        for tx in 0..N {
            let url = format!(
                "https://www.jma.go.jp/bosai/jmatile/data/nowc/{bt}/none/{bt}/surf/hrpns/{Z}/{}/{}.png",
                x0 + tx,
                y0 + ty
            );
            // Missing tiles (no rain anywhere in them) can 404: treat as dry.
            let Ok(png) = http_get(&url, 30) else {
                continue;
            };
            let Ok(img) = image::load_from_memory(&png) else {
                continue;
            };
            let img = img.to_rgba8();
            for (px, py, p) in img.enumerate_pixels() {
                let l = level_of(p.0);
                if l > 0 {
                    let gx = tx * 256 + px;
                    let gy = ty * 256 + py;
                    levels[(gy * TEX_SIZE + gx) as usize] = l;
                    max_level = max_level.max(l);
                }
            }
        }
    }
    let (w, n_, _, _) = tile_bounds(x0, y0);
    let (_, _, e, s) = tile_bounds(x0 + N - 1, y0 + N - 1);
    let label = format!(
        "JMA nowcast {}:{} · max lv{max_level}",
        &bt[8..10],
        &bt[10..12]
    );
    Ok(RainGrid {
        size: TEX_SIZE,
        levels,
        bounds: [w, n_, e, s],
        label,
        max_level,
    })
}

pub fn run(tx: Sender<DataMsg>, site_lon: f64, site_lat: f64) {
    let mut last_bt = String::new();
    loop {
        match latest_basetime() {
            Ok(bt) if bt != last_bt => match fetch_once(site_lon, site_lat) {
                Ok(g) => {
                    last_bt = bt;
                    if tx
                        .send(DataMsg::Rain {
                            size: g.size,
                            levels: g.levels,
                            bounds: g.bounds,
                            label: g.label,
                            max_level: g.max_level,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                Err(e) => log::warn!("rain fetch failed: {e:#}"),
            },
            Ok(_) => {}
            Err(e) => log::warn!("rain basetime failed: {e:#}"),
        }
        std::thread::sleep(std::time::Duration::from_secs(120));
    }
}
