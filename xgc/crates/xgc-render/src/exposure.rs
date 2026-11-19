//! 画面曝光：ACES 曲线之前的线性光乘子。
//!
//! 旧引擎把曝光放在项目级 `render.exposure`（`RenderConfig`），各宿主都读同一份
//! 配置；XGC 保持同一语义（目前只有 `xgc-cli` 接线：无头截图 / run 路径，编辑器与
//! 独立游戏的窗口宿主还没读 `project.ron`，与天空 / 光晕 / FXAA 是同一个既有缺口）：
//!
//! - 缺省 1.0（不增不减）：项目不写这一节、或显式写 1.0，都走同一条乘法 ⇒ 帧
//!   输出逐字节不变（旧端 `RenderConfig.exposure` 没有 `skip_serializing_if`，老
//!   项目其实总是显式落盘 `exposure: 1.0`）；
//! - 曝光 0 是"全黑画面"而不是"关掉曝光"，非正 / 非有限值一律落回缺省；
//! - 合法但离谱的量级夹进 [`MIN_EXPOSURE`] / [`MAX_EXPOSURE`]：20 已足够
//!   "夜里看太阳"，再大只是整幅纯白。
//!
//! 消费点有两处，而且必须用同一个乘子：tone-map 的合成（`tone_map.wgsl`）与
//! 光晕的亮部提取（`bloom.wgsl` 的 `fs_bright`）。光晕阈值是按**曝光后**的
//! 亮度定义的，只给一处乘曝光会让"多亮算亮"与实际画面分叉。

/// 缺省曝光（不增不减）。
pub const EXPOSURE_DEFAULT: f32 = 1.0;

/// 曝光下限：0 会让画面全黑，不是"关闭曝光"的形态。
pub const MIN_EXPOSURE: f32 = 0.05;

/// 曝光上限：再大只会把整幅画推到纯白。
pub const MAX_EXPOSURE: f32 = 20.0;

/// 按旧端 `RenderConfig::sanitized` 的口径消毒一个曝光值。
///
/// 清单是手改的文本：`NaN` / 无穷 / 非正值直接落回缺省（`f32::clamp` 对 NaN
/// 原样返回，不能靠它兜）；合法但越界的量级再夹进安全范围。
pub fn sanitize_exposure(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value.clamp(MIN_EXPOSURE, MAX_EXPOSURE)
    } else {
        EXPOSURE_DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::{sanitize_exposure, EXPOSURE_DEFAULT, MAX_EXPOSURE, MIN_EXPOSURE};

    #[test]
    fn default_leaves_the_image_untouched() {
        assert_eq!(EXPOSURE_DEFAULT, 1.0);
        assert_eq!(sanitize_exposure(1.0), 1.0);
    }

    #[test]
    fn non_finite_and_non_positive_values_fall_back_to_the_default() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -3.0] {
            assert_eq!(
                sanitize_exposure(value),
                EXPOSURE_DEFAULT,
                "曝光 {value} 必须落回缺省（0 = 全黑，不是关闭态）"
            );
        }
    }

    #[test]
    fn legal_values_are_clamped_into_the_safe_range() {
        assert_eq!(sanitize_exposure(0.0001), MIN_EXPOSURE);
        assert_eq!(sanitize_exposure(1e9), MAX_EXPOSURE);
        assert_eq!(sanitize_exposure(2.5), 2.5);
    }
}
