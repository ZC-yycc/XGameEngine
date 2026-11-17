//! 实例化绘制：按网格分组，一次 draw 覆盖同模型的多个实体。

use std::collections::HashMap;

use super::assets::MeshCache;
use super::camera::Mat4;
use super::draw::DrawList;
use super::gpu::{GpuContext, GpuError};
use super::gpu_mesh::GpuMeshCache;
use super::mesh::Mesh;
use super::pipeline::RenderCache;
use xgc_world::SceneEnvironment;

/// 帧 uniform：view-proj 64B + 四个 vec4（相机/强度、ambient、sun_dir、sun_color）
/// + 9 个 vec4 的天空球谐 + 一个天空开关 vec4 = 288B。
const FRAME_UNIFORM_BYTES: u64 = 288;

/// 帧级光照参数：环境光 + 场景第一盏可用方向光；没有方向光时强度为 0（只剩环境光）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameLighting {
    pub(crate) eye: [f32; 3],
    pub(crate) ambient: [f32; 3],
    pub(crate) sun_direction: [f32; 3],
    pub(crate) sun_intensity: f32,
    pub(crate) sun_color: [f32; 3],
    /// 天空漫反射环境（E/π 口径的 L2 球谐）；`None` = 天空关，只剩 `ambient`。
    pub(crate) sky_irradiance: Option<[[f32; 4]; 9]>,
}

impl FrameLighting {
    pub(crate) fn from_environment(environment: &SceneEnvironment, eye: [f32; 3]) -> Self {
        let sun = environment.sun();
        Self {
            eye,
            ambient: environment.ambient,
            sun_direction: sun
                .map(|light| light.normalized_direction())
                .unwrap_or([0.0, -1.0, 0.0]),
            sun_intensity: sun.map(|light| light.intensity).unwrap_or(0.0),
            sun_color: sun.map(|light| light.color).unwrap_or([1.0; 3]),
            sky_irradiance: None,
        }
    }

    /// 挂上天空环境照明：`None` = 天空关（片元整段跳过 SH 求值）。
    pub(crate) fn with_sky(mut self, sky_irradiance: Option<[[f32; 4]; 9]>) -> Self {
        self.sky_irradiance = sky_irradiance;
        self
    }

