//! 内建网格：`Mesh` 是位置 + 可选索引；真实资产加载在资产管线接上后替换 builtin。

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkinData {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
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
}

/// 已知 builtin 名称返回占位网格；未知模型返回 None，由调用方决定跳过或兜底。
pub fn builtin_mesh(model: &str) -> Option<Mesh> {
    match model {
        "builtin://quad" => Some(Mesh::quad()),
        "builtin://triangle" | "builtin://cube" | "builtin://sphere" | "builtin://plane"
        | "builtin://capsule" | "builtin://cylinder" => Some(Mesh::fullscreen_triangle()),
        _ => None,
    }
}
