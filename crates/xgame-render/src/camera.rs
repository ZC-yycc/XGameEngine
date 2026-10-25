use xgame_math::glam::{Mat4, Vec3};
use xgame_math::projection;

// ---------------------------------------------------------------------------
// GPU 统一变量（必须与 WGSL 布局逐字节对齐）
// ---------------------------------------------------------------------------

/// 与 WGSL `CameraUniform` 结构体逐字节对齐。
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    pub view_proj: [[f32; 4]; 4],
    pub position: [f32; 4],
}

// ---------------------------------------------------------------------------
// 光源 GPU 统一变量
// ---------------------------------------------------------------------------

/// 与 WGSL `LightUniform` 结构体对齐。
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LightUniform {
    /// 世界空间光源方向。w = 填充。
    pub direction: [f32; 4],
    /// 光源颜色 (RGB)。w = 强度倍率。
    pub color: [f32; 4],
    /// 环境光颜色 (RGB)。w = 填充。
    pub ambient: [f32; 4],
}

/// CPU 端方向光描述符。
#[derive(Debug, Clone)]
pub struct DirectionalLight {
    pub direction: Vec3,
    pub color: Vec3,
    pub intensity: f32,
    pub ambient: Vec3,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            direction: Vec3::new(-0.5, -1.0, -0.5).normalize(),
            color: Vec3::new(1.0, 0.95, 0.85),
            intensity: 2.0,
            ambient: Vec3::new(0.05, 0.05, 0.08),
        }
    }
}

impl DirectionalLight {
    pub fn uniform(&self) -> LightUniform {
        LightUniform {
            direction: self.direction.extend(0.0).to_array(),
            color: self.color.extend(self.intensity).to_array(),
            ambient: self.ambient.extend(0.0).to_array(),
        }
    }
}

// ---------------------------------------------------------------------------
// 相机
// ---------------------------------------------------------------------------

/// 透视相机，包含位置、观察目标和上方向。
#[derive(Debug, Clone)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    /// 垂直视场角，单位为弧度。
    pub fov: f32,
    pub near: f32,
    pub far: f32,
    pub aspect: f32,
}

impl Camera {
    pub fn new(aspect: f32) -> Self {
        Self {
            eye: Vec3::new(0.0, 0.0, 5.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov: 60f32.to_radians(),
            near: 0.1,
            far: 100.0,
            aspect,
        }
    }

    pub fn view_matrix(&self) -> Mat4 {
        projection::look_at(self.eye, self.target, self.up)
    }

    pub fn proj_matrix(&self) -> Mat4 {
        projection::perspective(self.fov, self.aspect, self.near, self.far)
    }

    pub fn view_proj(&self) -> Mat4 {
        self.proj_matrix() * self.view_matrix()
    }

    pub fn uniform(&self) -> CameraUniform {
        CameraUniform {
            view_proj: self.view_proj().to_cols_array_2d(),
            position: self.eye.extend(1.0).to_array(),
        }
    }
}

// ---------------------------------------------------------------------------
// 轨道相机控制器
// ---------------------------------------------------------------------------

/// 鼠标驱动的轨道相机：拖拽旋转、滚轮缩放、WASD 平移、Q/E 升降。
#[derive(Debug, Clone)]
pub struct OrbitCamera {
    pub camera: Camera,
    /// 水平旋转角度（弧度）。
    pub yaw: f32,
    /// 垂直旋转角度（弧度），限制以避免万向节锁定。
    pub pitch: f32,
    /// 到观察目标的距离。
    pub distance: f32,
    /// 鼠标拖拽灵敏度（弧度/像素）。
    pub sensitivity: f32,
    /// 滚轮缩放灵敏度。
    pub zoom_speed: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    /// 按住 WASD 时的平移速度（世界单位/秒）。
    pub pan_speed: f32,
}

impl OrbitCamera {
    pub fn new(aspect: f32) -> Self {
        let distance = 5.0;
        let yaw = std::f32::consts::FRAC_PI_4;
        let pitch = std::f32::consts::FRAC_PI_6;

        let mut cam = Camera::new(aspect);
        cam.eye = Self::spherical_to_cartesian(yaw, pitch, distance);
        cam.target = Vec3::ZERO;

        Self {
            camera: cam,
            yaw,
            pitch,
            distance,
            sensitivity: 0.005,
            zoom_speed: 0.5,
            min_distance: 0.5,
            max_distance: 50.0,
            pan_speed: 5.0,
        }
    }

    /// 按鼠标增量旋转（像素）。
    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * self.sensitivity;
        self.pitch += dy * self.sensitivity;
        self.pitch = self.pitch.clamp(
            -std::f32::consts::FRAC_PI_2 + 0.01,
            std::f32::consts::FRAC_PI_2 - 0.01,
        );
    }

    /// 按滚轮增量缩放。
    pub fn zoom(&mut self, delta: f32) {
        self.distance =
            (self.distance - delta * self.zoom_speed).clamp(self.min_distance, self.max_distance);
    }

    /// 从球坐标重新计算相机位置，相对于当前轨道目标。
    pub fn update(&mut self) {
        self.camera.eye =
            self.camera.target + Self::spherical_to_cartesian(self.yaw, self.pitch, self.distance);
    }

    fn spherical_to_cartesian(yaw: f32, pitch: f32, distance: f32) -> Vec3 {
        let x = distance * pitch.cos() * yaw.cos();
        let y = distance * pitch.sin();
        let z = distance * pitch.cos() * yaw.sin();
        Vec3::new(x, y, z)
    }

    /// 沿相机右方向平移目标和视点（水平投影）。
    /// `amount` 是每帧的世界单位位移，受 `pan_speed` 调制。
    pub fn pan_right(&mut self, amount: f32, dt: f32) {
        let fwd = (self.camera.target - self.camera.eye).normalize();
        // fwd × up 得出右手 Y-up 系中的相机右方向。
        let right = fwd.cross(self.camera.up).normalize();
        let offset = right * amount * self.pan_speed * dt;
        self.camera.target += offset;
        self.camera.eye += offset;
    }

    /// 沿相机前方向平移目标和视点（视线方向的水平投影）。
    pub fn pan_forward(&mut self, amount: f32, dt: f32) {
        let fwd = self.camera.target - self.camera.eye;
        // 投影到水平面（忽略 Y 分量）。
        let fwd_h = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
        let offset = fwd_h * amount * self.pan_speed * dt;
        self.camera.target += offset;
        self.camera.eye += offset;
    }

    /// 沿世界空间上/下方向移动相机目标和视点。
    pub fn move_up(&mut self, amount: f32, dt: f32) {
        let offset = Vec3::new(0.0, amount * self.pan_speed * dt, 0.0);
        self.camera.target += offset;
        self.camera.eye += offset;
    }
}
