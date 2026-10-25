//! 相机视图：把场景里的**主相机实体**解成"眼睛 / 朝向 / 投影"，三宿主共用。
//!
//! # 为什么这一层在 runtime 而不是 core / render
//!
//! - **core** 不认识投影约定（`Mat4::perspective_rh` + 0..1 深度这套是渲染侧
//!   的约定，`xgame_math::projection` 是唯一出口），所以 core 只存参数；
//! - **render** 不该认识 `Scene`（它的输入是 `&[Entity]` 与一个现成的
//!   `view_proj`，无头截图、编辑器视口、独立游戏各自解算）；
//! - **runtime** 正好是"场景 + 宿主窗口"的交界：它既有 `Scene`，又已经在
//!   `xgame-math` 之上。
//!
//! 三个宿主（独立游戏 / 编辑器 Play / 无头截图）都调 [`CameraView::from_scene`]，
//! 于是"编辑器里看到的"和"打包出来看到的"不会分叉 —— 这条纪律和
//! `World::update` 单入口是同一个理由。
//!
//! # 与"视图空间"的换算
//!
//! 实体本地的 **+Z 是镜头朝向**（见 `xgame_core::camera`），而右手系视图空间
//! 里镜头看向 **−Z**。换算不在数据层做：这里直接给
//! `projection::look_at(eye, eye + forward, up)`，它产出的矩阵天然满足
//! "前方 → −Z_view"，且 `up` 带着实体的滚转（相机歪着看也正确）。

use xgame_core::{Camera, EntityId, Scene};
use xgame_math::glam::{Mat4, Vec3};
use xgame_math::projection;

/// 一帧要用的相机位姿与投影参数（宽高比除外 —— 它属于视口）。
///
/// 只是**一份快照**：解析出来之后场景再变（脚本推相机）不会回写，
/// 想拿最新值就再调一次 [`CameraView::from_scene`]。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraView {
    /// 提供这份位姿的相机实体。
    pub entity: EntityId,
    /// 世界空间眼睛位置（**已含父节点层级**）。
    pub position: Vec3,
    /// 世界空间单位前方向。
    pub forward: Vec3,
    /// 世界空间单位上方向（含滚转）。
    pub up: Vec3,
    /// 垂直视场角（度，已钳到 1..179）。
    pub fov_deg: f32,
    /// 近裁剪面（米，已钳到 > 0）。
    pub near: f32,
    /// 远裁剪面（米，已钳到 > near）。
    pub far: f32,
}

impl CameraView {
    /// 解析场景主相机。没有相机实体 → `None`（宿主退回自己的旧机位）。
    ///
    /// 每帧一次线性扫描（[`Scene::active_camera_index`] 找到 active 就早退）。
    /// 不做缓存：脚本可以在任意一帧改相机的 `active` / 变换，缓存只会引入
    /// "切了机位但画面还是上一台"的时序 bug —— 每次少一次扫描不值得。
    pub fn from_scene(scene: &Scene) -> Option<Self> {
        let index = scene.active_camera_index()?;
        let entity = &scene.entities[index];
        let camera = entity.camera.as_ref()?.sanitized();
        // 世界变换：相机必须能被父节点带着走（肩视角挂在角色下面）。
        let world = scene.world_transform(index);

        // 旋转来自矩阵分解，理论上已是归一化的；坏数据（NaN 四元数、
        // 零缩放父节点）会退化成非有限值 —— 那时给一个确定朝向，
        // 而不是把 NaN 送进 look_at（出来是整帧空白）。
        let rotation = if world.rotation.is_finite() {
            world.rotation.normalize()
        } else {
            xgame_math::glam::Quat::IDENTITY
        };
        let mut forward = Camera::forward(rotation);
        let mut up = Camera::up(rotation);
        if !forward.is_finite() || forward.length_squared() < 1e-12 {
            forward = Vec3::Z;
            up = Vec3::Y;
        }
        if !up.is_finite() || up.cross(forward).length_squared() < 1e-12 {
            // 与 forward 平行的 up 会让 look_at 的叉乘退化。
            up = if forward.y.abs() > 0.99 {
                Vec3::Z
            } else {
                Vec3::Y
            };
        }

        Some(Self {
            entity: entity.id,
            position: world.position,
            forward: forward.normalize(),
            up: up.normalize(),
            fov_deg: camera.fov,
            near: camera.near,
            far: camera.far,
        })
    }

