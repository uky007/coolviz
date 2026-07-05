//! COOLVIZ // LIVE EARTH — a mission-control globe: live wind, satellites,
//! earthquakes. Cool for coolness' sake.
//!
//! Run the app:            cargo run --release
//! Headless screenshot:    cargo run --release -- --shot out.png
//!                         [--frames 240] [--size 1920x1080]
//!                         [--lat 26] [--lon 136] [--dist 3.3]

mod astro;
mod camera;
mod data;
mod scene;

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use chrono::Utc;
use eframe::egui;

use camera::OrbitCamera;
use data::{DataMsg, LightGpu, QuakeCpu, Source};
use scene::{FrameInput, Scene, SceneAssets, SceneMode};

// ------------------------------------------------------------- car sim ----

struct Car {
    path: usize,
    s: f32,
    speed: f32,
    dir: f32,
}

/// Tiny traffic simulation driving light sprites along OSM roads.
struct CarSim {
    paths: Vec<(Vec<glam::Vec2>, Vec<f32>, f32)>, // points, cumulative length, total
    cars: Vec<Car>,
}

impl CarSim {
    fn new(paths_msg: &[(Vec<[f32; 2]>, u8)], n_cars: usize) -> Self {
        let mut paths = Vec::new();
        for (pts, class) in paths_msg {
            if *class > 4 || pts.len() < 2 {
                continue;
            }
            let v: Vec<glam::Vec2> = pts.iter().map(|p| glam::Vec2::from(*p)).collect();
            let mut cum = vec![0.0f32];
            for seg in v.windows(2) {
                cum.push(cum.last().unwrap() + seg[0].distance(seg[1]));
            }
            let total = *cum.last().unwrap();
            if total > 80.0 {
                paths.push((v, cum, total));
            }
        }
        let mut cars = Vec::new();
        if !paths.is_empty() {
            for _ in 0..n_cars {
                let path = fastrand::usize(..paths.len());
                cars.push(Car {
                    path,
                    s: fastrand::f32() * paths[path].2,
                    speed: 7.0 + fastrand::f32() * 8.0,
                    dir: if fastrand::bool() { 1.0 } else { -1.0 },
                });
            }
        }
        Self { paths, cars }
    }

    fn step(&mut self, dt: f32) -> Vec<LightGpu> {
        let mut out = Vec::with_capacity(self.cars.len() * 2);
        for car in &mut self.cars {
            let (pts, cum, total) = &self.paths[car.path];
            car.s += car.speed * dt * car.dir;
            if car.s < 0.0 || car.s > *total {
                // Turn around at the end of the road.
                car.dir = -car.dir;
                car.s = car.s.clamp(0.0, *total);
            }
            // Locate the segment.
            let mut i = 1;
            while i < cum.len() - 1 && cum[i] < car.s {
                i += 1;
            }
            let seg_len = (cum[i] - cum[i - 1]).max(1e-3);
            let f = (car.s - cum[i - 1]) / seg_len;
            let d = (pts[i] - pts[i - 1]).normalize_or_zero() * car.dir;
            let pos = pts[i - 1].lerp(pts[i], f.clamp(0.0, 1.0));
            // Keep-left lane offset.
            let lane = glam::Vec2::new(-d.y, d.x) * 2.6;
            let p = pos + lane;
            let head = p + d * 2.1;
            let tail = p - d * 2.1;
            out.push(LightGpu {
                pos: [head.x, 1.0, head.y, 1.0],
                aux: [0.0, 0.0, 0.0, 1.0],
            });
            out.push(LightGpu {
                pos: [tail.x, 1.0, tail.y, 2.0],
                aux: [0.0, 0.0, 0.0, 1.0],
            });
        }
        out
    }
}

/// HUD orbit distance (1.18..20) -> meters, per local mode.
const TOKYO_DIST_SCALE: f32 = 450.0;
const OKINAWA_DIST_SCALE: f32 = 42.0;
const TOKYO_TARGET: glam::Vec3 = glam::Vec3::new(0.0, 70.0, 0.0);
const OKINAWA_TARGET: glam::Vec3 = glam::Vec3::new(0.0, 3.0, 0.0);

const ACCENT: egui::Color32 = egui::Color32::from_rgb(84, 220, 255);
const DIM: egui::Color32 = egui::Color32::from_rgb(110, 138, 158);
const LIGHT: egui::Color32 = egui::Color32::from_rgb(198, 219, 232);

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Slab-method ray/AABB intersection; returns entry distance.
fn ray_aabb(ro: glam::Vec3, rd: glam::Vec3, min: [f32; 3], max: [f32; 3]) -> Option<f32> {
    let inv = rd.recip();
    let t0 = (glam::Vec3::from(min) - ro) * inv;
    let t1 = (glam::Vec3::from(max) - ro) * inv;
    let tmin = t0.min(t1).max_element();
    let tmax = t0.max(t1).min_element();
    if tmax >= tmin.max(0.0) {
        Some(tmin.max(0.0))
    } else {
        None
    }
}

// ---- CPU port of the Okinawa terrain (ocean.wgsl) for depth hovering ----

fn oki_hash21(p: glam::Vec2) -> f32 {
    let mut q = (p * glam::Vec2::new(123.34, 456.21)).fract();
    q += glam::Vec2::splat(q.dot(q + glam::Vec2::splat(45.32)));
    (q.x * q.y).fract()
}

fn oki_vnoise(p: glam::Vec2) -> f32 {
    let i = p.floor();
    let f = p.fract();
    let u = f * f * (glam::Vec2::splat(3.0) - 2.0 * f);
    let a = oki_hash21(i);
    let b = oki_hash21(i + glam::Vec2::X);
    let c = oki_hash21(i + glam::Vec2::Y);
    let d = oki_hash21(i + glam::Vec2::ONE);
    lerp(lerp(a, b, u.x), lerp(c, d, u.x), u.y)
}

fn oki_fbm(p: glam::Vec2) -> f32 {
    let m = glam::Mat2::from_cols_array(&[1.6, -1.2, 1.2, 1.6]);
    let (mut v, mut a, mut q) = (0.0, 0.5, p);
    for _ in 0..4 {
        v += a * oki_vnoise(q);
        q = m * q;
        a *= 0.5;
    }
    v
}

