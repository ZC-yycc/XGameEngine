use glam::{Mat4, Vec3};

/// 构建右手系透视投影矩阵。
///
/// `fov` 是垂直视场角，单位为弧度。
pub fn perspective(fov: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    Mat4::perspective_rh(fov, aspect, near, far)
}

/// 构建右手系 look-at 视图矩阵。
pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    Mat4::look_at_rh(eye, target, up)
}