    /// 视图矩阵（右手系，前方映射到 −Z_view）。
    pub fn view_matrix(&self) -> Mat4 {
        projection::look_at(self.position, self.position + self.forward, self.up)
    }

    /// 投影矩阵。`aspect` = 宽 / 高（视口物理像素）。
    pub fn projection_matrix(&self, aspect: f32) -> Mat4 {
        // 视口还没尺寸时（最小化 / 首帧）给一个合法宽高比：
        // aspect = 0 会让投影矩阵整列变 0，画面全黑。
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        projection::perspective(self.fov_deg.to_radians(), aspect, self.near, self.far)
    }

    /// 视图投影矩阵（渲染侧唯一入口）。
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection_matrix(aspect) * self.view_matrix()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_core::{Entity, Transform};
    use xgame_math::glam::{Quat, Vec3};

    fn scene_with_camera(position: Vec3, rotation: Quat) -> Scene {
        let mut scene = Scene::new("Cam");
        scene.add_entity(
            Entity::new("Main Camera")
                .with_transform(Transform {
                    position,
                    rotation,
                    scale: Vec3::ONE,
                })
                .with_camera(Camera::default()),
        );
        scene
    }

    /// 把世界点投影到 NDC（透视除法后）。
    fn ndc(view: &CameraView, point: Vec3) -> Vec3 {
        let clip = view.view_projection(1.0) * point.extend(1.0);
        assert!(clip.w > 0.0, "点在相机后面: {point:?}");
        Vec3::new(clip.x / clip.w, clip.y / clip.w, clip.z / clip.w)
    }

    #[test]
    fn no_camera_entity_means_none() {
        let mut scene = Scene::new("Empty");
        scene.add_entity(Entity::new("Box"));
        assert!(CameraView::from_scene(&scene).is_none());
    }

    #[test]
    fn camera_looks_along_local_plus_z() {
        // 约定：本地 +Z 是镜头朝向。原点朝 +Z 的相机，前方 5 米的点在画面中心。
        let scene = scene_with_camera(Vec3::ZERO, Quat::IDENTITY);
        let view = CameraView::from_scene(&scene).expect("应当解析出相机");
        assert!((view.forward - Vec3::Z).length() < 1e-5);

        let front = ndc(&view, Vec3::new(0.0, 0.0, 5.0));
        assert!(
            front.x.abs() < 1e-4 && front.y.abs() < 1e-4,
            "正前方不在画面中心: {front:?}"
        );

        // 面朝 +Z 站着时，"右手边"是世界 −X：世界 +X 的点必须落在画面左侧。
        let right_of_world = ndc(&view, Vec3::new(2.0, 0.0, 5.0));
        assert!(
            right_of_world.x < 0.0,
            "世界 +X 应出现在画面左侧: {right_of_world:?}"
        );

        // 世界 +Y（上方）的点必须在上半屏。
        let above = ndc(&view, Vec3::new(0.0, 2.0, 5.0));
        assert!(above.y > 0.0, "世界 +Y 应在上半屏: {above:?}");
    }

    #[test]
    fn camera_rotation_steers_the_view() {
        // 绕 Y 转 90°：本地 +Z 转到世界 +X，那么世界 +X 的点就在正前方。
        let scene = scene_with_camera(Vec3::ZERO, Quat::from_rotation_y(90f32.to_radians()));
        let view = CameraView::from_scene(&scene).unwrap();
        assert!(
            (view.forward - Vec3::X).length() < 1e-4,
            "forward={:?}",
            view.forward
        );
        let center = ndc(&view, Vec3::new(5.0, 0.0, 0.0));
        assert!(center.x.abs() < 1e-4 && center.y.abs() < 1e-4);
    }

