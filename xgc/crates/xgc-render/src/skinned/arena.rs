//! 蒙皮 uniform 的竞技场：把逐 draw 的 palette / model 并进一张缓冲，用 dynamic offset 取槽。
//!
//! 合并提交（第 27 轮）之后一帧只剩一次 submit，但每条蒙皮 draw 仍各建一张
//! palette（16 KiB）与一张 model（96 B）缓冲加两个绑定组：N 个角色就是每帧
//! 2N 张缓冲 + 2N 个绑定组。缓冲要走分配器、绑定组要过校验，角色越多 CPU 越贵。
//! 旧端也是"palette 逐角色"，但它把槽位并进常驻缓冲、用 dynamic offset 偏移，
//! 所以逐角色只多一次 `set_bind_group`。这里照同一口径做。
//!
//! 复用安全：`queue.write_buffer` 与提交在同一条队列上按序执行，重写本帧缓冲
//! 一定排在上一帧提交之后，不会写出正被 GPU 读的字节；只有容量不够时才重建
//! 缓冲与绑定组（旧缓冲仍被在飞的帧持有，wgpu 自己保活）。
//!
//! 对齐：layout 里的绑定声明 `has_dynamic_offset: true`，偏移必须是设备
//! `min_uniform_buffer_offset_alignment` 的整数倍，所以槽距按该对齐向上取整
//! （16 KiB palette 在 256 对齐下仍是 16 KiB；96 B 的 model 会补到 256 B）。

use crate::gpu::GpuContext;

/// 一张常驻的 uniform 缓冲 + 按槽切分的 dynamic offset 绑定组。
pub(crate) struct UniformArena {
    label: &'static str,
    /// 一个槽被着色器读到的字节数（= layout 的 `min_binding_size`）。
    slot_bytes: u64,
    /// 相邻槽起点之差：`slot_bytes` 按设备 uniform 偏移对齐向上取整。
    stride: u64,
    buffer: Option<wgpu::Buffer>,
    bind: Option<wgpu::BindGroup>,
    /// 当前缓冲装得下几个槽；不足时按 2 的幂扩（只增不减：封顶在历史峰值槽数，
    /// 一次角色数尖峰之后缓冲常驻，不跟着回落，避免逐帧抖动重建）。
    slots: u32,
    /// 上传暂存区：跨帧复用，稳态下稳态只剩一次 memset 与按槽拷贝，不再每帧分配。
    staging: Vec<u8>,
}

impl UniformArena {
    pub(crate) fn new(context: &GpuContext, label: &'static str, slot_bytes: u64) -> Self {
        let alignment = u64::from(
            context
                .device()
                .limits()
                .min_uniform_buffer_offset_alignment
                .max(1),
        );
        Self {
            label,
            slot_bytes,
            stride: slot_bytes.div_ceil(alignment) * alignment,
            buffer: None,
            bind: None,
            slots: 0,
            staging: Vec::new(),
        }
    }

    /// 把本帧所有槽写进缓冲，返回"这一帧因此新建了几张缓冲"。
    ///
    /// 尺寸不变的第二帧起返回 0：只有角色数涨过当前容量才扩容重建。
    pub(crate) fn upload(
        &mut self,
        context: &GpuContext,
        layout: &wgpu::BindGroupLayout,
        values: &[&[u8]],
    ) -> usize {
        assert!(!values.is_empty(), "空竞技场上传没有意义");
        let mut created = 0;
        let needed = u32::try_from(values.len()).expect("槽数超出 u32");
        if self.slots < needed {
            // 2 的幂扩容：角色数逐帧波动时不必每帧重建（放大到能装下全部槽）。
            let slots = needed.next_power_of_two();
            let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: u64::from(slots) * self.stride,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let size = wgpu::BufferSize::new(self.slot_bytes).expect("槽大小必须非零");
            let bind = context
                .device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(self.label),
                    layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buffer,
                            offset: 0,
                            size: Some(size),
                        }),
                    }],
                });
            self.buffer = Some(buffer);
            self.bind = Some(bind);
            self.slots = slots;
            created = 1;
        }
        // 槽距可能大于槽字节数：补位字节一律写 0，不给着色器读到上一帧的残留
        // （暂存区复用，所以这里是"清空 + 重新填"而不是"只覆盖有效字节"）。
        let stride = usize::try_from(self.stride).expect("槽距超出 usize");
        let total = values.len() * stride;
        self.staging.clear();
        self.staging.resize(total, 0);
        for (slot, value) in values.iter().enumerate() {
            assert!(
                value.len() as u64 <= self.slot_bytes,
                "槽数据比 layout 声明的绑定还大：{} > {}",
                value.len(),
                self.slot_bytes
            );
            let start = slot * stride;
            self.staging[start..start + value.len()].copy_from_slice(value);
        }
        let buffer = self.buffer.as_ref().expect("缓冲必须已建好");
        context.queue().write_buffer(buffer, 0, &self.staging);
        created
    }

    /// 上传过之后才有绑定组可用。
    pub(crate) fn bind(&self) -> &wgpu::BindGroup {
        self.bind.as_ref().expect("竞技场尚未上传")
    }

    /// 第 `slot` 个槽在缓冲里的字节偏移（dynamic offset 必须是设备对齐的整数倍）。
    pub(crate) fn offset(&self, slot: usize) -> u32 {
        u32::try_from(slot as u64 * self.stride).expect("槽偏移超出 u32")
    }
}

