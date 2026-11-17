//! 方向光阴影：单级正交拟合 + 深度图 + 接收端 3×3 PCF。
//!
//! 与旧引擎同一条基线（单级 [`fit_directional_shadow`] 就是"关掉级联"的等价退路）：
//! 取**第一盏** active 且 `casts_shadow` 的方向光，按场景包围盒拟合正交视锥，
//! 深度图 2048²（`DEFAULT_SHADOW_RESOLUTION`），阴影 pass 只画深度，接收端用
//! `textureSampleCompareLevel` 做 3×3 PCF，偏置随 `N·L` 变小而放大。
//!
//! 这一刀**不做**：级联（CSM）、PCSS 软阴影、点光立方体阴影、蒙皮投射。

use super::assets::MeshCache;
use super::camera::{look_at_rh, mul, orthographic_rh, Mat4};
use super::draw::DrawItem;
use super::gpu::GpuContext;
use super::gpu_mesh::GpuMeshCache;
use super::shot::Bounds;
use super::texture_bind::TextureBinding;

/// 阴影图边长：与旧引擎 `DEFAULT_SHADOW_RESOLUTION` 一致。
pub const SHADOW_RESOLUTION: u32 = 2048;
/// 接收端深度偏置（NDC 深度单位），旧引擎同名常量。
pub const SHADOW_DEPTH_BIAS: f32 = 0.0022;
/// 拟合半径上限：防止"有物体飞出视野"把深度精度吃光。
const MAX_SHADOW_DISTANCE: f32 = 120.0;

/// 由光线传播方向 + 场景包围盒拟合正交光视图投影；空场景返回 `None`。
///
/// 与旧引擎逐行同式：光源在场景中心**逆着光**退开 `2r+1`，正交半宽 `r × 1.02`，
/// near / far 罩住整球。
pub fn fit_directional_shadow(direction: [f32; 3], bounds: &Bounds) -> Option<Mat4> {
    if bounds.is_empty() {
        return None;
    }
    let length =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    let dir = if length > 1e-6 {
        [
            direction[0] / length,
            direction[1] / length,
            direction[2] / length,
        ]
    } else {
        [0.0, -1.0, 0.0]
    };
    let center = bounds.center();
    let radius = bounds.radius().clamp(0.5, MAX_SHADOW_DISTANCE);
    let up = if dir[1].abs() > 0.99 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let distance = radius * 2.0 + 1.0;
    let eye = [
        center[0] - dir[0] * distance,
        center[1] - dir[1] * distance,
        center[2] - dir[2] * distance,
    ];
    let view = look_at_rh(eye, center, up);
    let extent = radius * 1.02;
    let near = (distance - radius - 1.0).max(0.01);
    let far = distance + radius + 1.0;
    let projection = orthographic_rh(-extent, extent, -extent, extent, near, far);
    Some(mul(projection, view))
}

const SHADOW_WGSL: &str = r#"
struct ShadowFrame {
    light_view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> frame: ShadowFrame;

struct Instance {
    model: mat4x4<f32>,
};

@group(1) @binding(0)
var<storage, read> instances: array<Instance>;

@vertex
fn vs(@location(0) position: vec3<f32>, @builtin(instance_index) index: u32) -> @builtin(position) vec4<f32> {
    let world = instances[index].model * vec4<f32>(position, 1.0);
    return frame.light_view_proj * world;
}
"#;

/// 阴影图 + 比较采样器 + 深度 pass 管线（设备生命周期内复用）。
pub(crate) struct ShadowTarget {
    map_view: wgpu::TextureView,
    /// 没有阴影时绑定的 1×1 假图：主 pass 的绑定组布局是固定的，不能"这次不绑"。
    fallback_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    frame_layout: wgpu::BindGroupLayout,
    instance_layout: wgpu::BindGroupLayout,
}

impl ShadowTarget {
    pub(crate) fn new(context: &GpuContext) -> Self {
        let device = context.device();
        let map = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xgc-shadow-map"),
            size: wgpu::Extent3d {
                width: SHADOW_RESOLUTION,
                height: SHADOW_RESOLUTION,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let fallback = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xgc-shadow-fallback"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        // 比较采样器：硬件替我们做"片元深度 vs 存图深度"的比较。
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xgc-shadow-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            compare: Some(wgpu::CompareFunction::Less),
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xgc-shadow"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_WGSL.into()),
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xgc-shadow-frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let instance_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xgc-shadow-instances"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xgc-shadow-layout"),
            bind_group_layouts: &[&frame_layout, &instance_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("xgc-shadow-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                // 与旧引擎阴影 pass 同款：constant 2 / slope_scale 2 抵消自阴影条纹。
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview: None,
            cache: None,
        });
        Self {
            map_view: map.create_view(&wgpu::TextureViewDescriptor::default()),
            fallback_view: fallback.create_view(&wgpu::TextureViewDescriptor::default()),
            sampler,
            pipeline,
            frame_layout,
            instance_layout,
        }
    }

