//! GPU shallow-water simulation instance (virtual pipe model).

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SweParamsCpu {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct SimMapCpu {
    /// x, z of grid cell (0,0); z = cell size; w = N.
    pub a: [f32; 4],
    /// x = enabled.
    pub b: [f32; 4],
}

pub struct SweConfig {
    pub n: u32,
    pub cell: f32,
    /// World xz of grid cell (0,0).
    pub origin: [f32; 2],
    /// 0 = none, 1 = levee breach, 2 = ocean swell.
    pub source_mode: u32,
    pub sea_level: f32,
    pub swell_amp: f32,
    pub swell_period: f32,
    /// Rain depth rate (m/s) at nowcast level 1.0.
    pub rain_rate: f32,
    /// Affine grid-uv -> rain-uv mapping (offset, scale).
    pub rain_affine: [f32; 4],
}

pub struct SweSim {
    pub cfg: SweConfig,
    h: [wgpu::Buffer; 2],
    flux: [wgpu::Buffer; 2],
    terr: wgpu::Buffer,
    params: wgpu::Buffer,
    pub map_buf: wgpu::Buffer,
    tex: wgpu::Texture,
    pub view: wgpu::TextureView,
    bgl: wgpu::BindGroupLayout,
    bgs: [wgpu::BindGroup; 2],
    pl_flux: wgpu::ComputePipeline,
    pl_height: wgpu::ComputePipeline,
    pl_blit: wgpu::ComputePipeline,
    front: usize,
    h0: Vec<f32>,
    pub sim_time: f32,
    /// Largest stable substep (s), from the deepest possible water column.
    max_sub: f32,
}

impl SweSim {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cfg: SweConfig,
        terrain: &[f32],
        h0: Vec<f32>,
        rain_view: &wgpu::TextureView,
        samp: &wgpu::Sampler,
    ) -> Self {
        let n = cfg.n as usize;
        assert_eq!(terrain.len(), n * n);
        assert_eq!(h0.len(), n * n);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("swe"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/swe.wgsl").into()),
        });

        let mk_f32 = |label: &str, data: &[f32]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(data),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            })
        };
        let zeros4 = vec![[0.0f32; 4]; n * n];
        let h = [mk_f32("swe-h0", &h0), mk_f32("swe-h1", &h0)];
        let flux = [
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("swe-flux0"),
                contents: bytemuck::cast_slice(&zeros4),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            }),
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("swe-flux1"),
                contents: bytemuck::cast_slice(&zeros4),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            }),
        ];
        let terr = mk_f32("swe-terrain", terrain);

        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("swe-params"),
            size: std::mem::size_of::<SweParamsCpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let map = SimMapCpu {
            a: [cfg.origin[0], cfg.origin[1], cfg.cell, cfg.n as f32],
            b: [1.0, 0.0, 0.0, 0.0],
        };
        let map_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("swe-map"),
            contents: bytemuck::bytes_of(&map),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("swe-out"),
            size: wgpu::Extent3d {
                width: cfg.n,
                height: cfg.n,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());

        let buf_entry = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("swe"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                buf_entry(1, true),
                buf_entry(2, true),
                buf_entry(3, true),
                buf_entry(4, false),
                buf_entry(5, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("swe"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let mk_pl = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pl_flux = mk_pl("cs_flux");
        let pl_height = mk_pl("cs_height");
        let pl_blit = mk_pl("cs_blit");

        let mk_bg = |i_in: usize, i_out: usize| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("swe"),
                layout: &bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: terr.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: h[i_in].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: flux[i_in].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: h[i_out].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: flux[i_out].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(rain_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::Sampler(samp),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                ],
            })
        };
        let bgs = [mk_bg(0, 1), mk_bg(1, 0)];

        // CFL bound from the deepest water this sim can plausibly hold:
        // ocean floor depth for the lagoon, a few metres of flood for Tokyo.
        let depth_est = terrain.iter().fold(0.0f32, |m, &t| m.max(-t)).max(8.0) + 4.0;
        let max_sub = 0.28 * cfg.cell / (9.81 * depth_est).sqrt();

        let _ = queue;
        Self {
            cfg,
            h,
            flux,
            terr,
            params,
            map_buf,
            tex,
            view,
            bgl,
            bgs,
            pl_flux,
            pl_height,
            pl_blit,
            front: 0,
            h0,
            sim_time: 0.0,
            max_sub,
        }
    }

    /// Advance the sim without rendering, one submit per chunk so each chunk
    /// sees its own params (a single submit would collapse the uniform
    /// writes into one). Used to develop the lagoon swell before first view.
    pub fn prewarm(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, seconds: f32) {
        let chunk = 20.0 * self.max_sub;
        let mut left = seconds;
        while left > 0.0 {
            let dt = chunk.min(left);
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("swe-prewarm"),
            });
            self.step(&mut enc, queue, dt, 1.0, None, 0.0, 0.0, None);
            queue.submit(Some(enc.finish()));
            left -= dt;
        }
    }

    /// Debug readback: (max depth m, mean depth m, wet cells > 2 cm).
    pub fn depth_stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> (f32, f32, u32) {
        let bytes = (self.cfg.n as u64 * self.cfg.n as u64) * 4;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("swe-read"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("swe-read"),
        });
        enc.copy_buffer_to_buffer(&self.h[self.front], 0, &staging, 0, bytes);
        queue.submit(Some(enc.finish()));
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let mapped = slice.get_mapped_range();
        let f: &[f32] = bytemuck::cast_slice(&mapped);
        let mut mx = 0.0f32;
        let mut sum = 0.0f64;
        let mut wet = 0u32;
        for &v in f {
            mx = mx.max(v);
            sum += v as f64;
            if v > 0.02 {
                wet += 1;
            }
        }
        // Debug heatmap: depth 0..2 m mapped to grey, row 0 at the top.
        if std::env::var_os("COOLVIZ_DEBUG_STAMP").is_some() {
            let n = self.cfg.n;
            let px: Vec<u8> = f
                .iter()
                .map(|&v| ((v / 2.0).clamp(0.0, 1.0) * 255.0) as u8)
                .collect();
            if let Some(img) = image::GrayImage::from_raw(n, n, px) {
                let _ = img.save("shots/swe_depth.png");
            }
        }
        (mx, (sum / f.len() as f64) as f32, wet)
    }

    pub fn reset(&mut self, queue: &wgpu::Queue) {
        for b in &self.h {
            queue.write_buffer(b, 0, bytemuck::cast_slice(&self.h0));
        }
        let zeros = vec![[0.0f32; 4]; self.h0.len()];
        for b in &self.flux {
            queue.write_buffer(b, 0, bytemuck::cast_slice(&zeros));
        }
        self.sim_time = 0.0;
    }

    /// Rebind the rain texture (Tokyo: called after each nowcast upload).
    pub fn set_rain(
        &mut self,
        device: &wgpu::Device,
        rain_view: &wgpu::TextureView,
        samp: &wgpu::Sampler,
    ) {
        let mk_bg = |i_in: usize, i_out: usize| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("swe"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.terr.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.h[i_in].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.flux[i_in].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.h[i_out].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: self.flux[i_out].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(rain_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::Sampler(samp),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(&self.view),
                    },
                ],
            })
        };
        self.bgs = [mk_bg(0, 1), mk_bg(1, 0)];
    }

    /// Advance the simulation and refresh the render texture.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        frame_dt: f32,
        speed: f32,
        source_mode_override: Option<u32>,
        flood_rate: f32,
        rain_level_rate: f32,
        splash: Option<[f32; 3]>, // world x, z, amount (m)
    ) {
        let sim_dt_total = (frame_dt * speed).min(60.0);
        let substeps = ((sim_dt_total / self.max_sub).ceil() as u32).clamp(1, 32);
        let dt = sim_dt_total / substeps as f32;
        self.sim_time += sim_dt_total;

        let mode = source_mode_override.unwrap_or(self.cfg.source_mode) as f32;
        let (cx, cy, amt) = match splash {
            Some([wx, wz, a]) => (
                (wx - self.cfg.origin[0]) / self.cfg.cell,
                (wz - self.cfg.origin[1]) / self.cfg.cell,
                a / substeps as f32,
            ),
            None => (0.0, 0.0, 0.0),
        };
        let p = SweParamsCpu {
            a: [dt, self.cfg.cell, self.cfg.n as f32, 9.81],
            b: [rain_level_rate, mode, self.sim_time, 0.9985],
            c: [cx, cy, 3.5, amt],
            d: [
                self.cfg.sea_level,
                self.cfg.swell_amp,
                std::f32::consts::TAU / self.cfg.swell_period.max(0.5),
                flood_rate,
            ],
            e: self.cfg.rain_affine,
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&p));

        let groups = self.cfg.n.div_ceil(16);
        for _ in 0..substeps {
            let bg = &self.bgs[self.front];
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("swe"),
                timestamp_writes: None,
            });
            cp.set_bind_group(0, bg, &[]);
            cp.set_pipeline(&self.pl_flux);
            cp.dispatch_workgroups(groups, groups, 1);
            cp.set_pipeline(&self.pl_height);
            cp.dispatch_workgroups(groups, groups, 1);
            drop(cp);
            self.front = 1 - self.front;
        }
        // Blit the freshest state: it lives in the h_out/flux_out slots of
        // the bind group used for the LAST substep (before the final flip).
        {
            let bg = &self.bgs[1 - self.front];
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("swe-blit"),
                timestamp_writes: None,
            });
            cp.set_bind_group(0, bg, &[]);
            cp.set_pipeline(&self.pl_blit);
            cp.dispatch_workgroups(groups, groups, 1);
        }
        let _ = &self.tex;
        let _ = &self.terr;
    }
}
