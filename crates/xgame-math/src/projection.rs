use glam::{Mat4, Vec3};

/// Build a right-handed perspective projection matrix.
///
/// `fov` is the vertical field-of-view in radians.
pub fn perspective(fov: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    Mat4::perspective_rh(fov, aspect, near, far)
}

/// Build a right-handed look-at view matrix.
pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    Mat4::look_at_rh(eye, target, up)
}