#[cfg(test)]
mod tests {
    use super::UniformArena;
    use crate::gpu::GpuContext;

    fn layout(context: &GpuContext, slot_bytes: u64) -> wgpu::BindGroupLayout {
        context
            .device()
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("arena-test"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(slot_bytes),
                    },
                    count: None,
                }],
            })
    }

    /// 槽距必须按设备对齐补齐，偏移也得是它的整数倍 —— 否则建绑定组时才报错。
    #[test]
    fn stride_follows_the_device_alignment() {
        let Ok(context) = GpuContext::headless() else {
            eprintln!("skip: 没有可用 GPU 适配器");
            return;
        };
        let alignment = u64::from(
            context
                .device()
                .limits()
                .min_uniform_buffer_offset_alignment
                .max(1),
        );
        let palette = UniformArena::new(&context, "arena-palette", 16 * 1024);
        let model = UniformArena::new(&context, "arena-model", 96);
        assert!(
            palette.offset(1) as u64 % alignment == 0,
            "palette 槽偏移必须对齐：{}",
            palette.offset(1)
        );
        assert!(
            model.offset(1) as u64 % alignment == 0,
            "model 槽偏移必须对齐：{}",
            model.offset(1)
        );
        // 16 KiB 本身是对齐的整数倍，所以 palettes 不该被撑大。
        assert!(
            palette.offset(1) == 16 * 1024,
            "16 KiB 槽不该被撑大：{}",
            palette.offset(1)
        );
        assert!(
            model.offset(1) >= 96,
            "96 B 槽至少要放下 96 B：{}",
            model.offset(1)
        );
    }

    /// 同尺寸的第二帧不该再建缓冲；角色数涨过容量才扩容，且扩容后旧槽偏移不变。
    #[test]
    fn upload_reuses_the_buffer_until_capacity_grows() {
        let Ok(context) = GpuContext::headless() else {
            eprintln!("skip: 没有可用 GPU 适配器");
            return;
        };
        let layout = layout(&context, 96);
        let mut arena = UniformArena::new(&context, "arena-reuse", 96);
        let first: [u8; 96] = [1; 96];
        let second: [u8; 96] = [2; 96];
        assert!(
            arena.upload(&context, &layout, &[&first]) == 1,
            "首次上传要建一张缓冲"
        );
        assert!(
            arena.upload(&context, &layout, &[&first]) == 0,
            "同尺寸的第二帧不该再建缓冲"
        );
        assert!(
            arena.upload(&context, &layout, &[&first, &second]) == 1,
            "两个槽装不下当前容量时要扩容一次"
        );
        assert!(
            arena.upload(&context, &layout, &[&first, &second]) == 0,
            "扩过之后同尺寸又该复用"
        );
        assert!(
            arena.offset(0) == 0 && arena.offset(2) == arena.offset(1) * 2,
            "槽距必须均匀：{} / {} / {}",
            arena.offset(0),
            arena.offset(1),
            arena.offset(2)
        );
    }
}
