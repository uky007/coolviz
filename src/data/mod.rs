//! Live + snapshot data feeds. Each feed runs on its own thread and reports
//! through an mpsc channel; the app uploads results to the GPU as they arrive.

pub mod coast;
pub mod himawari;
pub mod landmask;
pub mod quakes;
pub mod sats;
pub mod wind;

use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Live,
    Cache,
    Snapshot,
    Synthetic,
}

impl Source {
    pub fn tag(self) -> &'static str {
        match self {
            Source::Live => "LIVE",
            Source::Cache => "CACHE",
            Source::Snapshot => "SNAPSHOT",
            Source::Synthetic => "SYNTHETIC",
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SatGpu {
    /// xyz world (earth radii), w = kind (0 LEO, 1 MEO, 2 GEO, 3 HEO, 4 ISS).
    pub pos: [f32; 4],
    /// xyz world per second, w = brightness.
    pub vel: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct QuakeCpu {
    pub lon: f32,
    pub lat: f32,
    pub mag: f32,
    pub unix_ms: i64,
}

pub enum DataMsg {
    Wind {
        w: u32,
        h: u32,
        data: Vec<[u16; 2]>,
        label: String,
        source: Source,
    },
    Sats {
        t0_unix: f64,
        states: Vec<SatGpu>,
        label: String,
        source: Source,
    },
    Quakes {
        list: Vec<QuakeCpu>,
        label: String,
    },
    Clouds {
        w: u32,
        h: u32,
        rgba: Vec<u8>,
        label: String,
    },
    Note(String),
}

pub fn spawn(tx: Sender<DataMsg>) {
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("wind".into())
            .spawn(move || wind::run(tx))
            .ok();
    }
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("sats".into())
            .spawn(move || sats::run(tx))
            .ok();
    }
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("quakes".into())
            .spawn(move || quakes::run(tx))
            .ok();
    }
    std::thread::Builder::new()
        .name("himawari".into())
        .spawn(move || himawari::run(tx))
        .ok();
}

pub fn asset_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name)
}

pub fn cache_path(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".cache");
    std::fs::create_dir_all(&dir).ok();
    dir.join(name)
}

pub fn read_gz(path: &std::path::Path) -> anyhow::Result<Vec<u8>> {
    let raw = std::fs::read(path)?;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(&raw[..]).read_to_end(&mut out)?;
    Ok(out)
}

pub fn http_get(url: &str, timeout_secs: u64) -> anyhow::Result<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
        .build()
        .into();
    let mut res = agent
        .get(url)
        .header("user-agent", "coolviz/0.1 (hobby visualization)")
        .call()?;
    let bytes = res
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()?;
    Ok(bytes)
}
