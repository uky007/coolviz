//! TOKYO STORM: storm sky + wet ground + PLATEAU buildings + rain particles.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::{ADDITIVE_BLEND, DEPTH_FORMAT, HDR_FORMAT, storage_entry};

const DROPS: u32 = 350_000;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RainMapCpu {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RainSimCpu {
    v0: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DropCpu {
    pos: [f32; 3],
    speed: f32,
}

fn entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT.union(wgpu::ShaderStages::COMPUTE),
        ty,
        count: None,
    }
}

fn uniform_ty() -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}

fn texture_ty() -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: true },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    }
}

struct CityTileGpu {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    n_indices: u32,
    atlas_bg: wgpu::BindGroup,
    _atlas: wgpu::Texture,
}

pub struct TokyoPass {
    // City surfaces.
    city_bgl: wgpu::BindGroupLayout,
    city_bg: wgpu::BindGroup,
    atlas_bgl: wgpu::BindGroupLayout,
    sky_pl: wgpu::RenderPipeline,
    ground_pl: wgpu::RenderPipeline,
    bldg_pl: wgpu::RenderPipeline,
    bldg: Vec<CityTileGpu>,
    pub city_loaded: bool,
    // Rain.
    rainmap_buf: wgpu::Buffer,
    rainsim_buf: wgpu::Buffer,
    rain_tex: wgpu::Texture,
    rain_dims: (u32, u32),
    /// lon_w, lat_n, lon_e, lat_s of the live nowcast composite.
    bounds: [f64; 4],
    have_live: bool,
    sim_bgl: wgpu::BindGroupLayout,
    sim_bg: wgpu::BindGroup,
    sim_pl: wgpu::ComputePipeline,
    draw_bg: wgpu::BindGroup,
    draw_pl: wgpu::RenderPipeline,
    drops_buf: wgpu::Buffer,
}

