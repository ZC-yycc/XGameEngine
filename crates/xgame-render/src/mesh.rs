//! 网格数据与内置几何体。
//!
//! 文件分两部分：
//!
//! 1. [`Mesh`]——已经上传到 GPU 的网格（历史遗留的最小封装）。
//! 2. **内置基础图元**——Cube / Sphere / Plane / Cylinder 的 CPU 端几何生成，
//!    对应商业引擎 `GameObject > 3D Object` 菜单里那几个"开箱即用的形状"。
//!
//! 内置图元不依赖任何外部资产文件：用 [`BuiltinMesh::uri`] 生成
//! `builtin://cube` 这样的标识写进 `Entity::model`，加载时由
//! [`GltfMesh::load`](crate::gltf_loader::GltfMesh::load) 分派到几何生成器。
//! 这样"新建一个立方体"就是纯内存操作，不用先往磁盘写模型文件。

use wgpu::util::DeviceExt;

use crate::gltf_loader::GltfVertex;

/// GPU 驻留网格，包含顶点和索引缓冲区。
pub struct Mesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub num_indices: u32,
    pub index_format: wgpu::IndexFormat,
}

impl Mesh {
    /// 上传顶点和索引数据到 GPU。
    pub fn new(
        device: &wgpu::Device,
        label: &str,
        vertices: &[u8],
        indices: &[u32],
        _vertex_size: u64,
    ) -> Self {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{} vertex buffer", label)),
            contents: vertices,
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_data: &[u8] = bytemuck::cast_slice(indices);
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{} index buffer", label)),
            contents: index_data,
            usage: wgpu::BufferUsages::INDEX,
        });

        Self {
            vertex_buffer,
            index_buffer,
            num_indices: indices.len() as u32,
            index_format: wgpu::IndexFormat::Uint32,
        }
    }
}

// ---------------------------------------------------------------------------
// 内置基础图元
// ---------------------------------------------------------------------------

/// CPU 端的顶点 + 索引数据，等着被上传成 [`Mesh`] / glTF 图元。
pub struct MeshData {
    pub vertices: Vec<GltfVertex>,
    pub indices: Vec<u32>,
}

/// 引擎内置的基础几何体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinMesh {
    Cube,
    Sphere,
    Plane,
    Cylinder,
}

impl BuiltinMesh {
    /// `Entity::model` 里表示"这是内置网格"的 URI 前缀。
    ///
    /// 用 URI 风格而不是裸名字（`cube`），是为了和项目相对路径彻底区分开：
    /// 以后项目里真出现一个叫 `cube` 的目录，也不会撞上。
    pub const URI_PREFIX: &'static str = "builtin://";

    /// 创建菜单里列出来的全部内置图元（顺序即菜单顺序）。
    pub const ALL: [BuiltinMesh; 4] = [
        BuiltinMesh::Cube,
        BuiltinMesh::Sphere,
        BuiltinMesh::Plane,
        BuiltinMesh::Cylinder,
    ];

    /// URI 里的短标识。
    pub fn slug(self) -> &'static str {
        match self {
            BuiltinMesh::Cube => "cube",
            BuiltinMesh::Sphere => "sphere",
            BuiltinMesh::Plane => "plane",
            BuiltinMesh::Cylinder => "cylinder",
        }
    }

    /// 新建实体时的默认显示名。
    pub fn label(self) -> &'static str {
        match self {
            BuiltinMesh::Cube => "Cube",
            BuiltinMesh::Sphere => "Sphere",
            BuiltinMesh::Plane => "Plane",
            BuiltinMesh::Cylinder => "Cylinder",
        }
    }

    /// `builtin://cube` 形式的模型标识。
    pub fn uri(self) -> String {
        format!("{}{}", Self::URI_PREFIX, self.slug())
    }

    /// 判断一个 `Entity::model` 是不是内置网格，是的话解析出是哪一种。
    ///
    /// 认不出来返回 `None`，调用方按普通文件路径处理——宁可加载失败报个错，
    /// 也不要把不认识的字符串当成内置标识，那样错误信息会很难懂。
    pub fn from_model_path(model: &str) -> Option<Self> {
        let slug = model.strip_prefix(Self::URI_PREFIX)?;
        Self::ALL.into_iter().find(|m| m.slug() == slug)
    }

    /// 生成几何数据。单位尺寸（边长/直径 1），大小由 `Transform::scale` 控制。
    pub fn build(self) -> MeshData {
        match self {
            BuiltinMesh::Cube => cube(1.0),
            BuiltinMesh::Sphere => sphere(0.5, 24, 32),
            BuiltinMesh::Plane => plane(1.0),
            BuiltinMesh::Cylinder => cylinder(0.5, 1.0, 32),
        }
    }
}

/// 造一个顶点。
///
/// 顶点色统一给白色：内置图元不带固有颜色，颜色应该由材质给——否则
/// 一旦以后接上材质编辑，这个顶点色会像牛皮糖一样粘着改不掉。
fn vertex(position: [f32; 3], normal: [f32; 3], tex_coord: [f32; 2]) -> GltfVertex {
    GltfVertex {
        position,
        normal,
        color: [1.0, 1.0, 1.0],
        tex_coord,
        // 内置图元永远不蒙皮：关节占位全 0、权重全给 0 号 —— 着色器按
        // `params.x == 0` 走原始顶点分支，这两个通道根本不会被读。
        joints: [0.0; 4],
        weights: [1.0, 0.0, 0.0, 0.0],
    }
}

