//! 方向光阴影的 GPU 侧：2048² 深度数组（每层一个级联）+ 深度 pass 管线。
//!
//! CPU 侧的级联拟合在 `shadow_csm`（纯函数、可单测）；这里只负责资源与绘制。

use super::draw::DrawItem;
use super::gpu::GpuContext;

pub use super::shadow_csm::{
    cascade_shadow_matrices, fit_directional_shadow, ShadowCascades, SHADOW_CASCADES,
    SHADOW_DEPTH_BIAS, SHADOW_RESOLUTION,
};

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

/// 阴影 pass 与旧引擎同款：constant 2 / slope_scale 2 抵消自阴影条纹。
/// 静态与 GPU 蒙皮投射体共用这一份，否则同一表面的条纹会分成两套。
pub(super) const CASTER_DEPTH_BIAS: wgpu::DepthBiasState = wgpu::DepthBiasState {
    constant: 2,
    slope_scale: 2.0,
    clamp: 0.0,
};

/// 阴影图 + 比较采样器 + 深度 pass 管线（设备生命周期内复用）。
pub(crate) struct ShadowTarget {
    map_view: wgpu::TextureView,
    /// 每级一个 layer 视图：深度数组的每一层单独开一次 pass。
    layer_views: [wgpu::TextureView; SHADOW_CASCADES],
    /// 没有阴影时绑定的 1×1 假图：主 pass 的绑定组布局是固定的，不能"这次不绑"。
    fallback_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    /// GPU palette 变形的投射体（与静态那支共用同一批级联 pass）。
    skin: super::shadow_skin::SkinnedShadow,
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
                depth_or_array_layers: SHADOW_CASCADES as u32,
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
                bias: CASTER_DEPTH_BIAS,
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview: None,
            cache: None,
        });
        let array_desc = wgpu::TextureViewDescriptor {
            label: Some("xgc-shadow-array"),
            format: Some(wgpu::TextureFormat::Depth32Float),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            aspect: wgpu::TextureAspect::DepthOnly,
            base_mip_level: 0,
            mip_level_count: Some(1),
            base_array_layer: 0,
            array_layer_count: Some(SHADOW_CASCADES as u32),
        };
        let map_view = map.create_view(&array_desc);
        let mut layer_views = Vec::with_capacity(SHADOW_CASCADES);
        for layer in 0..SHADOW_CASCADES as u32 {
            layer_views.push(map.create_view(&wgpu::TextureViewDescriptor {
                label: Some("xgc-shadow-layer"),
                format: Some(wgpu::TextureFormat::Depth32Float),
                dimension: Some(wgpu::TextureViewDimension::D2),
                aspect: wgpu::TextureAspect::DepthOnly,
                base_mip_level: 0,
                mip_level_count: Some(1),
                base_array_layer: layer,
                array_layer_count: Some(1),
            }));
        }
        let layer_views: [wgpu::TextureView; SHADOW_CASCADES] =
            layer_views.try_into().expect("层数固定");
        let skin = super::shadow_skin::SkinnedShadow::new(context, &frame_layout);
        Self {
            map_view,
            layer_views,
            fallback_view: fallback.create_view(&wgpu::TextureViewDescriptor {
                label: Some("xgc-shadow-fallback-array"),
                format: Some(wgpu::TextureFormat::Depth32Float),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                aspect: wgpu::TextureAspect::DepthOnly,
                base_mip_level: 0,
                mip_level_count: Some(1),
                base_array_layer: 0,
                array_layer_count: Some(1),
            }),
            sampler,
            pipeline,
            skin,
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

    /// 把 draw list 画进深度数组：每级一个 pass（每层各自清一次、各用一份光视图投影）。
    /// 静态网格与蒙皮网格都要投射；蒙皮那支走 `shadow_skin` 的 palette 管线。
    pub(crate) fn render(
        &self,
        context: &GpuContext,
        cascades: &ShadowCascades,
        items: &[DrawItem],
        sources: &mut super::shadow_skin::CasterSources<'_>,
    ) -> Result<(), String> {
        if items.is_empty() || cascades.count == 0 {
            return Ok(());
        }
        let casters = super::shadow_skin::prepare(context, items, sources)?;
        if casters.is_empty() {
            return Ok(());
        }
        // 蒙皮绑定每帧只建一次（4 级级联复用），不在 pass 里逐级重建。
        let skin_bindings = self.skin.bindings(context, &casters);
        let skin_meshes = &*sources.skin_meshes;
        let mut bytes = Vec::with_capacity(casters.len() * 64);
        for caster in &casters {
            for value in caster.model {
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

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("xgc-shadow-encoder"),
        });
        for cascade in 0..cascades.count.min(SHADOW_CASCADES) {
            let mut frame_bytes = Vec::with_capacity(64);
            for value in cascades.view_proj[cascade] {
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
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-shadow-pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.layer_views[cascade],
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
            for (index, caster) in casters.iter().enumerate() {
                // GPU 蒙皮投射体由 `shadow_skin` 用 palette 管线画，这里跳过。
                if caster.skins_here() {
                    continue;
                }
                let Some(gpu) = sources.gpu_meshes.get(&caster.mesh_key) else {
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
            self.skin
                .draw(&mut pass, &frame_bind, &skin_bindings, skin_meshes);
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
