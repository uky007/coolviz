//! The renderer. Owns every GPU resource and draws one frame into an
//! offscreen chain: scene (HDR + depth) -> particle trails -> composite ->
//! bloom -> tonemap -> final RGBA8 sRGB texture (shown by egui or saved to PNG).

mod coast;
mod globe;
mod ocean;
mod particles;
mod post;
mod sprites;
mod swe;
mod tokyo;

pub use swe::SweConfig;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

pub use particles::ParticlePass;
pub use sprites::SpritePass;

pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const FINAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const BLOOM_MIPS: u32 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SceneMode {
    Earth,
    Tokyo,
    Okinawa,
}

pub struct SceneAssets {
    pub land_w: u32,
    pub land_h: u32,
    pub land_mask: Vec<u8>,
    pub coast_vertices: Vec<[f32; 3]>,
    pub coast_indices: Vec<u32>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    cam_pos: [f32; 4],
    sun_dir: [f32; 4],
    viewport: [f32; 4],
    params: [f32; 4],
    layers: [f32; 4],
    layers2: [f32; 4],
}

pub struct FrameInput {
    pub view_proj: Mat4,
    pub inv_view_proj: Mat4,
    pub cam_pos: Vec3,
    pub sun_dir: Vec3,
    /// App-relative time in seconds.
    pub time: f32,
    /// Seconds since the satellite state vector epoch (for GPU extrapolation).
    pub sat_dt: f32,
    pub trail_gain: f32,
    pub exposure: f32,
    /// HUD/marker scale relative to a 1200px-tall viewport.
    pub res_scale: f32,
    /// x wind, y sats, z quakes, w coast — each 0..1.
    pub layers: [f32; 4],
    /// Cloud layer opacity 0..1.
    pub clouds: f32,
    pub sim_dt: f32,
    pub warp: f32,
    pub trail_fade: f32,
    pub frame_index: u32,
    pub mode: SceneMode,
    /// Tokyo mode: use the procedural demo storm instead of live nowcast.
    pub rain_demo: bool,
    /// Shallow-water sim controls.
    pub swe_speed: f32,
    /// 0 = rain only, 1 = levee-breach scenario (Tokyo).
    pub flood_source: u32,
    /// Grid-uv -> rain-uv affine for the flood rain source.
    pub rain_affine: [f32; 4],
    /// Okinawa: world x, z, amount(m) splash from the mouse.
    pub splash: Option<[f32; 3]>,
}

struct Targets {
    size: (u32, u32),
    hdr_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    trail_views: [wgpu::TextureView; 2],
    comp_view: wgpu::TextureView,
    bloom_views: Vec<wgpu::TextureView>,
    final_tex: wgpu::Texture,
    final_view: wgpu::TextureView,
    // Bind groups that reference per-size views.
    fade_bgs: [wgpu::BindGroup; 2], // [write_into_i] samples 1-i
    comp_bgs: [wgpu::BindGroup; 2], // [trail_source_i]
    down_first_bg: wgpu::BindGroup,
    down_bgs: Vec<wgpu::BindGroup>,
    up_bgs: Vec<wgpu::BindGroup>,
    tone_bg: wgpu::BindGroup,
}

pub struct Scene {
    globals_buf: wgpu::Buffer,
    clamp_samp: wgpu::Sampler,
    wrap_samp: wgpu::Sampler,
    globe: globe::GlobePass,
    coast: coast::CoastPass,
    pub particles: ParticlePass,
    pub sats: SpritePass,
    pub quakes: SpritePass,
    post: post::PostPass,
    pub tokyo: tokyo::TokyoPass,
    ocean: ocean::OceanPass,
    pub swe_tokyo: Option<swe::SweSim>,
    pub swe_oki: Option<swe::SweSim>,
    dummy_rain_view: wgpu::TextureView,
    targets: Option<Targets>,
    trail_front: usize,
    have_clouds: bool,
}

