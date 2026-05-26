use xgame_math::glam::{Mat4, Quat, Vec3};
use xgame_math::Transform;

/// 模型（世界）矩阵的 GPU 统一变量。
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ModelUniform {
    pub model: [[f32; 4]; 4],
}

impl Default for ModelUniform {
    fn default() -> Self {
        Self {
            model: Mat4::IDENTITY.to_cols_array_2d(),
        }
    }
}

impl From<Mat4> for ModelUniform {
    fn from(m: Mat4) -> Self {
        Self {
            model: m.to_cols_array_2d(),
        }
    }
}

/// CPU 端模型变换，附带移动辅助方法。
#[derive(Debug, Clone)]
pub struct ModelTransform {
    pub transform: Transform,
    pub move_speed: f32,
}

impl Default for ModelTransform {
    fn default() -> Self {
        Self {
            transform: Transform::IDENTITY,
            move_speed: 3.0,
        }
    }
}

impl ModelTransform {
    pub fn matrix(&self) -> Mat4 {
        self.transform.local_matrix()
    }

    pub fn uniform(&self) -> ModelUniform {
        self.matrix().into()
    }

    /// Move the model in world-space directions.
    pub fn translate_local(&mut self, delta: Vec3) {
        self.transform.translation += delta;
    }

    /// 向前/向后移动（模型的局部 +Z 方向）。
    pub fn move_forward(&mut self, amount: f32, dt: f32) {
        let fwd = self.transform.forward();
        self.transform.translation += fwd * amount * dt * self.move_speed;
    }

    /// 向右/向左移动（模型的局部 +X 方向）。
    pub fn move_right(&mut self, amount: f32, dt: f32) {
        let right = self.transform.right();
        self.transform.translation += right * amount * dt * self.move_speed;
    }

    /// 向上/向下移动（世界 Y 方向）。
    pub fn move_up(&mut self, amount: f32, dt: f32) {
        self.transform.translation.y += amount * dt * self.move_speed;
    }

    /// 绕模型局部 Y 轴旋转。
    pub fn rotate_y(&mut self, angle: f32) {
        self.transform.rotation *= Quat::from_rotation_y(angle);
    }
}