fn oki_terrain(xz: glam::Vec2) -> f32 {
    let island = glam::Vec2::new(-330.0, -400.0);
    let rr = xz.distance(island) + (oki_fbm(xz * 0.004) - 0.5) * 70.0;
    let smooth = |e0: f32, e1: f32, x: f32| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let mut h = 30.0 * smooth(430.0, 60.0, rr)
        + (430.0 - rr) * 0.045
        + 4.0 * (oki_fbm(xz * 0.02) - 0.5) * smooth(400.0, 200.0, rr);
    h = h.min(34.0);
    let lagoon = -1.0 - 2.4 * smooth(430.0, 900.0, rr);
    h = h.min(lagoon.max((430.0 - rr) * 0.045));
    let coral_n = oki_vnoise(xz * 0.05 + 31.7) * 0.65 + oki_vnoise(xz * 0.15 + 7.1) * 0.35;
    let coral = smooth(0.58, 0.66, coral_n) * smooth(500.0, 590.0, rr) * smooth(980.0, 880.0, rr);
    h += coral * (1.4 + 0.6 * oki_vnoise(xz * 0.3));
    h += 2.4 * (-((rr - 940.0) / 55.0).powi(2)).exp();
    h -= 44.0 * smooth(955.0, 1250.0, rr);
    h
}

/// Parse the next CLI value as f32, rejecting non-finite input and clamping
/// to a sane range; falls back to `cur` when absent or invalid.
fn parse_f32(args: &mut impl Iterator<Item = String>, cur: f32, lo: f32, hi: f32) -> f32 {
    args.next()
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(lo, hi))
        .unwrap_or(cur)
}

fn parse_u32(args: &mut impl Iterator<Item = String>, cur: u32, lo: u32, hi: u32) -> u32 {
    args.next()
        .and_then(|s| s.parse::<u32>().ok())
        .map(|v| v.clamp(lo, hi))
        .unwrap_or(cur)
}

