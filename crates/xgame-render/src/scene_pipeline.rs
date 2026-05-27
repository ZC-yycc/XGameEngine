//! 共享 3D 场景渲染管线。
//!
//! 封装 wgpu 渲染管线、着色器、uniform 缓冲区和逐实体绘制逻辑。
//! 可在编辑器离屏渲染器和独立游戏二进制中复用。

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use xgame_math::glam::{Mat4, Vec3};

use xgame_core::Entity;
use crate::gltf_loader::{GltfMesh, GltfVertex};

/// 每帧最多渲染的实体数量。
const MAX_RENDER_ENTITIES: usize = 256;

// ---------------------------------------------------------------------------
// GPU uniform 类型（必须与 WGSL 布局逐字节对齐）
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct LightUniform {
    direction: [f32; 4],
    color: [f32; 4],
    ambient: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct ModelUniform {
    model: [[f32; 4]; 4],
}

// ---------------------------------------------------------------------------
// 内联 WGSL 着色器
// ---------------------------------------------------------------------------

const SCENE_VERT_SRC: &str = r#"
struct CameraUniform { view_proj: mat4x4<f32>, camera_pos: vec4<f32> }
struct LightUniform  { direction: vec4<f32>, color: vec4<f32>, ambient: vec4<f32> }
struct ModelUniform  { model: mat4x4<f32> }

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(0) @binding(1) var<uniform> light: LightUniform;
@group(0) @binding(2) var<uniform> model: ModelUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) tex_coord: vec2<f32>,
}
struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) color: vec3<f32>,
}

fn mat3_inverse(m: mat3x3<f32>) -> mat3x3<f32> {
    let a = m[0][0]; let b = m[0][1]; let c = m[0][2];
    let d = m[1][0]; let e = m[1][1]; let f = m[1][2];
    let g = m[2][0]; let h = m[2][1]; let i = m[2][2];
    let det = a * (e*i - f*h) - b * (d*i - f*g) + c * (d*h - e*g);
    let inv_det = 1.0 / det;
    return mat3x3<f32>(
        (e*i - f*h)*inv_det, (c*h - b*i)*inv_det, (b*f - c*e)*inv_det,
        (f*g - d*i)*inv_det, (a*i - c*g)*inv_det, (c*d - a*f)*inv_det,
        (d*h - e*g)*inv_det, (b*g - a*h)*inv_det, (a*e - b*d)*inv_det,
    );
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let world_pos = model.model * vec4<f32>(input.position, 1.0);
    let m = model.model;
    let m33 = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);
    let normal_mat = transpose(mat3_inverse(m33));
    var out: VertexOutput;
    out.clip_pos = camera.view_proj * world_pos;
    out.world_normal = normal_mat * input.normal;
    out.world_pos = world_pos.xyz;
    out.color = input.color;
    return out;
}
"#;

const SCENE_FRAG_SRC: &str = r#"
@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(0) @binding(1) var<uniform> light: LightUniform;

struct CameraUniform { view_proj: mat4x4<f32>, camera_pos: vec4<f32> }
struct LightUniform  { direction: vec4<f32>, color: vec4<f32>, ambient: vec4<f32> }
struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) color: vec3<f32>,
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let N = normalize(input.world_normal);
    let L = normalize(light.direction.xyz);
    let lit = light.ambient.xyz + max(dot(N, -L), 0.0) * light.color.xyz;
    return vec4<f32>(input.color * lit, 1.0);
}
"#;

// ---------------------------------------------------------------------------
// ScenePipeline
// ---------------------------------------------------------------------------

/// 共享 3D 场景渲染管线。
///
/// 创建一次，每帧调用 [`render`]。不持有离屏纹理或交换链——由调用者传入。
pub struct ScenePipeline {
    pipeline: wgpu::RenderPipeline,
    _bind_group_layout: wgpu::BindGroupLayout,
    camera_buffer: wgpu::Buffer,
    _light_buffer: wgpu::Buffer,
    model_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    uniform_alignment: u64,
}

impl ScenePipeline {
    /// 创建渲染管线、绑定组和 uniform 缓冲。
    ///
    /// `format` 是颜色附件的纹理格式（编辑器离屏或交换链）。
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_camera_ub"),
            contents: bytemuck::cast_slice(&[CameraUniform {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos: [0.0; 4],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_light_ub"),
            contents: bytemuck::cast_slice(&[LightUniform {
                direction: [-0.5, -1.0, -0.5, 0.0],
                color: [1.0, 0.95, 0.85, 2.0],
                ambient: [0.08, 0.08, 0.10, 0.0],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let model_buffer_size = MAX_RENDER_ENTITIES as u64 * uniform_alignment;
        let model_buffer_init = vec![0u8; model_buffer_size as usize];
        let model_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_model_ub"),
            contents: &model_buffer_init,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: light_buffer.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &model_buffer,
                        offset: 0,
                        size: Some(
                            std::num::NonZeroU64::new(
                                std::mem::size_of::<ModelUniform>() as u64,
                            )
                            .unwrap(),
                        ),
                    }),
                },
            ],
        });

        let pipeline = Self::build_pipeline(device, &bgl, format);

        Self {
            pipeline,
            _bind_group_layout: bgl,
            camera_buffer,
            _light_buffer: light_buffer,
            model_buffer,
            bind_group,
            uniform_alignment,
        }
    }

    fn build_pipeline(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_vs"),
            source: wgpu::ShaderSource::Wgsl(SCENE_VERT_SRC.into()),
        });
        let fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_fs"),
            source: wgpu::ShaderSource::Wgsl(SCENE_FRAG_SRC.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_pl"),
            bind_group_layouts: &[bgl],
            push_constant_ranges: &[],
        });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs,
                entry_point: Some("vs_main"),
                buffers: &[GltfVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        })
    }

    /// 执行一帧场景渲染。
    ///
    /// 写入 camera/model uniform，对每个有模型的实体发出绘制调用。
    /// 调用者需提前用 `queue.submit()` 提交 `encoder`。
    pub fn render(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        entities: &[Entity],
        model_cache: &HashMap<String, GltfMesh>,
        view_proj: &Mat4,
        camera_pos: &Vec3,
    ) {
        queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[CameraUniform {
                view_proj: view_proj.to_cols_array_2d(),
                camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 1.0],
            }]),
        );

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.12, g: 0.12, b: 0.14, a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        rpass.set_pipeline(&self.pipeline);

        for (i, entity) in entities.iter().enumerate() {
            if i >= MAX_RENDER_ENTITIES {
                break;
            }
            if entity.model.is_empty() {
                continue;
            }
            let Some(mesh) = model_cache.get(&entity.model) else {
                continue;
            };

            let model_mat = entity.transform.local_matrix();
            let offset = i as u64 * self.uniform_alignment;
            queue.write_buffer(
                &self.model_buffer,
                offset,
                bytemuck::cast_slice(&[ModelUniform {
                    model: model_mat.to_cols_array_2d(),
                }]),
            );

            rpass.set_bind_group(0, &self.bind_group, &[offset as u32]);

            for prim in &mesh.primitives {
                rpass.set_vertex_buffer(0, prim.vertex_buffer.slice(..));
                rpass.set_index_buffer(prim.index_buffer.slice(..), prim.index_format);
                rpass.draw_indexed(0..prim.num_indices, 0, 0..1);
            }
        }
    }
}
