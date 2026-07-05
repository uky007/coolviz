//! Satellite catalog: CelesTrak GP elements (live w/ 2h disk cache, vendored
//! snapshot fallback) propagated with SGP4 on a worker thread.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use glam::DVec3;
use rayon::prelude::*;

use crate::astro;

use super::{DataMsg, SatGpu, Source, asset_path, cache_path, http_get, read_gz};

const CELESTRAK_URL: &str = "https://celestrak.org/NORAD/elements/gp.php?GROUP=active&FORMAT=json";
const CACHE_MAX_AGE: Duration = Duration::from_secs(2 * 3600); // CelesTrak policy

struct Entry {
    constants: sgp4::Constants,
    epoch: chrono::NaiveDateTime,
    kind: u8,
    bright: f32,
}

pub struct SatSet {
    entries: Vec<Entry>,
    pub label: String,
    pub source: Source,
}

fn classify(el: &sgp4::Elements) -> u8 {
    if el.norad_id == 25544 {
        return 4; // ISS
    }
    let n = el.mean_motion;
    let e = el.eccentricity;
    if e > 0.25 {
        3 // HEO / Molniya-like
    } else if n > 11.25 {
        0 // LEO
    } else if (0.9..=1.1).contains(&n) {
        2 // GEO belt
    } else {
        1 // MEO
    }
}

fn hash01(x: u64) -> f32 {
    let mut h = x.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    (h & 0xFFFFFF) as f32 / 16_777_215.0
}

fn parse_elements(bytes: &[u8]) -> anyhow::Result<Vec<sgp4::Elements>> {
    Ok(serde_json::from_slice::<Vec<sgp4::Elements>>(bytes)?)
}

/// Returns raw element bytes plus provenance.
fn load_element_bytes(prefer_offline: bool) -> (Vec<u8>, Source) {
    let cache = cache_path("celestrak_active.json");
    let cache_fresh = std::fs::metadata(&cache)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.elapsed().ok())
        .is_some_and(|age| age < CACHE_MAX_AGE);

    if (cache_fresh || prefer_offline)
        && let Ok(bytes) = std::fs::read(&cache)
    {
        log::info!("satellites: using cached catalog");
        return (bytes, Source::Cache);
    }
    // A freshly-vendored snapshot is as good as a live fetch; respect
    // CelesTrak's one-download-per-2h policy and skip the request.
    let snap = asset_path("sats_snapshot.json.gz");
    let snap_fresh = std::fs::metadata(&snap)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.elapsed().ok())
        .is_some_and(|age| age < CACHE_MAX_AGE);
    if snap_fresh
        && !prefer_offline
        && let Ok(bytes) = read_gz(&snap)
    {
        log::info!("satellites: snapshot is fresh; skipping live fetch");
        return (bytes, Source::Snapshot);
    }
    if !prefer_offline {
        match http_get(CELESTRAK_URL, 90) {
            Ok(bytes) if bytes.len() > 100_000 => {
                std::fs::write(&cache, &bytes).ok();
                log::info!("satellites: fetched live catalog ({} B)", bytes.len());
                return (bytes, Source::Live);
            }
            Ok(bytes) => log::warn!("satellites: suspicious response ({} B)", bytes.len()),
            Err(e) => log::warn!("satellites: fetch failed: {e:#}"),
        }
        // A stale cache still beats the vendored snapshot.
        if let Ok(bytes) = std::fs::read(&cache) {
            return (bytes, Source::Cache);
        }
    }
    match read_gz(&asset_path("sats_snapshot.json.gz")) {
        Ok(bytes) => (bytes, Source::Snapshot),
        Err(e) => {
            log::error!("satellites: snapshot missing: {e:#}");
            (b"[]".to_vec(), Source::Snapshot)
        }
    }
}

