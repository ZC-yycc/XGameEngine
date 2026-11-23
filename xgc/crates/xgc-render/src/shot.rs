//! 截图取景：复刻旧引擎 `--screenshot` 的相机优先级与裁剪面口径。
//!
//! 旧端截图的相机优先级 = 显式机位 > **场景相机实体** > 顺光自动取景（整场景包围盒
//! 拟合进 45° 视场，视轴由场景主方向光推出来）。无论走哪一条，**近 / 远裁剪面都由
//! [`clip_planes`] 按场景包围球重算**——不是场景相机组件里的 near / far。这一条不能
//! 省：CSM 的级联拟合吃 `inv_view_proj` 反推的近平面角点，近平面差 0.08（0.1 → 0.02）
//! 就足以把高处投射体裁出第 0/1 级的深度图，画面上就是「盒子还悬在半空、地上却没有
//! 影子」。XGC 的 `render-frame` 必须逐项同口径，否则两张图连构图 / 阴影覆盖都对不上。
//!
//! 参数与旧实现逐项一致（45° 视场、6% 余量、方位角偏 35°、仰角 0.45 rad、
//! 空场景半径 5）。改这些常数等于改验收基线，必须连带更新 CUTOVER 的截图证据。

use xgc_world::{Camera, GlobalTransform, SceneEnvironment, World};

use super::assets::MeshCache;
use super::camera::{
    length, look_at_rh, mul, perspective_rh_zo, quat_rotate, sub, transform_point, Mat4,
};
use super::draw::build_draw_list;
use super::glb::MeshError;
use super::mesh::Mesh;

/// 截图视场角：与旧端 `DEFAULT_FOV_Y` 相同（π/4）。
pub const DEFAULT_FOV_Y: f32 = std::f32::consts::FRAC_PI_4;
/// 空场景也要给出一台合法相机；半径与旧端一致。
const EMPTY_SCENE_FALLBACK_RADIUS: f32 = 5.0;
/// 顺光视轴的仰角（弧度，≈26°）：既看得到地面又看得到立面。
const SHOT_ELEVATION: f32 = 0.45;
/// 顺光视轴的方位偏移（弧度，≈35°）：偏一点才有明暗交界。
const SHOT_AZIMUTH_OFFSET: f32 = 0.61;

/// 轴对齐包围盒；`EMPTY` 表示"还没有任何几何"。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    pub const EMPTY: Self = Self {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };

    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0] || self.min[1] > self.max[1] || self.min[2] > self.max[2]
    }

    pub fn center(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    /// 包围盒半对角线长度（包围球半径）。
    pub fn radius(&self) -> f32 {
        length(sub(self.max, self.min)) * 0.5
    }

    pub fn corners(&self) -> [[f32; 3]; 8] {
        let (min, max) = (self.min, self.max);
        [
            [min[0], min[1], min[2]],
            [max[0], min[1], min[2]],
            [min[0], max[1], min[2]],
            [max[0], max[1], min[2]],
            [min[0], min[1], max[2]],
            [max[0], min[1], max[2]],
            [min[0], max[1], max[2]],
            [max[0], max[1], max[2]],
        ]
    }

    pub fn union_point(&mut self, point: [f32; 3]) {
        for (axis, value) in point.into_iter().enumerate() {
            self.min[axis] = self.min[axis].min(value);
            self.max[axis] = self.max[axis].max(value);
        }
    }

    pub fn union(&mut self, other: &Self) {
        if other.is_empty() {
            return;
        }
        for corner in other.corners() {
            self.union_point(corner);
        }
    }

    /// 网格顶点的 AABB；空网格返回 `None`。
    pub fn from_mesh(mesh: &Mesh) -> Option<Self> {
        let mut bounds = Self::EMPTY;
        for position in &mesh.positions {
            bounds.union_point(*position);
        }
        (!bounds.is_empty()).then_some(bounds)
    }

    /// 用模型矩阵变换 8 个角再取 AABB（对旋转同样成立，且比中心 ± 半径保守得少）。
    pub fn transformed(&self, matrix: Mat4) -> Self {
        let mut out = Self::EMPTY;
        for corner in self.corners() {
            let [x, y, z, w] = transform_point(matrix, [corner[0], corner[1], corner[2], 1.0]);
            out.union_point([x / w, y / w, z / w]);
        }
        out
    }
}

