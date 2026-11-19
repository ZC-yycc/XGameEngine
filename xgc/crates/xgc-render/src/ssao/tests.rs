//! SSAO 的纯 CPU 回归：kernel 确定性、uniform 布局、设置消毒。

use super::*;

/// kernel 是固定种子 LCG 的产物：两次构建必须逐位相同（截图可复现的前提）。
#[test]
fn kernel_is_deterministic_and_stays_in_the_hemisphere() {
    let first = hemisphere_kernel();
    let second = hemisphere_kernel();
    assert_eq!(first, second, "kernel 必须是确定性常量表");
    for (index, sample) in first.iter().enumerate() {
        let length = (sample[0] * sample[0] + sample[1] * sample[1] + sample[2] * sample[2]).sqrt();
        // 向 +z 半球取点，长度落在 (0, 1]（二次分布缩放）。
        assert!(
            sample[2] >= 0.0,
            "第 {index} 个样本必须在 +z 半球：{sample:?}"
        );
        assert!(
            length > 0.0 && length <= 1.0,
            "第 {index} 个样本长度越界：{length}"
        );
        assert_eq!(sample[3], 0.0, "kernel 的 w 分量恒 0");
    }
}

/// uniform 必须与 `ssao.wgsl` 的 `SsaoParams` 逐字节对齐：448 B，
/// 偏移 0/64 两个矩阵、128 相机、144 kernel（16 × 16 B）、400 参数、
/// 416 全分辨率、432 半分辨率。
#[test]
fn params_match_the_wgsl_layout() {
    let kernel = hemisphere_kernel();
    let bytes = params_bytes(
        [1.0; 16],
        [2.0; 16],
        [3.0, 4.0, 5.0],
        &kernel,
        0.5,
        0.025,
        1.0,
        (640, 360),
        (320, 180),
    );
    assert_eq!(bytes.len(), PARAMS_BYTES, "SsaoParams 必须恰好 448 字节");
    let floats: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    assert_eq!(floats.len(), 112);
    assert_eq!(floats[0], 1.0, "偏移 0 = 逆视图投影第一项");
    assert_eq!(floats[16], 2.0, "偏移 64 = 视图投影第一项");
    assert_eq!(floats[32], 3.0, "偏移 128 = 相机 x");
    assert_eq!(floats[35], 1.0, "相机 w 固定 1.0（旧端同式）");
    // kernel 从第 36 个 f32 起，共 16 × 4。
    assert_eq!(floats[36], kernel[0][0], "偏移 144 = kernel 第一个分量");
    assert_eq!(floats[36 + 63], kernel[15][3], "kernel 末项");
    assert_eq!(floats[100], 0.5, "偏移 400 = 半径");
    assert_eq!(floats[101], 0.025, "偏移 404 = 偏置");
    assert_eq!(floats[102], 1.0, "偏移 408 = 强度");
    assert_eq!(floats[104], 640.0, "偏移 416 = 全分辨率宽");
    assert_eq!(floats[106], 1.0 / 640.0, "偏移 424 = 全分辨率 1/w");
    assert_eq!(floats[108], 320.0, "偏移 432 = 半分辨率宽");
    assert_eq!(floats[111], 1.0 / 180.0, "偏移 444 = 半分辨率 1/h");
}

/// 旧端 `SsaoConfig::sanitized` 同口径：非有限落回对应字段默认、合法值夹范围，
/// `enabled` 原样保留。
#[test]
fn sanitize_matches_the_old_engine_contract() {
    let defaults = SsaoSettings::default();
    assert!(!defaults.is_enabled(), "默认必须关");
    assert!(SsaoSettings::default().is_default());

    let garbage = SsaoSettings {
        enabled: true,
        strength: f32::NAN,
        radius: f32::INFINITY,
        bias: f32::NEG_INFINITY,
    }
    .sanitized();
    assert_eq!(garbage.strength, defaults.strength);
    assert_eq!(garbage.radius, defaults.radius);
    assert_eq!(garbage.bias, defaults.bias);

    let clamped = SsaoSettings {
        enabled: true,
        strength: 9.0,
        radius: 99.0,
        bias: 9.0,
    }
    .sanitized();
    assert_eq!(clamped.strength, SsaoSettings::MAX_STRENGTH);
    assert_eq!(clamped.radius, SsaoSettings::MAX_RADIUS);
    assert_eq!(clamped.bias, SsaoSettings::MAX_BIAS);

    let negative = SsaoSettings {
        enabled: true,
        strength: -1.0,
        radius: -2.0,
        bias: -3.0,
    }
    .sanitized();
    assert_eq!(negative.strength, 0.0);
    assert_eq!(negative.radius, 0.0);
    assert_eq!(negative.bias, 0.0);
    // 强度 0 与关等价：不跑四趟 pass。
    assert!(!negative.is_enabled());
}

/// 反投影守卫：`camera::invert` 只挡奇异矩阵，NaN / Inf 的行列式比较为假，
/// 会照样返回一张 NaN 矩阵。SSAO 必须自己把非有限也判成不可用（旧端
/// `determinant().is_finite() && != 0.0` 同式），否则遮蔽可能变 NaN。
#[test]
fn only_finite_invertible_matrices_allow_occlusion() {
    let projection = crate::camera::perspective_rh_zo(60f32.to_radians(), 1.0, 0.1, 100.0);
    assert!(
        invertible_view_projection(projection),
        "正常视图投影必须可用"
    );
    assert!(invertible_view_projection(IDENTITY), "单位矩阵可逆且有限");

    let singular = [0.0f32; 16];
    assert!(!invertible_view_projection(singular), "零矩阵必须判成退化");

    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut broken = projection;
        broken[5] = bad;
        assert!(
            !invertible_view_projection(broken),
            "非有限元素必须判成退化：{bad}"
        );
    }
}