    /// 主 pass 采样用：`(深度图视图, 比较采样器)`；没有阴影时用 1×1 假图。
    pub(crate) fn sample_source(&self, enabled: bool) -> (&wgpu::TextureView, &wgpu::Sampler) {
        let view = if enabled {
            &self.map_view
        } else {
            &self.fallback_view
        };
        (view, &self.sampler)
    }

    /// 把**静态** draw 画进深度图（蒙皮投射后续再补；GPU 蒙皮路径这里跳过）。
    pub(crate) fn render(
        &self,
        context: &GpuContext,
        light_view_proj: Mat4,
        items: &[DrawItem],
        gpu_meshes: &mut GpuMeshCache,
        meshes: &mut MeshCache,
        textures: &TextureBinding,
    ) -> Result<(), String> {
        if items.is_empty() {
            return Ok(());
        }
        let mut bytes = Vec::with_capacity(items.len() * 64);
        for item in items {
            for value in item.transform {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let device = context.device();
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-shadow-instances"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&instances, 0, &bytes);
        let instance_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xgc-shadow-instances-bind"),
            layout: &self.instance_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: instances.as_entire_binding(),
            }],
        });
        let mut frame_bytes = Vec::with_capacity(64);
        for value in light_view_proj {
            frame_bytes.extend_from_slice(&value.to_le_bytes());
        }
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-shadow-frame-buffer"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&frame, 0, &frame_bytes);
        let frame_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xgc-shadow-frame-bind"),
            layout: &self.frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            }],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("xgc-shadow-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-shadow-pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.map_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &frame_bind, &[]);
            pass.set_bind_group(1, &instance_bind, &[]);
            for (index, item) in items.iter().enumerate() {
                let Some(mesh) = meshes.get(&item.model).map_err(|err| err.to_string())? else {
                    continue;
                };
                if mesh.positions.is_empty() {
                    continue;
                }
                gpu_meshes.upload(context, &item.model, mesh, textures);
                let Some(gpu) = gpu_meshes.get(&item.model) else {
                    continue;
                };
                let slot = index as u32;
                pass.set_vertex_buffer(0, gpu.vertex_buffer.slice(..));
                if let Some(indices) = &gpu.index_buffer {
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..gpu.indices, 0, slot..slot + 1);
                } else {
                    pass.draw(0..gpu.vertices, slot..slot + 1);
                }
            }
        }
        context.queue().submit(Some(encoder.finish()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::fit_directional_shadow;
    use crate::shot::Bounds;
    use crate::transform_point;

    fn project(matrix: crate::Mat4, point: [f32; 3]) -> [f32; 3] {
        let [x, y, z, w] = transform_point(matrix, [point[0], point[1], point[2], 1.0]);
        [x / w, y / w, z / w]
    }

    #[test]
    fn empty_bounds_have_no_shadow_fit() {
        assert!(fit_directional_shadow([0.0, -1.0, 0.0], &Bounds::EMPTY).is_none());
    }

    #[test]
    fn fit_covers_the_whole_scene() {
        let bounds = Bounds {
            min: [-12.0, -1.0, -12.0],
            max: [12.0, 9.0, 12.0],
        };
        let matrix = fit_directional_shadow([0.45, -1.0, 0.35], &bounds).unwrap();
        for corner in bounds.corners() {
            let [x, y, z] = project(matrix, corner);
            assert!(
                x.abs() <= 1.0 && y.abs() <= 1.0 && (0.0..=1.0).contains(&z),
                "角点 {corner:?} 落在阴影视锥外：{x} {y} {z}"
            );
        }
    }

    #[test]
    fn shadow_depth_orders_by_distance_to_the_light() {
        let bounds = Bounds {
            min: [-5.0, -5.0, -5.0],
            max: [5.0, 5.0, 5.0],
        };
        let matrix = fit_directional_shadow([0.0, -1.0, 0.0], &bounds).unwrap();
        let top = project(matrix, [0.0, 5.0, 0.0])[2];
        let bottom = project(matrix, [0.0, -5.0, 0.0])[2];
        assert!(
            top < bottom,
            "离光源更近的点深度必须更小：{top} vs {bottom}"
        );
    }

    #[test]
    fn zero_direction_degrades_to_straight_down() {
        let bounds = Bounds {
            min: [-1.0; 3],
            max: [1.0; 3],
        };
        let matrix = fit_directional_shadow([0.0, 0.0, 0.0], &bounds).unwrap();
        let top = project(matrix, [0.0, 1.0, 0.0])[2];
        let bottom = project(matrix, [0.0, -1.0, 0.0])[2];
        assert!(top < bottom, "零方向退化成垂直向下：{top} vs {bottom}");
    }
}
