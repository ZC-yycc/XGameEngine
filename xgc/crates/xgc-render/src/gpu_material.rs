//! 材质 uniform 批量缓冲：跨帧复用，256 字节动态 offset 对齐。

const STRIDE: u64 = 256;

#[derive(Default)]
pub struct GpuMaterialBatch {
    buffer: Option<wgpu::Buffer>,
    bind_group: Option<wgpu::BindGroup>,
    capacity: u64,
    format: Option<wgpu::TextureFormat>,
}

impl GpuMaterialBatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// 保证至少 `slots` 个材质槽；容量 / 格式变化才重建缓冲与 bind group。
    pub fn ensure(
        &mut self,
        context: &super::gpu::GpuContext,
        format: wgpu::TextureFormat,
        layout: &wgpu::BindGroupLayout,
        slots: usize,
    ) -> (&wgpu::Buffer, &wgpu::BindGroup) {
        let needed = (slots.max(1) as u64) * STRIDE;
        let rebuild = self.format != Some(format) || self.capacity < needed;
        if rebuild {
            let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("xgc-material-batch"),
                size: needed.next_power_of_two().max(STRIDE),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = context
                .device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("xgc-material-batch-bind"),
                    layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buffer,
                            offset: 0,
                            size: wgpu::BufferSize::new(80),
                        }),
                    }],
                });
            self.capacity = buffer.size();
            self.buffer = Some(buffer);
            self.bind_group = Some(bind_group);
            self.format = Some(format);
        }
        (
            self.buffer.as_ref().unwrap(),
            self.bind_group.as_ref().unwrap(),
        )
    }
}
