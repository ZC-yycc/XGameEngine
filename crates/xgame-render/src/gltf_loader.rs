use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use wgpu::util::DeviceExt;

use xgame_math::glam::{Mat4, Quat, Vec3};

use crate::animation::{Interpolation, TrackPath};
use crate::texture::{Texture, TextureOptions};
use xgame_core::{
    read_import_settings, resolve_read_path, AnimationAsset, AnimationLoader, ImportSettings,
    MaterialAsset,
};

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
    ///
    /// 实现是"变换 8 个角点再重新求盒"——旋转之后盒子会变成平行六面体，
    /// 不能用对角线直接推。这也是 [`transformed_affine`](Self::transformed_affine)
    /// 的**参考实现**：后者用「绝对值矩阵」走解析式，二者对仿射矩阵必须逐值一致。
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

    /// 仿射矩阵下的轴对齐盒——解析式，比 [`transformed`](Self::transformed)
    /// 少 7 次点变换。
    ///
    /// 原理：平行六面体在第 i 轴上的半长 = Σ_j |M_ij| · e_j（局部半长 e）。
    /// 每帧要对**所有实体**做一次（阴影拟合 + 视锥剔除），8 角点版本在
    /// 2000 实体下是几十微秒的固定税；解析式把向量乘法从 24 次降到 9 次。
    ///
    /// 只对仿射矩阵成立（平移 / 旋转 / 缩放 / 错切都行，投影矩阵不行）——
    /// 世界矩阵按定义就是仿射的。空盒原样返回（保持"空"的哨兵语义）。
    pub fn transformed_affine(&self, m: &Mat4) -> Bounds {
        if self.is_empty() {
            return *self;
        }
        let center = self.center();
        let extent = self.half_extent();
        let world_center = m.transform_point3(center);
        // `Mat4` 的 3×3 部分：用各列的绝对值对 extent 做线性组合。
        // （等价于 |M| · e，行/列写法在这里只影响变量命名。）
        let x = m.x_axis;
        let y = m.y_axis;
        let z = m.z_axis;
        let world_extent = Vec3::new(
            x.x.abs() * extent.x + y.x.abs() * extent.y + z.x.abs() * extent.z,
            x.y.abs() * extent.x + y.y.abs() * extent.y + z.y.abs() * extent.z,
            x.z.abs() * extent.x + y.z.abs() * extent.y + z.z.abs() * extent.z,
        );
        Bounds {
            min: world_center - world_extent,
            max: world_center + world_extent,
        }
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
    /// 套用共享材质资产（`MaterialAsset`），得到"模型材质 + 资产"的结果。
    ///
    /// 语义见 [`xgame_core::MaterialAsset`]：
    /// - `base_color` **相乘**（tint 乘子，与覆盖同款）；
    /// - `metallic` / `roughness` **替换**（资产就是一份完整材质）；
    /// - `albedo` 不在这里处理 —— 贴图是 GPU 资源，由材质库负责换成
    ///   [`GltfPrimitive::texture`]（见 `ScenePipeline` 的绘制循环）；
    /// - `blend` 不动（透明与否是管线级属性，与覆盖同一取舍）。
    ///
    /// `None` 原样返回：没引用材质的实体与材质资产功能上线前逐位一致。
    pub fn with_asset(&self, asset: Option<&MaterialAsset>) -> Self {
        let Some(asset) = asset else {
            return *self;
        };
        Self {
            base_color: [
                self.base_color[0] * asset.base_color[0],
                self.base_color[1] * asset.base_color[1],
                self.base_color[2] * asset.base_color[2],
                self.base_color[3] * asset.base_color[3],
            ],
            metallic: asset.metallic,
            roughness: asset.roughness,
            blend: self.blend,
        }
    }

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
    /// 形变目标（morph target）。`None` = 这个图元没有顶点级形变。
    ///
    /// 带 morph 的图元会额外留一份 CPU 顶点（变形的基准），顶点缓冲也多一个
    /// `COPY_DST` 用途 —— 权重一变就整段重写（见 [`crate::morph`]）。
    pub morph: Option<crate::morph::MorphTargets>,
}

