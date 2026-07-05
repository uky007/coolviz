//! PLATEAU 3D Tiles loader (classic 2020 Tokyo distribution: plain glTF in
//! b3dm with CESIUM_RTC, no compression). Downloads the tiles around a site,
//! parses geometry with a minimal GLB reader, and outputs one merged mesh in
//! local ENU meters (x = east, y = up, z = south).

use anyhow::Context;
use glam::DVec3;

use super::{cache_path, http_get};

pub const TILESET_URL: &str = "https://plateau.geospatial.jp/main/data/3d-tiles/bldg/13100_tokyo/13101_chiyoda-ku/notexture/tileset.json";
/// Tokyo Station plaza.
pub const SITE_LON: f64 = 139.7660;
pub const SITE_LAT: f64 = 35.6812;
/// Half-size of the loaded square, in degrees of latitude (~1.6 km).
const LOAD_RADIUS_DEG: f64 = 0.015;

pub struct CityMesh {
    /// x,y,z in meters (ENU world), plus a per-building hash in w.
    pub verts: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub label: String,
}

// ---- WGS84 helpers ----

fn geodetic_to_ecef(lat_deg: f64, lon_deg: f64, h: f64) -> DVec3 {
    let a = 6378137.0;
    let e2 = 6.694379990141e-3;
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    let n = a / (1.0 - e2 * lat.sin() * lat.sin()).sqrt();
    DVec3::new(
        (n + h) * lat.cos() * lon.cos(),
        (n + h) * lat.cos() * lon.sin(),
        (n * (1.0 - e2) + h) * lat.sin(),
    )
}

struct Enu {
    origin: DVec3,
    east: DVec3,
    north: DVec3,
    up: DVec3,
}

impl Enu {
    fn new(lat_deg: f64, lon_deg: f64) -> Self {
        let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
        let east = DVec3::new(-lon.sin(), lon.cos(), 0.0);
        let north = DVec3::new(-lat.sin() * lon.cos(), -lat.sin() * lon.sin(), lat.cos());
        let up = DVec3::new(lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin());
        Self {
            origin: geodetic_to_ecef(lat_deg, lon_deg, 40.0),
            east,
            north,
            up,
        }
    }

    /// ECEF -> local render frame (x east, y up, z south).
    fn to_world(&self, p: DVec3) -> [f32; 3] {
        let d = p - self.origin;
        [
            d.dot(self.east) as f32,
            d.dot(self.up) as f32,
            -(d.dot(self.north)) as f32,
        ]
    }
}

// ---- minimal GLB / accessor reader ----

struct Glb {
    json: serde_json::Value,
    bin: Vec<u8>,
}

fn parse_glb(data: &[u8]) -> anyhow::Result<Glb> {
    anyhow::ensure!(data.len() > 20 && &data[0..4] == b"glTF", "not GLB");
    let mut off = 12;
    let mut json = None;
    let mut bin = Vec::new();
    while off + 8 <= data.len() {
        let clen = u32::from_le_bytes(data[off..off + 4].try_into()?) as usize;
        let ctype = &data[off + 4..off + 8];
        let body = &data[off + 8..(off + 8 + clen).min(data.len())];
        match ctype {
            b"JSON" => json = Some(serde_json::from_slice(body)?),
            b"BIN\0" => bin = body.to_vec(),
            _ => {}
        }
        off += 8 + clen.next_multiple_of(4);
    }
    Ok(Glb {
        json: json.context("GLB without JSON chunk")?,
        bin,
    })
}

fn accessor_info(glb: &Glb, idx: usize) -> anyhow::Result<(usize, usize, usize, usize, usize)> {
    // -> (offset, count, component_type, components, stride)
    let acc = &glb.json["accessors"][idx];
    let count = acc["count"].as_u64().context("count")? as usize;
    let ctype = acc["componentType"].as_u64().context("ctype")? as usize;
    let comps = match acc["type"].as_str().unwrap_or("") {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        t => anyhow::bail!("accessor type {t}"),
    };
    let bv = &glb.json["bufferViews"][acc["bufferView"].as_u64().context("bufferView")? as usize];
    let bv_off = bv["byteOffset"].as_u64().unwrap_or(0) as usize;
    let acc_off = acc["byteOffset"].as_u64().unwrap_or(0) as usize;
    let csize = match ctype {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        t => anyhow::bail!("component type {t}"),
    };
    let stride = bv["byteStride"].as_u64().unwrap_or((csize * comps) as u64) as usize;
    Ok((bv_off + acc_off, count, ctype, comps, stride))
}

