use anyhow::{Context, Result};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use wgpu::util::DeviceExt;

use crate::texture::Texture;

/// 材质槽位的全局自增 ID。
///
/// 渲染管线用它缓存「材质绑定组」——同一个图元在每一帧之间复用同一个
/// bind group，而不是每帧重建。ID 全局递增，所以不同模型里即使材质参数
/// 完全一样也不会串台；代价是重复纹理各绑一份采样器，可接受。
static NEXT_MATERIAL_ID: AtomicU64 = AtomicU64::new(1);

fn next_material_id() -> u64 {
    NEXT_MATERIAL_ID.fetch_add(1, Ordering::Relaxed)
}

/// 从 glTF 材质里提取的、渲染需要的参数。
///
/// 只覆盖 metallic-roughness 工作流里最关键的因子。normal / occlusion /
/// emissive 贴图和 KHR 扩展暂时没接——那属于完整材质系统，等有了
/// 材质资源再铺。
#[derive(Clone, Copy, Debug)]
pub struct MaterialParams {
    /// 基础色因子（线性 RGBA），会与 base color 贴图逐通道相乘。
    pub base_color: [f32; 4],
    /// 金属度，0 = 绝缘体，1 = 金属。
    pub metallic: f32,
    /// 粗糙度，0 = 镜面，1 = 完全漫反射。
    pub roughness: f32,
    /// 是否参与半透明混合（glTF `alphaMode == BLEND`）。
    pub blend: bool,
}

impl Default for MaterialParams {
    fn default() -> Self {
        Self {
            base_color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.45,
            blend: false,
        }
    }
}

/// 一次 glTF 网格图元的绘制调用。
pub struct GltfPrimitive {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub num_indices: u32,
    pub index_format: wgpu::IndexFormat,
    pub num_vertices: u32,
    /// 漫反射纹理（模型无纹理时使用白色回退）。
    pub texture: Texture,
    /// 该图元的材质因子。
    pub material: MaterialParams,
    /// 材质槽位 ID，渲染管线据此缓存绑定组。
    pub material_id: u64,
}

/// 从 glTF 文件加载的 GPU 驻留图元集合。
pub struct GltfMesh {
    pub primitives: Vec<GltfPrimitive>,
    /// 顶点步长（字节），所有图元统一。
    pub vertex_size: u64,
}

// ---------------------------------------------------------------------------
// 主机端顶点（位置 + 法线 + 颜色 + 纹理坐标）
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GltfVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    pub tex_coord: [f32; 2],
}

impl GltfVertex {
    pub const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x3, // position
        1 => Float32x3, // normal
        2 => Float32x3, // colour
        3 => Float32x2, // tex_coord
    ];

    pub const STRIDE: wgpu::BufferAddress = std::mem::size_of::<Self>() as wgpu::BufferAddress;

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: Self::STRIDE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: Self::ATTRIBUTES,
        }
    }
}

// ---------------------------------------------------------------------------
// 加载器
// ---------------------------------------------------------------------------

impl GltfMesh {
    /// 加载 `.gltf` 或 `.glb` 文件并将所有网格图元上传到 GPU。
    ///
    /// 缺失的顶点属性用合理默认值填充。
    pub fn from_file(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        let (document, buffers, images) =
            gltf::import(path.as_ref()).context("Failed to parse glTF file")?;

        let mut primitives = Vec::new();

        for mesh in document.meshes() {
            for primitive in mesh.primitives() {
                let reader = primitive.reader(|buf| Some(&buffers[buf.index()]));

                // --- 位置（必需）---
                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .context("Missing POSITION attribute")?
                    .collect();
                let vertex_count = positions.len();

                // --- 索引 ---
                let indices: Vec<u32> = reader
                    .read_indices()
                    .map(|iter| iter.into_u32().collect())
                    .unwrap_or_else(|| (0..vertex_count as u32).collect());

                // --- 法线 ---
                let normals: Vec<[f32; 3]> = match reader.read_normals() {
                    Some(iter) => iter.collect(),
                    None => {
                        log::info!("No NORMAL — computing flat normals");
                        compute_face_normals(&positions, &indices)
                    }
                };

                // --- 纹理坐标 ---
                let tex_coords: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|iter| iter.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.0, 0.0]; vertex_count]);

