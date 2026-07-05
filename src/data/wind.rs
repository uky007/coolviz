//! GFS 10 m wind: NOMADS GRIB-filter fetch, gribberish decode, snapshot and
//! synthetic fallbacks.

use std::sync::mpsc::Sender;

use anyhow::Context;
use chrono::{Datelike, DurationRound, Timelike, Utc};
use half::f16;

use super::{asset_path, http_get, DataMsg, Source};

pub struct WindGrid {
    pub w: u32,
    pub h: u32,
    pub data: Vec<[u16; 2]>,
    pub label: String,
    pub source: Source,
    pub cycle_key: String,
}

fn f16_pair(u: f64, v: f64) -> [u16; 2] {
    let su = if u.is_finite() { u as f32 } else { 0.0 };
    let sv = if v.is_finite() { v as f32 } else { 0.0 };
    [f16::from_f32(su).to_bits(), f16::from_f32(sv).to_bits()]
}

fn decode(bytes: &[u8], cycle_key: &str, source: Source) -> anyhow::Result<WindGrid> {
    let mut ug: Option<Vec<f64>> = None;
    let mut vg: Option<Vec<f64>> = None;
    for m in gribberish::message::read_messages(bytes) {
        let abbrev = m.variable_abbrev().unwrap_or_default();
        let name = m.variable_name().unwrap_or_default().to_lowercase();
        let is_u = abbrev == "UGRD" || name.contains("u-component");
        let is_v = abbrev == "VGRD" || name.contains("v-component");
        if !is_u && !is_v {
            continue;
        }
        let data = match m.data() {
            Ok(d) => d,
            Err(e) => {
                log::warn!("grib decode failed for {abbrev}: {e:?}");
                continue;
            }
        };
        if is_u {
            ug = Some(data);
        } else {
            vg = Some(data);
        }
    }
    let u = ug.context("no U wind message in GRIB")?;
    let v = vg.context("no V wind message in GRIB")?;
    anyhow::ensure!(u.len() == v.len(), "u/v grid size mismatch");
    let (w, h) = match u.len() {
        1_038_240 => (1440u32, 721u32), // GFS 0.25 deg
        259_920 => (720u32, 361u32),    // GFS 0.5 deg
        n => anyhow::bail!("unexpected GFS grid size: {n}"),
    };
    let data: Vec<[u16; 2]> = u
        .iter()
        .zip(v.iter())
        .map(|(&a, &b)| f16_pair(a, b))
        .collect();

    // Quick sanity log: jet-stream rows should out-blow the equator row.
    let row_mean = |row: u32| -> f64 {
        let start = (row * w) as usize;
        u[start..start + w as usize]
            .iter()
            .map(|x| x.abs())
            .sum::<f64>()
            / w as f64
    };
    let lat_row = |lat: f64| (((90.0 - lat) / 180.0) * (h - 1) as f64) as u32;
    log::info!(
        "wind decoded {w}x{h} | mean |u|: 45N={:.1} eq={:.1} 50S={:.1} m/s",
        row_mean(lat_row(45.0)),
        row_mean(lat_row(0.0)),
        row_mean(lat_row(-50.0)),
    );

    let label = if cycle_key.len() >= 10 {
        format!(
            "GFS {}-{}-{} {}Z",
            &cycle_key[0..4],
            &cycle_key[4..6],
            &cycle_key[6..8],
            &cycle_key[8..10]
        )
    } else {
        format!("GFS {cycle_key}")
    };

    Ok(WindGrid {
        w,
        h,
        data,
        label,
        source,
        cycle_key: cycle_key.to_string(),
    })
}

pub fn load_snapshot() -> anyhow::Result<WindGrid> {
    let bytes = std::fs::read(asset_path("wind_snapshot.grib2"))?;
    let cycle = std::fs::read_to_string(asset_path("wind_snapshot_cycle.txt"))
        .unwrap_or_default()
        .trim()
        .to_string();
    decode(&bytes, &cycle, Source::Snapshot)
}

