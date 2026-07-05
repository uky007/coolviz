//! Generic storage-buffer-driven billboard sprite pass (satellites, quakes).
//! Instances are pairs of vec4s; the shader decides what they mean.

use super::{ADDITIVE_BLEND, HDR_FORMAT, depth_test_no_write, storage_entry, uniform_entry};

pub struct SpritePass {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    buf: wgpu::Buffer,
    capacity: u32, // in instances (32 bytes each)
    pub count: u32,
    label: &'static str,
}

impl SpritePass {
    pub fn new(
        device: &wgpu::Device,
        globals: &wgpu::Buffer,
        shader_src: &str,
        label: &'static str,
        capacity: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(shader_src.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &[uniform_entry(0), storage_entry(1, true)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
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
                    blend: Some(ADDITIVE_BLEND),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_test_no_write()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: capacity as u64 * 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = Self::make_bg(device, &bgl, globals, &buf, label);

        Self {
            pipeline,
            bgl,
            bind_group,
            buf,
            capacity,
            count: 0,
            label,
        }
    }

    fn make_bg(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        buf: &wgpu::Buffer,
        label: &'static str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buf.as_entire_binding(),
                },
            ],
        })
    }

    /// `data` must be instances of 32 bytes each.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        globals: &wgpu::Buffer,
        data: &[u8],
    ) {
        let n = (data.len() / 32) as u32;
        if n > self.capacity {
            self.capacity = n.next_multiple_of(1024);
            self.buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: self.capacity as u64 * 32,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.bind_group = Self::make_bg(device, &self.bgl, globals, &self.buf, self.label);
        }
        if !data.is_empty() {
            queue.write_buffer(&self.buf, 0, data);
        }
        self.count = n;
    }

    pub fn record(&self, rp: &mut wgpu::RenderPass<'_>) {
        if self.count == 0 {
            return;
        }
        rp.set_pipeline(&self.pipeline);
        rp.set_bind_group(0, &self.bind_group, &[]);
        rp.draw(0..self.count * 6, 0..1);
    }
}