fn read_vec3(glb: &Glb, idx: usize) -> anyhow::Result<Vec<[f32; 3]>> {
    let (off, count, ctype, comps, stride) = accessor_info(glb, idx)?;
    anyhow::ensure!(ctype == 5126 && comps == 3, "expected f32 vec3");
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let p = off + i * stride;
        let read = |o: usize| f32::from_le_bytes(glb.bin[o..o + 4].try_into().unwrap());
        out.push([read(p), read(p + 4), read(p + 8)]);
    }
    Ok(out)
}

fn read_indices(glb: &Glb, idx: usize) -> anyhow::Result<Vec<u32>> {
    let (off, count, ctype, _comps, stride) = accessor_info(glb, idx)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let p = off + i * stride;
        let v = match ctype {
            5121 => glb.bin[p] as u32,
            5123 => u16::from_le_bytes(glb.bin[p..p + 2].try_into()?) as u32,
            5125 => u32::from_le_bytes(glb.bin[p..p + 4].try_into()?),
            t => anyhow::bail!("index type {t}"),
        };
        out.push(v);
    }
    Ok(out)
}

fn read_batch_ids(glb: &Glb, idx: usize) -> anyhow::Result<Vec<f32>> {
    let (off, count, ctype, _comps, stride) = accessor_info(glb, idx)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let p = off + i * stride;
        let v = match ctype {
            5126 => f32::from_le_bytes(glb.bin[p..p + 4].try_into()?),
            5123 => u16::from_le_bytes(glb.bin[p..p + 2].try_into()?) as f32,
            5125 => u32::from_le_bytes(glb.bin[p..p + 4].try_into()?) as f32,
            t => anyhow::bail!("batchid type {t}"),
        };
        out.push(v);
    }
    Ok(out)
}

fn hash01(x: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9E3779B9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EBCA6B);
    h ^= h >> 13;
    (h & 0xFFFFFF) as f32 / 16_777_215.0
}

// ---- b3dm ----

fn append_b3dm(bytes: &[u8], enu: &Enu, tile_seed: u32, mesh: &mut CityMesh) -> anyhow::Result<()> {
    anyhow::ensure!(bytes.len() > 28 && &bytes[0..4] == b"b3dm", "not b3dm");
    let u32at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
    let (ftj, ftb, btj, btb) = (u32at(12), u32at(16), u32at(20), u32at(24));
    let glb = parse_glb(&bytes[28 + ftj + ftb + btj + btb..])?;

    let rtc = glb.json["extensions"]["CESIUM_RTC"]["center"]
        .as_array()
        .map(|a| {
            DVec3::new(
                a[0].as_f64().unwrap_or(0.0),
                a[1].as_f64().unwrap_or(0.0),
                a[2].as_f64().unwrap_or(0.0),
            )
        })
        .unwrap_or(DVec3::ZERO);

    let empty = Vec::new();
    let meshes = glb.json["meshes"].as_array().unwrap_or(&empty).clone();
    for m in &meshes {
        for prim in m["primitives"].as_array().unwrap_or(&empty) {
            if prim.get("mode").and_then(|v| v.as_u64()).unwrap_or(4) != 4 {
                continue;
            }
            let Some(pos_idx) = prim["attributes"]["POSITION"].as_u64() else {
                continue;
            };
            let positions = read_vec3(&glb, pos_idx as usize)?;
            let batch = prim["attributes"]["_BATCHID"]
                .as_u64()
                .and_then(|b| read_batch_ids(&glb, b as usize).ok());
            let base = mesh.verts.len() as u32;
            for (i, p) in positions.iter().enumerate() {
                // 3D Tiles: glTF payloads are y-up, tileset space is z-up ECEF.
                let ecef = rtc + DVec3::new(p[0] as f64, -(p[2] as f64), p[1] as f64);
                let w = enu.to_world(ecef);
                let bid = batch.as_ref().map(|b| b[i] as u32).unwrap_or(0);
                mesh.verts
                    .push([w[0], w[1], w[2], hash01(bid ^ tile_seed.rotate_left(9))]);
            }
            match prim["indices"].as_u64() {
                Some(ii) => {
                    for i in read_indices(&glb, ii as usize)? {
                        mesh.indices.push(base + i);
                    }
                }
                None => {
                    for i in 0..positions.len() as u32 {
                        mesh.indices.push(base + i);
                    }
                }
            }
        }
    }
    Ok(())
}