impl Scene {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, assets: &SceneAssets) -> Self {
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let clamp_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let wrap_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wrap-x"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let globe =
            globe::GlobePass::new(device, queue, &globals_buf, &wrap_samp, &clamp_samp, assets);
        let coast = coast::CoastPass::new(device, &globals_buf, assets);
        let particles = ParticlePass::new(device, &globals_buf, &wrap_samp, 600_000);
        let sats = SpritePass::new(
            device,
            &globals_buf,
            include_str!("../shaders/sats.wgsl"),
            "sats",
            20_000,
        );
        let quakes = SpritePass::new(
            device,
            &globals_buf,
            include_str!("../shaders/quakes.wgsl"),
            "quakes",
            1_000,
        );
        let post = post::PostPass::new(device);
        let tokyo = tokyo::TokyoPass::new(device, &globals_buf, &clamp_samp);
        let dummy_rain = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dummy-rain"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dummy_rain_view = dummy_rain.create_view(&wgpu::TextureViewDescriptor::default());
        let ocean = ocean::OceanPass::new(device, &globals_buf, &dummy_rain_view, &clamp_samp);

        Self {
            globals_buf,
            clamp_samp,
            wrap_samp,
            globe,
            coast,
            particles,
            sats,
            quakes,
            post,
            tokyo,
            ocean,
            swe_tokyo: None,
            swe_oki: None,
            dummy_rain_view,
            targets: None,
            trail_front: 0,
            have_clouds: false,
        }
    }

    /// Create the Tokyo flood sim once terrain + buildings are stamped.
    pub fn init_swe_tokyo(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cfg: SweConfig,
        terrain: &[f32],
    ) {
        let h0 = vec![0.0f32; terrain.len()];
        let rain_view = self.tokyo.rain_view();
        let sim = swe::SweSim::new(
            device,
            queue,
            cfg,
            terrain,
            h0,
            &rain_view,
            &self.clamp_samp,
        );
        self.tokyo.set_water(
            device,
            &self.globals_buf,
            &sim.map_buf,
            &sim.view,
            &self.clamp_samp,
        );
        self.swe_tokyo = Some(sim);
    }

    /// Lazily create the interactive Okinawa lagoon sim.
    fn ensure_swe_oki(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.swe_oki.is_some() {
            return;
        }
        let n = 512u32;
        let cell = 5.0f32;
        let origin = [-330.0 - 1280.0, -400.0 - 1280.0];
        let mut terr = vec![0.0f32; (n * n) as usize];
        let mut h0 = vec![0.0f32; (n * n) as usize];
        for gy in 0..n {
            for gx in 0..n {
                let x = origin[0] + (gx as f32 + 0.5) * cell;
                let z = origin[1] + (gy as f32 + 0.5) * cell;
                let b = crate::data::oki_terrain(glam::Vec2::new(x, z));
                let i = (gy * n + gx) as usize;
                terr[i] = b;
                h0[i] = (-b).max(0.0);
            }
        }
        // Source mode 2 reuses the rain-affine slot for the swell geometry:
        // island centre in grid cells, plus wavenumber per cell (~90 m swell).
        let island_g = [(-330.0 - origin[0]) / cell, (-400.0 - origin[1]) / cell];
        let cfg = SweConfig {
            n,
            cell,
            origin,
            source_mode: 2,
            sea_level: 0.0,
            swell_amp: 0.5,
            swell_period: 8.0,
            rain_rate: 0.0,
            rain_affine: [
                island_g[0],
                island_g[1],
                std::f32::consts::TAU * cell / 90.0,
                0.0,
            ],
        };
        let mut sim = swe::SweSim::new(
            device,
            queue,
            cfg,
            &terr,
            h0,
            &self.dummy_rain_view,
            &self.clamp_samp,
        );
        // Let the swell cross the lagoon before anyone sees it.
        sim.prewarm(device, queue, 90.0);
        self.ocean.set_sim(
            device,
            &self.globals_buf,
            &sim.map_buf,
            &sim.view,
            &self.clamp_samp,
        );
        self.swe_oki = Some(sim);
        log::info!("okinawa lagoon sim initialised (512x512, 5 m cells)");
    }

    pub fn reset_swe_tokyo(&mut self, queue: &wgpu::Queue) {
        if let Some(s) = &mut self.swe_tokyo {
            s.reset(queue);
        }
    }

    pub fn tokyo_sim_time(&self) -> Option<f32> {
        self.swe_tokyo.as_ref().map(|s| s.sim_time)
    }

    pub fn upload_city(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tiles: &[crate::data::plateau::CityTile],
    ) {
        self.tokyo
            .upload_city(device, queue, &self.clamp_samp, tiles);
    }

    pub fn upload_roads(&mut self, device: &wgpu::Device, verts: &[[f32; 4]], indices: &[u32]) {
        self.tokyo.upload_roads(device, verts, indices);
    }

    pub fn upload_static_lights(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
    ) {
        self.tokyo
            .lights_static
            .upload(device, queue, &self.globals_buf, data);
    }

    pub fn upload_car_lights(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
        self.tokyo
            .lights_cars
            .upload(device, queue, &self.globals_buf, data);
    }

    pub fn upload_rain(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: u32,
        levels: &[u8],
        bounds: [f64; 4],
    ) {
        self.tokyo.upload_rain(
            device,
            queue,
            &self.globals_buf,
            &self.clamp_samp,
            size,
            levels,
            bounds,
        );
        // The nowcast texture may have just been (re)created; point the
        // flood sim's rain source at the fresh one.
        if let Some(s) = &mut self.swe_tokyo {
            s.set_rain(device, &self.tokyo.rain_view(), &self.clamp_samp);
        }
    }

    pub fn upload_wind(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        w: u32,
        h: u32,
        data: &[[u16; 2]],
    ) {
        self.particles
            .upload_wind(device, queue, &self.wrap_samp, w, h, data);
    }

    pub fn set_particle_count(&mut self, device: &wgpu::Device, n: u32) {
        self.particles.set_count(device, &self.globals_buf, n);
    }

    pub fn upload_sats(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
        self.sats.upload(device, queue, &self.globals_buf, data);
    }

    pub fn upload_quakes(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
        self.quakes.upload(device, queue, &self.globals_buf, data);
    }

    pub fn upload_clouds(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        w: u32,
        h: u32,
        rgba: &[u8],
    ) {
        self.globe.upload_clouds(
            device,
            queue,
            &self.globals_buf,
            &self.wrap_samp,
            &self.clamp_samp,
            w,
            h,
            rgba,
        );
        self.have_clouds = true;
    }

    pub fn final_texture(&self) -> Option<&wgpu::Texture> {
        self.targets.as_ref().map(|t| &t.final_tex)
    }

    fn make_tex(
        device: &wgpu::Device,
        label: &str,
        w: u32,
        h: u32,
        format: wgpu::TextureFormat,
        mips: u32,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let (w, h) = (size.0.max(8), size.1.max(8));
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;

        let hdr = Self::make_tex(device, "hdr", w, h, HDR_FORMAT, 1, rt);
        let depth = Self::make_tex(
            device,
            "depth",
            w,
            h,
            DEPTH_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let trail0 = Self::make_tex(device, "trail0", w, h, HDR_FORMAT, 1, rt);
        let trail1 = Self::make_tex(device, "trail1", w, h, HDR_FORMAT, 1, rt);
        let comp = Self::make_tex(device, "comp", w, h, HDR_FORMAT, 1, rt);
        let bloom = Self::make_tex(device, "bloom", w / 2, h / 2, HDR_FORMAT, BLOOM_MIPS, rt);
        let final_tex = Self::make_tex(
            device,
            "final",
            w,
            h,
            FINAL_FORMAT,
            1,
            rt | wgpu::TextureUsages::COPY_SRC,
        );

        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let hdr_view = view(&hdr);
        let depth_view = view(&depth);
        let trail_views = [view(&trail0), view(&trail1)];
        let comp_view = view(&comp);
        let final_view = view(&final_tex);
        let bloom_views: Vec<wgpu::TextureView> = (0..BLOOM_MIPS)
            .map(|m| {
                bloom.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("bloom-mip"),
                    base_mip_level: m,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();

        // Clear both trail buffers so we don't composite garbage.
        // (Done via a throwaway encoder below.)

        let fade_bgs = [
            self.particles
                .make_fade_bg(device, &trail_views[1], &self.clamp_samp), // write into 0, sample 1
            self.particles
                .make_fade_bg(device, &trail_views[0], &self.clamp_samp), // write into 1, sample 0
        ];
        let comp_bgs = [
            self.post.make_comp_bg(
                device,
                &self.globals_buf,
                &hdr_view,
                &trail_views[0],
                &self.clamp_samp,
            ),
            self.post.make_comp_bg(
                device,
                &self.globals_buf,
                &hdr_view,
                &trail_views[1],
                &self.clamp_samp,
            ),
        ];
        let down_first_bg = self.post.make_comp_bg(
            device,
            &self.globals_buf,
            &comp_view,
            &comp_view,
            &self.clamp_samp,
        );
        let mut down_bgs = Vec::new();
        for view in &bloom_views[..(BLOOM_MIPS - 1) as usize] {
            down_bgs.push(self.post.make_sample_bg(device, view, &self.clamp_samp));
        }
        let mut up_bgs = Vec::new();
        for m in (1..BLOOM_MIPS as usize).rev() {
            up_bgs.push(
                self.post
                    .make_sample_bg(device, &bloom_views[m], &self.clamp_samp),
            );
        }
        let tone_bg = self.post.make_tone_bg(
            device,
            &self.globals_buf,
            &comp_view,
            &bloom_views[0],
            &self.clamp_samp,
        );

        self.targets = Some(Targets {
            size,
            hdr_view,
            depth_view,
            trail_views,
            comp_view,
            bloom_views,
            final_tex,
            final_view,
            fade_bgs,
            comp_bgs,
            down_first_bg,
            down_bgs,
            up_bgs,
            tone_bg,
        });
        self.trail_front = 0;
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        f: &FrameInput,
    ) -> &wgpu::TextureView {
        self.ensure_targets(device, size);

        let g = Globals {
            view_proj: f.view_proj.to_cols_array_2d(),
            inv_view_proj: f.inv_view_proj.to_cols_array_2d(),
            cam_pos: [f.cam_pos.x, f.cam_pos.y, f.cam_pos.z, f.time],
            sun_dir: [f.sun_dir.x, f.sun_dir.y, f.sun_dir.z, 0.0],
            viewport: [
                size.0 as f32,
                size.1 as f32,
                1.0 / size.0.max(1) as f32,
                1.0 / size.1.max(1) as f32,
            ],
            params: [f.trail_gain, f.sat_dt, f.exposure, f.res_scale],
            layers: f.layers,
            layers2: [
                f.clouds,
                if self.have_clouds { 1.0 } else { 0.0 },
                // Bloom threshold: bright daylight scenes only glow above it.
                if f.mode == SceneMode::Okinawa {
                    0.55
                } else {
                    0.0
                },
                0.0,
            ],
        };
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&g));
        self.particles
            .write_params(queue, f.sim_dt, f.warp, f.frame_index, f.trail_fade);
        if f.mode == SceneMode::Tokyo {
            self.tokyo
                .write_params(queue, f.rain_demo, f.sim_dt, f.frame_index);
        }

        let front = self.trail_front;
        let back = 1 - front;

        if f.mode == SceneMode::Okinawa {
            self.ensure_swe_oki(device, queue);
        }

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });

        // Shallow-water simulation substeps.
        match f.mode {
            SceneMode::Tokyo => {
                if let Some(s) = &mut self.swe_tokyo {
                    s.cfg.rain_affine = f.rain_affine;
                    // No nowcast mapping yet -> no rain source.
                    let cfg_rain = if f.rain_affine == [0.0; 4] {
                        0.0
                    } else {
                        s.cfg.rain_rate
                    };
                    s.step(
                        &mut enc,
                        queue,
                        f.sim_dt,
                        f.swe_speed,
                        Some(f.flood_source),
                        0.0035, // river stage rise (m/s)
                        cfg_rain,
                        None,
                    );
                }
            }
            SceneMode::Okinawa => {
                if let Some(s) = &mut self.swe_oki {
                    s.step(&mut enc, queue, f.sim_dt, 1.0, None, 0.0, 0.0, f.splash);
                }
            }
            SceneMode::Earth => {}
        }

        let t = self.targets.as_ref().unwrap();

        // 1+2. Mode-specific scene pass and simulation pass.
        {
            if f.mode == SceneMode::Earth && f.layers[0] > 0.001 {
                let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("wind-sim"),
                    timestamp_writes: None,
                });
                self.particles.record_sim(&mut cp);
            }
            if f.mode == SceneMode::Tokyo {
                let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("rain-sim"),
                    timestamp_writes: None,
                });
                self.tokyo.record_sim(&mut cp);
            }

            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.hdr_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            match f.mode {
                SceneMode::Earth => {
                    self.globe.record(&mut rp);
                    if f.layers[3] > 0.001 {
                        self.coast.record(&mut rp);
                    }
                    if f.layers[1] > 0.001 {
                        self.sats.record(&mut rp);
                    }
                    if f.layers[2] > 0.001 {
                        self.quakes.record(&mut rp);
                    }
                }
                SceneMode::Tokyo => self.tokyo.record(&mut rp),
                SceneMode::Okinawa => self.ocean.record(&mut rp),
            }
        }

        // 3. Trail fade: back <- front * fade.
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trail-fade"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.trail_views[back],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.particles.record_fade(&mut rp, &t.fade_bgs[back]);
        }

        // 4. Particle segments, additively, depth-tested against the globe.
        if f.mode == SceneMode::Earth && f.layers[0] > 0.001 {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trail-segments"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.trail_views[back],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.particles.record_segments(&mut rp);
        }

        // 5. Composite HDR + trails.
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.comp_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.post.record_composite(&mut rp, &t.comp_bgs[back]);
        }

        // 6. Bloom chain.
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom-down-first"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.bloom_views[0],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.post.record_down_first(&mut rp, &t.down_first_bg);
        }
        for (view, bg) in t.bloom_views[1..].iter().zip(&t.down_bgs) {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom-down"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.post.record_down(&mut rp, bg);
        }
        for (i, m) in (0..(BLOOM_MIPS as usize - 1)).rev().enumerate() {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom-up"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.bloom_views[m],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.post.record_up(&mut rp, &t.up_bgs[i]);
        }

        // 7. Tonemap into the final sRGB texture.
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tonemap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.final_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.post.record_tonemap(&mut rp, &t.tone_bg);
        }

        queue.submit(Some(enc.finish()));
        self.trail_front = back;

        &self.targets.as_ref().unwrap().final_view
    }
}

/// Standard bind-group-layout helpers shared by passes.
pub(crate) fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub(crate) fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

pub(crate) fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

pub(crate) fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        // Writable storage in the vertex stage needs an extra feature; keep
        // writes compute-only.
        visibility: if read_only {
            wgpu::ShaderStages::VERTEX_FRAGMENT.union(wgpu::ShaderStages::COMPUTE)
        } else {
            wgpu::ShaderStages::COMPUTE
        },
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub(crate) const ADDITIVE_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

pub(crate) fn depth_test_no_write() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::LessEqual),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}
