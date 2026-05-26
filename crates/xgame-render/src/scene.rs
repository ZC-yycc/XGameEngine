use serde::{Deserialize, Serialize};
use xgame_math::Transform;

/// 场景图中的单个游戏对象。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    pub transform: Transform,
    /// glTF 模型路径（相对于工作区）。
    pub model: String,
}

/// 包含实体列表的场景。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    pub entities: Vec<Entity>,
}
