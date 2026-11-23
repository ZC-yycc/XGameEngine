//! 内建网格：`Mesh` 保存首个图元的几何、PBR 基色因子、UV 与基色贴图。

use super::texture_data::TextureData;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkinData {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub joint_nodes: Vec<usize>,
    pub inverse_bind: Vec<[f32; 16]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    /// glTF `baseColorFactor`；逐实体颜色在提交 draw 时与它相乘。
    pub base_color_factor: [f32; 4],
    /// 模型材质的金属度；实体没有 `render.material` 时它就是有效值。
    ///
    /// glTF 规范缺省 1.0（旧端 `read_material` 照 glTF spec 取），内建图元与
    /// 手工构造的网格用旧端 `MaterialParams::default()` 的 0.0。
    pub metallic: f32,
    /// 模型材质的粗糙度；实体没有 `render.material` 时它就是有效值。
    ///
    /// glTF 规范缺省 1.0，内建图元用旧端 `MaterialParams::default()` 的 0.45
    /// （旧端内建图元的 `material` 字段就是那个 default）。
    pub roughness: f32,
    pub skin: Option<SkinData>,
    /// 每个顶点的 UV（`TEXCOORD_0`）；空 = 没有 UV，采样坐标为 (0,0)。
    pub uvs: Vec<[f32; 2]>,
    /// 每个顶点的法线（glTF `NORMAL`，模型空间、不保证归一）。
    ///
    /// glTF 模型没导 `NORMAL` 时，加载期按几何生成面积加权的顶点法线
    /// （`glb/normals.rs::face_normals`，与旧端 `compute_face_normals` 同式）；
    /// 空只出现在不是 glTF 的来源上（内建图元 / 手工构造），片元着色退回屏幕
    /// 导数叉积 —— 两者在折角处观感不同：导数只看当前像素所在三角面（硬边），
    /// 顶点法线是插值的（旧端就是这么平滑的）。
    ///
    /// 有法线时必须用逆转置矩阵变换（非均匀缩放会把法线拉歪），旧端
    /// `scene_pipeline.rs` 的 `normal_mat = transpose(mat3_inverse(model))` 同式。
    pub normals: Vec<[f32; 3]>,
    /// glTF 基色贴图（旧引擎 PBR 的 albedo 贴图）；`None` = 纯色。
    pub texture: Option<std::sync::Arc<TextureData>>,
}