impl GltfPrimitive {
    /// 为一个实体创建独立的形变顶点缓冲；静态图元返回 `None`。
    pub fn create_morph_buffer(&self, device: &wgpu::Device) -> Option<crate::morph::MorphBuffer> {
        self.morph.as_ref().map(|morph| morph.create_buffer(device))
    }
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

/// 解析磁盘模型的真实文件与并排导入设置。
///
/// 先把 GPU 路径原样收成纯函数，便于用不需要显卡的回归测试钉住
/// `search_paths` 与 `.meta` 必须和 CPU 动画装载器同解。
fn resolve_model_source(
    project_root: &Path,
    model: &str,
    search_paths: &[String],
) -> Result<(PathBuf, ImportSettings)> {
    let path = resolve_read_path(project_root, model, search_paths)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    // `.meta` 与真正命中的文件并排；search_paths 命中时不能仍去直接路径旁找，
    // 否则渲染与根运动会吃到不同的 model_scale。
    let relative = path
        .strip_prefix(project_root)
        .unwrap_or(path.as_path())
        .to_string_lossy()
        .replace('\\', "/");
    let settings = read_import_settings(project_root, &relative);
    Ok((path, settings))
}

/// 给 runtime 使用的**纯 CPU** glTF 动画装载器。
///
/// 它与 [`GltfMesh`] 共用本文件的 [`parse_animation_data_scaled`]，所以节点、剪辑和
/// `model_scale` 不会出现“渲染看到一套、根运动采另一套”。这里只读 glTF 缓冲，不建
/// wgpu 设备，也不上传顶点；普通 `--headless` 因而能完整推进根运动。
#[derive(Clone, Debug)]
pub struct GltfAnimationLoader {
    root: PathBuf,
    search_paths: Vec<String>,
}

impl GltfAnimationLoader {
    pub fn new(root: impl Into<PathBuf>, search_paths: Vec<String>) -> Self {
        Self {
            root: root.into(),
            search_paths,
        }
    }
}

impl AnimationLoader for GltfAnimationLoader {
    fn load_animation(&self, model: &str) -> Result<AnimationAsset, String> {
        // 内置图元没有 glTF 节点 / 剪辑，直接给空资产；这不是加载失败，也不该每帧告警。
        if crate::mesh::BuiltinMesh::from_model_path(model).is_some() {
            return Ok(AnimationAsset::default());
        }
        let (path, settings) = resolve_model_source(&self.root, model, &self.search_paths)
            .map_err(|error| format!("{error:#}"))?;
        let (document, buffers, _) = gltf::import(&path)
            .with_context(|| format!("解析 glTF '{}'", path.display()))
            .map_err(|err| format!("{err:#}"))?;
        let data = parse_animation_data_scaled(&document, &buffers, settings.model_scale);
        Ok(AnimationAsset {
            nodes: data.nodes,
            clips: data.animations,
        })
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

    /// 清零超出骨槽容量的关节影响（固定容量超限策略的顶点侧一半）。
    ///
    /// 槽位只有 [`crate::scene_pipeline::BONES_PER_SLOT`] 根骨，超过的骨架在
    /// 上传时被截断；顶点若还引用截断区的骨，着色器的 `base + index` 会读进
    /// **下一个实体的槽**（别人一动它跟着变形）。这里把越界影响的权重清零、
    /// 下标归 0 占位：shader 的 `1/wsum` 会把剩下的影响重归一；全清零 = 冻结
    /// 在绑定姿态（`wsum == 0` 分支）。宁可末梢不动，不能读别人的骨。
    pub fn clamp_skin_influences(&mut self) {
        for slot in self.joints.iter_mut().zip(self.weights.iter_mut()) {
            let (joint, weight) = slot;
            // NaN 下标同样清零（比较为假走这里）。
            if !matches!(
                (*joint).partial_cmp(&(crate::scene_pipeline::BONES_PER_SLOT as f32)),
                Some(std::cmp::Ordering::Less)
            ) {
                *joint = 0.0;
                *weight = 0.0;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 加载器
// ---------------------------------------------------------------------------

impl GltfMesh {
    /// 加载 `.gltf` / `.glb`，使用默认导入设置（等价于 `.meta` 全是默认值）。
    pub fn from_file(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::from_file_with(device, queue, path, ImportSettings::default())
    }

    /// 加载 `.gltf` / `.glb`，按资产的导入设置处理缩放与内嵌贴图。
    ///
    /// 缺失的顶点属性用合理默认值填充。
    pub fn from_file_with(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: impl AsRef<Path>,
        settings: ImportSettings,
    ) -> Result<Self> {
        let (document, buffers, images) =
            gltf::import(path.as_ref()).context("Failed to parse glTF file")?;

        // `model_scale` 是**烘焙进几何与骨架**的，而不是乘在实体变换上：
        // 后者会与用户在 Inspector 里设的 Transform 缩放打架（改导入缩放
        // 把摆放位置也一起改了）。烘焙的完整规则见 `parse_animation_data_scaled`。
        let scale = sanitize_model_scale(settings.model_scale);
        // 骨骼层级 / 蒙皮 / 动画剪辑：纯 CPU 解析（可单测，不碰 GPU）。
        let anim = parse_animation_data_scaled(&document, &buffers, scale);
        let texture_options = TextureOptions {
            srgb: settings.texture_srgb,
            nearest: settings.texture_nearest,
        };

        let mut primitives = Vec::new();
        // 整个网格的本地包围盒：各图元的顶点位置并起来。阴影拟合只关心这个。
        let mut bounds = Bounds::EMPTY;

        for mesh in document.meshes() {
            // 这个 mesh 被哪个节点以哪张蒙皮引用（见 `parse_animation_data`）。
            let mesh_skin = anim.mesh_skins.get(&mesh.index()).copied();
            for primitive in mesh.primitives() {
                let reader = primitive.reader(|buf| Some(&buffers[buf.index()]));

                // --- 位置（必需）---
                // 导入缩放直接乘进顶点：动画的节点位移 / IBM 平移也同步缩放
                // （见 `parse_animation_data_scaled`），三者不一起变会撕裂蒙皮。
                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .context("Missing POSITION attribute")?
                    .map(|p| [p[0] * scale, p[1] * scale, p[2] * scale])
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
                            .map(|j| [j[0] as f32, j[1] as f32, j[2] as f32, j[3] as f32])
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
                let texture =
                    load_primitive_texture(device, queue, &primitive, &images, texture_options);

                // --- 材质因子 ---
                let material = read_material(&primitive);
                let material_id = next_material_id();

                // 交错排列为 GltfVertex 数组
                let mut vertices: Vec<GltfVertex> = (0..vertex_count)
                    .map(|i| GltfVertex {
                        position: positions[i],
                        normal: normals[i],
                        color: colors[i],
                        tex_coord: tex_coords[i],
                        joints: joints[i],
                        weights: weights[i],
                    })
                    .collect();
                // 固定容量超限策略的顶点侧：越界关节影响清零（防读进邻槽）。
                vertices
                    .iter_mut()
                    .for_each(GltfVertex::clamp_skin_influences);

                let vertex_bytes: &[u8] = bytemuck::cast_slice(&vertices);

                // --- 形变目标（morph target）---
                //
                // 增量在这里就把 `model_scale` 乘进去了：与顶点位置同一套烘焙
                // 规则，否则改导入缩放会让"表情幅度"和"模型尺寸"脱钩。
                let morph = parse_morph_targets(&mesh, &primitive, &buffers, &vertices, scale);
                if let Some(morph) = &morph {
                    bounds = bounds.union(&morph.conservative_bounds());
                }
                // 带 morph 的图元要能被 `write_buffer` 整段重写（CPU 变形）。
                let mut vertex_usage = wgpu::BufferUsages::VERTEX;
                if morph.is_some() {
                    vertex_usage |= wgpu::BufferUsages::COPY_DST;
                }

                let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("glTF vertex buffer"),
                    contents: vertex_bytes,
                    usage: vertex_usage,
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
                    morph,
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
        let no_joints = [0f32; 4]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>();
        let root_weight = [1f32, 0.0, 0.0, 0.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>();

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
                morph: None,
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
        // 与真实导入同一条超限策略：越界关节影响清零（见 `clamp_skin_influences`）。
        let mut vertices = data.vertices.clone();
        vertices
            .iter_mut()
            .for_each(GltfVertex::clamp_skin_influences);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(&vertices),
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
                morph: None,
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
        Self::load_with_search_paths(device, queue, project_root, model, &[])
    }

    /// 与 [`Self::load`] 相同，但按项目清单的 `content.search_paths` 有序兜底。
    ///
    /// 宿主应传入与 [`GltfAnimationLoader`] 相同的列表，保证 GPU 网格与 CPU
    /// 动画命中同一文件及其并排 `.meta`。保留 [`Self::load`] 作为空列表兼容入口。
    pub fn load_with_search_paths(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        project_root: &Path,
        model: &str,
        search_paths: &[String],
    ) -> Result<Self> {
        if let Some(builtin) = crate::mesh::BuiltinMesh::from_model_path(model) {
            return Ok(Self::from_mesh_data(
                device,
                queue,
                &builtin.build(),
                builtin.label(),
            ));
        }
        // 读 `.meta`（只读）。没有 `.meta` 或解析失败 → 默认设置，
        // 与资产管线"坏 meta 不阻断出图"的约定一致。
        let (path, settings) = resolve_model_source(project_root, model, search_paths)?;
        Self::from_file_with(device, queue, path, settings)
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
/// - `CUBICSPLINE` **完整保留**（每帧入切线 / 值 / 出切线三份），采样端走 Hermite
///   求值 —— 降成线性会把 Blender / Maya 默认导出的加减速抹平；
/// - 形变目标权重（`MorphTargetWeights`）**收下**但不进姿态：它是网格属性，
///   单独一条轨道（[`crate::animation::TrackPath::MorphWeights`]）；
/// - 节点的 `matrix` 形态做一次矩阵分解成 TRS（少数老工具才这么写）。
pub fn parse_animation_data(document: &gltf::Document, buffers: &[gltf::buffer::Data]) -> AnimData {
    parse_animation_data_scaled(document, buffers, 1.0)
}

/// `model_scale` 的合法化：非有限 / 非正数一律回 1.0。
///
/// 负缩放会让三角形绕序翻转（背面剔除把模型剔没），0 会让包围盒塌成
/// 一个点（阴影拟合除零）；这两种"用户手滑"不该变成黑屏。
fn sanitize_model_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// 同 [`parse_animation_data`]，但把所有**平移量**乘以 `scale`。
///
/// 这是 `model_scale` 烘焙规则的另一半（几何顶点在 `from_file_with` 里乘）：
///
/// - 节点默认位移 × scale；
/// - 动画 Translation 轨道的每个关键帧值 × scale；
/// - 蒙皮逆绑定矩阵的平移分量 × scale（`S·IBM·S⁻¹` 对均匀缩放的化简，
///   线性部分不变）。
///
/// 旋转 / 缩放通道**不动**：均匀全局缩放不改变朝向与相对比例。
/// `scale == 1.0` 时跳过全部乘法，保证默认路径逐位不变。
pub fn parse_animation_data_scaled(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    scale: f32,
) -> AnimData {
    let scale = sanitize_model_scale(scale);
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
            let (translation, rotation, node_scale) = node.transform().decomposed();
            let mut translation = Vec3::from_array(translation);
            if scale != 1.0 {
                translation *= scale;
            }
            crate::animation::AnimNode {
                // u32::MAX 是"没父"的哨兵（children 反填时的初值）。
                parent: (parents[node.index()] != u32::MAX).then_some(parents[node.index()]),
                // glTF 节点名（可能没有）—— 骨骼遮罩的选择面。
                name: node.name().unwrap_or_default().to_owned(),
                translation,
                rotation: Quat::from_array(rotation),
                scale: Vec3::from_array(node_scale),
            }
        })
        .collect();

    // -- 蒙皮 --
    let mut skins = Vec::new();
    for skin in document.skins() {
        let mut joints: Vec<u32> = skin.joints().map(|j| j.index() as u32).collect();
        if joints.len() > crate::scene_pipeline::BONES_PER_SLOT {
            log::warn!(
                "skin '{}' has {} joints — only the first {} are supported (v1 limit)",
                skin.name().unwrap_or("<unnamed>"),
                joints.len(),
                crate::scene_pipeline::BONES_PER_SLOT
            );
        }
        // 固定容量超限策略：关节表**截断**到骨槽容量（末梢骨丢弃），保证
        // `bones[k]` 与顶点 JOINTS_0 的下标空间一致 —— 顶点侧的越界影响由
        // `GltfVertex::clamp_skin_influences` 清零，两边共同堵死"读进邻槽"。
        joints.truncate(crate::scene_pipeline::BONES_PER_SLOT);
        let reader = skin.reader(|buf| Some(&buffers[buf.index()]));
        // 缺 IBM accessor 的蒙皮按"单位阵"处理（= 不做绑定空间校正），
        // 顶多姿态不准，不会黑屏或 panic。
        let mut inverse_bind: Vec<Mat4> = reader
            .read_inverse_bind_matrices()
            .map(|iter| iter.map(|m| Mat4::from_cols_array_2d(&m)).collect())
            .unwrap_or_else(|| vec![Mat4::IDENTITY; joints.len()]);
        inverse_bind.truncate(crate::scene_pipeline::BONES_PER_SLOT);
        if scale != 1.0 {
            // 均匀缩放 s 下 IBM' = S·IBM·S⁻¹：线性部分不变，平移乘 s。
            // （推导见 `parse_animation_data_scaled` 的文档。）
            for m in &mut inverse_bind {
                m.w_axis.x *= scale;
                m.w_axis.y *= scale;
                m.w_axis.z *= scale;
            }
        }
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
    use gltf::animation::{util::ReadOutputs, Property as GltfProperty};
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
                // 形变目标权重：**不是**骨骼动画，但也别丢 —— 它是口型 / 表情
                // 的唯一来源。单独一条轨道类型，由 `sample_morph_weights` 消费。
                GltfProperty::MorphTargetWeights => {
                    let Some(ReadOutputs::MorphTargetWeights(iter)) = reader.read_outputs() else {
                        continue;
                    };
                    let interpolation = parse_interpolation(sampler.interpolation());
                    // 标量流按"时点 × 组"分组：每帧 1 组（线性 / 阶梯）
                    // 或 3 组（入切线 / 值 / 出切线）。
                    let flat: Vec<f32> = iter.into_f32().collect();
                    let groups = times.len() * interpolation.slots();
                    if flat.is_empty() || !flat.len().is_multiple_of(groups) {
                        continue;
                    }
                    tracks.push(crate::animation::Track {
                        node: channel.target().node().index() as u32,
                        path: TrackPath::MorphWeights,
                        interpolation,
                        times: times.clone(),
                        values: flat,
                    });
                    continue;
                }
            };
            let interpolation = parse_interpolation(sampler.interpolation());
            // 变换通道的分量数是固定的：旋转 4，位移 / 缩放 3。
            let components = if path == TrackPath::Rotation { 4 } else { 3 };
            // 输出数组**原样收下**：CUBICSPLINE 每帧三份（入切线 / 值 / 出切线），
            // 线性与阶梯每帧一份。切线是内容的一部分，丢了就补不回来。
            let mut values: Vec<f32> = match reader.read_outputs() {
                Some(ReadOutputs::Translations(iter)) => iter.flat_map(|v| v.to_vec()).collect(),
                Some(ReadOutputs::Scales(iter)) => iter.flat_map(|v| v.to_vec()).collect(),
                Some(ReadOutputs::Rotations(iter)) => {
                    iter.into_f32().flat_map(|v| v.to_vec()).collect()
                }
                _ => continue,
            };
            // 形状对不上（资产脏）就整条丢掉：硬读会串帧，画出来是抽搐。
            if values.len() != times.len() * components * interpolation.slots() {
                continue;
            }
            if path == TrackPath::Translation && scale != 1.0 {
                // 值本身和切线都是位移量（切线单位是米/秒），一起乘。
                for value in values.as_chunks_mut::<3>().0 {
                    value[0] *= scale;
                    value[1] *= scale;
                    value[2] *= scale;
                }
            }

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

/// 解析一个图元的形变目标（glTF `mesh.primitives[].targets`）。
///
/// 纯 CPU、不碰 GPU —— 这样它能在没有显卡的 CI 上用合成 glTF 直接测。
///
/// 布局是 **target-major**：`positions[target * vertex_count + vertex]`。
/// 变形内层循环是"一个权重扫一遍全部顶点"（同权重的访存连续），
/// 所以按 target 分段比按顶点分段省一次跨步跳跃。
///
/// 任何长度不自洽的情况一律**整段放弃**（返回 `None`，图元当静态网格渲染）：
/// 增量错位会把顶点挪到不存在的位置，画出来是一团谁都认不出的几何，
/// 比"没有表情"难查得多。
fn parse_morph_targets(
    mesh: &gltf::Mesh<'_>,
    primitive: &gltf::Primitive<'_>,
    buffers: &[gltf::buffer::Data],
    base: &[GltfVertex],
    scale: f32,
) -> Option<crate::morph::MorphTargets> {
    let targets: Vec<gltf::mesh::MorphTarget<'_>> = primitive.morph_targets().collect();
    if targets.is_empty() || base.is_empty() {
        return None;
    }
    let target_count = targets.len();
    let vertex_count = base.len();
    let mut positions = vec![[0.0f32; 3]; target_count * vertex_count];
    let mut normals = vec![[0.0f32; 3]; target_count * vertex_count];
    let mut has_normals = false;

    for (t, target) in targets.iter().enumerate() {
        let offset = t * vertex_count;
        if let Some(accessor) = target.positions() {
            let Some(iter) =
                gltf::accessor::Iter::<[f32; 3]>::new(accessor, |buf| Some(&buffers[buf.index()]))
            else {
                log::warn!(
                    "morph target {t}: unreadable POSITION — morph disabled for this primitive"
                );
                return None;
            };
            let mut read = 0;
            for (v, delta) in iter.enumerate() {
                if v >= vertex_count {
                    break;
                }
                // 与顶点位置同一套烘焙规则（见 `from_file_with` 的注释）。
                positions[offset + v] = [delta[0] * scale, delta[1] * scale, delta[2] * scale];
                read += 1;
            }
            if read != vertex_count {
                log::warn!(
                    "morph target {t}: {read} deltas but mesh has {vertex_count} vertices — morph disabled"
                );
                return None;
            }
        } else {
            // 规范的 morph target 必须有 POSITION；缺了当"零位移"处理，
            // 目标仍然存在（权重表对得上号），只是不改变几何。
            log::warn!("morph target {t}: no POSITION delta — treated as zero displacement");
        }
        if let Some(accessor) = target.normals() {
            let Some(iter) =
                gltf::accessor::Iter::<[f32; 3]>::new(accessor, |buf| Some(&buffers[buf.index()]))
            else {
                continue;
            };
            // 法线增量不做 `model_scale`：均匀缩放不改变方向（长度由归一化收回）。
            for (v, delta) in iter.enumerate().take(vertex_count) {
                normals[offset + v] = delta;
            }
            has_normals = true;
        }
    }

    let names = morph_target_names(mesh, target_count);
    let mut defaults = mesh
        .weights()
        .map(|weights| weights.to_vec())
        .unwrap_or_default();
    defaults.resize(target_count, 0.0);
    if !has_normals {
        normals.clear();
    }
    let built = crate::morph::MorphTargets::new(names, defaults, positions, normals, base.to_vec());
    if built.is_none() {
        log::warn!("morph targets rejected (inconsistent shape) — primitive stays static");
    }
    built
}

/// 目标名：glTF 把它们放在 `mesh.extras.targetNames`（非标准字段，但 Blender /
/// Maya / Unity 的导出器都写这里）。
///
/// `extras` 在 gltf crate 里是**未解析的 JSON 原文**（`Box<RawValue>`），
/// 所以要自己反序列化一次。没有名字时退回 `Target N` —— Inspector 上
/// 一排"Target 0..19"很难用，但总比不显示好。
fn morph_target_names(mesh: &gltf::Mesh<'_>, count: usize) -> Vec<String> {
    let mut names: Vec<String> = mesh
        .extras()
        .as_ref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).ok())
        .and_then(|value| value.get("targetNames").and_then(|n| n.as_array()).cloned())
        .map(|list| {
            list.iter()
                .take(count)
                .map(|v| v.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default();
    names.resize(count, String::new());
    for (i, name) in names.iter_mut().enumerate() {
        if name.trim().is_empty() {
            *name = format!("Target {i}");
        }
    }
    names
}

/// glTF 的三种插值一一对应，**不做任何降级**。
///
/// 早先这里把 `CUBICSPLINE` 映射成线性、读入时丢掉切线，动作会变"匀速"；
/// 现在原样保留，由采样端的 Hermite 求值消费。
fn parse_interpolation(interpolation: gltf::animation::Interpolation) -> Interpolation {
    use gltf::animation::Interpolation as GltfInterp;
    // 穷举匹配而非 `_ =>`：gltf 升级后新增插值类型时这里会编译不过，
    // 逼着做一次显式决断，而不是悄悄退化成线性。
    match interpolation {
        GltfInterp::Linear => Interpolation::Linear,
        GltfInterp::Step => Interpolation::Step,
        GltfInterp::CubicSpline => Interpolation::CubicSpline,
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
    options: TextureOptions,
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
                return Texture::from_rgba8_with(
                    device,
                    queue,
                    data.width,
                    data.height,
                    &rgba,
                    "glTF diffuse",
                    options,
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

pub(crate) fn compute_face_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
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

        normals[i0][0] += nx;
        normals[i0][1] += ny;
        normals[i0][2] += nz;
        normals[i1][0] += nx;
        normals[i1][1] += ny;
        normals[i1][2] += nz;
        normals[i2][0] += nx;
        normals[i2][1] += ny;
        normals[i2][2] += nz;
    }

    for n in &mut normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n[0] /= len;
            n[1] /= len;
            n[2] /= len;
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
pub(crate) mod tests {
    use super::*;
    use crate::animation::{clip_time, compute_bones, TrackPath};

    /// CUBICSPLINE 的输出是"每帧三份"，加载器必须整段保留。
    ///
    /// 旧实现只挑中间那份（切线全丢），这条断言正是那个行为的反义：
    /// 值换了位置、切线也在，`frame_len` 必须是 3 × stride。
    #[test]
    fn regression_r04_spline_keeps_tangents() {
        let data = cubic_translation_fixture([0.0, 1.0, 2.0]);
        let track = &data.animations[0].tracks[0];
        assert_eq!(track.interpolation, Interpolation::CubicSpline);
        assert_eq!(track.stride(), 3, "vec3 的值分量数");
        assert_eq!(track.frame_len(), 9, "每帧 3 组 × 3 分量");
        assert_eq!(track.values.len(), 3 * track.frame_len(), "3 帧完整数据");
        // 第 1 帧布局：入切线 (0,0,0) / 值 (0,1,0) / 出切线 (0,0,0)
        assert_eq!(
            &track.values[9..18],
            &[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn regression_r04_morph_spline_three_keys_two_targets() {
        let json = r#"{
            "asset":{"version":"2.0"}, "nodes":[{}],
            "buffers":[{"byteLength":84}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":12},
                           {"buffer":0,"byteOffset":12,"byteLength":72}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"SCALAR","min":[0],"max":[2]},
                         {"bufferView":1,"componentType":5126,"count":18,"type":"SCALAR"}],
            "animations":[{"samplers":[{"input":0,"output":1,"interpolation":"CUBICSPLINE"}],
                "channels":[{"sampler":0,"target":{"node":0,"path":"weights"}}]}]
        }"#;
        let gltf = gltf::Gltf::from_slice(json.as_bytes()).unwrap();
        let values = [
            0.0f32, 1.0, 2.0, -1.0, -2.0, 0.1, 0.2, -3.0, -4.0, -5.0, -6.0, 0.3, 0.4, -7.0, -8.0,
            -9.0, -10.0, 0.5, 0.6, -11.0, -12.0,
        ];
        let buffers = [gltf::buffer::Data(
            values.into_iter().flat_map(f32::to_le_bytes).collect(),
        )];
        let data = parse_animation_data(&gltf.document, &buffers);
        let track = &data.animations[0].tracks[0];
        assert_eq!(track.times, [0.0, 1.0, 2.0]);
        // 三帧 × (入切线, 值, 出切线) × 2 个目标 —— 切线必须原样保留，
        // 降采样成 [0.1,0.2,0.3,0.4,0.5,0.6] 就等于把曲线拍平。
        #[rustfmt::skip]
        assert_eq!(
            track.values,
            [
                -1.0, -2.0, 0.1, 0.2, -3.0, -4.0,
                -5.0, -6.0, 0.3, 0.4, -7.0, -8.0,
                -9.0, -10.0, 0.5, 0.6, -11.0, -12.0,
            ]
        );
        // 段 [0,1] 中点：target0 = .5·0.1 + .125·(-3) + .5·0.3 - .125·(-5) = 0.45，
        // target1 同理 0.55；线性插值只会给 0.2 / 0.3。
        let mut out = Vec::new();
        assert!(crate::animation::sample_morph_weights(
            &data.animations[0],
            0.5,
            &mut out
        ));
        assert!((out[0] - 0.45).abs() < 1e-5, "target0 实得 {}", out[0]);
        assert!((out[1] - 0.55).abs() < 1e-5, "target1 实得 {}", out[1]);
    }

    /// 单节点、单平移通道的 CUBICSPLINE 合成资产。
    ///
    /// 值曲线是 `0 → 1 → 0`，只有第 0 帧带 2/秒的出切线，第 1 帧的入切线为 0。
    /// 段 [t0,t1] 中点上：真样条给 `0.5·p0 + 0.125·(2·dt) + 0.5·p1`，
    /// 线性插值给 `0.5·(p0+p1)` —— 两者差 0.25，不会互相混淆。
    fn cubic_translation_fixture(times: [f32; 3]) -> AnimData {
        let json = r#"{
            "asset":{"version":"2.0"}, "nodes":[{}],
            "buffers":[{"byteLength":120}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":12},
                           {"buffer":0,"byteOffset":12,"byteLength":108}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"SCALAR","min":[0],"max":[4]},
                         {"bufferView":1,"componentType":5126,"count":9,"type":"VEC3"}],
            "animations":[{"samplers":[{"input":0,"output":1,"interpolation":"CUBICSPLINE"}],
                "channels":[{"sampler":0,"target":{"node":0,"path":"translation"}}]}]
        }"#;
        let gltf = gltf::Gltf::from_slice(json.as_bytes()).unwrap();
        // 时间 0/1/2，随后每帧 (入切线, 值, 出切线) 三份 vec3。
        #[rustfmt::skip]
        let numbers = [
            times[0], times[1], times[2],
            0.0, 0.0, 0.0,   0.0, 0.0, 0.0,   0.0, 2.0, 0.0,
            0.0, 0.0, 0.0,   0.0, 1.0, 0.0,   0.0, 0.0, 0.0,
            0.0, 0.0, 0.0,   0.0, 0.0, 0.0,   0.0, 0.0, 0.0,
        ];
        let buffers = [gltf::buffer::Data(
            numbers.into_iter().flat_map(f32::to_le_bytes).collect(),
        )];
        parse_animation_data(&gltf.document, &buffers)
    }

    /// R04 / 032 的核心断言：CUBICSPLINE 的中点是 Hermite 值，不是线性值。
    ///
    /// 旧实现（读入时降采样成线性、丢切线）这里必然是 0.5 —— 这正是
    /// `Docs/差距分析.md` 里"Hermite 待做"那一条的红灯形式。
    #[test]
    fn regression_r04_cubic_spline_midpoint_is_hermite_not_linear() {
        use crate::animation::{default_pose, sample_clip_into};
        let data = cubic_translation_fixture([0.0, 1.0, 2.0]);
        let clip = &data.animations[0];
        let mut pose = default_pose(&data.nodes);
        sample_clip_into(clip, 0.5, &mut pose);
        let y = pose[0].translation.y;
        assert!(
            (y - 0.75).abs() < 1e-5,
            "段中点应为 Hermite 0.75（线性降采样会给 0.5），实得 {y}"
        );

        // 端点必须精确等于关键帧值，否则整段动画会整体漂移。
        let mut at_start = default_pose(&data.nodes);
        sample_clip_into(clip, 0.0, &mut at_start);
        assert_eq!(at_start[0].translation.y, 0.0, "首帧");
        let mut at_end = default_pose(&data.nodes);
        sample_clip_into(clip, 2.0, &mut at_end);
        assert_eq!(at_end[0].translation.y, 0.0, "末帧");
    }

    /// 切线单位是**每秒**（glTF 规范 C.2），段长 2 秒时 Hermite 里要乘 2。
    ///
    /// 忘了乘 dt 会把曲线抬得不够：段中点得 0.75 而不是 1.0。
    #[test]
    fn regression_r04_cubic_spline_scales_tangent_by_segment_length() {
        use crate::animation::{default_pose, sample_clip_into};
        let data = cubic_translation_fixture([0.0, 2.0, 4.0]);
        let clip = &data.animations[0];
        let mut pose = default_pose(&data.nodes);
        sample_clip_into(clip, 1.0, &mut pose);
        let y = pose[0].translation.y;
        assert!(
            (y - 1.0).abs() < 1e-5,
            "2 秒段的切线换算后中点应为 1.0，实得 {y}"
        );
    }

    /// 四元数的三次样条逐分量插值必然脱离单位球 —— 采样端必须归一化。
    ///
    /// 不归一化的话，蒙皮矩阵带一个恒定缩放，网格会随着动画越画越大。
    #[test]
    fn regression_r04_cubic_spline_rotation_is_normalized() {
        use crate::animation::{default_pose, sample_clip_into};
        let json = r#"{
            "asset":{"version":"2.0"}, "nodes":[{}],
            "buffers":[{"byteLength":160}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":12},
                           {"buffer":0,"byteOffset":12,"byteLength":144}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"SCALAR","min":[0],"max":[2]},
                         {"bufferView":1,"componentType":5126,"count":9,"type":"VEC4"}],
            "animations":[{"samplers":[{"input":0,"output":1,"interpolation":"CUBICSPLINE"}],
                "channels":[{"sampler":0,"target":{"node":0,"path":"rotation"}}]}]
        }"#;
        let gltf = gltf::Gltf::from_slice(json.as_bytes()).unwrap();
        // 0° → 90° → 0°（绕 Z 轴），出切线故意给大，逼出非单位长度的样条值。
        use std::f32::consts::FRAC_1_SQRT_2;
        #[rustfmt::skip]
        let numbers = [
            0.0f32, 1.0, 2.0,
            0.0, 0.0, 0.0, 0.0,   0.0, 0.0, 0.0, 1.0,   0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 0.0,   0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2,   0.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 0.0,   0.0, 0.0, 0.0, 1.0,   0.0, 0.0, 0.0, 0.0,
        ];
        let buffers = [gltf::buffer::Data(
            numbers.into_iter().flat_map(f32::to_le_bytes).collect(),
        )];
        let data = parse_animation_data(&gltf.document, &buffers);
        for t in [0.25f32, 0.5, 0.75, 1.25] {
            let mut pose = default_pose(&data.nodes);
            sample_clip_into(&data.animations[0], t, &mut pose);
            let len = pose[0].rotation.length();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "t={t} 的四元数长度应为 1，实得 {len}"
            );
        }
    }

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

    // -- 材质资产合并 -------------------------------------------------------

    /// 共享材质：颜色相乘、金属度 / 粗糙度替换、`blend` 不动。
    #[test]
    fn material_asset_multiplies_color_and_replaces_pbr() {
        let base = MaterialParams {
            base_color: [0.5, 0.5, 0.5, 0.8],
            metallic: 0.1,
            roughness: 0.9,
            blend: true,
        };
        let asset = xgame_core::MaterialAsset {
            base_color: [0.4, 1.0, 2.0, 0.5],
            metallic: 0.25,
            roughness: 0.7,
            albedo: String::new(),
        };
        let merged = base.with_asset(Some(&asset));
        assert_eq!(merged.base_color, [0.2, 0.5, 1.0, 0.4]);
        assert_eq!(merged.metallic, 0.25, "资产替换模型金属度");
        assert_eq!(merged.roughness, 0.7, "资产替换模型粗糙度");
        assert!(merged.blend, "透明与否是管线属性，资产不该改");

        let same = base.with_asset(None);
        assert_eq!(same.base_color, base.base_color);
        assert_eq!(same.metallic, base.metallic);
        assert_eq!(same.roughness, base.roughness);
    }

    /// `model_scale` 必须同时烘焙进节点位移、动画位移轨道与 IBM 平移：
    /// 只改几何的话，骨骼动画会把模型"拉回"原尺寸 / 错位。
    #[test]
    fn model_scale_bakes_into_nodes_tracks_and_bind_matrices() {
        let (doc, buffers) = fox();
        let base = parse_animation_data(&doc, &buffers);
        let scaled = parse_animation_data_scaled(&doc, &buffers, 2.0);

        assert_eq!(scaled.nodes.len(), base.nodes.len());
        for (a, b) in base.nodes.iter().zip(&scaled.nodes) {
            assert_eq!(b.translation, a.translation * 2.0, "节点位移必须缩放");
            assert_eq!(b.rotation, a.rotation, "旋转不受均匀缩放影响");
            assert_eq!(b.scale, a.scale, "节点自身缩放不受影响");
        }

        assert_eq!(scaled.skins.len(), base.skins.len());
        for (a, b) in base.skins.iter().zip(&scaled.skins) {
            for (m0, m1) in a.inverse_bind.iter().zip(&b.inverse_bind) {
                assert_eq!(
                    [m1.w_axis.x, m1.w_axis.y, m1.w_axis.z],
                    [m0.w_axis.x * 2.0, m0.w_axis.y * 2.0, m0.w_axis.z * 2.0],
                    "IBM 平移必须缩放"
                );
                assert_eq!(m1.x_axis, m0.x_axis, "IBM 线性部分不变");
                assert_eq!(m1.y_axis, m0.y_axis);
                assert_eq!(m1.z_axis, m0.z_axis);
            }
        }

        // 动画的 Translation 轨道：每个关键帧的三个分量都乘 2。
        let base_walk = base.animations.iter().find(|c| c.name == "Walk").unwrap();
        let scaled_walk = scaled.animations.iter().find(|c| c.name == "Walk").unwrap();
        let base_track = base_walk
            .tracks
            .iter()
            .find(|t| t.path == TrackPath::Translation)
            .expect("Walk 应含位移轨道");
        let scaled_track = scaled_walk
            .tracks
            .iter()
            .find(|t| t.path == TrackPath::Translation)
            .unwrap();
        assert_eq!(
            scaled_track.values,
            base_track
                .values
                .iter()
                .map(|v| v * 2.0)
                .collect::<Vec<_>>()
        );

        // 非法缩放（NaN / 0 / 负）退回 1.0，不能把模型弄没。
        let fallback = parse_animation_data_scaled(&doc, &buffers, f32::NAN);
        for (a, b) in base.nodes.iter().zip(&fallback.nodes) {
            assert_eq!(b.translation, a.translation);
        }
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
        assert_eq!(
            data.skins[0].inverse_bind.len(),
            24,
            "IBM 必须与关节数一一对应"
        );

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
            // `Track::stride` 现算（MorphWeights 的 stride 是逐轨道的目标数，
            // 不能像变换通道那样写死）；每帧在 `values` 里占 `frame_len` 个浮点
            // —— 样条是 3 倍（入切线 / 值 / 出切线）。
            assert_eq!(
                track.values.len(),
                track.times.len() * track.frame_len(),
                "轨道 {} 的值/时间必须对齐",
                track.node
            );
            assert!(
                track.times.windows(2).all(|w| w[0] <= w[1]),
                "关键帧时间必须单调不减"
            );
        }
    }

    /// runtime 的 CPU 装载器必须与 GPU 模型加载共用同一份解析与导入缩放。
    #[test]
    fn animation_loader_reads_fox_without_a_gpu() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/SampleScene");
        let loader = GltfAnimationLoader::new(&root, Vec::new());
        let asset = loader
            .load_animation("Resources/Fox.gltf")
            .expect("CPU 动画装载不应需要 GPU");
        assert_eq!(asset.clips.len(), 3);
        assert!(
            asset.node_index("b_Hip_01").is_some(),
            "根运动节点必须可按名找到"
        );
        assert!(asset.clip("Walk").is_some());
    }

    /// CPU 根运动与 GPU 网格必须命中 `search_paths` 里的同一个文件，并读取该文件
    /// 旁边的 `.meta`。旧 GPU 路径直接 `project_root.join(model)`，这里会回到不存在的
    /// `Resources/Fox.gltf`，同时把缩放误读成默认 1.0。
    #[test]
    fn model_source_search_paths_and_meta_match_cpu_loader() {
        struct TempRoot(PathBuf);
        impl Drop for TempRoot {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let root = TempRoot(std::env::temp_dir().join(format!(
            "xgame-model-source-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
        let fallback = root.0.join("Mods/Resources/Fox.gltf");
        std::fs::create_dir_all(fallback.parent().unwrap()).unwrap();
        std::fs::write(&fallback, b"model placeholder").unwrap();

        let mut meta = xgame_core::AssetMeta::fresh(xgame_core::AssetKind::Model);
        meta.import.model_scale = 2.5;
        std::fs::write(fallback.with_extension("gltf.meta"), meta.to_ron().unwrap()).unwrap();

        let search_paths = vec!["Mods".to_owned()];
        let (path, settings) =
            resolve_model_source(&root.0, "Resources/Fox.gltf", &search_paths).unwrap();
        assert_eq!(path, fallback, "GPU 与 CPU 必须命中同一个模型文件");
        assert_eq!(settings.model_scale, 2.5, "必须读取命中文件旁的 .meta");
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
            assert!(
                (pose[i].translation - node.translation).length() < 1e-6,
                "节点 {i} 位移被改动"
            );
            // 逐分量比，不用 angle_between：资产里的四元数常常只是"近似归一"
            // （1e-6 量级的模长误差会被 acos 放大成 ~1e-3 弧度的假差异），
            // 而 default_pose 是**原样拷贝**，逐位相等才是它该有的承诺。
            assert_eq!(
                pose[i].rotation.to_array(),
                node.rotation.to_array(),
                "节点 {i} 旋转被改动"
            );
            assert!(
                (pose[i].scale - node.scale).length() < 1e-6,
                "节点 {i} 缩放被改动"
            );
        }
    }

    // -- 形变目标（morph target）--------------------------------------------

    /// 最小 base64 编码器。
    ///
    /// 测试要把顶点数据塞进 `data:` URI，而本 crate 只间接依赖 base64
    /// （gltf 的 `import` 特性内部用），拿不到它。17 行自造一个比加依赖干净。
    fn base64(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(TABLE[(n >> 18) as usize & 63] as char);
            out.push(TABLE[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                TABLE[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                TABLE[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    /// 合成一个带两个形变目标的三角形 glTF（内嵌缓冲，不碰磁盘）。
    ///
    /// 为什么要合成而不是用现成资产：仓库里的 `Fox.glb` 没有 morph 目标，
    /// 而"目标名 / 默认权重 / 增量有没有乘导入缩放"这三件事必须能在
    /// **没有显卡**的 CI 上断言。
    pub(crate) fn morph_gltf() -> String {
        // 三个顶点：原点 / X 单位 / Y 单位。
        let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let smile = [[0.1f32, 0.2, 0.3]; 3];
        let frown = [[0.0f32, 0.0, 0.5]; 3];
        let mut bytes = Vec::with_capacity(108);
        for p in positions.iter().chain(smile.iter()).chain(frown.iter()) {
            for value in p {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let template = r#"{
  "asset": {"version": "2.0"},
  "scene": 0,
  "scenes": [{"nodes": [0]}],
  "nodes": [{"mesh": 0}],
  "meshes": [{
    "name": "Face",
    "extras": {"targetNames": ["Smile", "Frown"]},
    "weights": [0.25, 0.0],
    "primitives": [{
      "attributes": {"POSITION": 0},
      "targets": [{"POSITION": 1}, {"POSITION": 2}]
    }]
  }],
  "accessors": [
    {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]},
    {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
    {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC3"}
  ],
  "bufferViews": [
    {"buffer": 0, "byteOffset": 0, "byteLength": 36},
    {"buffer": 0, "byteOffset": 36, "byteLength": 36},
    {"buffer": 0, "byteOffset": 72, "byteLength": 36}
  ],
  "buffers": [{"byteLength": 108, "uri": "data:application/octet-stream;base64,%B64%"}]
}"#;
        template
            .replace("%B64%", &base64(&bytes))
            .replace("%LEN%", &bytes.len().to_string())
    }

    /// 三个基准顶点（与合成资产的 POSITION 一致）。
    fn morph_base_vertices() -> Vec<GltfVertex> {
        [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
            .into_iter()
            .map(|position| GltfVertex {
                position,
                normal: [0.0, 0.0, 1.0],
                color: [1.0; 3],
                tex_coord: [0.0; 2],
                joints: [0.0; 4],
                weights: [1.0, 0.0, 0.0, 0.0],
            })
            .collect()
    }

    /// 目标名 / 默认权重 / 目标数都要从资产里**读出来**，不能靠调用方猜。
    #[test]
    fn morph_targets_carry_names_defaults_and_count() {
        let json = morph_gltf();
        let (document, buffers, _images) =
            gltf::import_slice(json.as_bytes()).expect("合成 glTF 应能解析");
        let mesh = document.meshes().next().expect("一个 mesh");
        let primitive = mesh.primitives().next().expect("一个图元");
        let base = morph_base_vertices();

        let morph = parse_morph_targets(&mesh, &primitive, &buffers, &base, 1.0)
            .expect("这个图元带两个形变目标");
        assert_eq!(morph.names(), ["Smile", "Frown"]);
        assert_eq!(morph.defaults(), [0.25, 0.0], "mesh.weights 应是默认权重");
        assert_eq!(morph.target_count(), 2);
        assert_eq!(morph.vertex_count(), 3);
    }

    /// 增量必须乘上 `model_scale` —— 与顶点位置同一套烘焙规则。
    ///
    /// 不乘的话，把导入缩放从 1 改成 2，模型会大一倍而表情幅度纹丝不动，
    /// 看起来像"表情失效了"。
    #[test]
    fn morph_deltas_are_baked_with_model_scale() {
        let json = morph_gltf();
        let (document, buffers, _images) = gltf::import_slice(json.as_bytes()).unwrap();
        let mesh = document.meshes().next().unwrap();
        let primitive = mesh.primitives().next().unwrap();
        let base = morph_base_vertices();

        let doubled = parse_morph_targets(&mesh, &primitive, &buffers, &base, 2.0).unwrap();
        let deltas = doubled.position_deltas();
        // target-major：第 0 个目标的前三个分量就是顶点 0 的增量。
        assert_eq!(deltas[0], [0.2, 0.4, 0.6], "增量应已乘 2");
        // 第 1 个目标（Frown）从下标 1 * 3 = 3 开始（3 个顶点 × 前一个目标）。
        assert_eq!(deltas[3], [0.0, 0.0, 1.0], "第二个目标不该串到第一个上");

        let plain = parse_morph_targets(&mesh, &primitive, &buffers, &base, 1.0).unwrap();
        assert_eq!(plain.position_deltas()[0], [0.1, 0.2, 0.3], "缩放 1 时原样");
    }

    /// 没有 targets 的网格不该凭空长出一个空 `MorphTargets`。
    #[test]
    fn mesh_without_targets_has_no_morph() {
        let json = morph_gltf().replace(
            r#""targets": [{"POSITION": 1}, {"POSITION": 2}]"#,
            r#""targets": []"#,
        );
        let (document, buffers, _images) = gltf::import_slice(json.as_bytes()).unwrap();
        let mesh = document.meshes().next().unwrap();
        let primitive = mesh.primitives().next().unwrap();
        let base = morph_base_vertices();
        assert!(
            parse_morph_targets(&mesh, &primitive, &buffers, &base, 1.0).is_none(),
            "没有形变目标的图元应返回 None"
        );
    }
    /// 合成一个 300 骨蒙皮的三角形 glTF（内嵌缓冲，不碰磁盘）。
    ///
    /// 骨槽固定 256 个位置（`BONES_PER_SLOT`）：三个顶点的影响分别跨在
    /// 边界上 —— v0 = [255, 256] 各 0.5（一半越界）、v1 = 四个全越界、
    /// v2 = 只在界内；动画平移节点 255（界内最后一根骨）。超限策略要求
    /// 蒙皮截断到 256 根、越界影响清零（渲染侧回归见 screenshot.rs 的
    /// `oversized_skin_influences_freeze_instead_of_leaking_into_the_next_slot`）。
    pub(crate) fn oversized_skin_gltf() -> String {
        let mut bytes: Vec<u8> = Vec::new();
        // POSITION：原点 / X 单位 / Y 单位。
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for v in p {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        // NORMAL：都朝 +Z（相机在 +Z 侧看得见正面）。
        for _ in 0..3 {
            for v in [0.0f32, 0.0, 1.0] {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        // JOINTS_0（u16×4）。
        for row in [[255u16, 256, 0, 0], [256, 257, 298, 299], [255, 0, 0, 0]] {
            for v in row {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        // WEIGHTS_0（f32×4）。
        for row in [
            [0.5f32, 0.5, 0.0, 0.0],
            [0.25, 0.25, 0.25, 0.25],
            [1.0, 0.0, 0.0, 0.0],
        ] {
            for v in row {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        // 索引（u16×3），之后补 2 字节让 f32 对齐到 4。
        for v in [0u16, 1, 2] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(&[0u8; 2]);
        // 动画时间轴（f32×2）。
        for v in [0.0f32, 1.0] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        // 动画值：节点 255 的平移 (0,0,0) → (1,0,0)。
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0]] {
            for v in p {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }

        let nodes = (0..300)
            .map(|_| r#"{"translation": [0.0, 0.0, 0.0]}"#)
            .chain(std::iter::once(r#"{"mesh": 0, "skin": 0}"#))
            .collect::<Vec<_>>()
            .join(",");
        let joint_list = (0..300)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let template = r#"{
  "asset": {"version": "2.0"},
  "scene": 0,
  "scenes": [{"nodes": [300, 0]}],
  "nodes": [%NODES%],
  "skins": [{"joints": [%JOINTS%]}],
  "animations": [{
    "name": "move",
    "channels": [{"sampler": 0, "target": {"node": 255, "path": "translation"}}],
    "samplers": [{"input": 5, "output": 6, "interpolation": "LINEAR"}]
  }],
  "meshes": [{
    "primitives": [{
      "attributes": {"POSITION": 0, "NORMAL": 1, "JOINTS_0": 2, "WEIGHTS_0": 3},
      "indices": 4
    }]
  }],
  "accessors": [
    {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]},
    {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
    {"bufferView": 2, "componentType": 5123, "count": 3, "type": "VEC4"},
    {"bufferView": 3, "componentType": 5126, "count": 3, "type": "VEC4"},
    {"bufferView": 4, "componentType": 5123, "count": 3, "type": "SCALAR"},
    {"bufferView": 5, "componentType": 5126, "count": 2, "type": "SCALAR", "min": [0.0], "max": [1.0]},
    {"bufferView": 6, "componentType": 5126, "count": 2, "type": "VEC3"}
  ],
  "bufferViews": [
    {"buffer": 0, "byteOffset": 0, "byteLength": 36},
    {"buffer": 0, "byteOffset": 36, "byteLength": 36},
    {"buffer": 0, "byteOffset": 72, "byteLength": 24},
    {"buffer": 0, "byteOffset": 96, "byteLength": 48},
    {"buffer": 0, "byteOffset": 144, "byteLength": 6},
    {"buffer": 0, "byteOffset": 152, "byteLength": 8},
    {"buffer": 0, "byteOffset": 160, "byteLength": 24}
  ],
  "buffers": [{"byteLength": %LEN%, "uri": "data:application/octet-stream;base64,%B64%"}]
}"#;
        template
            .replace("%NODES%", &nodes)
            .replace("%JOINTS%", &joint_list)
            .replace("%B64%", &base64(&bytes))
            .replace("%LEN%", &bytes.len().to_string())
    }

    /// 300 骨的蒙皮必须**截断到骨槽容量**：`Skin` 的关节表与逆绑定矩阵都
    /// 只留前 256 根（顶点影响的钳制是另一半，见 `clamp_skin_influences`）。
    /// 不截断的话 `bones[k]` 与顶点下标对不上号，超出部分还会顶到邻槽。
    #[test]
    fn oversized_skin_truncates_to_the_bone_slot_capacity() {
        let path =
            std::env::temp_dir().join(format!("xgame-oversized-trunc-{}.gltf", std::process::id()));
        std::fs::write(&path, oversized_skin_gltf()).unwrap();
        let (doc, buffers, _) = gltf::import(&path).expect("合成超限骨架应能解析");
        std::fs::remove_file(&path).unwrap();
        let data = parse_animation_data(&doc, &buffers);
        assert_eq!(data.skins.len(), 1);
        assert_eq!(data.skins[0].joints.len(), 256, "关节表必须截断到骨槽容量");
        assert_eq!(
            data.skins[0].inverse_bind.len(),
            256,
            "逆绑定矩阵必须与关节表同步截断"
        );
    }

    /// 越界关节影响的清零规则（固定容量超限策略的顶点侧一半）：
    /// 下标 ≥ 骨槽容量的影响权重清零、下标归 0 占位；界内影响原样保留。
    /// 全越界 = 全清零，shader 的 `wsum == 0` 分支会把顶点冻结在绑定姿态。
    #[test]
    fn clamp_skin_influences_zeroes_out_of_slot_indices() {
        let mut mixed = GltfVertex {
            position: [0.0; 3],
            normal: [0.0, 1.0, 0.0],
            color: [1.0; 3],
            tex_coord: [0.0; 2],
            joints: [255.0, 256.0, 300.0, 0.0],
            weights: [0.4, 0.3, 0.2, 0.1],
        };
        mixed.clamp_skin_influences();
        assert_eq!(mixed.joints, [255.0, 0.0, 0.0, 0.0], "越界下标归 0 占位");
        assert_eq!(mixed.weights, [0.4, 0.0, 0.0, 0.1], "只清越界的权重");

        let mut frozen = GltfVertex {
            position: [0.0; 3],
            normal: [0.0, 1.0, 0.0],
            color: [1.0; 3],
            tex_coord: [0.0; 2],
            joints: [256.0, 257.0, 298.0, 299.0],
            weights: [0.25; 4],
        };
        frozen.clamp_skin_influences();
        assert_eq!(
            frozen.weights, [0.0; 4],
            "全越界 = 全清零（冻结在绑定姿态）"
        );

        let mut kept = GltfVertex {
            position: [0.0; 3],
            normal: [0.0, 1.0, 0.0],
            color: [1.0; 3],
            tex_coord: [0.0; 2],
            joints: [0.0, 1.0, 254.0, 255.0],
            weights: [0.25; 4],
        };
        kept.clamp_skin_influences();
        assert_eq!(kept.joints, [0.0, 1.0, 254.0, 255.0], "界内影响不动");
        assert_eq!(kept.weights, [0.25; 4]);
    }
}
