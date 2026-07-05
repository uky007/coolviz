//! Post-processing: composite, bloom pyramid, tonemap.

use super::{
    ADDITIVE_BLEND, FINAL_FORMAT, HDR_FORMAT, sampler_entry, texture_entry, uniform_entry,
};

pub struct PostPass {
    comp_pipeline: wgpu::RenderPipeline,
    comp_bgl: wgpu::BindGroupLayout,
    down_first_pipeline: wgpu::RenderPipeline,
    down_pipeline: wgpu::RenderPipeline,
    up_pipeline: wgpu::RenderPipeline,
    sample_bgl: wgpu::BindGroupLayout,
    tone_pipeline: wgpu::RenderPipeline,
    tone_bgl: wgpu::BindGroupLayout,
}

fn full_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    label: &str,
    fs_entry: &str,
    bgl: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_full"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

impl PostPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/post.wgsl").into()),
        });

        let comp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite"),
            entries: &[
                uniform_entry(0),
                texture_entry(1),
                texture_entry(2),
                sampler_entry(3),
            ],
        });
        let sample_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sample"),
            entries: &[texture_entry(1), sampler_entry(3)],
        });
        let tone_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tonemap"),
            entries: &[
                uniform_entry(0),
                texture_entry(1),
                texture_entry(2),
                sampler_entry(3),
            ],
        });

        let comp_pipeline = full_pipeline(
            device,
            &shader,
            "composite",
            "fs_composite",
            &comp_bgl,
            HDR_FORMAT,
            None,
        );
        let down_first_pipeline = full_pipeline(
            device,
            &shader,
            "bloom-down-first",
            "fs_down_first",
            &comp_bgl,
            HDR_FORMAT,
            None,
        );
        let down_pipeline = full_pipeline(
            device,
            &shader,
            "bloom-down",
            "fs_down",
            &sample_bgl,
            HDR_FORMAT,
            None,
        );
        let up_pipeline = full_pipeline(
            device,
            &shader,
            "bloom-up",
            "fs_up",
            &sample_bgl,
            HDR_FORMAT,
            Some(ADDITIVE_BLEND),
        );
        let tone_pipeline = full_pipeline(
            device,
            &shader,
            "tonemap",
            "fs_tonemap",
            &tone_bgl,
            FINAL_FORMAT,
            None,
        );

        Self {
            comp_pipeline,
            comp_bgl,
            down_first_pipeline,
            down_pipeline,
            up_pipeline,
            sample_bgl,
            tone_pipeline,
            tone_bgl,
        }
    }

    pub fn make_comp_bg(
        &self,
        device: &wgpu::Device,
        globals: &wgpu::Buffer,
        hdr: &wgpu::TextureView,
        trail: &wgpu::TextureView,
        samp: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite"),
            layout: &self.comp_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(hdr),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(trail),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(samp),
                },
            ],
        })
    }

    pub fn make_sample_bg(
        &self,
        device: &wgpu::Device,
        src: &wgpu::TextureView,
        samp: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sample"),
            layout: &self.sample_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(samp),
                },
            ],
        })
    }

    pub fn make_tone_bg(
        &self,
        device: &wgpu::Device,
        globals: &wgpu::Buffer,
        comp: &wgpu::TextureView,
        bloom: &wgpu::TextureView,
        samp: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tonemap"),
            layout: &self.tone_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(comp),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(bloom),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(samp),
                },
            ],
        })
    }

    pub fn record_composite(&self, rp: &mut wgpu::RenderPass<'_>, bg: &wgpu::BindGroup) {
        rp.set_pipeline(&self.comp_pipeline);
        rp.set_bind_group(0, bg, &[]);
        rp.draw(0..3, 0..1);
    }

    pub fn record_down_first(&self, rp: &mut wgpu::RenderPass<'_>, bg: &wgpu::BindGroup) {
        rp.set_pipeline(&self.down_first_pipeline);
        rp.set_bind_group(0, bg, &[]);
        rp.draw(0..3, 0..1);
    }

    pub fn record_down(&self, rp: &mut wgpu::RenderPass<'_>, bg: &wgpu::BindGroup) {
        rp.set_pipeline(&self.down_pipeline);
        rp.set_bind_group(0, bg, &[]);
        rp.draw(0..3, 0..1);
    }

    pub fn record_up(&self, rp: &mut wgpu::RenderPass<'_>, bg: &wgpu::BindGroup) {
        rp.set_pipeline(&self.up_pipeline);
        rp.set_bind_group(0, bg, &[]);
        rp.draw(0..3, 0..1);
    }

    pub fn record_tonemap(&self, rp: &mut wgpu::RenderPass<'_>, bg: &wgpu::BindGroup) {
        rp.set_pipeline(&self.tone_pipeline);
        rp.set_bind_group(0, bg, &[]);
        rp.draw(0..3, 0..1);
    }
}