/// 一台截图相机；`eye` / `target` 是世界坐标。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotCamera {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    /// 垂直视场角（弧度）。
    pub fov_y: f32,
}

impl ShotCamera {
    /// 视线接近垂直时换 Z 轴当 up，否则 `look_at` 会在 forward ∥ up 处算出 NaN。
    pub fn looking_at(eye: [f32; 3], target: [f32; 3], fov_y: f32) -> Self {
        let direction = normalize_or(sub(target, eye), [0.0, 0.0, -1.0]);
        let up = if direction[1].abs() > 0.99 {
            [0.0, 0.0, 1.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        Self {
            eye,
            target,
            up,
            fov_y,
        }
    }
}

/// 取景结果：相机 + 场景半径（进 / 远裁剪面由它推）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotFrame {
    pub camera: ShotCamera,
    pub scene_radius: f32,
}

impl ShotFrame {
    pub fn view_projection(&self, width: u32, height: u32) -> Mat4 {
        let aspect = aspect(width, height);
        let (near, far) = clip_planes(&self.camera, self.scene_radius);
        let view = look_at_rh(self.camera.eye, self.camera.target, self.camera.up);
        let projection = perspective_rh_zo(self.camera.fov_y, aspect, near, far);
        mul(projection, view)
    }
}

/// 没有可用方向光时的兜底视轴：右上前方四分之三视角。
pub fn default_shot_direction() -> [f32; 3] {
    normalize_or([1.0, 0.62, 1.25], [0.0, 0.0, -1.0])
}

/// 由场景主方向光推顺光视轴：方位取"表面指向光源"的方向再偏 35°，仰角固定。
pub fn shot_direction(environment: &SceneEnvironment) -> [f32; 3] {
    let Some(light) = environment.sun() else {
        return default_shot_direction();
    };
    let [dx, dy, dz] = light.normalized_direction();
    let to_light = [-dx, -dy, -dz];
    let azimuth = to_light[0].atan2(to_light[2]) + SHOT_AZIMUTH_OFFSET;
    let cos = SHOT_ELEVATION.cos();
    [
        azimuth.sin() * cos,
        SHOT_ELEVATION.sin(),
        azimuth.cos() * cos,
    ]
}

/// 由包围盒拟合"看得全"的相机：半径 / sin(较小半视场) × 1.06。
pub fn frame_bounds(
    bounds: Option<&Bounds>,
    aspect: f32,
    fov_y: f32,
    direction: [f32; 3],
) -> ShotFrame {
    let (center, radius) = match bounds {
        Some(bounds) if !bounds.is_empty() => (bounds.center(), bounds.radius().max(0.5)),
        _ => ([0.0; 3], EMPTY_SCENE_FALLBACK_RADIUS),
    };
    let fov_y = fov_y.clamp(5.0_f32.to_radians(), 120.0_f32.to_radians());
    let aspect = aspect.max(1e-3);
    let half_v = fov_y * 0.5;
    // 水平半视场：tan(h) = tan(v) · aspect；宽图以垂直为准，竖图以水平为准。
    let half_h = (half_v.tan() * aspect).atan();
    let half_min = half_v.min(half_h);
    let dist = radius / half_min.sin().max(1e-3) * 1.06;
    let direction = normalize_or(direction, default_shot_direction());
    let eye = [
        center[0] + direction[0] * dist,
        center[1] + direction[1] * dist,
        center[2] + direction[2] * dist,
    ];
    ShotFrame {
        camera: ShotCamera::looking_at(eye, center, fov_y),
        scene_radius: radius,
    }
}

/// 近 / 远裁剪面：相机在包围球外时近平面留在球面之前，远平面罩住整球再留余量。
pub fn clip_planes(camera: &ShotCamera, scene_radius: f32) -> (f32, f32) {
    let dist = length(sub(camera.eye, camera.target)).max(1e-3);
    let radius = scene_radius.max(1e-3);
    let near = (dist - radius * 1.5).max(dist * 0.02).max(0.02);
    let far = dist + radius * 4.0 + 20.0;
    (near, far)
}

/// 整场景包围盒：按 draw list 的模型矩阵把各网格顶点 AABB 并起来。
///
/// 蒙皮实体必须用**当前姿态**的包围盒：绑定姿态的盒子会把被关节移出去的顶点漏在
/// 取景 / 级联拟合之外（旧端同样把 posed bounds 与保守盒求并后使用）。
pub fn scene_bounds(world: &World, meshes: &mut MeshCache) -> Result<Option<Bounds>, MeshError> {
    let mut bounds = Bounds::EMPTY;
    for item in build_draw_list(world).items {
        let mesh = meshes.get(&item.model)?;
        let Some(mesh) = mesh else {
            continue;
        };
        let Some(local) = Bounds::from_mesh(mesh) else {
            continue;
        };
        let posed = item
            .pose
            .as_deref()
            .and_then(|pose| super::skinned::posed_bounds(mesh, pose))
            .unwrap_or(local);
        bounds.union(&posed.transformed(item.transform));
    }
    Ok((!bounds.is_empty()).then_some(bounds))
}

/// 场景相机实体 → 截图相机（第一台 `active` 的，按 `EntityId` 升序取）。
///
/// `target = eye + forward`、`up` 用实体自己的上方向 —— 与旧端 `headless.rs`
/// 把 `camera_view()` 转成 `ShotCamera` 的写法逐项一致（带滚转的机位照原样截）。
fn scene_camera(world: &World) -> Option<ShotCamera> {
    let mut selected: Option<(u64, ShotCamera)> = None;
    world.for_each::<Camera>(|key, camera| {
        if !camera.active {
            return;
        }
        let Some(entity) = world.entity(key) else {
            return;
        };
        if selected
            .as_ref()
            .is_some_and(|(existing, _)| *existing <= entity.raw())
        {
            return;
        }
        let Some(transform) = world.get::<GlobalTransform>(key) else {
            return;
        };
        let forward = quat_rotate(transform.rotation, [0.0, 0.0, -1.0]);
        let eye = transform.position;
        let target = [
            eye[0] + forward[0],
            eye[1] + forward[1],
            eye[2] + forward[2],
        ];
        let up = quat_rotate(transform.rotation, [0.0, 1.0, 0.0]);
        selected = Some((
            entity.raw(),
            ShotCamera {
                eye,
                target,
                up,
                fov_y: camera.fov_y_deg.to_radians(),
            },
        ));
    });
    selected.map(|(_, camera)| camera)
}

/// 截图取景入口（相机优先级 + 统一裁剪面）：`render-frame` 与回归测试的唯一口径。
///
/// 有场景相机 → 用它（`scene_radius` 仍来自场景包围盒）；没有 → 顺光自动取景。
/// 两种情况的近 / 远裁剪面都由 [`ShotFrame::view_projection`] 走 [`clip_planes`]。
pub fn shot_frame(
    world: &World,
    meshes: &mut MeshCache,
    width: u32,
    height: u32,
) -> Result<ShotFrame, MeshError> {
    let bounds = scene_bounds(world, meshes)?;
    if let Some(camera) = scene_camera(world) {
        let scene_radius = bounds
            .as_ref()
            .map(|bounds| bounds.radius())
            .unwrap_or(EMPTY_SCENE_FALLBACK_RADIUS);
        return Ok(ShotFrame {
            camera,
            scene_radius,
        });
    }
    let direction = shot_direction(world.environment());
    Ok(frame_bounds(
        bounds.as_ref(),
        aspect(width, height),
        DEFAULT_FOV_Y,
        direction,
    ))
}

/// 截图取景入口（强制顺光自动取景）：只给"没有场景相机"的调用方 / 测试用。
pub fn frame_scene(
    world: &World,
    meshes: &mut MeshCache,
    width: u32,
    height: u32,
) -> Result<ShotFrame, MeshError> {
    let bounds = scene_bounds(world, meshes)?;
    let direction = shot_direction(world.environment());
    Ok(frame_bounds(
        bounds.as_ref(),
        aspect(width, height),
        DEFAULT_FOV_Y,
        direction,
    ))
}

fn aspect(width: u32, height: u32) -> f32 {
    width.max(1) as f32 / height.max(1) as f32
}

fn normalize_or(value: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let normalized = super::camera::normalize(value);
    if length(normalized) > 0.5 {
        normalized
    } else {
        fallback
    }
}
