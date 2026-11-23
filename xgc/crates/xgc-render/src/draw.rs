//! 从世界抽取渲染 draw list：`render.mesh` + `render.material` + `GlobalTransform`。
//!
//! 当前还没有真实网格资产，`model` 只作为 key 记录；GPU 执行层先用内建三角形
//! 验证"按实体出 draw call"的路径，网格加载在资产管线接上后替换。

use xgc_base::EntityId;
use xgc_world::{
    Animator, GlobalTransform, Material, MeshRenderer, Parent, Skeleton, SkeletonPose, World,
};

use super::camera::{model_matrix, Mat4};

/// 实体要播的 glTF 剪辑（`animation.animator` 的渲染侧投影）。
///
/// 只有实体**没有**场景骨骼姿态（`SkeletonPose`）时才带上它：场景里自己搭的
/// 骨骼走原生 `.ron` 剪辑路径，模型自带动画只服务迁移过来的 glTF 角色。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimatorSample {
    pub clip: String,
    pub time: f32,
    pub looping: bool,
    /// 转场来源剪辑（空 = 没有交叉淡入）。
    pub blend_from: String,
    /// 旧剪辑自己的时钟 / 循环方式（切换时刻定格，按旧状态速度推进）。
    pub blend_from_time: f32,
    pub blend_from_looping: bool,
    /// 转场已进行时间 / 总时长；渲染侧算 `alpha = min(elapsed / duration, 1)`。
    pub blend_elapsed: f32,
    pub blend_duration: f32,
    /// 叠加层剪辑名（空 = 没有叠加层）。
    pub additive_clip: String,
    /// 叠加层权重；0 时渲染侧直接跳过采样（与旧端零权重逐位一致）。
    pub additive_weight: f32,
    /// 叠加层自己的时钟（由 `AnimatorSystem` 按 `additive_speed` 推进）。
    pub additive_time: f32,
    pub additive_looping: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DrawItem {
    pub entity: EntityId,
    pub model: String,
    /// 实体覆盖的基色乘子（`render.material` 缺省 = 白）。
    pub color: [f32; 4],
    /// 有效粗糙度。`compose_colors` 之前的值只在**实体带 `render.material`**
    /// 时有意义（那是覆盖值）；没有组件时由模型材质决定。
    pub roughness: f32,
    /// 有效金属度，由 `compose_colors` 从模型材质 / `.mat` 资产解析。
    pub metallic: f32,
    /// `.mat` 资产引用（空串 = 不用资产）；渲染时与模型材质因子一起乘进 `color`。
    pub material_asset: String,
    /// 实体是否带 `render.material` 组件。
    ///
    /// 旧端叠加顺序是 `模型材质 → .mat 资产 → 实体覆盖`：没有组件时模型材质的
    /// metallic / roughness 必须保留，不能被 `Material::default()` 顶掉。
    pub has_material: bool,
    pub transform: Mat4,
    pub pose: Option<Vec<[f32; 16]>>,
    /// 要采样的 glTF 剪辑；渲染侧据此把 `pose` 补成动画后的关节矩阵。
    pub animation: Option<AnimatorSample>,
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
        let material = world.get::<Material>(key).cloned();
        let pose = skeleton_pose(world, key);
        // 场景骨骼姿态优先：迁移过来的 glTF 角色没有 Skeleton 实体，走模型自带动画。
        let animation = if pose.is_some() {
            None
        } else {
            world
                .get::<Animator>(key)
                .filter(|animator| !animator.clip.is_empty())
                .map(|animator| AnimatorSample {
                    clip: animator.clip.clone(),
                    time: animator.time,
                    looping: animator.looping,
                    blend_from: animator.blend_from.clone(),
                    blend_from_time: animator.blend_from_time,
                    blend_from_looping: animator.blend_from_looping,
                    blend_elapsed: animator.blend_elapsed,
                    blend_duration: animator.blend_duration,
                    additive_clip: animator.additive_clip.clone(),
                    additive_weight: animator.additive_weight,
                    additive_time: animator.additive_time,
                    additive_looping: animator.additive_looping,
                })
        };
        items.push(DrawItem {
            entity,
            model: mesh.model.clone(),
            color: material
                .as_ref()
                .map(|material| material.base_color)
                .unwrap_or([1.0; 4]),
            roughness: material
                .as_ref()
                .map(|material| material.roughness)
                .unwrap_or(0.0),
            metallic: 0.0,
            material_asset: material
                .as_ref()
                .map(|material| material.asset.clone())
                .unwrap_or_default(),
            has_material: material.is_some(),
            transform: model_matrix(transform),
            pose,
            animation,
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