/// 单位立方体（边长 `size`，中心在原点）。
///
/// 每个面用「法线 + 两个切向」描述，顶点由 `center ± u ± v` 算出来。
/// 这样只要保证 `u × v == n`，四个角天然就是逆时针（CCW）绕序——
/// 手写 24 个坐标最容易在绕序上翻车，而绕序错了面就被背面剔掉，
/// 表现是"模型有一半是空的"，非常难查。
pub fn cube(size: f32) -> MeshData {
    let h = size * 0.5;

    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];

    // 四个角的 (切向系数 du/dv, 纹理坐标)。顺序即 CCW 顺序。
    let corners: [([f32; 2], [f32; 2]); 4] = [
        ([-1.0, -1.0], [0.0, 0.0]),
        ([1.0, -1.0], [1.0, 0.0]),
        ([1.0, 1.0], [1.0, 1.0]),
        ([-1.0, 1.0], [0.0, 1.0]),
    ];

    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);

    for (n, u, v) in faces {
        let base = vertices.len() as u32;
        for (d, uv) in corners {
            vertices.push(vertex(
                [
                    n[0] * h + u[0] * h * d[0] + v[0] * h * d[1],
                    n[1] * h + u[1] * h * d[0] + v[1] * h * d[1],
                    n[2] * h + u[2] * h * d[0] + v[2] * h * d[1],
                ],
                n,
                uv,
            ));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    MeshData { vertices, indices }
}

/// UV 球体。
pub fn sphere(radius: f32, rings: u32, sectors: u32) -> MeshData {
    let rings = rings.max(2);
    let sectors = sectors.max(3);

    let mut vertices = Vec::with_capacity(((rings + 1) * (sectors + 1)) as usize);
    let mut indices = Vec::new();

    // phi：0 = 北极，π = 南极；theta：绕 Y 轴一圈
    for r in 0..=rings {
        let phi = std::f32::consts::PI * r as f32 / rings as f32;
        let (sin_phi, cos_phi) = phi.sin_cos();
        for s in 0..=sectors {
            let theta = std::f32::consts::TAU * s as f32 / sectors as f32;
            let (sin_t, cos_t) = theta.sin_cos();
            let n = [sin_phi * cos_t, cos_phi, sin_phi * sin_t];
            vertices.push(vertex(
                [n[0] * radius, n[1] * radius, n[2] * radius],
                n,
                [s as f32 / sectors as f32, r as f32 / rings as f32],
            ));
        }
    }

    let stride = sectors + 1;
    for r in 0..rings {
        for s in 0..sectors {
            let a = r * stride + s;
            let b = a + stride;
            indices.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
        }
    }

    MeshData { vertices, indices }
}

/// XZ 平面上的正方形（边长 `size`，中心在原点），法线朝 +Y。
pub fn plane(size: f32) -> MeshData {
    let h = size * 0.5;
    let n = [0.0, 1.0, 0.0];
    let corners: [([f32; 3], [f32; 2]); 4] = [
        ([-h, 0.0, h], [0.0, 0.0]),
        ([h, 0.0, h], [1.0, 0.0]),
        ([h, 0.0, -h], [1.0, 1.0]),
        ([-h, 0.0, -h], [0.0, 1.0]),
    ];
    let vertices = corners.into_iter().map(|(p, uv)| vertex(p, n, uv)).collect();
    MeshData {
        vertices,
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}

/// 圆柱体（底面半径 `radius`，高 `height`，中心在原点）。
pub fn cylinder(radius: f32, height: f32, segments: u32) -> MeshData {
    let segments = segments.max(3);
    let half_h = height * 0.5;

    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    // --- 侧面：法线沿径向，上下各一圈 ---
    for i in 0..=segments {
        let theta = std::f32::consts::TAU * i as f32 / segments as f32;
        let (sin_t, cos_t) = theta.sin_cos();
        let n = [cos_t, 0.0, sin_t];
        let u = i as f32 / segments as f32;
        vertices.push(vertex([cos_t * radius, half_h, sin_t * radius], n, [u, 0.0]));
        vertices.push(vertex([cos_t * radius, -half_h, sin_t * radius], n, [u, 1.0]));
    }
    for i in 0..segments {
        let top0 = i * 2;
        let bot0 = top0 + 1;
        let top1 = top0 + 2;
        let bot1 = top0 + 3;
        indices.extend_from_slice(&[top0, top1, bot0, top1, bot1, bot0]);
    }

    // --- 顶盖：法线 +Y，从上方看逆时针 ---
    let top_center = vertices.len() as u32;
    vertices.push(vertex([0.0, half_h, 0.0], [0.0, 1.0, 0.0], [0.5, 0.5]));
    for i in 0..=segments {
        let theta = std::f32::consts::TAU * i as f32 / segments as f32;
        let (sin_t, cos_t) = theta.sin_cos();
        vertices.push(vertex(
            [cos_t * radius, half_h, sin_t * radius],
            [0.0, 1.0, 0.0],
            [cos_t * 0.5 + 0.5, sin_t * 0.5 + 0.5],
        ));
    }
    for i in 0..segments {
        let a = top_center + 1 + i;
        let b = a + 1;
        indices.extend_from_slice(&[top_center, b, a]);
    }

    // --- 底盖：法线 -Y，从下方看逆时针（等价于从上方看顺时针）---
    let bottom_center = vertices.len() as u32;
    vertices.push(vertex([0.0, -half_h, 0.0], [0.0, -1.0, 0.0], [0.5, 0.5]));
    for i in 0..=segments {
        let theta = std::f32::consts::TAU * i as f32 / segments as f32;
        let (sin_t, cos_t) = theta.sin_cos();
        vertices.push(vertex(
            [cos_t * radius, -half_h, sin_t * radius],
            [0.0, -1.0, 0.0],
            [cos_t * 0.5 + 0.5, 1.0 - (sin_t * 0.5 + 0.5)],
        ));
    }
    for i in 0..segments {
        let a = bottom_center + 1 + i;
        let b = a + 1;
        indices.extend_from_slice(&[bottom_center, a, b]);
    }

    MeshData { vertices, indices }
}

/// 创建单位大小的彩色立方体网格（历史 API，供 glTF 加载失败时回退）。
///
/// 返回 (顶点字节, 顶点字节大小, 索引数组)，顶点格式是旧的 `[pos×3, col×3]`。
pub fn create_coloured_cube() -> (Vec<u8>, u64, Vec<u32>) {
    #[repr(C)]
    #[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
    struct Vertex {
        position: [f32; 3],
        color: [f32; 3],
    }

    let vertices: &[Vertex] = &[
        // +X face (red)
        Vertex { position: [ 0.5,  0.5,  0.5], color: [1.0, 0.0, 0.0] },
        Vertex { position: [ 0.5, -0.5,  0.5], color: [1.0, 0.0, 0.0] },
        Vertex { position: [ 0.5, -0.5, -0.5], color: [1.0, 0.0, 0.0] },
        Vertex { position: [ 0.5,  0.5, -0.5], color: [1.0, 0.0, 0.0] },
        // -X face (green)
        Vertex { position: [-0.5,  0.5, -0.5], color: [0.0, 1.0, 0.0] },
        Vertex { position: [-0.5, -0.5, -0.5], color: [0.0, 1.0, 0.0] },
        Vertex { position: [-0.5, -0.5,  0.5], color: [0.0, 1.0, 0.0] },
        Vertex { position: [-0.5,  0.5,  0.5], color: [0.0, 1.0, 0.0] },
        // +Y face (blue)
        Vertex { position: [-0.5,  0.5, -0.5], color: [0.0, 0.0, 1.0] },
        Vertex { position: [-0.5,  0.5,  0.5], color: [0.0, 0.0, 1.0] },
        Vertex { position: [ 0.5,  0.5,  0.5], color: [0.0, 0.0, 1.0] },
        Vertex { position: [ 0.5,  0.5, -0.5], color: [0.0, 0.0, 1.0] },
        // -Y face (yellow)
        Vertex { position: [-0.5, -0.5,  0.5], color: [1.0, 1.0, 0.0] },
        Vertex { position: [-0.5, -0.5, -0.5], color: [1.0, 1.0, 0.0] },
        Vertex { position: [ 0.5, -0.5, -0.5], color: [1.0, 1.0, 0.0] },
        Vertex { position: [ 0.5, -0.5,  0.5], color: [1.0, 1.0, 0.0] },
        // +Z face (cyan)
        Vertex { position: [-0.5,  0.5,  0.5], color: [0.0, 1.0, 1.0] },
        Vertex { position: [-0.5, -0.5,  0.5], color: [0.0, 1.0, 1.0] },
        Vertex { position: [ 0.5, -0.5,  0.5], color: [0.0, 1.0, 1.0] },
        Vertex { position: [ 0.5,  0.5,  0.5], color: [0.0, 1.0, 1.0] },
        // -Z face (magenta)
        Vertex { position: [ 0.5,  0.5, -0.5], color: [1.0, 0.0, 1.0] },
        Vertex { position: [ 0.5, -0.5, -0.5], color: [1.0, 0.0, 1.0] },
        Vertex { position: [-0.5, -0.5, -0.5], color: [1.0, 0.0, 1.0] },
        Vertex { position: [-0.5,  0.5, -0.5], color: [1.0, 0.0, 1.0] },
    ];

    let indices: &[u32] = &[
        // +X       -X       +Y        -Y        +Z        -Z
         0,1,2,    4,5,6,   8,9,10,   12,13,14, 16,17,18, 20,21,22,
         0,2,3,    4,6,7,   8,10,11,  12,14,15, 16,18,19, 20,22,23,
    ];

    (bytemuck::cast_slice(vertices).to_vec(), std::mem::size_of::<Vertex>() as u64, indices.to_vec())
}
