//! 从世界抽取渲染 draw list：`render.mesh` + `render.material` + `GlobalTransform`。
//!
//! 当前还没有真实网格资产，`model` 只作为 key 记录；GPU 执行层先用内建三角形
//! 验证"按实体出 draw call"的路径，网格加载在资产管线接上后替换。

use xgc_base::EntityId;
use xgc_world::{GlobalTransform, Material, MeshRenderer, Parent, Skeleton, SkeletonPose, World};

use super::camera::{model_matrix, Mat4};

#[derive(Clone, Debug, PartialEq)]
pub struct DrawItem {
    pub entity: EntityId,
    pub model: String,
    pub color: [f32; 4],
    pub roughness: f32,
    pub transform: Mat4,
    pub pose: Option<Vec<[f32; 16]>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawList {
    pub items: Vec<DrawItem>,
}

impl DrawList {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// 只收同时有 `GlobalTransform` 与 `render.mesh` 的实体；结果按 EntityId 排序，
/// 与 archetype / 行序无关。
pub fn build_draw_list(world: &World) -> DrawList {
    let mut items = Vec::new();
    world.for_each::<MeshRenderer>(|key, mesh| {
        let Some(entity) = world.entity(key) else {
            return;
        };
        let Some(transform) = world.get::<GlobalTransform>(key) else {
            return;
        };
        let material = world.get::<Material>(key).copied().unwrap_or_default();
        items.push(DrawItem {
            entity,
            model: mesh.model.clone(),
            color: material.base_color,
            roughness: material.roughness,
            transform: model_matrix(transform),
            pose: skeleton_pose(world, key),
        });
    });
    items.sort_by_key(|item| item.entity);
    DrawList { items }
}

/// 蒙皮网格约定：沿 Parent 向上找到 Skeleton 根，读取该根的 `animation.pose`。
fn skeleton_pose(world: &World, mut key: xgc_base::EntityKey) -> Option<Vec<[f32; 16]>> {
    for _ in 0..64 {
        if world.get::<Skeleton>(key).is_some() {
            let pose = world.get::<SkeletonPose>(key)?;
            return ron::from_str(&pose.matrices).ok();
        }
        let parent = world.get::<Parent>(key)?;
        key = world.key(parent.entity)?;
    }
    None
}
