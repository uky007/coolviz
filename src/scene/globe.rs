//! Fullscreen raytraced globe + starfield pass, with the runtime land mask
//! and the live Himawari-9 cloud disk.

use super::{sampler_entry, texture_entry, uniform_entry, DEPTH_FORMAT, HDR_FORMAT};

pub struct GlobePass {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    _land_tex: wgpu::Texture,
    land_view: wgpu::TextureView,
    cloud_tex: wgpu::Texture,
    cloud_dims: (u32, u32),
}

fn make_cloud_tex(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("himawari"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

impl GlobePass {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        globals: &wgpu::Buffer,
        wrap_samp: &wgpu::Sampler,
        clamp_samp: &wgpu::Sampler,
        assets: &super::SceneAssets,
    ) -> Self {
        let land_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("land-mask"),
            size: wgpu::Extent3d {
                width: assets.land_w,
                height: assets.land_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &land_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &assets.land_mask,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(assets.land_w),
                rows_per_image: Some(assets.land_h),
            },
            wgpu::Extent3d {
                width: assets.land_w,
                height: assets.land_h,
                depth_or_array_layers: 1,
            },
        );
        let land_view = land_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let cloud_tex = make_cloud_tex(device, 8, 8);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("globe"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/globe.wgsl").into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globe"),
            entries: &[
                uniform_entry(0),
                texture_entry(1),
                sampler_entry(2),
                texture_entry(3),
                sampler_entry(4),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("globe"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("globe"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
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
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let cloud_view = cloud_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = Self::make_bg(
            device, &bgl, globals, &land_view, wrap_samp, &cloud_view, clamp_samp,
        );

        Self {
            pipeline,
            bgl,
            bind_group,
            _land_tex: land_tex,
            land_view,
            cloud_tex,
            cloud_dims: (8, 8),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_bg(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        land_view: &wgpu::TextureView,
        wrap_samp: &wgpu::Sampler,
        cloud_view: &wgpu::TextureView,
        clamp_samp: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globe"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(land_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(wrap_samp),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(cloud_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(clamp_samp),
                },
            ],
        })
    }

    pub fn upload_clouds(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        globals: &wgpu::Buffer,
        wrap_samp: &wgpu::Sampler,
        clamp_samp: &wgpu::Sampler,
        w: u32,
        h: u32,
        rgba: &[u8],
    ) {
        if (w, h) != self.cloud_dims {
            self.cloud_tex = make_cloud_tex(device, w, h);
            self.cloud_dims = (w, h);
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.cloud_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
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
        let cloud_view = self.cloud_tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.bind_group = Self::make_bg(
            device,
            &self.bgl,
            globals,
            &self.land_view,
            wrap_samp,
            &cloud_view,
            clamp_samp,
        );
    }

    pub fn record(&self, rp: &mut wgpu::RenderPass<'_>) {
        rp.set_pipeline(&self.pipeline);
        rp.set_bind_group(0, &self.bind_group, &[]);
        rp.draw(0..3, 0..1);
    }
}