fn cycle_candidates() -> Vec<(String, String)> {
    // GFS f000 typically lands ~3.5h after cycle time.
    let base = Utc::now() - chrono::Duration::minutes(215);
    (0..3)
        .map(|i| {
            let t = base - chrono::Duration::hours(6 * i);
            let hour = (t.hour() / 6) * 6;
            (
                format!("{:04}{:02}{:02}", t.year(), t.month(), t.day()),
                format!("{hour:02}"),
            )
        })
        .collect()
}

fn fetch_cycle(date: &str, hour: &str) -> anyhow::Result<Vec<u8>> {
    let url = format!(
        "https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25.pl?\
         dir=%2Fgfs.{date}%2F{hour}%2Fatmos&file=gfs.t{hour}z.pgrb2.0p25.f000\
         &var_UGRD=on&var_VGRD=on&lev_10_m_above_ground=on"
    );
    let bytes = http_get(&url, 120)?;
    anyhow::ensure!(
        bytes.len() > 100_000 && &bytes[0..4] == b"GRIB",
        "response does not look like GRIB2 ({} bytes)",
        bytes.len()
    );
    Ok(bytes)
}

pub fn synthetic() -> WindGrid {
    let (w, h) = (720u32, 361u32);
    let mut data = Vec::with_capacity((w * h) as usize);
    let g = |x: f64, c: f64, s: f64| (-((x - c) / s).powi(2)).exp();
    for r in 0..h {
        let lat = 90.0 - r as f64 * 0.5;
        for c in 0..w {
            let lon = c as f64 * 0.5;
            let jets = 26.0 * g(lat, 42.0, 11.0) + 24.0 * g(lat, -48.0, 10.0)
                - 9.0 * g(lat, 12.0, 8.0)
                - 9.0 * g(lat, -12.0, 8.0);
            let wob = 1.0 + 0.35 * ((lon * 0.05).sin() + (lon * 0.023 + lat * 0.08).cos());
            let u = jets * wob;
            let v = 7.0 * ((lon * 0.09).sin() * (lat * 0.05).cos())
                + 4.0 * ((lon * 0.031 + 2.0).cos() * (lat * 0.11).sin());
            data.push(f16_pair(u, v));
        }
    }
    WindGrid {
        w,
        h,
        data,
        label: "procedural (offline)".into(),
        source: Source::Synthetic,
        cycle_key: String::new(),
    }
}

fn send(tx: &Sender<DataMsg>, g: WindGrid) -> bool {
    tx.send(DataMsg::Wind {
        w: g.w,
        h: g.h,
        data: g.data,
        label: g.label,
        source: g.source,
    })
    .is_ok()
}

pub fn run(tx: Sender<DataMsg>) {
    let mut loaded = String::new();

    match load_snapshot() {
        Ok(g) => {
            loaded = g.cycle_key.clone();
            if !send(&tx, g) {
                return;
            }
        }
        Err(e) => {
            log::warn!("wind snapshot failed: {e:#}");
            if !send(&tx, synthetic()) {
                return;
            }
        }
    }

    loop {
        for (date, hour) in cycle_candidates() {
            let key = format!("{date}{hour}");
            if key == loaded {
                break; // nothing newer exists yet
            }
            match fetch_cycle(&date, &hour) {
                Ok(bytes) => match decode(&bytes, &key, Source::Live) {
                    Ok(g) => {
                        loaded = g.cycle_key.clone();
                        let _ = tx.send(DataMsg::Note(format!("wind updated to {}", g.label)));
                        if !send(&tx, g) {
                            return;
                        }
                        break;
                    }
                    Err(e) => log::warn!("wind decode {key}: {e:#}"),
                },
                Err(e) => log::debug!("wind fetch {key}: {e:#}"),
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(600));
    }
}

// Keep chrono trait imports used even if refactors drop some call sites.
#[allow(unused)]
fn _keep(t: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    t.duration_round(chrono::Duration::hours(1)).unwrap_or(t)
}
