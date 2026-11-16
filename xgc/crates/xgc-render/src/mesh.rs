//! 内建网格：`Mesh` 是位置 + 可选索引；真实资产加载在资产管线接上后替换 builtin。
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
    pub skin: Option<SkinData>,
}

impl Mesh {
    pub fn new(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Self {
        Self {
            positions,
            indices,
            skin: None,
        }
    }

    pub fn fullscreen_triangle() -> Self {
        Self {
            positions: vec![[-1.0, -1.0, 0.0], [3.0, -1.0, 0.0], [-1.0, 3.0, 0.0]],
            indices: Vec::new(),
            skin: None,
        }
    }

    pub fn quad() -> Self {
        Self {
            positions: vec![
                [-0.5, -0.5, 0.0],
                [0.5, -0.5, 0.0],
                [0.5, 0.5, 0.0],
                [-0.5, 0.5, 0.0],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            skin: None,
        }
    }

    pub fn cube() -> Self {
        let positions = vec![
            [-0.5, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [0.5, 0.5, -0.5],
            [-0.5, 0.5, -0.5],
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [0.5, 0.5, 0.5],
            [-0.5, 0.5, 0.5],
        ];
        let indices = vec![
            0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 1, 2, 6, 1, 6,
            5, 0, 4, 7, 0, 7, 3,
        ];
        Self::new(positions, indices)
    }

    pub fn plane() -> Self {
        Self::new(
            vec![
                [-0.5, 0.0, -0.5],
                [0.5, 0.0, -0.5],
                [0.5, 0.0, 0.5],
                [-0.5, 0.0, 0.5],
            ],
            vec![0, 2, 1, 0, 3, 2],
        )
    }

    pub fn sphere() -> Self {
        let rings = 8usize;
        let segments = 16usize;
        let mut positions = Vec::with_capacity((rings + 1) * (segments + 1));
        let mut indices = Vec::new();
        for ring in 0..=rings {
            let phi = ring as f32 / rings as f32 * std::f32::consts::PI;
            for segment in 0..=segments {
                let theta = segment as f32 / segments as f32 * std::f32::consts::TAU;
                positions.push([
                    0.5 * phi.sin() * theta.cos(),
                    0.5 * phi.cos(),
                    0.5 * phi.sin() * theta.sin(),
                ]);
            }
        }
        for ring in 0..rings {
            for segment in 0..segments {
                let a = (ring * (segments + 1) + segment) as u32;
                let b = a + segments as u32 + 1;
                indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        Self::new(positions, indices)
    }

    pub fn cylinder() -> Self {
        let segments = 16usize;
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for segment in 0..=segments {
            let theta = segment as f32 / segments as f32 * std::f32::consts::TAU;
            let x = 0.5 * theta.cos();
            let z = 0.5 * theta.sin();
            positions.push([x, -0.5, z]);
            positions.push([x, 0.5, z]);
        }
        for segment in 0..segments {
            let base = (segment * 2) as u32;
            indices.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
        }
        let top = positions.len() as u32;
        positions.push([0.0, 0.5, 0.0]);
        let bottom = positions.len() as u32;
        positions.push([0.0, -0.5, 0.0]);
        for segment in 0..segments {
            let a = (segment * 2) as u32;
            let b = (((segment + 1) % segments) * 2) as u32;
            indices.extend_from_slice(&[top, a + 1, b + 1, bottom, b, a]);
        }
        Self::new(positions, indices)
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