                // --- 顶点颜色 ---
                let colors: Vec<[f32; 3]> = reader
                    .read_colors(0)
                    .map(|iter| iter.into_rgb_f32().collect())
                    .unwrap_or_else(|| vec![[1.0, 1.0, 1.0]; vertex_count]);

                // --- 纹理 ---
                let texture = load_primitive_texture(device, queue, &primitive, &images);

                // --- 材质因子 ---
                let material = read_material(&primitive);
                let material_id = next_material_id();

                // 交错排列为 GltfVertex 数组
                let vertices: Vec<GltfVertex> = (0..vertex_count)
                    .map(|i| GltfVertex {
                        position: positions[i],
                        normal: normals[i],
                        color: colors[i],
                        tex_coord: tex_coords[i],
                    })
                    .collect();

                let vertex_bytes: &[u8] = bytemuck::cast_slice(&vertices);

                let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("glTF vertex buffer"),
                    contents: vertex_bytes,
                    usage: wgpu::BufferUsages::VERTEX,
                });

                let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("glTF index buffer"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

                primitives.push(GltfPrimitive {
                    vertex_buffer,
                    index_buffer,
                    num_indices: indices.len() as u32,
                    index_format: wgpu::IndexFormat::Uint32,
                    num_vertices: vertex_count as u32,
                    texture,
                    material,
                    material_id,
                });
            }
        }

        Ok(Self {
            primitives,
            vertex_size: GltfVertex::STRIDE,
        })
    }

    /// 构建单位立方体作为 glTF 加载失败时的回退。
    pub fn fallback_cube(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let (verts, _vtx_size, indices) = crate::mesh::create_coloured_cube();

        // 旧格式：[pos×3, col×3] = 24 字节
        // 新格式：[pos, normal, col, tex_coord] = 44 字节
        let default_normal = f32::to_le_bytes(0.0)
            .into_iter()
            .chain(f32::to_le_bytes(0.0))
            .chain(f32::to_le_bytes(1.0))
            .collect::<Vec<u8>>();
        let default_tex = f32::to_le_bytes(0.0)
            .into_iter()
            .chain(f32::to_le_bytes(0.0))
            .collect::<Vec<u8>>();

        let old_vertex_count = verts.len() / 24;
        let mut new_verts: Vec<u8> = Vec::with_capacity(old_vertex_count * 44);

        for i in 0..old_vertex_count {
            let base = i * 24;
            new_verts.extend_from_slice(&verts[base..base + 12]); // position
            new_verts.extend_from_slice(&default_normal); // normal
            new_verts.extend_from_slice(&verts[base + 12..base + 24]); // colour
            new_verts.extend_from_slice(&default_tex); // tex_coord
        }

        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fallback cube VB"),
            contents: &new_verts,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fallback cube IB"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        GltfMesh {
            primitives: vec![GltfPrimitive {
                vertex_buffer: vb,
                index_buffer: ib,
                num_indices: indices.len() as u32,
                index_format: wgpu::IndexFormat::Uint32,
                num_vertices: old_vertex_count as u32,
                texture: Texture::white(device, queue),
                material: MaterialParams::default(),
                material_id: next_material_id(),
            }],
            vertex_size: GltfVertex::STRIDE,
        }
    }

    /// 从 CPU 端网格数据构造 GPU 网格（内置基础图元的入口）。
    pub fn from_mesh_data(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &crate::mesh::MeshData,
        label: &str,
    ) -> Self {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(&data.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(&data.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            primitives: vec![GltfPrimitive {
                vertex_buffer,
                index_buffer,
                num_indices: data.indices.len() as u32,
                index_format: wgpu::IndexFormat::Uint32,
                num_vertices: data.vertices.len() as u32,
                texture: Texture::white(device, queue),
                material: MaterialParams::default(),
                material_id: next_material_id(),
            }],
            vertex_size: GltfVertex::STRIDE,
        }
    }

    /// 加载一个模型标识：内置图元（`builtin://...`）或磁盘上的 glTF 文件。
    ///
    /// 编辑器与游戏共用的**唯一**模型加载入口。"内置还是文件"的判断收在
    /// 这一处，调用方不必各写一遍 if；将来增加内置图元也只需要改 [`BuiltinMesh`]。
    ///
    /// [`BuiltinMesh`]: crate::mesh::BuiltinMesh
    pub fn load(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        project_root: &Path,
        model: &str,
    ) -> Result<Self> {
        if let Some(builtin) = crate::mesh::BuiltinMesh::from_model_path(model) {
            return Ok(Self::from_mesh_data(device, queue, &builtin.build(), builtin.label()));
        }
        Self::from_file(device, queue, project_root.join(model))
    }
}

