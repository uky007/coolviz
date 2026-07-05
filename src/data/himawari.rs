//! Himawari-9 live full-disk imagery (NICT). Polls `latest.json` (cheap)
//! every few minutes and downloads the 4x4 tile mosaic (2200x2200) only when
//! a new image timestamp appears (the source updates every 10 minutes).
//! Cached to disk for offline boots.
//! Imagery courtesy of NICT — non-commercial use.

use std::sync::mpsc::Sender;

use anyhow::Context;

use super::{DataMsg, cache_path, http_get};

const LEVEL: u32 = 4; // 4x4 tiles of 550px = 2200x2200
const TILE: u32 = 550;
const POLL_SECS: u64 = 180;

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
    anyhow::ensure!(date.len() >= 19, "unexpected timestamp: {date}");
    Ok(date.to_string()) // "2026-07-05 05:40:00"
}

fn fetch_tiles(ts: &str) -> anyhow::Result<CloudImage> {
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
            anyhow::ensure!(
                tile.width() == TILE && tile.height() == TILE,
                "bad tile size"
            );
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

fn save_cache(img: &CloudImage, ts: &str) {
    let (png, txt) = cache_files();
    let buf: Option<image::RgbaImage> = image::RgbaImage::from_raw(img.w, img.h, img.rgba.clone());
    if let Some(buf) = buf {
        if let Err(e) = buf.save(&png) {
            log::warn!("himawari cache save failed: {e}");
            return;
        }
        std::fs::write(&txt, format!("{ts}\n{}", img.label)).ok();
    }
}

/// Returns the cached image plus the timestamp it was taken at.
fn load_cache() -> Option<(CloudImage, String)> {
    let (png, txt) = cache_files();
    let meta = std::fs::read_to_string(&txt).ok()?;
    let mut lines = meta.lines();
    let ts = lines.next()?.to_string();
    let label = lines.next().unwrap_or("Himawari-9").to_string();
    let img = image::open(&png).ok()?.to_rgba8();
    Some((
        CloudImage {
            w: img.width(),
            h: img.height(),
            rgba: img.into_raw(),
            label: format!("{label} (cache)"),
        },
        ts,
    ))
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
    let mut loaded_ts = String::new();
    if let Some((c, ts)) = load_cache() {
        log::info!("himawari: cached image {}", c.label);
        loaded_ts = ts;
        if !send(&tx, c) {
            return;
        }
    }
    loop {
        match latest_timestamp() {
            Ok(ts) if ts != loaded_ts => match fetch_tiles(&ts) {
                Ok(c) => {
                    log::info!("himawari: fetched {}", c.label);
                    save_cache(&c, &ts);
                    loaded_ts = ts;
                    if !send(&tx, c) {
                        return;
                    }
                }
                Err(e) => log::warn!("himawari tile fetch failed: {e:#}"),
            },
            Ok(_) => {} // no new image yet
            Err(e) => log::warn!("himawari latest.json failed: {e:#}"),
        }
        std::thread::sleep(std::time::Duration::from_secs(POLL_SECS));
    }
}

/// One-shot fetch for headless screenshots (falls back to cache).
pub fn fetch_once() -> Option<CloudImage> {
    match latest_timestamp().and_then(|ts| fetch_tiles(&ts).map(|c| (c, ts))) {
        Ok((c, ts)) => {
            save_cache(&c, &ts);
            Some(c)
        }
        Err(e) => {
            log::warn!("himawari one-shot failed ({e}), trying cache");
            load_cache().map(|(c, _)| c)
        }
    }
}