    #[test]
    fn camera_follows_parent_transform() {
        // 肩视角的实际形态：相机挂在角色下面。父节点平移旋转，相机必须跟着走。
        let mut scene = Scene::new("Rig");
        let rig = scene.add_entity(
            Entity::new("Player Rig")
                .with_transform(Transform::from_translation(Vec3::new(10.0, 0.0, 0.0))),
        );
        let cam = scene.add_entity(Entity::new("Shoulder Cam").with_camera(Camera::default()));
        // `set_parent` 是"保留世界变换"语义（编辑器拖拽要的），所以先挂再接
        // 局部偏移 —— 作者在 Inspector 里也是这么配的（挂进去之后调局部值）。
        assert!(scene.set_parent(
            scene.index_of(cam).unwrap(),
            Some(scene.index_of(rig).unwrap())
        ));
        let cam_index = scene.index_of(cam).unwrap();
        scene.entities[cam_index].transform.position = Vec3::new(0.0, 1.6, -2.0);

        let view = CameraView::from_scene(&scene).unwrap();
        assert_eq!(view.entity, scene.entities[scene.index_of(cam).unwrap()].id);
        // 局部 (0, 1.6, -2) + 父 (10, 0, 0) = 世界 (10, 1.6, -2)。
        assert!((view.position - Vec3::new(10.0, 1.6, -2.0)).length() < 1e-4);
    }

    #[test]
    fn inactive_camera_is_skipped() {
        let mut scene = Scene::new("Cams");
        let idle = Camera {
            active: false,
            ..Camera::default()
        };
        scene.add_entity(Entity::new("Idle").with_camera(idle));
        let main = scene.add_entity(Entity::new("Main").with_camera(Camera::default()));
        let view = CameraView::from_scene(&scene).unwrap();
        assert_eq!(
            view.entity,
            scene.entities[scene.index_of(main).unwrap()].id
        );
    }

    #[test]
    fn wider_fov_moves_points_toward_center() {
        let mut scene = Scene::new("Fov");
        let wide = Camera {
            fov: 100.0,
            ..Camera::default()
        };
        scene.add_entity(Entity::new("Cam").with_camera(wide.clone()));
        let wide_view = CameraView::from_scene(&scene).unwrap();

        let mut scene2 = Scene::new("Fov60");
        scene2.add_entity(Entity::new("Cam").with_camera(Camera::default()));
        let narrow_view = CameraView::from_scene(&scene2).unwrap();

        let p = Vec3::new(1.0, 0.0, 5.0);
        assert!(
            ndc(&wide_view, p).x.abs() < ndc(&narrow_view, p).x.abs(),
            "视野更宽时同一个点应更靠近画面中心"
        );
    }

    #[test]
    fn broken_camera_params_are_clamped_not_propagated() {
        // 场景数据是人手改的 RON / 脚本 / MCP 写的，坏值不许变成 NaN 矩阵。
        let mut scene = Scene::new("Bad");
        scene.add_entity(Entity::new("Cam").with_camera(Camera {
            fov: 0.0,
            near: 0.0,
            far: -1.0,
            active: true,
        }));
        let view = CameraView::from_scene(&scene).unwrap();
        assert!(view.fov_deg >= 1.0 && view.near > 0.0 && view.far > view.near);
        let m = view.view_projection(0.0);
        assert!(m.is_finite(), "坏参数不该产出非有限矩阵: {m:?}");
    }

    #[test]
    fn vertical_look_keeps_view_matrix_finite() {
        // 正下方俯视（俯视角玩法）：+90° 绕 X 让本地 +Z 指向世界 −Y。
        let scene = scene_with_camera(
            Vec3::new(0.0, 5.0, 0.0),
            Quat::from_rotation_x(90f32.to_radians()),
        );
        let view = CameraView::from_scene(&scene).unwrap();
        assert!(
            (view.forward - Vec3::NEG_Y).length() < 1e-4,
            "forward={:?}",
            view.forward
        );
        assert!(view.view_projection(1.5).is_finite());
        // 俯视时地面上正下方的点应在画面中心。
        let below = ndc(&view, Vec3::new(0.0, 0.0, 0.0));
        assert!(below.x.abs() < 1e-4 && below.y.abs() < 1e-4, "{below:?}");
    }

    #[test]
    fn aspect_zero_falls_back_to_square() {
        // 最小化窗口 / 首帧：宽高比可能是 0。
        let scene = scene_with_camera(Vec3::ZERO, Quat::IDENTITY);
        let view = CameraView::from_scene(&scene).unwrap();
        assert!(view.view_projection(0.0).is_finite());
        assert!(view.view_projection(f32::NAN).is_finite());
    }
}
