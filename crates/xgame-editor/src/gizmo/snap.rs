//! Gizmo 吸附：把拖拽结果吸到步长的整数倍。
//!
//! # 为什么吸附的是**绝对值**而不是增量
//!
//! 位置与缩放本来就是"绝对数字"（Inspector 里显示的就是它们），把结果吸到
//! `0.5` 的整数倍，作者能一眼对上"这一格是 2.0"。如果把**位移增量**吸到 0.5 的
//! 整数倍，起点在 0.3 的物体永远停在 `0.3 + 0.5k` 这种读不出来的数上 —— 那正是
//! 商业引擎都不这么做的原因。
//!
//! 旋转也一样：吸附的是**欧拉角分量**（与 Inspector 显示的同一把尺子），所以
//! 拖到 15° 步长时看到的就是 `15 / 30 / 45`，而不是 `8.3 + 15k`。
//!
//! # 0 与非法值
//!
//! 步长 `<= 0` 或非有限值 = **不吸附**（返回原值）。这既是"关闭"的表达，
//! 也挡掉 `editor.ron` 里手改出来的 `NaN` —— 否则 `round(NaN / step)` 会把
//! 整个变换毒成 NaN，物体当场从画面消失。

use xgame_math::glam::Vec3;

/// 三个工具各自的吸附步长（`<= 0` = 该工具不吸附）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GizmoSnap {
    /// 平移步长（世界单位 / 米）。
    pub translate: f32,
    /// 旋转步长（度）。
    pub rotate_deg: f32,
    /// 缩放步长（倍率）。
    pub scale: f32,
}

impl GizmoSnap {
    /// 全关（拖拽不吸附）。
    pub const OFF: Self = Self {
        translate: 0.0,
        rotate_deg: 0.0,
        scale: 0.0,
    };

    /// 是否至少有一个工具在吸附（UI 的按钮高亮用）。
    pub fn is_active(&self) -> bool {
        self.translate > 0.0 || self.rotate_deg > 0.0 || self.scale > 0.0
    }

    /// 吸附一个标量；`step <= 0` 或非有限值原样返回。
    pub fn snap_scalar(value: f32, step: f32) -> f32 {
        if !step.is_finite() || step <= 0.0 || !value.is_finite() {
            return value;
        }
        (value / step).round() * step
    }

    /// 逐分量吸附位置 / 缩放。
    pub fn snap_vec(value: Vec3, step: f32) -> Vec3 {
        if !step.is_finite() || step <= 0.0 {
            return value;
        }
        Vec3::new(
            Self::snap_scalar(value.x, step),
            Self::snap_scalar(value.y, step),
            Self::snap_scalar(value.z, step),
        )
    }
}

impl Default for GizmoSnap {
    fn default() -> Self {
        Self::OFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_snaps_to_the_nearest_multiple() {
        assert_eq!(GizmoSnap::snap_scalar(1.24, 0.5), 1.0);
        assert_eq!(GizmoSnap::snap_scalar(1.26, 0.5), 1.5);
        assert_eq!(GizmoSnap::snap_scalar(-1.26, 0.5), -1.5, "负方向对称");
        // 37 离 30 更近（差 7），离 45 差 8 —— 就近取 30，不是向上取。
        assert_eq!(GizmoSnap::snap_scalar(37.0, 15.0), 30.0, "角度同款就近");
    }

    #[test]
    fn zero_and_invalid_steps_are_passthrough() {
        assert_eq!(GizmoSnap::snap_scalar(1.23, 0.0), 1.23);
        assert_eq!(GizmoSnap::snap_scalar(1.23, -1.0), 1.23);
        assert_eq!(GizmoSnap::snap_scalar(1.23, f32::NAN), 1.23);
        // 步长合法但值坏了：也原样返回（不许把 NaN 变成 0 之外的意外）。
        assert!(GizmoSnap::snap_scalar(f32::NAN, 0.5).is_nan());
    }

    #[test]
    fn vectors_snap_per_component() {
        let snapped = GizmoSnap::snap_vec(Vec3::new(1.2, -0.3, 4.9), 1.0);
        assert_eq!(snapped, Vec3::new(1.0, -0.0, 5.0));
        assert_eq!(GizmoSnap::snap_vec(Vec3::new(1.2, 2.0, 3.0), 0.0).x, 1.2);
    }

    #[test]
    fn active_flag_follows_any_nonzero_step() {
        assert!(!GizmoSnap::OFF.is_active());
        assert!(GizmoSnap {
            translate: 0.5,
            ..GizmoSnap::OFF
        }
        .is_active());
    }
}
