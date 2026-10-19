use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use wgpu::util::DeviceExt;

use xgame_math::glam::{Mat4, Quat, Vec3};

use crate::animation::{Interpolation, TrackPath};
use crate::texture::Texture;

// ---------------------------------------------------------------------------
// 轴对齐包围盒
// ---------------------------------------------------------------------------

/// 轴对齐包围盒（AABB）。
///
/// 目前唯一的用途是**拟合方向光的阴影正交视锥**——阴影贴图是一张固定分辨率的
/// 深度图，它只覆盖一个有限的立方体区域，这个区域必须刚好罩住整个场景，
/// 否则要么边缘的物体没有阴影，要么精度被稀释成一片噪点。
///
/// 用"空盒"（`min > max`）表示"还没并进任何点"，这样 [`union`](Self::union)
/// 第一句就能自洽，不需要额外的 `Option` 或 `initialized` 标记。
///
/// [`union`]: Self::union
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    /// 空盒。任何点并进来之后才有意义。
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    /// 是否为空盒（没有并进任何点）。
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    /// 由一组点求包围盒。
    pub fn from_points(points: impl IntoIterator<Item = [f32; 3]>) -> Self {
        points.into_iter().fold(Self::EMPTY, |b, p| {
            let v = Vec3::from(p);
            Self {
                min: b.min.min(v),
                max: b.max.max(v),
            }
        })
    }

    /// 求并（两个盒子的最小外壳）。
    pub fn union(&self, other: &Bounds) -> Bounds {
        Bounds {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn half_extent(&self) -> Vec3 {
        (self.max - self.min) * 0.5
    }

    /// 外接球半径：半边长向量的长度。拟合正交视锥时用它一句话定尺寸。
    pub fn radius(&self) -> f32 {
        self.half_extent().length()
    }

    /// 八个角点。变换包围盒只能变换角点再重新求盒（旋转之后无法用对角线直接推）。
    pub fn corners(&self) -> [Vec3; 8] {
        let (a, b) = (self.min, self.max);
        [
            Vec3::new(a.x, a.y, a.z),
            Vec3::new(b.x, a.y, a.z),
            Vec3::new(a.x, b.y, a.z),
            Vec3::new(b.x, b.y, a.z),
            Vec3::new(a.x, a.y, b.z),
            Vec3::new(b.x, a.y, b.z),
            Vec3::new(a.x, b.y, b.z),
            Vec3::new(b.x, b.y, b.z),
        ]
    }

    /// 用一个仿射矩阵（通常是世界矩阵）变换本盒，返回新的轴对齐盒。
    pub fn transformed(&self, m: &Mat4) -> Bounds {
        if self.is_empty() {
            return *self;
        }
        Bounds::from_points(
            self.corners()
                .into_iter()
                .map(|c| (*m * c.extend(1.0)).truncate().to_array()),
        )
    }
}

impl Default for Bounds {
    fn default() -> Self {
        Self::EMPTY
    }
}

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

impl MaterialParams {
    /// 套用逐实体材质覆盖，得到该 draw 的**有效材质**。
    ///
    /// 字段级语义（见 [`xgame_core::MaterialOverride`]）：
    /// - `base_color` **相乘**（tint 乘子）：内置图元原色是白，乘即上色；
    ///   带贴图的 glTF 只被调"整体色调"，贴图细节照常参与。
    ///   透明度（a 分量）一并相乘——"半透明一点"就这么来表达。
    /// - `metallic` / `roughness` **替换**：它们是材料的身份参数，
    ///   "乘以 0.5 的金属度"没有直觉含义。
    /// - `blend` 不参与：透明与否决定走哪条管线（混合开关是管线级的），
    ///   改它等于换材质**行为**，不是调参数——留作后续的独立功能。
    pub fn merged_with(&self, over: Option<&xgame_core::MaterialOverride>) -> Self {
        let Some(o) = over else {
            return *self;
        };
        Self {
            base_color: match o.base_color {
                Some(c) => [
                    self.base_color[0] * c[0],
                    self.base_color[1] * c[1],
                    self.base_color[2] * c[2],
                    self.base_color[3] * c[3],
                ],
                None => self.base_color,
            },
            metallic: o.metallic.unwrap_or(self.metallic),
            roughness: o.roughness.unwrap_or(self.roughness),
            blend: self.blend,
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
    /// 这个图元挂在哪张蒙皮上（[`GltfMesh::skins`] 的下标）。
    ///
    /// `None` = 静态网格，顶点不参与蒙皮。绑定姿态（实体没配动画）时也走
    /// `None` 的等价路径 —— 着色器里 `params.x == 0` 直接用原始顶点。
    pub skin: Option<usize>,
}

/// 从 glTF 文件加载的 GPU 驻留图元集合。
pub struct GltfMesh {
    pub primitives: Vec<GltfPrimitive>,
    /// 顶点步长（字节），所有图元统一。
    pub vertex_size: u64,
    /// 本地空间的包围盒（未施加任何变换）。阴影拟合时用世界矩阵把它变到世界空间。
    pub bounds: Bounds,
    /// glTF 场景节点层级（含骨骼），[`crate::animation::AnimNode`] 同序。
    ///
    /// 空 = 静态模型（内置图元 / 无场景结构的 glTF）。
    pub nodes: Vec<crate::animation::AnimNode>,
    /// 蒙皮列表（关节 + 逆绑定矩阵）。
    pub skins: Vec<crate::animation::Skin>,
    /// 动画剪辑（glTF `animations[]`）。
    pub animations: Vec<crate::animation::AnimClip>,
}

impl GltfMesh {
    /// 按名字找剪辑（[`crate::animation::Animator::clip`] 的查询入口）。
    pub fn clip(&self, name: &str) -> Option<&crate::animation::AnimClip> {
        self.animations.iter().find(|c| c.name == name)
    }

    /// 这个模型有没有可用的蒙皮动画（Inspector 决定要不要显示动画区块）。
    pub fn has_animation(&self) -> bool {
        !self.animations.is_empty() && !self.skins.is_empty()
    }
}

// ---------------------------------------------------------------------------
// 主机端顶点（位置 + 法线 + 颜色 + 纹理坐标 + 蒙皮权重）
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GltfVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    pub tex_coord: [f32; 2],
    /// 蒙皮关节下标（`JOINTS_0`，u16 索引转成 f32 存）。
    ///
    /// 用 f32 而不是 u16x4：wgpu 的整数顶点格式在 WGSL 侧没有稳定的
    /// `vec4<u8>/<u16>` 输入通路，f32 是所有后端都走得通的公分母。
    /// 索引值都很小（< 2^24），f32 表示精确无损。
    pub joints: [f32; 4],
    /// 每个关节的权重（`WEIGHTS_0`）。未蒙皮顶点给 `[1,0,0,0]`（绑定姿态）。
    pub weights: [f32; 4],
}

impl GltfVertex {
    pub const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x3, // position
        1 => Float32x3, // normal
        2 => Float32x3, // colour
        3 => Float32x2, // tex_coord
        4 => Float32x4, // joints（u16→f32）
        5 => Float32x4, // weights
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

        // 骨骼层级 / 蒙皮 / 动画剪辑：纯 CPU 解析（可单测，不碰 GPU）。
        let anim = parse_animation_data(&document, &buffers);

        let mut primitives = Vec::new();
        // 整个网格的本地包围盒：各图元的顶点位置并起来。阴影拟合只关心这个。
        let mut bounds = Bounds::EMPTY;

        for mesh in document.meshes() {
            // 这个 mesh 被哪个节点以哪张蒙皮引用（见 `parse_animation_data`）。
            let mesh_skin = anim.mesh_skins.get(&mesh.index()).copied();
            for primitive in mesh.primitives() {
                let reader = primitive.reader(|buf| Some(&buffers[buf.index()]));

                // --- 位置（必需）---
                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .context("Missing POSITION attribute")?
                    .collect();
                let vertex_count = positions.len();
                bounds = bounds.union(&Bounds::from_points(positions.iter().copied()));

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

                // --- 蒙皮关节与权重 ---
                //
                // 缺属性 / 数量对不上一律退回"第 0 号关节、权重 1"——即绑定姿态。
                // 无蒙皮的图元后面根本不读这两个通道（着色器按 params.x 分支），
                // 但顶点布局是全局统一的，占位值必须填对，否则 stride 会错位。
                let mut joints: Vec<[f32; 4]> = reader
                    .read_joints(0)
                    .map(|iter| {
                        // JOINTS_0 是 u8/u16 两种组件类型，统一转 u16 再落 f32。
                        iter.into_u16()
                            .map(|j| {
                                [
                                    j[0] as f32,
                                    j[1] as f32,
                                    j[2] as f32,
                                    j[3] as f32,
                                ]
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut weights: Vec<[f32; 4]> = reader
                    .read_weights(0)
                    .map(|iter| iter.into_f32().collect())
                    .unwrap_or_default();
                if joints.len() != vertex_count || weights.len() != vertex_count {
                    joints = vec![[0.0; 4]; vertex_count];
                    weights = vec![[1.0, 0.0, 0.0, 0.0]; vertex_count];
                }

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
                        joints: joints[i],
                        weights: weights[i],
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
                    skin: mesh_skin,
                });
            }
        }

        Ok(Self {
            primitives,
            vertex_size: GltfVertex::STRIDE,
            bounds,
            nodes: anim.nodes,
            skins: anim.skins,
            animations: anim.animations,
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
        // 记录布局 = 位置(12) + 法线(12) + 颜色(12) + UV(8) + 关节(16) + 权重(16) = 76。
        let mut new_verts: Vec<u8> = Vec::with_capacity(old_vertex_count * 76);
        let no_joints = [0f32; 4].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        let root_weight = [1f32, 0.0, 0.0, 0.0].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();

        for i in 0..old_vertex_count {
            let base = i * 24;
            new_verts.extend_from_slice(&verts[base..base + 12]); // position
            new_verts.extend_from_slice(&default_normal); // normal
            new_verts.extend_from_slice(&verts[base + 12..base + 24]); // colour
            new_verts.extend_from_slice(&default_tex); // tex_coord
            new_verts.extend_from_slice(&no_joints); // joints（内置图元不蒙皮）
            new_verts.extend_from_slice(&root_weight); // weights
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
                skin: None,
            }],
            vertex_size: GltfVertex::STRIDE,
            // 回退立方体就是边长 1、中心在原点的单位盒。
            bounds: Bounds {
                min: Vec3::splat(-0.5),
                max: Vec3::splat(0.5),
            },
            nodes: Vec::new(),
            skins: Vec::new(),
            animations: Vec::new(),
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
                skin: None,
            }],
            vertex_size: GltfVertex::STRIDE,
            bounds: Bounds::from_points(data.vertices.iter().map(|v| v.position)),
            nodes: Vec::new(),
            skins: Vec::new(),
            animations: Vec::new(),
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
// 骨骼层级 / 蒙皮 / 动画解析（纯 CPU，不碰 GPU —— 可单测）
// ---------------------------------------------------------------------------

/// [`parse_animation_data`] 的产物。
pub struct AnimData {
    /// glTF 节点（默认局部姿态 + 父引用），按节点下标排。
    pub nodes: Vec<crate::animation::AnimNode>,
    /// 蒙皮列表。
    pub skins: Vec<crate::animation::Skin>,
    /// 动画剪辑（glTF `animations[]`，文件顺序）。
    pub animations: Vec<crate::animation::AnimClip>,
    /// mesh 下标 → 蒙皮下标（该 mesh 被带 `skin` 的节点引用时的蒙皮）。
    ///
    /// glTF 的蒙皮挂在**节点**上而不是 mesh 上，而网格数据是按 mesh 组织的 ——
    /// 这张映射表就是"顶点属性里的关节下标该配哪组逆绑定矩阵"的唯一依据。
    pub mesh_skins: HashMap<usize, usize>,
}

/// 解析 glTF 的场景节点、蒙皮与动画剪辑。
///
/// # 为什么单独成函数
///
/// [`GltfMesh::from_file`] 把 GPU 上传和数据解析搅在一起时，动画这条链路
/// 没法在无头环境里测试（CI 没显卡）。拆出这个纯函数后，测试直接喂
/// `gltf::import` 的产物，断言剪辑名 / 关节数 / 采样数值 —— 渲染器那半
/// 只剩"把算好的矩阵搬上 GPU"。
///
/// # v1 的取舍
///
/// - `CUBICSPLINE` 降采样成线性（见下）；
/// - morph target（`weights` 通道）跳过并告警 —— 那是网格变形，不是骨骼动画；
/// - 节点的 `matrix` 形态做一次矩阵分解成 TRS（少数老工具才这么写）。
pub fn parse_animation_data(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
) -> AnimData {
    // -- 节点层级 --
    let node_count = document.nodes().count();
    let mut parents = vec![u32::MAX; node_count];
    for node in document.nodes() {
        for child in node.children() {
            parents[child.index()] = node.index() as u32;
        }
    }
    let nodes: Vec<crate::animation::AnimNode> = document
        .nodes()
        .map(|node| {
            // `decomposed()` 对 Matrix 形态做分解、TRS 形态原样取出 ——
            // 骨架资产几乎都是 TRS，这条统一入口省掉一个手写 match。
            let (translation, rotation, scale) = node.transform().decomposed();
            crate::animation::AnimNode {
                // u32::MAX 是"没父"的哨兵（children 反填时的初值）。
                parent: (parents[node.index()] != u32::MAX).then_some(parents[node.index()]),
                // glTF 节点名（可能没有）—— 骨骼遮罩的选择面。
                name: node.name().unwrap_or_default().to_owned(),
                translation: Vec3::from_array(translation),
                rotation: Quat::from_array(rotation),
                scale: Vec3::from_array(scale),
            }
        })
        .collect();

    // -- 蒙皮 --
    let mut skins = Vec::new();
    for skin in document.skins() {
        let joints: Vec<u32> = skin.joints().map(|j| j.index() as u32).collect();
        if joints.len() > crate::scene_pipeline::BONES_PER_SLOT {
            log::warn!(
                "skin '{}' has {} joints — only the first {} are supported (v1 limit)",
                skin.name().unwrap_or("<unnamed>"),
                joints.len(),
                crate::scene_pipeline::BONES_PER_SLOT
            );
        }
        let reader = skin.reader(|buf| Some(&buffers[buf.index()]));
        // 缺 IBM accessor 的蒙皮按"单位阵"处理（= 不做绑定空间校正），
        // 顶多姿态不准，不会黑屏或 panic。
        let inverse_bind: Vec<Mat4> = reader
            .read_inverse_bind_matrices()
            .map(|iter| iter.map(|m| Mat4::from_cols_array_2d(&m)).collect())
            .unwrap_or_else(|| vec![Mat4::IDENTITY; joints.len()]);
        skins.push(crate::animation::Skin {
            joints,
            inverse_bind,
        });
    }

    // -- mesh → 蒙皮 --
    let mut mesh_skins = HashMap::new();
    for node in document.nodes() {
        if let (Some(mesh), Some(skin)) = (node.mesh(), node.skin()) {
            mesh_skins.entry(mesh.index()).or_insert(skin.index());
        }
    }

    // -- 动画剪辑 --
    use gltf::animation::{util::ReadOutputs, Interpolation as GltfInterp, Property as GltfProperty};
    let mut animations = Vec::new();
    for anim in document.animations() {
        let mut tracks = Vec::new();
        let mut duration = 0.0f32;
        for channel in anim.channels() {
            let sampler = channel.sampler();
            // 读数据的 reader 挂在 **channel** 上（它才知道目标属性的类型）。
            let reader = channel.reader(|buf| Some(&buffers[buf.index()]));

            // 输入 = 关键帧时间。`ReadInputs` 是 `accessor::Iter<f32>` 的别名，
            // 本身就是 f32 迭代器，直接 collect。
            let Some(inputs) = reader.read_inputs() else {
                continue;
            };
            let times: Vec<f32> = inputs.collect();
            if times.is_empty() {
                continue;
            }
            if let Some(&last) = times.last() {
                duration = duration.max(last);
            }

            let path = match channel.target().property() {
                GltfProperty::Translation => TrackPath::Translation,
                GltfProperty::Rotation => TrackPath::Rotation,
                GltfProperty::Scale => TrackPath::Scale,
                // morph target 不是骨骼动画，跳过（有资产用到时日志会说明去向）。
                GltfProperty::MorphTargetWeights => {
                    log::warn!(
                        "animation '{}': morph-target channels are not supported yet — skipped",
                        anim.name().unwrap_or("<unnamed>")
                    );
                    continue;
                }
            };
            let cubic = matches!(sampler.interpolation(), GltfInterp::CubicSpline);
            let interpolation = match sampler.interpolation() {
                GltfInterp::Step => Interpolation::Step,
                // CUBICSPLINE 降采样成线性：见下面 `take_spline_values`。
                _ => Interpolation::Linear,
            };

            let values = match reader.read_outputs() {
                Some(ReadOutputs::Translations(iter)) => take_spline_values(
                    iter.map(|v| v.to_vec()).collect(),
                    cubic,
                ),
                Some(ReadOutputs::Scales(iter)) => take_spline_values(
                    iter.map(|v| v.to_vec()).collect(),
                    cubic,
                ),
                Some(ReadOutputs::Rotations(iter)) => take_spline_values(
                    iter.into_f32().map(|v| v.to_vec()).collect(),
                    cubic,
                ),
                _ => continue,
            };

            tracks.push(crate::animation::Track {
                node: channel.target().node().index() as u32,
                path,
                interpolation,
                times,
                values,
            });
        }

        animations.push(crate::animation::AnimClip {
            name: anim.name().unwrap_or("").to_string(),
            duration,
            tracks,
        });
    }

    AnimData {
        nodes,
        skins,
        animations,
        mesh_skins,
    }
}

/// CUBICSPLINE 输出取"值"那一段，其余插值原样返回。
///
/// glTF 的三次样条输出是 3N 个元素的**三段块**（前 N 个入切线、中 N 个值、
/// 后 N 个出切线，见规范 `animation.sampler.output`）。切线是给样条求值用的，
/// 我们 v1 走线性插值 → 只留中间那段值，两端丢弃。
fn take_spline_values(mut items: Vec<Vec<f32>>, cubic: bool) -> Vec<f32> {
    if cubic && items.len() >= 3 {
        let n = items.len() / 3;
        let tail = items.split_off(n); // tail = [值段, 出切线段]
        items = tail;
        items.truncate(n); // 只留值段
    }
    items.into_iter().flatten().collect()
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

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{clip_time, compute_bones, TrackPath};

    // -- 材质覆盖合并 -------------------------------------------------------

    /// 基础色是**乘子**：与模型自带色逐通道相乘（含透明度）。
    #[test]
    fn base_color_override_multiplies() {
        let base = MaterialParams {
            base_color: [0.5, 0.5, 0.5, 0.8],
            ..Default::default()
        };
        let over = xgame_core::MaterialOverride {
            base_color: Some([0.4, 1.0, 2.0, 0.5]),
            ..Default::default()
        };
        let merged = base.merged_with(Some(&over));
        assert_eq!(merged.base_color, [0.2, 0.5, 1.0, 0.4]);
        assert_eq!(merged.metallic, base.metallic, "没写的字段保持不变");
        assert_eq!(merged.roughness, base.roughness);
    }

    /// 金属度 / 粗糙度是**替换**：写了就覆盖，没写就保留。
    #[test]
    fn metallic_roughness_override_replaces() {
        let base = MaterialParams {
            metallic: 0.1,
            roughness: 0.9,
            ..Default::default()
        };
        let merged = base.merged_with(Some(&xgame_core::MaterialOverride {
            roughness: Some(0.2),
            ..Default::default()
        }));
        assert_eq!(merged.metallic, 0.1, "没写 metallic 保持原值");
        assert_eq!(merged.roughness, 0.2, "写了 roughness 用覆盖值");

        let merged = base.merged_with(Some(&xgame_core::MaterialOverride {
            metallic: Some(1.0),
            ..Default::default()
        }));
        assert_eq!(merged.metallic, 1.0);
        assert_eq!(merged.roughness, 0.9);
    }

    /// `None` 覆盖 = 原样返回（无覆盖实体的行为与覆盖功能上线前逐位一致）；
    /// `blend` 永远不参与覆盖（管线级属性）。
    #[test]
    fn no_override_is_identity_and_blend_never_merges() {
        let base = MaterialParams {
            base_color: [0.1, 0.2, 0.3, 1.0],
            metallic: 0.5,
            roughness: 0.5,
            blend: true,
        };
        let same = base.merged_with(None);
        assert_eq!(same.base_color, base.base_color);
        assert_eq!(same.metallic, base.metallic);
        assert_eq!(same.roughness, base.roughness);
        assert!(same.blend, "blend 不受覆盖影响");

        // 即使覆盖写满，blend 也不动。
        let over = xgame_core::MaterialOverride {
            base_color: Some([2.0; 4]),
            metallic: Some(0.0),
            roughness: Some(1.0),
        };
        assert!(base.merged_with(Some(&over)).blend);
    }

    /// 仓库自带的示例狐狸（3 段剪辑 + 24 骨蒙皮）——解析测试的固定夹具。
    ///
    /// 用 `CARGO_MANIFEST_DIR` 拼绝对路径：测试在哪个工作目录下跑都成立，
    /// 不依赖 `cargo test` 的 cwd。
    fn fox() -> (gltf::Document, Vec<gltf::buffer::Data>) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../projects/SampleScene/Resources/Fox.gltf");
        let (doc, buffers, _) = gltf::import(&path).expect("应当能解析示例 Fox.gltf");
        (doc, buffers)
    }

    /// 解析出来的狐狸必须"齐装满员"：剪辑、蒙皮、节点、mesh→skin 映射，
    /// 以及每条轨道的值/时间对齐（值数错位是采样越界的经典来源）。
    #[test]
    fn fox_parses_clips_skins_and_nodes() {
        let (doc, buffers) = fox();
        let data = parse_animation_data(&doc, &buffers);

        let names: Vec<&str> = data.animations.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Survey", "Walk", "Run"], "三段剪辑及顺序");

        assert_eq!(data.skins.len(), 1, "狐狸有一张蒙皮");
        assert_eq!(data.skins[0].joints.len(), 24, "24 根骨头");
        assert_eq!(data.skins[0].inverse_bind.len(), 24, "IBM 必须与关节数一一对应");

        assert_eq!(data.nodes.len(), 26, "26 个节点（场景根 + 网格 + 骨架）");
        assert_eq!(data.mesh_skins.get(&0), Some(&0), "mesh0 被挂在蒙皮0 上");

        let walk = data
            .animations
            .iter()
            .find(|c| c.name == "Walk")
            .expect("Walk 剪辑存在");
        assert!(walk.duration > 0.0, "时长必须为正，实得 {}", walk.duration);
        assert!(!walk.tracks.is_empty(), "Walk 不能是空轨道");
        for track in &walk.tracks {
            let stride = match track.path {
                TrackPath::Translation | TrackPath::Scale => 3,
                TrackPath::Rotation => 4,
            };
            assert_eq!(
                track.values.len(),
                track.times.len() * stride,
                "轨道 {} 的值/时间必须对齐",
                track.node
            );
            assert!(
                track.times.windows(2).all(|w| w[0] <= w[1]),
                "关键帧时间必须单调不减"
            );
        }
    }

    /// 端到端：Walk 中点采样出 24 个有限矩阵，且与第 0 帧**不同** ——
    /// "解析 + 采样 + 层级 + 蒙皮"整条链路真的在产出动画，而不是恒等变换。
    #[test]
    fn fox_walk_midtime_samples_finite_changing_bones() {
        let (doc, buffers) = fox();
        let data = parse_animation_data(&doc, &buffers);
        let walk = data
            .animations
            .iter()
            .find(|c| c.name == "Walk")
            .expect("Walk 剪辑存在");

        let t = clip_time(walk.duration * 0.5, walk.duration, true);
        let bones = compute_bones(&data.nodes, walk, &data.skins[0], t);
        assert_eq!(bones.len(), 24);
        for (i, b) in bones.iter().enumerate() {
            assert!(
                b.to_cols_array().iter().all(|v| v.is_finite()),
                "骨头 {i} 含非有限值：{b:?}"
            );
        }

        let first = compute_bones(&data.nodes, walk, &data.skins[0], 0.0);
        let changed = bones
            .iter()
            .zip(first.iter())
            .any(|(a, b)| (*a - *b).to_cols_array().iter().any(|v| v.abs() > 1e-4));
        assert!(changed, "Walk 中帧必须与第 0 帧不同 —— 否则剪辑没在动");
    }

    /// 绑定姿态自洽：用第 0 帧的骨骼反推 IBM 应当还原出"未播放"状态 ——
    /// 这里退一步验证**默认节点姿态**能被采样保留（轨道没瞄到的节点不动）。
    #[test]
    fn untapped_nodes_keep_their_default_pose() {
        let (doc, buffers) = fox();
        let data = parse_animation_data(&doc, &buffers);
        let pose = crate::animation::default_pose(&data.nodes);
        for (i, node) in data.nodes.iter().enumerate() {
            assert!((pose[i].translation - node.translation).length() < 1e-6, "节点 {i} 位移被改动");
            // 逐分量比，不用 angle_between：资产里的四元数常常只是"近似归一"
            // （1e-6 量级的模长误差会被 acos 放大成 ~1e-3 弧度的假差异），
            // 而 default_pose 是**原样拷贝**，逐位相等才是它该有的承诺。
            assert_eq!(
                pose[i].rotation.to_array(),
                node.rotation.to_array(),
                "节点 {i} 旋转被改动"
            );
            assert!((pose[i].scale - node.scale).length() < 1e-6, "节点 {i} 缩放被改动");
        }
    }
}