fn load_assets() -> anyhow::Result<SceneAssets> {
    let t0 = Instant::now();
    let (coast_vertices, coast_indices) = data::coast::load()?;
    let (land_w, land_h, land_mask) = data::landmask::build(2048, 1024)?;
    log::info!(
        "assets ready in {:.0?} ({} coast vertices)",
        t0.elapsed(),
        coast_vertices.len()
    );
    Ok(SceneAssets {
        land_w,
        land_h,
        land_mask,
        coast_vertices,
        coast_indices,
    })
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuakeGpu {
    pos: [f32; 4],
    meta: [f32; 4],
}

fn pack_quakes(list: &[QuakeCpu], app_epoch_unix: f64) -> Vec<QuakeGpu> {
    list.iter()
        .map(|q| {
            let p = astro::latlon_to_world(q.lat, q.lon) * 1.004;
            let t_rel = (q.unix_ms as f64 / 1000.0 - app_epoch_unix) as f32;
            let hash = (q.unix_ms % 997) as f32 / 997.0;
            QuakeGpu {
                pos: [p.x, p.y, p.z, q.mag],
                meta: [t_rel, hash, 0.0, 0.0],
            }
        })
        .collect()
}

// ---------------------------------------------------------------- app ----

struct Status {
    wind: Option<(Source, String)>,
    sats: Option<(Source, String, usize)>,
    quakes: Option<(String, usize)>,
    clouds: Option<String>,
    city: Option<String>,
    rain: Option<(String, u8)>,
}

struct App {
    scene: Scene,
    cam: OrbitCamera,
    cam_city: OrbitCamera,
    cam_ocean: OrbitCamera,
    mode: SceneMode,
    tx: std::sync::mpsc::Sender<DataMsg>,
    city_spawned: bool,
    rain_demo: bool,
    rx: Receiver<DataMsg>,
    status: Status,
    app_epoch_unix: f64,
    last_frame: Instant,
    frame_index: u32,
    fps: f32,
    sat_t0_unix: Option<f64>,
    iss: Option<([f32; 3], [f32; 3])>,
    // CPU copies for the hover inspector.
    sat_states: Vec<data::SatGpu>,
    sat_idxs: Vec<u32>,
    sat_names: Vec<String>,
    quake_list: Vec<QuakeCpu>,
    wind_grid: Option<(u32, u32, Vec<[u16; 2]>)>,
    cars: Option<CarSim>,
    lamp_lights: Vec<LightGpu>,
    beacon_lights: Vec<LightGpu>,
    buildings: Vec<data::plateau::BuildingInfo>,
    rain_grid: Option<(u32, Vec<u8>, [f64; 4])>,
    tex: Option<(egui::TextureId, (u32, u32))>,
    // UI state
    show_wind: bool,
    show_sats: bool,
    show_quakes: bool,
    show_coast: bool,
    show_clouds: bool,
    particle_count_k: u32,
    warp: f32,
    trail_gain: f32,
    exposure: f32,
    show_hud: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, assets: SceneAssets) -> Self {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .expect("coolviz requires the wgpu backend");
        apply_style(&cc.egui_ctx);
        install_jp_font(&cc.egui_ctx);
        let scene = Scene::new(&rs.device, &rs.queue, &assets);
        let (tx, rx) = std::sync::mpsc::channel();
        data::spawn(tx.clone());
        let now = Utc::now();
        let mut cam_city = OrbitCamera::new();
        cam_city.lat = 26.0;
        cam_city.lon = 205.0;
        cam_city.dist = 3.4;
        let mut cam_ocean = OrbitCamera::new();
        cam_ocean.lat = 11.0;
        cam_ocean.lon = 145.0;
        cam_ocean.dist = 3.6;
        Self {
            scene,
            cam: OrbitCamera::new(),
            cam_city,
            cam_ocean,
            mode: SceneMode::Earth,
            tx,
            city_spawned: false,
            rain_demo: true,
            rx,
            status: Status {
                wind: None,
                sats: None,
                quakes: None,
                clouds: None,
                city: None,
                rain: None,
            },
            app_epoch_unix: astro::unix_seconds(now),
            last_frame: Instant::now(),
            frame_index: 0,
            fps: 60.0,
            sat_t0_unix: None,
            iss: None,
            sat_states: Vec::new(),
            sat_idxs: Vec::new(),
            sat_names: Vec::new(),
            quake_list: Vec::new(),
            wind_grid: None,
            cars: None,
            lamp_lights: Vec::new(),
            beacon_lights: Vec::new(),
            buildings: Vec::new(),
            rain_grid: None,
            tex: None,
            show_wind: true,
            show_sats: true,
            show_quakes: true,
            show_coast: true,
            show_clouds: true,
            particle_count_k: 600,
            warp: 6000.0,
            trail_gain: 1.0,
            exposure: 1.15,
            show_hud: true,
        }
    }

    fn drain_data(&mut self, rs: &egui_wgpu::RenderState) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                DataMsg::Wind {
                    w,
                    h,
                    data,
                    label,
                    source,
                } => {
                    self.scene.upload_wind(&rs.device, &rs.queue, w, h, &data);
                    self.wind_grid = Some((w, h, data));
                    self.status.wind = Some((source, label));
                }
                DataMsg::Sats {
                    t0_unix,
                    states,
                    idxs,
                    label,
                    source,
                } => {
                    let n = states.len();
                    self.iss = states.iter().find(|s| s.pos[3] as u32 == 4).map(|s| {
                        (
                            [s.pos[0], s.pos[1], s.pos[2]],
                            [s.vel[0], s.vel[1], s.vel[2]],
                        )
                    });
                    self.scene
                        .upload_sats(&rs.device, &rs.queue, bytemuck::cast_slice(&states));
                    self.sat_states = states;
                    self.sat_idxs = idxs;
                    self.sat_t0_unix = Some(t0_unix);
                    self.status.sats = Some((source, label, n));
                }
                DataMsg::SatNames(names) => self.sat_names = names,
                DataMsg::Quakes { list, label } => {
                    let packed = pack_quakes(&list, self.app_epoch_unix);
                    let n = packed.len();
                    self.scene
                        .upload_quakes(&rs.device, &rs.queue, bytemuck::cast_slice(&packed));
                    self.quake_list = list;
                    self.status.quakes = Some((label, n));
                }
                DataMsg::Clouds { w, h, rgba, label } => {
                    self.scene.upload_clouds(&rs.device, &rs.queue, w, h, &rgba);
                    self.status.clouds = Some(label);
                }
                DataMsg::CityMesh {
                    tiles,
                    beacons,
                    buildings,
                    label,
                } => {
                    self.scene.upload_city(&rs.device, &rs.queue, &tiles);
                    self.beacon_lights = beacons;
                    self.buildings = buildings;
                    self.refresh_static_lights(rs);
                    self.status.city = Some(label);
                }
                DataMsg::Roads {
                    paths,
                    ribbon_verts,
                    ribbon_indices,
                    lamps,
                } => {
                    self.scene
                        .upload_roads(&rs.device, &ribbon_verts, &ribbon_indices);
                    self.lamp_lights = lamps;
                    self.refresh_static_lights(rs);
                    self.cars = Some(CarSim::new(&paths, 340));
                }
                DataMsg::Rain {
                    size,
                    levels,
                    bounds,
                    label,
                    max_level,
                } => {
                    self.scene
                        .upload_rain(&rs.device, &rs.queue, size, &levels, bounds);
                    self.rain_grid = Some((size, levels, bounds));
                    // Auto-switch to live rain the moment there is real rain.
                    if max_level > 0 {
                        self.rain_demo = false;
                    }
                    self.status.rain = Some((label, max_level));
                }
                DataMsg::Note(s) => log::info!("{s}"),
            }
        }
    }

    fn frame_input(&self, px: (u32, u32), dt: f32) -> FrameInput {
        let aspect = px.0 as f32 / px.1.max(1) as f32;
        let (vp, ivp, eye) = match self.mode {
            SceneMode::Earth => self.cam.matrices(aspect),
            SceneMode::Tokyo => self.cam_city.matrices_local(
                aspect,
                TOKYO_TARGET,
                self.cam_city.dist * TOKYO_DIST_SCALE,
            ),
            SceneMode::Okinawa => self.cam_ocean.matrices_local(
                aspect,
                OKINAWA_TARGET,
                self.cam_ocean.dist * OKINAWA_DIST_SCALE,
            ),
        };
        let utc = Utc::now();
        let unix = astro::unix_seconds(utc);
        let sat_dt = self.sat_t0_unix.map(|t0| (unix - t0) as f32).unwrap_or(0.0);
        FrameInput {
            view_proj: vp,
            inv_view_proj: ivp,
            cam_pos: eye,
            sun_dir: astro::sun_dir_world(utc),
            time: (unix - self.app_epoch_unix) as f32,
            sat_dt,
            trail_gain: self.trail_gain,
            exposure: self.exposure,
            res_scale: (px.1 as f32 / 1200.0).clamp(0.5, 3.0),
            layers: [
                if self.show_wind { 1.0 } else { 0.0 },
                if self.show_sats { 1.0 } else { 0.0 },
                if self.show_quakes { 1.0 } else { 0.0 },
                if self.show_coast { 1.0 } else { 0.0 },
            ],
            clouds: if self.show_clouds { 1.0 } else { 0.0 },
            sim_dt: dt.clamp(0.001, 0.05),
            warp: self.warp,
            trail_fade: lerp(0.958, 0.85, self.cam.motion.min(1.0)),
            frame_index: self.frame_index,
            mode: self.mode,
            rain_demo: self.rain_demo,
        }
    }

    fn refresh_static_lights(&mut self, rs: &egui_wgpu::RenderState) {
        let mut all = self.lamp_lights.clone();
        all.extend_from_slice(&self.beacon_lights);
        if !all.is_empty() {
            self.scene
                .upload_static_lights(&rs.device, &rs.queue, bytemuck::cast_slice(&all));
        }
    }

    fn set_mode(&mut self, mode: SceneMode) {
        self.mode = mode;
        if mode == SceneMode::Tokyo && !self.city_spawned {
            self.city_spawned = true;
            data::spawn_city(self.tx.clone());
            data::spawn_rain(self.tx.clone());
            data::spawn_roads(self.tx.clone());
        }
    }

    fn hud(&mut self, ctx: &egui::Context, rs: &egui_wgpu::RenderState) {
        let frame = egui::Frame::NONE
            .fill(egui::Color32::from_rgba_unmultiplied(5, 12, 20, 216))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(26, 62, 86)))
            .corner_radius(8)
            .inner_margin(egui::Margin::same(12));

        egui::Window::new("coolviz-hud")
            .title_bar(false)
            .resizable(false)
            .default_pos([16.0, 14.0])
            .frame(frame)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 5.0;
                ui.label(
                    egui::RichText::new("COOLVIZ ▸ LIVE EARTH")
                        .monospace()
                        .size(15.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.label(
                    egui::RichText::new(format!("{}", Utc::now().format("%Y-%m-%d %H:%M:%S UTC")))
                        .monospace()
                        .size(11.5)
                        .color(DIM),
                );
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let mut m = self.mode;
                    for (mode, name) in [
                        (SceneMode::Earth, "EARTH"),
                        (SceneMode::Tokyo, "TOKYO"),
                        (SceneMode::Okinawa, "OKINAWA"),
                    ] {
                        if ui
                            .selectable_label(
                                m == mode,
                                egui::RichText::new(name).monospace().size(12.0),
                            )
                            .clicked()
                        {
                            m = mode;
                        }
                    }
                    if m != self.mode {
                        self.set_mode(m);
                    }
                });
                ui.separator();

                if self.mode == SceneMode::Tokyo {
                    match &self.status.city {
                        Some(label) => status_row(
                            ui,
                            "CITY",
                            egui::Color32::from_rgb(70, 225, 255),
                            label.clone(),
                        ),
                        None => status_row(
                            ui,
                            "CITY",
                            egui::Color32::from_rgb(255, 210, 100),
                            "loading PLATEAU tiles…".into(),
                        ),
                    }
                    match &self.status.rain {
                        Some((label, max)) => {
                            let c = if *max > 0 {
                                egui::Color32::from_rgb(70, 225, 255)
                            } else {
                                egui::Color32::from_rgb(255, 210, 100)
                            };
                            status_row(ui, "RAIN", c, label.clone());
                        }
                        None => status_row(
                            ui,
                            "RAIN",
                            egui::Color32::from_rgb(255, 90, 90),
                            "waiting…".into(),
                        ),
                    }
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.rain_demo, "demo storm");
                        ui.checkbox(&mut self.cam_city.auto_orbit, "auto-orbit");
                    });
                    if self.rain_demo {
                        ui.label(
                            egui::RichText::new("synthetic squall — uncheck for live JMA nowcast")
                                .monospace()
                                .size(10.0)
                                .color(egui::Color32::from_rgb(255, 170, 110)),
                        );
                    }
                    ui.add(egui::Slider::new(&mut self.exposure, 0.5..=2.5).text("exposure"));
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("{:>5.1} fps", self.fps))
                            .monospace()
                            .size(11.0)
                            .color(DIM),
                    );
                    return;
                }
                if self.mode == SceneMode::Okinawa {
                    ui.label(
                        egui::RichText::new("procedural reef lagoon — no data feeds")
                            .monospace()
                            .size(10.5)
                            .color(DIM),
                    );
                    ui.checkbox(&mut self.cam_ocean.auto_orbit, "auto-orbit");
                    ui.add(egui::Slider::new(&mut self.exposure, 0.5..=2.5).text("exposure"));
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("{:>5.1} fps", self.fps))
                            .monospace()
                            .size(11.0)
                            .color(DIM),
                    );
                    return;
                }

                match &self.status.wind {
                    Some((src, label)) => status_row(
                        ui,
                        "WIND",
                        source_color(*src),
                        format!("{label} · {}", src.tag()),
                    ),
                    None => status_row(
                        ui,
                        "WIND",
                        egui::Color32::from_rgb(255, 90, 90),
                        "waiting…".into(),
                    ),
                }
                match &self.status.sats {
                    Some((src, label, _n)) => status_row(
                        ui,
                        "SATS",
                        source_color(*src),
                        format!("{label} · {}", src.tag()),
                    ),
                    None => status_row(
                        ui,
                        "SATS",
                        egui::Color32::from_rgb(255, 90, 90),
                        "waiting…".into(),
                    ),
                }
                match &self.status.quakes {
                    Some((label, _n)) => status_row(
                        ui,
                        "QUAKES",
                        egui::Color32::from_rgb(255, 150, 70),
                        label.clone(),
                    ),
                    None => status_row(
                        ui,
                        "QUAKES",
                        egui::Color32::from_rgb(255, 90, 90),
                        "waiting…".into(),
                    ),
                }
                match &self.status.clouds {
                    Some(label) => {
                        let c = if label.contains("(cache)") {
                            egui::Color32::from_rgb(255, 210, 100)
                        } else {
                            egui::Color32::from_rgb(70, 225, 255)
                        };
                        status_row(ui, "CLOUDS", c, label.clone());
                    }
                    None => status_row(
                        ui,
                        "CLOUDS",
                        egui::Color32::from_rgb(255, 90, 90),
                        "waiting…".into(),
                    ),
                }
                ui.separator();

                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.show_wind, "wind");
                    ui.checkbox(&mut self.show_sats, "sats");
                    ui.checkbox(&mut self.show_quakes, "quakes");
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.show_clouds, "clouds");
                    ui.checkbox(&mut self.show_coast, "coast");
                    ui.checkbox(&mut self.cam.auto_orbit, "auto-orbit");
                });
                ui.add_space(2.0);

                let resp = ui.add(
                    egui::Slider::new(&mut self.particle_count_k, 100..=2000)
                        .logarithmic(true)
                        .suffix("k")
                        .text("particles"),
                );
                if resp.drag_stopped() || resp.lost_focus() {
                    self.scene
                        .set_particle_count(&rs.device, self.particle_count_k * 1000);
                }
                ui.add(
                    egui::Slider::new(&mut self.warp, 1000.0..=15000.0)
                        .logarithmic(true)
                        .text("wind ×"),
                );
                ui.add(egui::Slider::new(&mut self.trail_gain, 0.0..=2.0).text("trails"));
                ui.add(egui::Slider::new(&mut self.exposure, 0.5..=2.5).text("exposure"));
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "{:>5.1} fps · {} particles · {} sats",
                        self.fps, self.scene.particles.count, self.scene.sats.count,
                    ))
                    .monospace()
                    .size(11.0)
                    .color(DIM),
                );
            });
    }
}

