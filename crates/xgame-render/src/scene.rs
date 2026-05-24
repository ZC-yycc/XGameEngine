use serde::{Deserialize, Serialize};
use xgame_math::Transform;

/// A single game object in the scene graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    pub transform: Transform,
    /// glTF model path (relative to workspace).
    pub model: String,
}

/// A scene containing a list of entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    pub entities: Vec<Entity>,
}
