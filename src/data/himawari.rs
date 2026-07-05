//! Himawari-9 live full-disk imagery (NICT). Fetches the 4x4 tile mosaic
//! (2200x2200) every 10 minutes; cached to disk for offline boots.
//! Imagery courtesy of NICT — non-commercial use.

use std::sync::mpsc::Sender;

use anyhow::Context;

use super::{cache_path, http_get, DataMsg};

const LEVEL: u32 = 4; // 4x4 tiles of 550px = 2200x2200
const TILE: u32 = 550;

pub struct CloudImage {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    pub label: String,
}

fn latest_timestamp() -> anyhow::Result<String> {
    let bytes = http_get("https://himawari8.nict.go.jp/img/D531106/latest.json", 30)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)?;
    let date = v["date"].as_str().context("no date in latest.json")?;
    Ok(date.to_string()) // "2026-07-05 05:40:00"
}

fn fetch_disk() -> anyhow::Result<CloudImage> {
    let ts = latest_timestamp()?;
    // "YYYY-MM-DD HH:MM:SS" -> path parts
    anyhow::ensure!(ts.len() >= 19, "unexpected timestamp: {ts}");
    let (date, time) = (&ts[0..10], &ts[11..19]);
    let ymd: Vec<&str> = date.split('-').collect();
    let hms: String = time.chars().filter(|c| *c != ':').collect();
    anyhow::ensure!(ymd.len() == 3, "unexpected date: {date}");

    let size = LEVEL * TILE;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for ty in 0..LEVEL {
        for tx in 0..LEVEL {
            let url = format!(
                "https://himawari8.nict.go.jp/img/D531106/{LEVEL}d/{TILE}/{}/{}/{}/{}_{}_{}.png",
                ymd[0], ymd[1], ymd[2], hms, tx, ty
            );
            let png = http_get(&url, 60)?;
            let tile = image::load_from_memory(&png)
                .with_context(|| format!("decode tile {tx},{ty}"))?
                .to_rgb8();
            anyhow::ensure!(tile.width() == TILE && tile.height() == TILE, "bad tile size");
            for row in 0..TILE {
                for col in 0..TILE {
                    let p = tile.get_pixel(col, row);
                    let x = tx * TILE + col;
                    let y = ty * TILE + row;
                    let o = ((y * size + x) * 4) as usize;
                    rgba[o] = p[0];
                    rgba[o + 1] = p[1];
                    rgba[o + 2] = p[2];
                    rgba[o + 3] = 255;
                }
            }
        }
    }
    Ok(CloudImage {
        w: size,
        h: size,
        rgba,
        label: format!("Himawari-9 {} UTC", &ts[11..16]),
    })
}

fn cache_files() -> (std::path::PathBuf, std::path::PathBuf) {
    (cache_path("himawari.png"), cache_path("himawari.txt"))
}

fn save_cache(img: &CloudImage) {
    let (png, txt) = cache_files();
    let buf: Option<image::RgbaImage> =
        image::RgbaImage::from_raw(img.w, img.h, img.rgba.clone());
    if let Some(buf) = buf {
        if let Err(e) = buf.save(&png) {
            log::warn!("himawari cache save failed: {e}");
            return;
        }
        std::fs::write(&txt, &img.label).ok();
    }
}

fn load_cache() -> Option<CloudImage> {
    let (png, txt) = cache_files();
    let label = std::fs::read_to_string(&txt).ok()?;
    let img = image::open(&png).ok()?.to_rgba8();
    Some(CloudImage {
        w: img.width(),
        h: img.height(),
        rgba: img.into_raw(),
        label: format!("{label} (cache)"),
    })
}

fn send(tx: &Sender<DataMsg>, c: CloudImage) -> bool {
    tx.send(DataMsg::Clouds {
        w: c.w,
        h: c.h,
        rgba: c.rgba,
        label: c.label,
    })
    .is_ok()
}

pub fn run(tx: Sender<DataMsg>) {
    if let Some(c) = load_cache() {
        log::info!("himawari: cached image {}", c.label);
        if !send(&tx, c) {
            return;
        }
    }
    let mut last_label = String::new();
    loop {
        match fetch_disk() {
            Ok(c) => {
                if c.label != last_label {
                    last_label = c.label.clone();
                    log::info!("himawari: fetched {}", c.label);
                    save_cache(&c);
                    if !send(&tx, c) {
                        return;
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(300));
            }
            Err(e) => {
                log::warn!("himawari fetch failed: {e:#}");
                std::thread::sleep(std::time::Duration::from_secs(120));
            }
        }
    }
}

/// One-shot fetch for headless screenshots (falls back to cache).
pub fn fetch_once() -> Option<CloudImage> {
    match fetch_disk() {
        Ok(c) => {
            save_cache(&c);
            Some(c)
        }
        Err(e) => {
            log::warn!("himawari one-shot failed ({e}), trying cache");
            load_cache()
        }
    }
}