pub fn build_set(prefer_offline: bool) -> SatSet {
    let (bytes, source) = load_element_bytes(prefer_offline);
    let elements = match parse_elements(&bytes) {
        Ok(e) => e,
        Err(e) => {
            log::error!("satellites: parse failed: {e:#}");
            Vec::new()
        }
    };
    let total = elements.len();
    let entries: Vec<Entry> = elements
        .par_iter()
        .filter_map(|el| {
            let constants = sgp4::Constants::from_elements(el).ok()?;
            Some(Entry {
                constants,
                epoch: el.datetime,
                kind: classify(el),
                bright: 0.55 + 0.65 * hash01(el.norad_id),
            })
        })
        .collect();
    let label = format!("{} objects", entries.len());
    log::info!(
        "satellites: {} usable of {} ({})",
        entries.len(),
        total,
        source.tag()
    );
    SatSet {
        entries,
        label,
        source,
    }
}

pub fn propagate_all(set: &SatSet, t: DateTime<Utc>) -> Vec<SatGpu> {
    let gmst = astro::gmst_rad(t);
    let naive = t.naive_utc();
    set.entries
        .par_iter()
        .filter_map(|s| {
            let minutes = (naive - s.epoch).num_milliseconds() as f64 / 60_000.0;
            let p = s
                .constants
                .propagate(sgp4::MinutesSinceEpoch(minutes))
                .ok()?;
            let r_teme = DVec3::new(p.position[0], p.position[1], p.position[2]);
            let v_teme = DVec3::new(p.velocity[0], p.velocity[1], p.velocity[2]);
            let r_km = r_teme.length();
            if !(6_400.0..=90_000.0).contains(&r_km) {
                return None;
            }
            let r_e = astro::teme_to_ecef(gmst, r_teme);
            let v_e = astro::teme_vel_to_ecef(gmst, r_e, v_teme);
            let rw = astro::ecef_to_world(r_e) / astro::EARTH_RADIUS_KM;
            let vw = astro::ecef_to_world(v_e) / astro::EARTH_RADIUS_KM;
            Some(SatGpu {
                pos: [rw.x as f32, rw.y as f32, rw.z as f32, s.kind as f32],
                vel: [vw.x as f32, vw.y as f32, vw.z as f32, s.bright],
            })
        })
        .collect()
}

/// One-shot propagation for headless screenshots.
pub fn offline_states() -> (Vec<SatGpu>, f64, String, Source) {
    let set = build_set(true);
    let t = Utc::now();
    let states = propagate_all(&set, t);
    (
        states,
        astro::unix_seconds(t),
        set.label.clone(),
        set.source,
    )
}

pub fn run(tx: Sender<DataMsg>) {
    let mut set = build_set(false);
    let _ = tx.send(DataMsg::Note(format!(
        "satellite catalog ready: {} ({})",
        set.label,
        set.source.tag()
    )));
    let mut last_reload = Instant::now();

    loop {
        let t = Utc::now();
        let states = propagate_all(&set, t);
        let msg = DataMsg::Sats {
            t0_unix: astro::unix_seconds(t),
            states,
            label: set.label.clone(),
            source: set.source,
        };
        if tx.send(msg).is_err() {
            return; // app closed
        }

        if last_reload.elapsed() > Duration::from_secs(2 * 3600 + 120) {
            let fresh = build_set(false);
            if !fresh.entries.is_empty() {
                set = fresh;
                let _ = tx.send(DataMsg::Note(format!(
                    "satellite catalog refreshed: {}",
                    set.label
                )));
            }
            last_reload = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(1500));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{asset_path, read_gz};

    #[test]
    fn vendored_catalog_parses_and_classifies() {
        let bytes = read_gz(&asset_path("sats_snapshot.json.gz")).expect("snapshot readable");
        let els = super::parse_elements(&bytes).expect("catalog parses");
        assert!(els.len() > 10_000, "only {} elements", els.len());
        let iss = els
            .iter()
            .find(|e| e.norad_id == 25544)
            .expect("ISS in catalog");
        assert_eq!(super::classify(iss), 4);
        // A GEO bird should classify as 2: look for mean motion ~1 rev/day.
        assert!(els.iter().any(|e| super::classify(e) == 2));
    }
}
