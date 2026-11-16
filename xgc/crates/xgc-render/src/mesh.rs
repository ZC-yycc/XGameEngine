//! 内建网格：真实顶点数据。资产管线接上之前，`builtin://*` 用全屏三角形占位，
//! 保证从 draw list 到 vertex buffer / draw call 的链路是真实的。

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
}

impl Mesh {
    pub fn fullscreen_triangle() -> Self {
        Self {
            positions: vec![[-1.0, -1.0, 0.0], [3.0, -1.0, 0.0], [-1.0, 3.0, 0.0]],
        }
    }

    pub fn quad() -> Self {
        Self {
            positions: vec![
                [-0.5, -0.5, 0.0],
                [0.5, -0.5, 0.0],
                [0.5, 0.5, 0.0],
                [-0.5, -0.5, 0.0],
                [0.5, 0.5, 0.0],
                [-0.5, 0.5, 0.0],
            ],
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