    /// 按 WGSL `Frame` 的字段顺序打包；不足 16 字节的部分用 0 补齐。
    fn write(&self, view_proj: Mat4, bytes: &mut Vec<u8>) {
        for value in view_proj {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [self.eye[0], self.eye[1], self.eye[2], self.sun_intensity] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [self.ambient[0], self.ambient[1], self.ambient[2], 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [
            self.sun_direction[0],
            self.sun_direction[1],
            self.sun_direction[2],
            0.0,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [self.sun_color[0], self.sun_color[1], self.sun_color[2], 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let sky = self.sky_irradiance.unwrap_or([[0.0; 4]; 9]);
        for coefficient in sky {
            for value in coefficient {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let enabled: f32 = if self.sky_irradiance.is_some() {
            1.0
        } else {
            0.0
        };
        for value in [enabled, 0.0, 0.0, 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
}

pub struct InstancedPipeline {
    pipeline: wgpu::RenderPipeline,
    camera_layout: wgpu::BindGroupLayout,
    instance_layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
}

impl InstancedPipeline {
    pub(crate) fn raw(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    pub(crate) fn camera_layout(&self) -> &wgpu::BindGroupLayout {
        &self.camera_layout
    }

    pub(crate) fn instance_layout(&self) -> &wgpu::BindGroupLayout {
        &self.instance_layout
    }
}

const INSTANCED_WGSL: &str = r#"
struct Frame {
    view_proj: mat4x4<f32>,
    // xyz = 相机世界坐标（用来把面法线朝向相机），w = 方向光强度
    eye_intensity: vec4<f32>,
    ambient: vec4<f32>,
    // xyz = 光线传播方向（归一化）
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    // 天空漫反射环境（E/π 口径的 L2 球谐），与 sky.rs 的 sh_project 逐项一致
    sky_sh: array<vec4<f32>, 9>,
    // x = 天空是否开启（1 / 0）
    sky_params: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> frame: Frame;

// 色调映射：与 tone.rs 的 Rust 镜像逐式对应（ACES RRT+ODT 拟合，1/0.6 亮度基准确认）。
// 旧引擎在合成 pass 里过这条曲线；这里在片元输出前套同一条，随后由 sRGB 目标编码。
fn aces_input_mat(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.59719 * c.x + 0.35458 * c.y + 0.04823 * c.z,
        0.07600 * c.x + 0.90834 * c.y + 0.01566 * c.z,
        0.02840 * c.x + 0.13383 * c.y + 0.83777 * c.z,
    );
}

fn aces_output_mat(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        1.60475 * c.x - 0.53108 * c.y - 0.07367 * c.z,
        -0.10208 * c.x + 1.10813 * c.y - 0.00605 * c.z,
        -0.00327 * c.x - 0.07276 * c.y + 1.07602 * c.z,
    );
}

fn aces_rrt(c: vec3<f32>) -> vec3<f32> {
    let a = c * (c + vec3<f32>(0.0245786)) - vec3<f32>(0.000090537);
    let b = c * (vec3<f32>(0.983729) * c + vec3<f32>(0.432951)) + vec3<f32>(0.238081);
    return a / b;
}

// E/π 的球谐求值（系数里已折好余弦卷积）；基函数与 sky.rs 的 sh_basis 逐项一致。
fn sky_sh_eval(n: vec3<f32>) -> vec3<f32> {
    return frame.sky_sh[0].rgb * 0.282095
         + frame.sky_sh[1].rgb * (0.488603 * n.y)
         + frame.sky_sh[2].rgb * (0.488603 * n.z)
         + frame.sky_sh[3].rgb * (0.488603 * n.x)
         + frame.sky_sh[4].rgb * (1.092548 * n.x * n.y)
         + frame.sky_sh[5].rgb * (1.092548 * n.y * n.z)
         + frame.sky_sh[6].rgb * (0.315392 * (3.0 * n.z * n.z - 1.0))
         + frame.sky_sh[7].rgb * (1.092548 * n.x * n.z)
         + frame.sky_sh[8].rgb * (0.546274 * (n.x * n.x - n.y * n.y));
}

fn aces(linear: vec3<f32>) -> vec3<f32> {
    var c = max(linear, vec3<f32>(0.0)) / 0.6;
    c = aces_input_mat(c);
    c = aces_rrt(c);
    c = aces_output_mat(c);
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

struct Instance {
    model: mat4x4<f32>,
    color: vec4<f32>,
};

@group(1) @binding(0)
var<storage, read> instances: array<Instance>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) world: vec3<f32>,
};

@vertex
fn vs(@location(0) position: vec3<f32>, @builtin(instance_index) index: u32) -> VsOut {
    let instance = instances[index];
    var out: VsOut;
    let world = instance.model * vec4<f32>(position, 1.0);
    out.position = frame.view_proj * world;
    out.world = world.xyz;
    out.color = instance.color;
    return out;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    // 面法线从屏幕导数叉积得到，符号按"朝向相机"归一，闭合网格可见面才受光。
    let raw = cross(dpdx(input.world), dpdy(input.world));
    var normal = normalize(raw + vec3<f32>(0.0, 0.0, 1e-9));
    let to_camera = normalize(frame.eye_intensity.xyz - input.world);
    if (dot(normal, to_camera) < 0.0) {
        normal = -normal;
    }
    let to_light = -normalize(frame.sun_dir.xyz);
    let diffuse = max(dot(normal, to_light), 0.0);
    // 环境光 + 天空 IBL（两者同标定，直接相加）+ 场景主方向光。
    var light = frame.ambient.rgb;
    if (frame.sky_params.x > 0.5) {
        light = light + sky_sh_eval(normal);
    }
    light = light + frame.sun_color.rgb * (frame.eye_intensity.w * diffuse);
    let lit = input.color.rgb * light;
    return vec4<f32>(aces(lit), input.color.a);
}
"#;

impl GpuContext {
    pub fn create_instanced_pipeline(
        &self,
        format: wgpu::TextureFormat,
        depth: bool,
    ) -> InstancedPipeline {
        let shader = self
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xgc-instanced"),
                source: wgpu::ShaderSource::Wgsl(INSTANCED_WGSL.into()),
            });
        let camera_layout =
            self.device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-instanced-camera"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(FRAME_UNIFORM_BYTES),
                        },
                        count: None,
                    }],
                });
        let instance_layout =
            self.device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xgc-instanced-models"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(80),
                        },
                        count: None,
                    }],
                });
        let pipeline_layout =
            self.device()
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("xgc-instanced-layout"),
                    bind_group_layouts: &[&camera_layout, &instance_layout],
                    push_constant_ranges: &[],
                });
        let pipeline = self
            .device()
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("xgc-instanced-pipeline"),
                layout: Some(&pipeline_layout),
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
                depth_stencil: depth.then_some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            });
        InstancedPipeline {
            pipeline,
            camera_layout,
            instance_layout,
            format,
        }
    }
}

struct Group {
    key: String,
    source: String,
    pose: Option<Vec<[f32; 16]>>,
    instances: Vec<(Mat4, [f32; 4])>,
}

