//! USGS earthquake feed (M2.5+, last 24 h), refreshed every 5 minutes.

use std::sync::mpsc::Sender;

use super::{http_get, DataMsg, QuakeCpu};

const URL: &str =
    "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/2.5_day.geojson";

pub fn fetch_once() -> anyhow::Result<(Vec<QuakeCpu>, String)> {
    let bytes = http_get(URL, 30)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)?;
    let empty = Vec::new();
    let features = v["features"].as_array().unwrap_or(&empty);
    let mut list = Vec::new();
    let mut max_mag = 0.0f32;
    for f in features {
        let Some(mag) = f["properties"]["mag"].as_f64() else {
            continue;
        };
        let Some(time) = f["properties"]["time"].as_i64() else {
            continue;
        };
        let coords = &f["geometry"]["coordinates"];
        let (Some(lon), Some(lat)) = (coords[0].as_f64(), coords[1].as_f64()) else {
            continue;
        };
        let q = QuakeCpu {
            lon: lon as f32,
            lat: lat as f32,
            mag: mag as f32,
            unix_ms: time,
        };
        max_mag = max_mag.max(q.mag);
        list.push(q);
    }
    let label = format!("{} ev/24h · max M{:.1}", list.len(), max_mag);
    Ok((list, label))
}

pub fn run(tx: Sender<DataMsg>) {
    loop {
        match fetch_once() {
            Ok((list, label)) => {
                if tx.send(DataMsg::Quakes { list, label }).is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_secs(300));
            }
            Err(e) => {
                log::warn!("quake fetch failed: {e:#}");
                let _ = tx.send(DataMsg::Note(format!("quake feed error: {e}")));
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        }
    }
}