fn make_rain_tex(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rain-intensity"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

impl TokyoPass {
    pub fn new(device: &wgpu::Device, globals: &wgpu::Buffer, clamp_samp: &wgpu::Sampler) -> Self {
        let city_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("city"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/city.wgsl").into()),
        });
        let rain_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rain"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/rain.wgsl").into()),
        });

        let rainmap_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rainmap"),
            size: std::mem::size_of::<RainMapCpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rainsim_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rainsim"),
            size: std::mem::size_of::<RainSimCpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rain_tex = make_rain_tex(device, 4, 4);

        let mut rng = fastrand::Rng::with_seed(0x0A17_5EED);
        let drops: Vec<DropCpu> = (0..DROPS)
            .map(|_| DropCpu {
                pos: [
                    rng.f32() * 5200.0 - 2600.0,
                    -30.0,
                    rng.f32() * 5200.0 - 2600.0,
                ],
                speed: 100.0,
            })
            .collect();
        let drops_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("drops"),
            contents: bytemuck::cast_slice(&drops),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // ---- city bind group ----
        let city_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("city"),
            entries: &[
                entry(0, uniform_ty()),
                entry(1, uniform_ty()),
                entry(2, texture_ty()),
                entry(
                    3,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let make_city_bg =
            |device: &wgpu::Device, bgl: &wgpu::BindGroupLayout, view: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("city"),
                    layout: bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: globals.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: rainmap_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(clamp_samp),
                        },
                    ],
                })
            };
        let rain_view = rain_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let city_bg = make_city_bg(device, &city_bgl, &rain_view);

        let city_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("city"),
            bind_group_layouts: &[Some(&city_bgl)],
            immediate_size: 0,
        });

        let mk_city_pl = |label: &str,
                          vs: &str,
                          fs: &str,
                          buffers: &[wgpu::VertexBufferLayout],
                          depth_compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&city_layout),
                vertex: wgpu::VertexState {
                    module: &city_shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &city_shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(depth_compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let sky_pl = mk_city_pl(
            "tokyo-sky",
            "vs_sky",
            "fs_sky",
            &[],
            wgpu::CompareFunction::Always,
        );
        let ground_pl = mk_city_pl(
            "tokyo-ground",
            "vs_ground",
            "fs_ground",
            &[],
            wgpu::CompareFunction::LessEqual,
        );

        // Buildings carry a facade-photo atlas in bind group 1.
        let atlas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas"),
            entries: &[
                entry(0, texture_ty()),
                entry(
                    1,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let bldg_layout_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bldg"),
            bind_group_layouts: &[Some(&city_bgl), Some(&atlas_bgl)],
            immediate_size: 0,
        });
        let bldg_vlayout = wgpu::VertexBufferLayout {
            array_stride: 24,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 12,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 20,
                    shader_location: 2,
                },
            ],
        };
        let bldg_pl = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tokyo-bldg"),
            layout: Some(&bldg_layout_pl),
            vertex: wgpu::VertexState {
                module: &city_shader,
                entry_point: Some("vs_bldg"),
                compilation_options: Default::default(),
                buffers: std::slice::from_ref(&bldg_vlayout),
            },
            fragment: Some(wgpu::FragmentState {
                module: &city_shader,
                entry_point: Some("fs_bldg"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // ---- rain sim ----
        let sim_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rain-sim"),
            entries: &[
                entry(0, uniform_ty()),
                entry(1, uniform_ty()),
                entry(2, texture_ty()),
                entry(
                    3,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                entry(4, uniform_ty()),
                storage_entry(5, false),
            ],
        });
        let make_sim_bg =
            |device: &wgpu::Device, bgl: &wgpu::BindGroupLayout, view: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("rain-sim"),
                    layout: bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: globals.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: rainmap_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(clamp_samp),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: rainsim_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: drops_buf.as_entire_binding(),
                        },
                    ],
                })
            };
        let sim_bg = make_sim_bg(device, &sim_bgl, &rain_view);
        let sim_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rain-sim"),
            bind_group_layouts: &[Some(&sim_bgl)],
            immediate_size: 0,
        });
        let sim_pl = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("rain-sim"),
            layout: Some(&sim_layout),
            module: &rain_shader,
            entry_point: Some("cs_rain"),
            compilation_options: Default::default(),
            cache: None,
        });

        // ---- rain draw ----
        let draw_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rain-draw"),
            entries: &[
                entry(0, uniform_ty()),
                entry(1, uniform_ty()),
                storage_entry(6, true),
            ],
        });
        let draw_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rain-draw"),
            layout: &draw_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: rainmap_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: drops_buf.as_entire_binding(),
                },
            ],
        });
        let draw_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rain-draw"),
            bind_group_layouts: &[Some(&draw_bgl)],
            immediate_size: 0,
        });
        let draw_pl = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rain-draw"),
            layout: Some(&draw_layout),
            vertex: wgpu::VertexState {
                module: &rain_shader,
                entry_point: Some("vs_rain"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &rain_shader,
                entry_point: Some("fs_rain"),
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
            depth_stencil: Some(super::depth_test_no_write()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            city_bgl,
            city_bg,
            atlas_bgl,
            sky_pl,
            ground_pl,
            bldg_pl,
            bldg: Vec::new(),
            city_loaded: false,
            rainmap_buf,
            rainsim_buf,
            rain_tex,
            rain_dims: (4, 4),
            bounds: [0.0; 4],
            have_live: false,
            sim_bgl,
            sim_bg,
            sim_pl,
            draw_bg,
            draw_pl,
            drops_buf,
        }
    }

    pub fn upload_city(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        clamp_samp: &wgpu::Sampler,
        tiles: &[crate::data::plateau::CityTile],
    ) {
        let asize = crate::data::plateau::ATLAS_SIZE;
        self.bldg = tiles
            .iter()
            .map(|t| {
                let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("bldg-verts"),
                    contents: bytemuck::cast_slice(&t.verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("bldg-indices"),
                    contents: bytemuck::cast_slice(&t.indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
                let atlas = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("facade-atlas"),
                    size: wgpu::Extent3d {
                        width: asize,
                        height: asize,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &atlas,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &t.atlas,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(asize * 4),
                        rows_per_image: Some(asize),
                    },
                    wgpu::Extent3d {
                        width: asize,
                        height: asize,
                        depth_or_array_layers: 1,
                    },
                );
                let view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
                let atlas_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("atlas"),
                    layout: &self.atlas_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(clamp_samp),
                        },
                    ],
                });
                CityTileGpu {
                    vbuf,
                    ibuf,
                    n_indices: t.indices.len() as u32,
                    atlas_bg,
                    _atlas: atlas,
                }
            })
            .collect();
        self.city_loaded = !self.bldg.is_empty();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upload_rain(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        globals: &wgpu::Buffer,
        clamp_samp: &wgpu::Sampler,
        size: u32,
        levels: &[u8],
        bounds: [f64; 4],
    ) {
        let data: Vec<u8> = levels.iter().map(|&l| l.saturating_mul(31)).collect();
        if (size, size) != self.rain_dims {
            self.rain_tex = make_rain_tex(device, size, size);
            self.rain_dims = (size, size);
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.rain_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        self.bounds = bounds;
        self.have_live = true;
        // Rebuild the two bind groups that reference the texture.
        let view = self
            .rain_tex
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.city_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("city"),
            layout: &self.city_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.rainmap_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(clamp_samp),
                },
            ],
        });
        self.sim_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rain-sim"),
            layout: &self.sim_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.rainmap_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(clamp_samp),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.rainsim_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.drops_buf.as_entire_binding(),
                },
            ],
        });
    }

    pub fn write_params(&self, queue: &wgpu::Queue, demo: bool, dt: f32, frame: u32) {
        let lat = crate::data::plateau::SITE_LAT;
        let m_per_deg_lat = 111_132.0;
        let m_per_deg_lon = 111_320.0 * lat.to_radians().cos();
        let rm = RainMapCpu {
            a: [
                crate::data::plateau::SITE_LON as f32,
                lat as f32,
                (1.0 / m_per_deg_lon) as f32,
                (1.0 / m_per_deg_lat) as f32,
            ],
            b: [
                self.bounds[0] as f32,
                self.bounds[1] as f32,
                if self.bounds[2] != self.bounds[0] {
                    (1.0 / (self.bounds[2] - self.bounds[0])) as f32
                } else {
                    0.0
                },
                if self.bounds[1] != self.bounds[3] {
                    (1.0 / (self.bounds[1] - self.bounds[3])) as f32
                } else {
                    0.0
                },
            ],
            c: [
                if demo || !self.have_live { 1.0 } else { 0.0 },
                0.0,
                14.0, // wind, m/s
                0.0,
            ],
        };
        queue.write_buffer(&self.rainmap_buf, 0, bytemuck::bytes_of(&rm));
        let sim = RainSimCpu {
            v0: [dt, DROPS as f32, (frame % 1_000_000) as f32, 0.5],
        };
        queue.write_buffer(&self.rainsim_buf, 0, bytemuck::bytes_of(&sim));
    }

    pub fn record_sim(&self, cp: &mut wgpu::ComputePass<'_>) {
        cp.set_pipeline(&self.sim_pl);
        cp.set_bind_group(0, &self.sim_bg, &[]);
        cp.dispatch_workgroups(DROPS.div_ceil(256), 1, 1);
    }

    pub fn record(&self, rp: &mut wgpu::RenderPass<'_>) {
        rp.set_pipeline(&self.sky_pl);
        rp.set_bind_group(0, &self.city_bg, &[]);
        rp.draw(0..3, 0..1);
        rp.set_pipeline(&self.ground_pl);
        rp.draw(0..6, 0..1);
        if !self.bldg.is_empty() {
            rp.set_pipeline(&self.bldg_pl);
            for tile in &self.bldg {
                rp.set_bind_group(1, &tile.atlas_bg, &[]);
                rp.set_vertex_buffer(0, tile.vbuf.slice(..));
                rp.set_index_buffer(tile.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..tile.n_indices, 0, 0..1);
            }
        }
        rp.set_pipeline(&self.draw_pl);
        rp.set_bind_group(0, &self.draw_bg, &[]);
        rp.draw(0..DROPS * 2, 0..1);
    }
}