// ---- tileset traversal + download ----

fn walk_regions(node: &serde_json::Value, out: &mut Vec<(Vec<f64>, String)>) {
    if let Some(uri) = node["content"]["uri"]
        .as_str()
        .or_else(|| node["content"]["url"].as_str())
        && let Some(region) = node["boundingVolume"]["region"].as_array()
    {
        let r: Vec<f64> = region.iter().filter_map(|v| v.as_f64()).collect();
        if r.len() >= 4 {
            out.push((r, uri.to_string()));
        }
    }
    if let Some(children) = node["children"].as_array() {
        for c in children {
            walk_regions(c, out);
        }
    }
}

fn cached_get(url: &str, name: &str) -> anyhow::Result<Vec<u8>> {
    let dir = cache_path("plateau");
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join(name);
    if let Ok(bytes) = std::fs::read(&path)
        && !bytes.is_empty()
    {
        return Ok(bytes);
    }
    let bytes = http_get(url, 120)?;
    std::fs::write(&path, &bytes).ok();
    Ok(bytes)
}

pub fn load_city() -> anyhow::Result<CityMesh> {
    let tileset = cached_get(TILESET_URL, "tileset.json")?;
    let ts: serde_json::Value = serde_json::from_slice(&tileset)?;
    let mut tiles = Vec::new();
    walk_regions(&ts["root"], &mut tiles);
    anyhow::ensure!(!tiles.is_empty(), "no tiles in tileset");

    let (lon0, lat0) = (SITE_LON.to_radians(), SITE_LAT.to_radians());
    let rad = LOAD_RADIUS_DEG.to_radians();
    let wanted: Vec<&(Vec<f64>, String)> = tiles
        .iter()
        .filter(|(r, _)| {
            r[0] <= lon0 + rad && r[2] >= lon0 - rad && r[1] <= lat0 + rad && r[3] >= lat0 - rad
        })
        .collect();
    log::info!(
        "plateau: {} of {} tiles intersect the site box",
        wanted.len(),
        tiles.len()
    );

    let base = TILESET_URL.rsplit_once('/').map(|(b, _)| b).unwrap_or("");
    let enu = Enu::new(SITE_LAT, SITE_LON);
    let mut mesh = CityMesh {
        verts: Vec::new(),
        indices: Vec::new(),
        label: String::new(),
    };
    let mut failed = 0usize;
    for (i, (_, uri)) in wanted.iter().enumerate() {
        let url = format!("{base}/{uri}");
        let name = uri.replace('/', "_");
        match cached_get(&url, &name) {
            Ok(bytes) => {
                if let Err(e) = append_b3dm(&bytes, &enu, i as u32 + 1, &mut mesh) {
                    log::warn!("plateau: {uri}: {e:#}");
                    failed += 1;
                }
            }
            Err(e) => {
                log::warn!("plateau: fetch {uri}: {e:#}");
                failed += 1;
            }
        }
    }
    anyhow::ensure!(!mesh.verts.is_empty(), "no city geometry loaded");

    // Ground calibration: put the 3rd-percentile height at y = 0.
    let mut ys: Vec<f32> = mesh.verts.iter().map(|v| v[1]).collect();
    ys.sort_by(f32::total_cmp);
    let ground = ys[ys.len() / 33];
    for v in &mut mesh.verts {
        v[1] -= ground;
    }

    mesh.label = format!(
        "PLATEAU 千代田区 LOD1 · {} tiles · {}k tris{}",
        wanted.len() - failed,
        mesh.indices.len() / 3000,
        if failed > 0 {
            format!(" ({failed} failed)")
        } else {
            String::new()
        }
    );
    log::info!(
        "plateau: {} verts, {} tris (ground offset {ground:.1} m)",
        mesh.verts.len(),
        mesh.indices.len() / 3
    );
    Ok(mesh)
}
