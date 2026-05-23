use xgame_math::glam::{Mat4, Vec3};
use xgame_math::projection;

// ---------------------------------------------------------------------------
// GPU uniform (must match WGSL layout)
// ---------------------------------------------------------------------------

/// Matches the WGSL `CameraUniform` struct byte-for-byte.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    pub view_proj: [[f32; 4]; 4],
    pub position: [f32; 4],
}

// ---------------------------------------------------------------------------
// Light GPU uniform
// ---------------------------------------------------------------------------

/// Matches the WGSL `LightUniform` struct.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LightUniform {
    /// World-space light direction. w = padding.
    pub direction: [f32; 4],
    /// Light colour (RGB). w = intensity multiplier.
    pub color: [f32; 4],
    /// Ambient colour (RGB). w = padding.
    pub ambient: [f32; 4],
}

/// CPU-side directional light descriptor.
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
// Camera
// ---------------------------------------------------------------------------

/// A perspective camera with position, look-at target, and up vector.
#[derive(Debug, Clone)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    /// Vertical field-of-view in radians.
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
// Orbit camera controller
// ---------------------------------------------------------------------------

/// Mouse-driven orbit camera: drag to rotate, scroll to zoom.
#[derive(Debug, Clone)]
pub struct OrbitCamera {
    pub camera: Camera,
    /// Horizontal rotation angle (radians).
    pub yaw: f32,
    /// Vertical rotation angle (radians), clamped to avoid gimbal lock.
    pub pitch: f32,
    /// Distance from the look-at target.
    pub distance: f32,
    /// Sensitivity for mouse drag (radians per pixel).
    pub sensitivity: f32,
    /// Sensitivity for scroll zoom.
    pub zoom_speed: f32,
    pub min_distance: f32,
    pub max_distance: f32,
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
        }
    }

    /// Rotate by mouse delta (in pixels).
    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * self.sensitivity;
        self.pitch += dy * self.sensitivity;
        self.pitch = self.pitch.clamp(
            -std::f32::consts::FRAC_PI_2 + 0.01,
            std::f32::consts::FRAC_PI_2 - 0.01,
        );
    }

    /// Zoom by scroll delta.
    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance - delta * self.zoom_speed)
            .clamp(self.min_distance, self.max_distance);
    }

    /// Recompute camera position from spherical coordinates.
    pub fn update(&mut self) {
        self.camera.eye = Self::spherical_to_cartesian(self.yaw, self.pitch, self.distance);
    }

    fn spherical_to_cartesian(yaw: f32, pitch: f32, distance: f32) -> Vec3 {
        let x = distance * pitch.cos() * yaw.cos();
        let y = distance * pitch.sin();
        let z = distance * pitch.cos() * yaw.sin();
        Vec3::new(x, y, z)
    }
}