impl Mesh {
    pub fn new(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Self {
        Self {
            positions,
            indices,
            base_color_factor: [1.0; 4],
            // 旧端内建图元 / 手工构网格的材质参数都取 `MaterialParams::default()`。
            metallic: 0.0,
            roughness: 0.45,
            skin: None,
            uvs: Vec::new(),
            normals: Vec::new(),
            texture: None,
        }
    }

    pub fn fullscreen_triangle() -> Self {
        Self::new(
            vec![[-1.0, -1.0, 0.0], [3.0, -1.0, 0.0], [-1.0, 3.0, 0.0]],
            Vec::new(),
        )
    }

    pub fn quad() -> Self {
        Self::new(
            vec![
                [-0.5, -0.5, 0.0],
                [0.5, -0.5, 0.0],
                [0.5, 0.5, 0.0],
                [-0.5, 0.5, 0.0],
            ],
            vec![0, 1, 2, 0, 2, 3],
        )
    }

    /// 模型材质因子与实体材质都是乘法因子，顺序不改变结果。
    pub fn tint(&self, entity_color: [f32; 4]) -> [f32; 4] {
        std::array::from_fn(|index| self.base_color_factor[index] * entity_color[index])
    }

    /// 内建图元的位置 / 法线 / UV 三流一次构造。
    ///
    /// 旧端 `BuiltinMesh::vertex` 还会填顶点色白与关节占位；XGC 的 `Mesh` 没有
    /// 这两列（颜色由实体 / 材质给，蒙皮只属于 glTF 模型），所以只搬几何属性。
    fn textured(
        positions: Vec<[f32; 3]>,
        indices: Vec<u32>,
        normals: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
    ) -> Self {
        Self {
            normals,
            uvs,
            ..Self::new(positions, indices)
        }
    }

    /// 与旧端 `BuiltinMesh::cube(1.0)` 逐顶点一致：每面 4 个角、共享面法线、带 UV。
    ///
    /// 24 顶点而不是 8 个复用角点：法线 / UV 在棱角上不连续，旧端就是靠重复顶点
    /// 才能同时得到硬边法线与正确的 UV；片元着色优先用顶点法线，棱角才不糊。
    pub fn cube() -> Self {
        const HALF: f32 = 0.5;
        // (面法线, 面内 u 轴, 面内 v 轴)，顺序与旧端 `cube()` 一致。
        const FACES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
            ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
            ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
            ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ];
        // (u 方向, v 方向, UV)，顺序与旧端 `cube()` 的四角一致。
        const CORNERS: [([f32; 2], [f32; 2]); 4] = [
            ([-1.0, -1.0], [0.0, 0.0]),
            ([1.0, -1.0], [1.0, 0.0]),
            ([1.0, 1.0], [1.0, 1.0]),
            ([-1.0, 1.0], [0.0, 1.0]),
        ];
        let mut positions = Vec::with_capacity(24);
        let mut normals = Vec::with_capacity(24);
        let mut uvs = Vec::with_capacity(24);
        let mut indices = Vec::with_capacity(36);
        for (normal, axis_u, axis_v) in FACES {
            let base = positions.len() as u32;
            for (corner, uv) in CORNERS {
                positions.push([
                    normal[0] * HALF + axis_u[0] * HALF * corner[0] + axis_v[0] * HALF * corner[1],
                    normal[1] * HALF + axis_u[1] * HALF * corner[0] + axis_v[1] * HALF * corner[1],
                    normal[2] * HALF + axis_u[2] * HALF * corner[0] + axis_v[2] * HALF * corner[1],
                ]);
                normals.push(normal);
                uvs.push(uv);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        Self::textured(positions, indices, normals, uvs)
    }

    /// 与旧端 `BuiltinMesh::plane(1.0)` 逐顶点一致：+Y 法线、UV 铺满四角。
    pub fn plane() -> Self {
        const HALF: f32 = 0.5;
        let positions = vec![
            [-HALF, 0.0, HALF],
            [HALF, 0.0, HALF],
            [HALF, 0.0, -HALF],
            [-HALF, 0.0, -HALF],
        ];
        let normals = vec![[0.0, 1.0, 0.0]; 4];
        let uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        Self::textured(positions, vec![0, 1, 2, 0, 2, 3], normals, uvs)
    }

    /// 与旧端 `BuiltinMesh::sphere(0.5, 24, 32)` 逐顶点一致：24 圈 × 32 段，
    /// 顶点法线就是归一化位置（平滑着色），UV 按经纬度铺满 0..1。
    ///
    /// 环数 / 段数与旧端相同才有相同的轮廓与高光形状；少一圈就重新出现分面感。
    pub fn sphere() -> Self {
        const RADIUS: f32 = 0.5;
        const RINGS: usize = 24;
        const SECTORS: usize = 32;
        let stride = SECTORS + 1;
        let mut positions = Vec::with_capacity((RINGS + 1) * stride);
        let mut normals = Vec::with_capacity((RINGS + 1) * stride);
        let mut uvs = Vec::with_capacity((RINGS + 1) * stride);
        for ring in 0..=RINGS {
            let phi = std::f32::consts::PI * ring as f32 / RINGS as f32;
            for sector in 0..=SECTORS {
                let theta = std::f32::consts::TAU * sector as f32 / SECTORS as f32;
                let normal = [phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin()];
                positions.push([normal[0] * RADIUS, normal[1] * RADIUS, normal[2] * RADIUS]);
                normals.push(normal);
                uvs.push([sector as f32 / SECTORS as f32, ring as f32 / RINGS as f32]);
            }
        }
        let mut indices = Vec::with_capacity(RINGS * SECTORS * 6);
        for ring in 0..RINGS {
            for sector in 0..SECTORS {
                let a = (ring * stride + sector) as u32;
                let b = a + stride as u32;
                indices.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
            }
        }
        Self::textured(positions, indices, normals, uvs)
    }

    /// 与旧端 `BuiltinMesh::cylinder(0.5, 1.0, 32)` 逐顶点一致：32 段侧面（径向
    /// 平滑法线）+ 顶 / 底两个三角扇盖（平面 ±Y 法线），全部带 UV。
    pub fn cylinder() -> Self {
        const RADIUS: f32 = 0.5;
        const HEIGHT: f32 = 1.0;
        const SEGMENTS: usize = 32;
        let half_h = HEIGHT * 0.5;
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        for segment in 0..=SEGMENTS {
            let theta = std::f32::consts::TAU * segment as f32 / SEGMENTS as f32;
            let normal = [theta.cos(), 0.0, theta.sin()];
            let u = segment as f32 / SEGMENTS as f32;
            positions.push([normal[0] * RADIUS, half_h, normal[2] * RADIUS]);
            normals.push(normal);
            uvs.push([u, 0.0]);
            positions.push([normal[0] * RADIUS, -half_h, normal[2] * RADIUS]);
            normals.push(normal);
            uvs.push([u, 1.0]);
        }
        for segment in 0..SEGMENTS {
            let top0 = (segment * 2) as u32;
            let bot0 = top0 + 1;
            let top1 = top0 + 2;
            let bot1 = top0 + 3;
            indices.extend_from_slice(&[top0, top1, bot0, top1, bot1, bot0]);
        }
        let top_center = positions.len() as u32;
        positions.push([0.0, half_h, 0.0]);
        normals.push([0.0, 1.0, 0.0]);
        uvs.push([0.5, 0.5]);
        for segment in 0..=SEGMENTS {
            let theta = std::f32::consts::TAU * segment as f32 / SEGMENTS as f32;
            positions.push([theta.cos() * RADIUS, half_h, theta.sin() * RADIUS]);
            normals.push([0.0, 1.0, 0.0]);
            uvs.push([theta.cos() * 0.5 + 0.5, theta.sin() * 0.5 + 0.5]);
        }
        for segment in 0..SEGMENTS {
            let a = top_center + 1 + segment as u32;
            let b = a + 1;
            indices.extend_from_slice(&[top_center, b, a]);
        }
        let bottom_center = positions.len() as u32;
        positions.push([0.0, -half_h, 0.0]);
        normals.push([0.0, -1.0, 0.0]);
        uvs.push([0.5, 0.5]);
        for segment in 0..=SEGMENTS {
            let theta = std::f32::consts::TAU * segment as f32 / SEGMENTS as f32;
            positions.push([theta.cos() * RADIUS, -half_h, theta.sin() * RADIUS]);
            normals.push([0.0, -1.0, 0.0]);
            uvs.push([theta.cos() * 0.5 + 0.5, 1.0 - (theta.sin() * 0.5 + 0.5)]);
        }
        for segment in 0..SEGMENTS {
            let a = bottom_center + 1 + segment as u32;
            let b = a + 1;
            indices.extend_from_slice(&[bottom_center, a, b]);
        }
        Self::textured(positions, indices, normals, uvs)
    }
}

/// 已知 builtin 名称返回占位网格；未知模型返回 None，由调用方决定跳过或兜底。
pub fn builtin_mesh(model: &str) -> Option<Mesh> {
    match model {
        "builtin://quad" => Some(Mesh::quad()),
        "builtin://triangle" => Some(Mesh::new(
            vec![[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.0, 0.5, 0.0]],
            vec![0, 1, 2],
        )),
        "builtin://cube" => Some(Mesh::cube()),
        "builtin://sphere" | "builtin://capsule" => Some(Mesh::sphere()),
        "builtin://plane" => Some(Mesh::plane()),
        "builtin://cylinder" => Some(Mesh::cylinder()),
        _ => None,
    }
}