fn status_row(ui: &mut egui::Ui, name: &str, dot: egui::Color32, text: String) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("●").size(10.0).color(dot));
        ui.label(
            egui::RichText::new(format!("{name:<7}"))
                .monospace()
                .size(11.5)
                .color(LIGHT),
        );
        ui.label(egui::RichText::new(text).monospace().size(11.5).color(DIM));
    });
}

fn source_color(s: Source) -> egui::Color32 {
    match s {
        Source::Live => egui::Color32::from_rgb(70, 225, 255),
        Source::Cache | Source::Snapshot => egui::Color32::from_rgb(255, 210, 100),
        Source::Synthetic => egui::Color32::from_rgb(255, 140, 90),
    }
}

/// Load a system CJK font so building names and addresses render properly.
fn install_jp_font(ctx: &egui::Context) {
    let candidates = [
        "/System/Library/Fonts/ヒラギノ角ゴシック W4.ttc",
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "C:\\Windows\\Fonts\\meiryo.ttc",
        "C:\\Windows\\Fonts\\YuGothM.ttc",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("jp".into(), egui::FontData::from_owned(bytes).into());
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push("jp".into());
            }
            ctx.set_fonts(fonts);
            log::info!("Japanese font loaded from {path}");
            return;
        }
    }
    log::warn!("no CJK font found; Japanese labels may not render");
}