// ---------------------------------------------------------------------------
// 从 glTF 图像加载纹理
// ---------------------------------------------------------------------------

/// 读取图元的 PBR 材质因子。
///
/// `primitive.material().index() == None` 表示文件里**根本没写材质**——
/// 这时 glTF 规范给的默认值是 metallic=1 / roughness=1，也就是"全金属全粗糙"，
/// 渲出来又黑又平。示例模型大多是这种情况，所以这里换成一套看起来正常的
/// 默认（绝缘体 + 中等粗糙），同时不改动模型的原始颜色。
fn read_material(primitive: &gltf::Primitive<'_>) -> MaterialParams {
    let material = primitive.material();
    let pbr = material.pbr_metallic_roughness();
    let blend = matches!(material.alpha_mode(), gltf::material::AlphaMode::Blend);

    if material.index().is_none() {
        return MaterialParams {
            blend,
            ..MaterialParams::default()
        };
    }

    MaterialParams {
        base_color: pbr.base_color_factor(),
        metallic: pbr.metallic_factor(),
        roughness: pbr.roughness_factor(),
        blend,
    }
}

fn load_primitive_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    primitive: &gltf::Primitive<'_>,
    images: &[gltf::image::Data],
) -> Texture {
    // Try to get the base colour texture from the PBR material
    let base_color_texture = primitive
        .material()
        .pbr_metallic_roughness()
        .base_color_texture();

    if let Some(info) = base_color_texture {
        let gltf_texture = info.texture();
        let source = gltf_texture.source();
        let idx = source.index();

        if let Some(data) = images.get(idx) {
            if let Some(rgba) = decode_image_data(data) {
                return Texture::from_rgba8(
                    device, queue,
                    data.width, data.height,
                    &rgba, "glTF diffuse",
                );
            }
        }
    }

    // 回退：1×1 白色
    Texture::white(device, queue)
}

/// 将 glTF 图像数据解码为 RGBA8 字节。
fn decode_image_data(data: &gltf::image::Data) -> Option<Vec<u8>> {
    use gltf::image::Format;
    match data.format {
        Format::R8G8B8 => {
            // 将 RGB 扩展为 RGBA
            let mut rgba = Vec::with_capacity((data.width * data.height * 4) as usize);
            for chunk in data.pixels.chunks(3) {
                rgba.extend_from_slice(chunk);
                rgba.push(255);
            }
            Some(rgba)
        }
        Format::R8G8B8A8 => Some(data.pixels.clone()),
        _ => {
            log::warn!("Unsupported glTF image format: {:?}", data.format);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// 面法线计算
// ---------------------------------------------------------------------------

fn compute_face_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let vertex_count = positions.len();
    let mut normals = vec![[0.0f32; 3]; vertex_count];

    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            break;
        }
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);

        let ax = positions[i1][0] - positions[i0][0];
        let ay = positions[i1][1] - positions[i0][1];
        let az = positions[i1][2] - positions[i0][2];
        let bx = positions[i2][0] - positions[i0][0];
        let by = positions[i2][1] - positions[i0][1];
        let bz = positions[i2][2] - positions[i0][2];

        let nx = ay * bz - az * by;
        let ny = az * bx - ax * bz;
        let nz = ax * by - ay * bx;

        normals[i0][0] += nx; normals[i0][1] += ny; normals[i0][2] += nz;
        normals[i1][0] += nx; normals[i1][1] += ny; normals[i1][2] += nz;
        normals[i2][0] += nx; normals[i2][1] += ny; normals[i2][2] += nz;
    }

    for n in &mut normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n[0] /= len; n[1] /= len; n[2] /= len;
        } else {
            *n = [0.0, 0.0, 1.0];
        }
    }

    normals
}
