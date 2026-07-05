//! GPU wind particles: compute-shader advection + additive trail segments.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::{
    depth_test_no_write, sampler_entry, storage_entry, texture_entry, uniform_entry,
    ADDITIVE_BLEND, HDR_FORMAT,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ParticleCpu {
    cur: [f32; 2],
    prev: [f32; 2],
    age: f32,
    life: f32,
    speed: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SimParams {
    v0: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FadeParams {
    v0: [f32; 4],
}

pub struct ParticlePass {
    sim_pipeline: wgpu::ComputePipeline,
    sim_bgl: wgpu::BindGroupLayout,
    sim_bg: wgpu::BindGroup,
    sim_params: wgpu::Buffer,
    fade_pipeline: wgpu::RenderPipeline,
    fade_bgl: wgpu::BindGroupLayout,
    fade_params: wgpu::Buffer,
    seg_pipeline: wgpu::RenderPipeline,
    seg_bgl: wgpu::BindGroupLayout,
    seg_bg: wgpu::BindGroup,
    buf: wgpu::Buffer,
    pub count: u32,
    capacity: u32,
    wind_tex: wgpu::Texture,
    pub wind_dims: (u32, u32),
}

fn init_particles(n: u32) -> Vec<ParticleCpu> {
    let mut rng = fastrand::Rng::with_seed(0xC001_C001);
    (0..n)
        .map(|_| {
            let lon = rng.f32() * 360.0 - 180.0;
            let lat = (rng.f32() * 2.0 - 1.0).clamp(-0.999, 0.999).asin().to_degrees();
            let life = 4.0 + 9.0 * rng.f32();
            ParticleCpu {
                cur: [lon, lat],
                prev: [lon, lat],
                age: rng.f32() * life,
                life,
                speed: 0.0,
                _pad: 0.0,
            }
        })
        .collect()
}

impl ParticlePass {
    pub fn new(
        device: &wgpu::Device,
        globals: &wgpu::Buffer,
        wrap_samp: &wgpu::Sampler,
        count: u32,
    ) -> Self {
        let sim_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles-sim"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/particles.wgsl").into()),
        });
        let trail_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("trails"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/trails.wgsl").into()),
        });

        // --- simulation ---
        let sim_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sim"),
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
                storage_entry(1, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sim_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sim"),
            bind_group_layouts: &[Some(&sim_bgl)],
            immediate_size: 0,
        });
        let sim_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sim"),
            layout: Some(&sim_layout),
            module: &sim_shader,
            entry_point: Some("cs_update"),
            compilation_options: Default::default(),
            cache: None,
        });

        let sim_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sim-params"),
            size: std::mem::size_of::<SimParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Placeholder wind texture until real data arrives.
        let (wind_tex, wind_dims) = Self::create_wind_tex(device, 16, 8);

        let capacity = count.max(1);
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particles"),
            contents: bytemuck::cast_slice(&init_particles(capacity)),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // --- fade ---
        let fade_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fade"),
            entries: &[texture_entry(0), sampler_entry(1), uniform_entry(2)],
        });
        let fade_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fade"),
            bind_group_layouts: &[Some(&fade_bgl)],
            immediate_size: 0,
        });
        let fade_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fade"),
            layout: Some(&fade_layout),
            vertex: wgpu::VertexState {
                module: &trail_shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &trail_shader,
                entry_point: Some("fs_fade"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let fade_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fade-params"),
            size: std::mem::size_of::<FadeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- segments ---
        let seg_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("segments"),
            entries: &[uniform_entry(3), storage_entry(4, true)],
        });
        let seg_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("segments"),
            bind_group_layouts: &[Some(&seg_bgl)],
            immediate_size: 0,
        });
        let seg_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("segments"),
            layout: Some(&seg_layout),
            vertex: wgpu::VertexState {
                module: &trail_shader,
                entry_point: Some("vs_seg"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &trail_shader,
                entry_point: Some("fs_seg"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(ADDITIVE_BLEND),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(depth_test_no_write()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let wind_view = wind_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let sim_bg = Self::make_sim_bg(device, &sim_bgl, &sim_params, &buf, &wind_view, wrap_samp);
        let seg_bg = Self::make_seg_bg(device, &seg_bgl, globals, &buf);

        Self {
            sim_pipeline,
            sim_bgl,
            sim_bg,
            sim_params,
            fade_pipeline,
            fade_bgl,
            fade_params,
            seg_pipeline,
            seg_bgl,
            seg_bg,
            buf,
            count,
            capacity,
            wind_tex,
            wind_dims,
        }
    }

    fn create_wind_tex(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, (u32, u32)) {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wind"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        (tex, (w, h))
    }

    fn make_sim_bg(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        params: &wgpu::Buffer,
        buf: &wgpu::Buffer,
        wind_view: &wgpu::TextureView,
        samp: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sim"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(wind_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(samp),
                },
            ],
        })
    }

    fn make_seg_bg(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        buf: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("segments"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: buf.as_entire_binding(),
                },
            ],
        })
    }

    pub fn make_fade_bg(
        &self,
        device: &wgpu::Device,
        source: &wgpu::TextureView,
        samp: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fade"),
            layout: &self.fade_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(samp),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.fade_params.as_entire_binding(),
                },
            ],
        })
    }

    pub fn upload_wind(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        wrap_samp: &wgpu::Sampler,
        w: u32,
        h: u32,
        data: &[[u16; 2]],
    ) {
        if (w, h) != self.wind_dims {
            let (tex, dims) = Self::create_wind_tex(device, w, h);
            self.wind_tex = tex;
            self.wind_dims = dims;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.wind_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let wind_view = self
            .wind_tex
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.sim_bg = Self::make_sim_bg(
            device,
            &self.sim_bgl,
            &self.sim_params,
            &self.buf,
            &wind_view,
            wrap_samp,
        );
    }

    pub fn set_count(&mut self, device: &wgpu::Device, globals: &wgpu::Buffer, n: u32) {
        let n = n.clamp(10_000, 3_000_000);
        if n <= self.capacity {
            self.count = n;
            return;
        }
        self.capacity = n;
        self.count = n;
        self.buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particles"),
            contents: bytemuck::cast_slice(&init_particles(n)),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let wind_view = self
            .wind_tex
            .create_view(&wgpu::TextureViewDescriptor::default());
        // Sampler lives in Scene; sim bind group is rebuilt on next upload_wind…
        // …but we must not leave a stale buffer reference, so rebuild with a
        // temporary default sampler-compatible view now:
        self.seg_bg = Self::make_seg_bg(device, &self.seg_bgl, globals, &self.buf);
        // sim_bg rebuilt here with existing texture view; sampler recreated cheaply.
        let samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wrap-x-tmp"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        self.sim_bg = Self::make_sim_bg(
            device,
            &self.sim_bgl,
            &self.sim_params,
            &self.buf,
            &wind_view,
            &samp,
        );
    }

    pub fn write_params(
        &self,
        queue: &wgpu::Queue,
        dt: f32,
        warp: f32,
        frame_index: u32,
        fade: f32,
    ) {
        let p = SimParams {
            v0: [dt, warp, self.count as f32, (frame_index % 1_000_000) as f32],
        };
        queue.write_buffer(&self.sim_params, 0, bytemuck::bytes_of(&p));
        let fp = FadeParams {
            v0: [fade, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.fade_params, 0, bytemuck::bytes_of(&fp));
    }

    pub fn record_sim(&self, cp: &mut wgpu::ComputePass<'_>) {
        cp.set_pipeline(&self.sim_pipeline);
        cp.set_bind_group(0, &self.sim_bg, &[]);
        cp.dispatch_workgroups(self.count.div_ceil(256), 1, 1);
    }

    pub fn record_fade(&self, rp: &mut wgpu::RenderPass<'_>, bg: &wgpu::BindGroup) {
        rp.set_pipeline(&self.fade_pipeline);
        rp.set_bind_group(0, bg, &[]);
        rp.draw(0..3, 0..1);
    }

    pub fn record_segments(&self, rp: &mut wgpu::RenderPass<'_>) {
        rp.set_pipeline(&self.seg_pipeline);
        rp.set_bind_group(0, &self.seg_bg, &[]);
        rp.draw(0..self.count * 2, 0..1);
    }
}