fn apply_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = egui::Color32::from_rgb(2, 4, 8);
    v.window_fill = egui::Color32::from_rgba_unmultiplied(5, 12, 20, 216);
    v.selection.bg_fill = egui::Color32::from_rgb(16, 96, 128);
    v.slider_trailing_fill = true;
    v.widgets.inactive.bg_fill = egui::Color32::from_rgb(16, 30, 42);
    v.widgets.hovered.bg_fill = egui::Color32::from_rgb(24, 48, 66);
    v.widgets.active.bg_fill = egui::Color32::from_rgb(20, 70, 96);
    ctx.set_visuals(v);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.frame_index = self.frame_index.wrapping_add(1);
        if dt > 0.0 {
            self.fps = self.fps * 0.95 + 0.05 / dt.max(1e-4);
        }

        let rs = frame
            .wgpu_render_state()
            .expect("wgpu render state")
            .clone();

        self.drain_data(&rs);

        let avail = ui.available_size();
        let (rect, resp) = ui.allocate_exact_size(avail, egui::Sense::click_and_drag());
        let ppp = ctx.pixels_per_point();
        let px = (
            ((avail.x * ppp).round() as u32).max(8),
            ((avail.y * ppp).round() as u32).max(8),
        );

        let drag = if resp.dragged() {
            let d = resp.drag_delta();
            (d.x, d.y)
        } else {
            (0.0, 0.0)
        };
        let (scroll, pinch) = if resp.hovered() {
            ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()))
        } else {
            (0.0, 1.0)
        };
        if resp.double_clicked() {
            match self.mode {
                SceneMode::Earth => self.cam.reset(),
                SceneMode::Tokyo => {
                    self.cam_city.lat = 26.0;
                    self.cam_city.lon = 205.0;
                    self.cam_city.dist = 3.4;
                }
                SceneMode::Okinawa => {
                    self.cam_ocean.lat = 11.0;
                    self.cam_ocean.lon = 145.0;
                    self.cam_ocean.dist = 3.6;
                }
            }
        }
        match self.mode {
            SceneMode::Earth => self.cam.update(dt, drag, scroll, pinch, avail.y),
            SceneMode::Tokyo => {
                self.cam_city.update(dt, drag, scroll, pinch, avail.y);
                self.cam_city.lat = self.cam_city.lat.max(4.0);
                if let Some(cars) = &mut self.cars {
                    let lights = cars.step(dt);
                    self.scene.upload_car_lights(
                        &rs.device,
                        &rs.queue,
                        bytemuck::cast_slice(&lights),
                    );
                }
            }
            SceneMode::Okinawa => {
                self.cam_ocean.update(dt, drag, scroll, pinch, avail.y);
                self.cam_ocean.lat = self.cam_ocean.lat.clamp(2.0, 60.0);
            }
        }

        let fi = self.frame_input(px, dt);
        let view = self.scene.render(&rs.device, &rs.queue, px, &fi);

        let need_register = match self.tex {
            Some((_, s)) => s != px,
            None => true,
        };
        if need_register {
            let mut renderer = rs.renderer.write();
            if let Some((old, _)) = self.tex.take() {
                renderer.free_texture(&old);
            }
            let id = renderer.register_native_texture(&rs.device, view, wgpu::FilterMode::Linear);
            self.tex = Some((id, px));
        }
        if let Some((id, _)) = self.tex {
            ui.painter().image(
                id,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        // ISS marker + label, projected and occlusion-tested on the CPU.
        if self.mode == SceneMode::Earth
            && self.show_sats
            && self.show_hud
            && let Some((p, v)) = self.iss
        {
            let wp = glam::Vec3::from(p) + glam::Vec3::from(v) * fi.sat_dt;
            let cam = fi.cam_pos;
            let to = wp - cam;
            let dist = to.length();
            let dir = to / dist.max(1e-6);
            let t_close = -cam.dot(dir);
            let hidden = t_close > 0.0 && t_close < dist && (cam + dir * t_close).length() < 0.995;
            if !hidden {
                let clip = fi.view_proj * wp.extend(1.0);
                if clip.w > 0.0 {
                    let ndc = clip.truncate() / clip.w;
                    if ndc.x.abs() < 1.05 && ndc.y.abs() < 1.05 {
                        let sp = egui::pos2(
                            rect.min.x + (ndc.x * 0.5 + 0.5) * rect.width(),
                            rect.min.y + (0.5 - ndc.y * 0.5) * rect.height(),
                        );
                        let c = egui::Color32::from_rgb(215, 246, 255);
                        let painter = ui.painter();
                        painter.circle_stroke(sp, 7.0, egui::Stroke::new(1.2, c));
                        let alt_km = (wp.length() - 1.0) * 6371.0;
                        let spd = glam::Vec3::from(v).length() * 6371.0;
                        painter.text(
                            sp + egui::vec2(11.0, -11.0),
                            egui::Align2::LEFT_BOTTOM,
                            format!("ISS · {alt_km:.0} km · {spd:.2} km/s"),
                            egui::FontId::monospace(11.0),
                            c,
                        );
                    }
                }
            }
        }

        // Hover inspector: satellites/quakes/wind on Earth, buildings/rain in
        // Tokyo, water depth in Okinawa.
        if self.show_hud
            && let Some(cur) = resp.hover_pos()
        {
            let mut lines: Vec<String> = Vec::new();
            let ray_dir = {
                let nx = (cur.x - rect.min.x) / rect.width() * 2.0 - 1.0;
                let ny = -((cur.y - rect.min.y) / rect.height() * 2.0 - 1.0);
                let pw = fi.inv_view_proj * glam::Vec4::new(nx, ny, 0.6, 1.0);
                (pw.truncate() / pw.w - fi.cam_pos).normalize()
            };
            if self.mode == SceneMode::Tokyo {
                let cam = fi.cam_pos;
                let mut best_t = f32::MAX;
                let mut best: Option<usize> = None;
                for (i, b) in self.buildings.iter().enumerate() {
                    if let Some(t) = ray_aabb(cam, ray_dir, b.min, b.max)
                        && t < best_t
                    {
                        best_t = t;
                        best = Some(i);
                    }
                }
                if let Some(i) = best {
                    let b = &self.buildings[i];
                    lines.push(if b.name.is_empty() {
                        "(名称データなし)".to_string()
                    } else {
                        b.name.clone()
                    });
                    let mut spec = Vec::new();
                    if b.height > 0.0 {
                        spec.push(format!("H {:.0}m", b.height));
                    }
                    if b.floors > 0 {
                        if b.floors_below > 0 {
                            spec.push(format!("{}F/B{}", b.floors, b.floors_below));
                        } else {
                            spec.push(format!("{}F", b.floors));
                        }
                    }
                    if !b.usage.is_empty() {
                        spec.push(b.usage.clone());
                    }
                    if !b.structure.is_empty() {
                        spec.push(b.structure.clone());
                    }
                    if !spec.is_empty() {
                        lines.push(spec.join(" · "));
                    }
                    if !b.address.is_empty() {
                        lines.push(b.address.clone());
                    }
                    if b.flood >= 0.05 {
                        lines.push(format!("⚠ 想定最大浸水 {:.1}m（{}）", b.flood, b.flood_src));
                    }
                } else if ray_dir.y < -1e-4 {
                    let t = -cam.y / ray_dir.y;
                    let p = cam + ray_dir * t;
                    if self.rain_demo {
                        lines.push("rain: DEMO storm".into());
                    } else if let Some((size, levels, bounds)) = &self.rain_grid {
                        const MMH: [&str; 9] = [
                            "",
                            "<1mm/h",
                            "1-5mm/h",
                            "5-10mm/h",
                            "10-20mm/h",
                            "20-30mm/h",
                            "30-50mm/h",
                            "50-80mm/h",
                            "80mm/h+",
                        ];
                        let m_lon = 111_320.0 * data::plateau::SITE_LAT.to_radians().cos();
                        let lon = data::plateau::SITE_LON + p.x as f64 / m_lon;
                        let lat = data::plateau::SITE_LAT - p.z as f64 / 111_132.0;
                        let u = (lon - bounds[0]) / (bounds[2] - bounds[0]);
                        let v = (bounds[1] - lat) / (bounds[1] - bounds[3]);
                        if (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v) {
                            let x = (u * *size as f64) as usize % *size as usize;
                            let y = (v * *size as f64) as usize % *size as usize;
                            let lv = levels[y * *size as usize + x] as usize;
                            lines.push(if lv == 0 {
                                "降雨なし (live)".to_string()
                            } else {
                                format!("雨 lv{lv} ({})", MMH[lv.min(8)])
                            });
                        }
                    }
                }
            } else if self.mode == SceneMode::Okinawa {
                if ray_dir.y < -1e-4 {
                    let t = -fi.cam_pos.y / ray_dir.y;
                    let p = fi.cam_pos + ray_dir * t;
                    let h = oki_terrain(glam::Vec2::new(p.x, p.z));
                    if h > 0.3 {
                        lines.push(format!("陸地 · 標高 ~{h:.1} m"));
                    } else if h > -0.05 {
                        lines.push("波打ち際".to_string());
                    } else {
                        lines.push(format!("水深 ~{:.1} m", -h));
                    }
                }
            } else {
                let to_screen = |w: glam::Vec3| -> Option<egui::Pos2> {
                    let clip = fi.view_proj * w.extend(1.0);
                    if clip.w <= 0.0 {
                        return None;
                    }
                    let ndc = clip.truncate() / clip.w;
                    Some(egui::pos2(
                        rect.min.x + (ndc.x * 0.5 + 0.5) * rect.width(),
                        rect.min.y + (0.5 - ndc.y * 0.5) * rect.height(),
                    ))
                };
                let cam = fi.cam_pos;
                let occluded = |p: glam::Vec3| -> bool {
                    let to = p - cam;
                    let dist = to.length();
                    let dir = to / dist.max(1e-6);
                    let t_close = -cam.dot(dir);
                    t_close > 0.0 && t_close < dist && (cam + dir * t_close).length() < 0.995
                };

                let mut best_sat: Option<usize> = None;
                let mut best_sat_d2 = 11.0f32 * 11.0;
                if self.show_sats {
                    for (i, s) in self.sat_states.iter().enumerate() {
                        let wp = glam::Vec3::from([s.pos[0], s.pos[1], s.pos[2]])
                            + glam::Vec3::from([s.vel[0], s.vel[1], s.vel[2]]) * fi.sat_dt;
                        if let Some(sp) = to_screen(wp) {
                            let d2 = sp.distance_sq(cur);
                            if d2 < best_sat_d2 && !occluded(wp) {
                                best_sat_d2 = d2;
                                best_sat = Some(i);
                            }
                        }
                    }
                }
                let mut best_q: Option<usize> = None;
                let mut best_q_d2 = 14.0f32 * 14.0;
                if self.show_quakes {
                    for (i, q) in self.quake_list.iter().enumerate() {
                        let wp = astro::latlon_to_world(q.lat, q.lon) * 1.004;
                        if let Some(sp) = to_screen(wp) {
                            let d2 = sp.distance_sq(cur);
                            if d2 < best_q_d2 && !occluded(wp) {
                                best_q_d2 = d2;
                                best_q = Some(i);
                            }
                        }
                    }
                }

                if let Some(i) = best_q {
                    let q = &self.quake_list[i];
                    let age_h =
                        (astro::unix_seconds(Utc::now()) - q.unix_ms as f64 / 1000.0) / 3600.0;
                    lines.push(format!("M{:.1} · {}", q.mag, q.place));
                    lines.push(format!("{age_h:.1} h ago"));
                } else if let Some(i) = best_sat {
                    let s = &self.sat_states[i];
                    let name = self
                        .sat_idxs
                        .get(i)
                        .and_then(|&ix| self.sat_names.get(ix as usize))
                        .cloned()
                        .unwrap_or_else(|| "satellite".into());
                    let kind = match s.pos[3] as u32 {
                        0 => "LEO",
                        1 => "MEO",
                        2 => "GEO",
                        3 => "HEO",
                        _ => "ISS",
                    };
                    let wp = glam::Vec3::from([s.pos[0], s.pos[1], s.pos[2]]);
                    let alt = (wp.length() - 1.0) * 6371.0;
                    let spd = glam::Vec3::from([s.vel[0], s.vel[1], s.vel[2]]).length() * 6371.0;
                    lines.push(name);
                    lines.push(format!("{kind} · {alt:.0} km · {spd:.2} km/s"));
                } else {
                    let ndc = glam::Vec2::new(
                        (cur.x - rect.min.x) / rect.width() * 2.0 - 1.0,
                        -((cur.y - rect.min.y) / rect.height() * 2.0 - 1.0),
                    );
                    let pw = fi.inv_view_proj * glam::Vec4::new(ndc.x, ndc.y, 0.6, 1.0);
                    let dir = (pw.truncate() / pw.w - cam).normalize();
                    let b = cam.dot(dir);
                    let c = cam.dot(cam) - 1.0;
                    let h2 = b * b - c;
                    if h2 > 0.0 && -b - h2.sqrt() > 0.0 {
                        let p = cam + dir * (-b - h2.sqrt());
                        let lat = p.y.asin().to_degrees();
                        let lon = (-p.z).atan2(p.x).to_degrees();
                        lines.push(format!(
                            "{:.2}°{} {:.2}°{}",
                            lat.abs(),
                            if lat >= 0.0 { "N" } else { "S" },
                            lon.abs(),
                            if lon >= 0.0 { "E" } else { "W" },
                        ));
                        if self.show_wind
                            && let Some((w, h, grid)) = &self.wind_grid
                        {
                            let col = ((lon.rem_euclid(360.0) / 360.0) * *w as f32) as usize
                                % *w as usize;
                            let row = (((90.0 - lat) / 180.0) * (*h as f32 - 1.0))
                                .clamp(0.0, *h as f32 - 1.0)
                                as usize;
                            let px = grid[row * *w as usize + col];
                            let u = half::f16::from_bits(px[0]).to_f32();
                            let v = half::f16::from_bits(px[1]).to_f32();
                            lines.push(format!("WIND {:.1} m/s", (u * u + v * v).sqrt()));
                        }
                    }
                }
            }

            if !lines.is_empty() {
                let painter = ui.painter();
                let galley = painter.layout_no_wrap(
                    lines.join("\n"),
                    egui::FontId::monospace(11.0),
                    egui::Color32::from_rgb(205, 235, 250),
                );
                let pad = egui::vec2(8.0, 6.0);
                let pos = cur + egui::vec2(14.0, 12.0);
                let r = egui::Rect::from_min_size(pos, galley.size() + pad * 2.0);
                painter.rect_filled(
                    r,
                    5.0,
                    egui::Color32::from_rgba_unmultiplied(5, 12, 20, 225),
                );
                painter.rect_stroke(
                    r,
                    5.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(26, 62, 86)),
                    egui::StrokeKind::Outside,
                );
                painter.galley(pos + pad, galley, egui::Color32::WHITE);
            }
        }

        if resp.clicked_by(egui::PointerButton::Secondary) {
            self.show_hud = !self.show_hud;
        }

        if self.show_hud {
            self.hud(&ctx, &rs);
            egui::Area::new(egui::Id::new("hint"))
                .anchor(egui::Align2::RIGHT_BOTTOM, [-14.0, -10.0])
                .show(&ctx, |ui| {
                    ui.label(
                        egui::RichText::new(
                            "drag rotate · scroll zoom · double-click reset · right-click hud",
                        )
                        .monospace()
                        .size(10.5)
                        .color(egui::Color32::from_rgba_unmultiplied(140, 170, 190, 140)),
                    );
                });
        }

        ctx.request_repaint();
    }
}