struct Prepared {
    mesh_key: String,
    instance_bind: wgpu::BindGroup,
    count: u32,
}

pub(crate) struct InstancedRequest<'a> {
    pub(crate) texture: &'a wgpu::Texture,
    pub(crate) list: &'a DrawList,
    pub(crate) meshes: &'a mut MeshCache,
    pub(crate) gpu_meshes: &'a mut GpuMeshCache,
    pub(crate) view_proj: Mat4,
    pub(crate) lighting: FrameLighting,
    pub(crate) depth: &'a wgpu::TextureView,
    pub(crate) clear_color: wgpu::Color,
}

impl RenderCache {
    /// 按模型分组实例化绘制；返回实际 draw call 数。
    pub(crate) fn draw_list_instanced(
        &mut self,
        context: &GpuContext,
        request: &mut InstancedRequest<'_>,
    ) -> Result<usize, GpuError> {
        let texture = request.texture;
        let list = request.list;
        let meshes = &mut *request.meshes;
        let gpu_meshes = &mut *request.gpu_meshes;
        let view_proj = request.view_proj;
        let depth = request.depth;
        if list.items.is_empty() {
            context.clear_texture(texture, request.clear_color)?;
            return Ok(0);
        }
        let format = texture.format();
        if self
            .instanced
            .as_ref()
            .is_none_or(|(current, _)| *current != format)
        {
            self.instanced = Some((format, context.create_instanced_pipeline(format, true)));
        }
        let pipeline = &self.instanced.as_ref().unwrap().1;
        if pipeline.format != format {
            return Err(GpuError::Pipeline("实例化管线格式不一致".to_string()));
        }

        let mut groups: HashMap<String, usize> = HashMap::new();
        let mut buckets = Vec::new();
        for item in &list.items {
            let key = if item.pose.is_some() {
                format!("skin:{}:{}", item.entity.raw(), item.model)
            } else {
                item.model.clone()
            };
            let index = *groups.entry(key.clone()).or_insert_with(|| {
                buckets.push(Group {
                    key,
                    source: item.model.clone(),
                    pose: item.pose.clone(),
                    instances: Vec::new(),
                });
                buckets.len() - 1
            });
            buckets[index].instances.push((item.transform, item.color));
        }

        let mut camera_bytes = Vec::with_capacity(FRAME_UNIFORM_BYTES as usize);
        request.lighting.write(view_proj, &mut camera_bytes);
        let camera = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("xgc-instanced-frame-buffer"),
            size: FRAME_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context.queue().write_buffer(&camera, 0, &camera_bytes);
        let camera_bind = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xgc-instanced-camera-bind"),
                layout: pipeline.camera_layout(),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                }],
            });

        let mut prepared = Vec::with_capacity(buckets.len());
        for group in &buckets {
            let loaded = meshes
                .get(&group.source)
                .map_err(|err| GpuError::Mesh(err.to_string()))?;
            let fallback = loaded.is_none().then(Mesh::fullscreen_triangle);
            let mesh = loaded.unwrap_or_else(|| fallback.as_ref().unwrap());
            if mesh.positions.is_empty() {
                continue;
            }
            if let Some(pose) = &group.pose {
                let positions = super::skinning::skin_positions(mesh, pose);
                gpu_meshes.upload_deformed(context, &group.key, &positions, &mesh.indices);
            } else {
                gpu_meshes.upload(context, &group.key, mesh);
            }
            let mut bytes = Vec::with_capacity(group.instances.len() * 80);
            for (model, color) in &group.instances {
                for value in model {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                for value in color {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            let instances = context.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("xgc-instanced-models-buffer"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            context.queue().write_buffer(&instances, 0, &bytes);
            let instance_bind = context
                .device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("xgc-instanced-models-bind"),
                    layout: pipeline.instance_layout(),
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: instances.as_entire_binding(),
                    }],
                });
            prepared.push(Prepared {
                mesh_key: group.key.clone(),
                instance_bind,
                count: group.instances.len() as u32,
            });
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xgc-instanced-encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xgc-instanced-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(request.clear_color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline.raw());
            pass.set_bind_group(0, &camera_bind, &[]);
            for draw in &prepared {
                pass.set_bind_group(1, &draw.instance_bind, &[]);
                let Some(mesh) = gpu_meshes.get(&draw.mesh_key) else {
                    continue;
                };
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                if let Some(indices) = &mesh.index_buffer {
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.indices, 0, 0..draw.count);
                } else {
                    pass.draw(0..mesh.vertices, 0..draw.count);
                }
            }
        }
        context.queue().submit(Some(encoder.finish()));
        Ok(prepared.len())
    }
}
