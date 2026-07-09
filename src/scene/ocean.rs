//! OKINAWA SEA: fullscreen raymarch pass, optionally driven by the
//! shallow-water simulation texture inside its domain.

use wgpu::util::DeviceExt;

use super::{DEPTH_FORMAT, HDR_FORMAT, sampler_entry, texture_entry, uniform_entry};

pub struct OceanPass {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    fallback_map: wgpu::Buffer,
}

impl OceanPass {
    pub fn new(
        device: &wgpu::Device,
        globals: &wgpu::Buffer,
        dummy_tex: &wgpu::TextureView,
        samp: &wgpu::Sampler,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ocean"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/ocean.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean"),
            entries: &[
                uniform_entry(0),
                uniform_entry(1),
                texture_entry(2),
                sampler_entry(3),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ocean"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ocean"),
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
        // Disabled sim map until the lagoon sim exists.
        let fallback_map = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ocean-simmap-off"),
            contents: bytemuck::cast_slice(&[0.0f32; 8]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = Self::make_bg(device, &bgl, globals, &fallback_map, dummy_tex, samp);
        Self {
            pipeline,
            bgl,
            bind_group,
            fallback_map,
        }
    }

    fn make_bg(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        map: &wgpu::Buffer,
        tex: &wgpu::TextureView,
        samp: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ocean"),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: map.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(tex),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(samp),
                },
            ],
        })
    }

    pub fn set_sim(
        &mut self,
        device: &wgpu::Device,
        globals: &wgpu::Buffer,
        map: &wgpu::Buffer,
        tex: &wgpu::TextureView,
        samp: &wgpu::Sampler,
    ) {
        self.bind_group = Self::make_bg(device, &self.bgl, globals, map, tex, samp);
        let _ = &self.fallback_map;
    }

    pub fn record(&self, rp: &mut wgpu::RenderPass<'_>) {
        rp.set_pipeline(&self.pipeline);
        rp.set_bind_group(0, &self.bind_group, &[]);
        rp.draw(0..3, 0..1);
    }
}