// ------------------------------------------------------------- headless ----

struct ShotOpts {
    path: PathBuf,
    frames: u32,
    size: (u32, u32),
    lat: f32,
    lon: f32,
    dist: f32,
    framedump: Option<PathBuf>,
    spin: f32,
    warmup: u32,
    /// Camera distance multiplier per second (1.0 = hold, <1 = push in).
    dzoom: f32,
    trail_fade: f32,
    mode: SceneMode,
    demo: bool,
}

fn read_final(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &Scene,
    w: u32,
    h: u32,
) -> anyhow::Result<image::RgbaImage> {
    use anyhow::Context as _;
    let bpr = (w * 4).next_multiple_of(256);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: bpr as u64 * h as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let tex = scene.final_texture().context("no final texture")?;
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    let (s, r) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |res| {
        s.send(res).ok();
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    r.recv()??;
    let mapped = slice.get_mapped_range();
    let mut img = image::RgbaImage::new(w, h);
    for y in 0..h {
        let row = &mapped[(y * bpr) as usize..(y * bpr + w * 4) as usize];
        for x in 0..w {
            let i = (x * 4) as usize;
            img.put_pixel(x, y, image::Rgba([row[i], row[i + 1], row[i + 2], 255]));
        }
    }
    Ok(img)
}

fn run_shot(o: &ShotOpts, assets: SceneAssets) -> anyhow::Result<()> {
    use anyhow::Context as _;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .context("no GPU adapter")?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .context("no device")?;

    let mut scene = Scene::new(&device, &queue, &assets);
    let app_epoch_unix = astro::unix_seconds(Utc::now());

    let mut car_sim: Option<CarSim> = None;
    if o.mode == SceneMode::Tokyo {
        let mesh = data::plateau::load_city()?;
        log::info!("shot: {}", mesh.label);
        scene.upload_city(&device, &queue, &mesh.tiles);
        match data::rain::fetch_once(data::plateau::SITE_LON, data::plateau::SITE_LAT) {
            Ok(g) => {
                log::info!("shot: rain {} (max lv{})", g.label, g.max_level);
                scene.upload_rain(&device, &queue, g.size, &g.levels, g.bounds);
            }
            Err(e) => log::warn!("shot: rain unavailable ({e})"),
        }
        match data::roads::load_roads() {
            Ok(net) => {
                scene.upload_roads(&device, &net.ribbon_verts, &net.ribbon_indices);
                let mut statics = net.lamps.clone();
                statics.extend_from_slice(&mesh.beacons);
                scene.upload_static_lights(&device, &queue, bytemuck::cast_slice(&statics));
                let paths: Vec<(Vec<[f32; 2]>, u8)> =
                    net.paths.into_iter().map(|p| (p.pts, p.class)).collect();
                car_sim = Some(CarSim::new(&paths, 340));
            }
            Err(e) => log::warn!("shot: roads unavailable ({e})"),
        }
    }

    let mut sat_t0 = app_epoch_unix;
    if o.mode == SceneMode::Earth {
        match data::wind::load_snapshot() {
            Ok(g) => {
                log::info!("shot: wind {} ({})", g.label, g.source.tag());
                scene.upload_wind(&device, &queue, g.w, g.h, &g.data);
            }
            Err(e) => {
                log::warn!("shot: wind snapshot failed ({e:#}), using synthetic");
                let g = data::wind::synthetic();
                scene.upload_wind(&device, &queue, g.w, g.h, &g.data);
            }
        }
        let (states, t0, label, source) = data::sats::offline_states();
        sat_t0 = t0;
        log::info!("shot: {} sats ({} · {})", states.len(), label, source.tag());
        scene.upload_sats(&device, &queue, bytemuck::cast_slice(&states));
        match data::quakes::fetch_once() {
            Ok((list, label)) => {
                log::info!("shot: quakes {label}");
                let packed = pack_quakes(&list, app_epoch_unix);
                scene.upload_quakes(&device, &queue, bytemuck::cast_slice(&packed));
            }
            Err(e) => log::warn!("shot: quakes unavailable ({e})"),
        }
        if let Some(c) = data::himawari::fetch_once() {
            log::info!("shot: clouds {}", c.label);
            scene.upload_clouds(&device, &queue, c.w, c.h, &c.rgba);
        }
    }

    let mut cam = OrbitCamera::new();
    cam.lat = o.lat;
    cam.lon = o.lon;
    cam.dist = o.dist;
    cam.auto_orbit = false;

    if let Some(dir) = &o.framedump {
        std::fs::create_dir_all(dir)?;
    }
    let total = o.frames + if o.framedump.is_some() { o.warmup } else { 0 };
    let dt = 1.0 / 60.0;
    for i in 0..total {
        if let Some(cars) = &mut car_sim {
            let lights = cars.step(dt);
            scene.upload_car_lights(&device, &queue, bytemuck::cast_slice(&lights));
        }
        cam.lon += o.spin * dt;
        cam.dist *= o.dzoom.powf(dt);
        cam.update(dt, (0.0, 0.0), 0.0, 1.0, o.size.1 as f32);
        let aspect = o.size.0 as f32 / o.size.1 as f32;
        let (vp, ivp, eye) = match o.mode {
            SceneMode::Earth => cam.matrices(aspect),
            SceneMode::Tokyo => {
                cam.matrices_local(aspect, TOKYO_TARGET, cam.dist * TOKYO_DIST_SCALE)
            }
            SceneMode::Okinawa => {
                cam.matrices_local(aspect, OKINAWA_TARGET, cam.dist * OKINAWA_DIST_SCALE)
            }
        };
        let utc = Utc::now();
        let unix = astro::unix_seconds(utc);
        let fi = FrameInput {
            view_proj: vp,
            inv_view_proj: ivp,
            cam_pos: eye,
            sun_dir: astro::sun_dir_world(utc),
            time: (unix - app_epoch_unix) as f32,
            sat_dt: (unix - sat_t0) as f32,
            trail_gain: 1.0,
            exposure: 1.15,
            res_scale: (o.size.1 as f32 / 1200.0).clamp(0.5, 3.0),
            layers: [1.0, 1.0, 1.0, 1.0],
            clouds: 1.0,
            sim_dt: dt,
            warp: 6000.0,
            trail_fade: o.trail_fade,
            frame_index: i,
            mode: o.mode,
            rain_demo: o.demo,
        };
        scene.render(&device, &queue, o.size, &fi);
        if let Some(dir) = &o.framedump
            && i >= o.warmup
        {
            let img = read_final(&device, &queue, &scene, o.size.0, o.size.1)?;
            img.save(dir.join(format!("{:04}.png", i - o.warmup)))?;
        }
    }

    let img = read_final(&device, &queue, &scene, o.size.0, o.size.1)?;
    img.save(&o.path)?;
    println!("saved {}", o.path.display());
    Ok(())
}

// ----------------------------------------------------------------- main ----

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn,zbus=warn"),
    )
    .init();

    let mut shot: Option<PathBuf> = None;
    let mut frames = 240u32;
    let mut size = (1920u32, 1080u32);
    let (mut lat, mut lon, mut dist) = (26.0f32, 136.0f32, 3.3f32);
    let mut framedump: Option<PathBuf> = None;
    let mut spin = 0.0f32;
    let mut warmup = 150u32;
    let mut dzoom = 1.0f32;
    let mut trail_fade = 0.958f32;
    let mut mode = SceneMode::Earth;
    let mut demo = true;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--shot" => {
                shot = Some(PathBuf::from(
                    args.next().unwrap_or_else(|| "shot.png".into()),
                ))
            }
            "--frames" => frames = parse_u32(&mut args, frames, 1, 20_000),
            "--size" => {
                if let Some(s) = args.next()
                    && let Some((a, b)) = s.split_once('x')
                    && let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>())
                {
                    size = (a.clamp(16, 4096), b.clamp(16, 4096));
                }
            }
            "--lat" => lat = parse_f32(&mut args, lat, -88.0, 88.0),
            "--lon" => lon = parse_f32(&mut args, lon, -720.0, 720.0),
            "--dist" => dist = parse_f32(&mut args, dist, 1.06, 40.0),
            "--framedump" => framedump = args.next().map(PathBuf::from),
            "--spin" => spin = parse_f32(&mut args, spin, -90.0, 90.0),
            "--warmup" => warmup = parse_u32(&mut args, warmup, 0, 5_000),
            "--dzoom" => dzoom = parse_f32(&mut args, dzoom, 0.5, 1.5),
            "--trailfade" => trail_fade = parse_f32(&mut args, trail_fade, 0.5, 0.998),
            "--mode" => {
                mode = match args.next().as_deref() {
                    Some("tokyo") => SceneMode::Tokyo,
                    Some("okinawa") => SceneMode::Okinawa,
                    _ => SceneMode::Earth,
                }
            }
            "--demo" => demo = args.next().as_deref() != Some("0"),
            other => log::warn!("unknown arg: {other}"),
        }
    }

    let assets = load_assets()?;

    if let Some(path) = shot {
        return run_shot(
            &ShotOpts {
                path,
                frames,
                size,
                lat,
                lon,
                dist,
                framedump,
                spin,
                warmup,
                dzoom,
                trail_fade,
                mode,
                demo,
            },
            assets,
        );
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1500.0, 950.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("COOLVIZ // LIVE EARTH"),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "coolviz",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, assets)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}

#[cfg(test)]
mod tests {
    use super::{parse_f32, parse_u32};

    #[test]
    fn cli_values_are_clamped_and_nan_rejected() {
        let mut it = vec!["NaN".to_string()].into_iter();
        assert_eq!(parse_f32(&mut it, 3.3, 1.06, 40.0), 3.3);

        let mut it = vec!["-5".to_string()].into_iter();
        assert_eq!(parse_f32(&mut it, 1.0, 0.5, 1.5), 0.5);

        let mut it = vec!["9999999".to_string()].into_iter();
        assert_eq!(parse_u32(&mut it, 240, 1, 20_000), 20_000);

        let mut it = std::iter::empty::<String>();
        assert_eq!(parse_u32(&mut it, 7, 1, 10), 7);

        let mut it = vec!["abc".to_string()].into_iter();
        assert_eq!(parse_f32(&mut it, 2.0, 0.0, 4.0), 2.0);
    }
}
