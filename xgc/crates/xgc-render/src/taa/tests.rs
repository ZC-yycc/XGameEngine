//! `taa` 的纯函数回归：抖动相位、单位换算、投影平移、设置消毒。

use super::*;

/// Halton 序列是纯函数：同一帧号永远同一个偏移，8 帧覆盖 8 个相位。
#[test]
fn jitter_is_a_pure_function_of_the_frame() {
    assert_eq!(jitter_offset(3), jitter_offset(3));
    assert_eq!(jitter_offset(0), jitter_offset(JITTER_SAMPLES));
    let samples: Vec<[f32; 2]> = (0..JITTER_SAMPLES).map(jitter_offset).collect();
    for (index, sample) in samples.iter().enumerate() {
        assert!(
            (-0.5..=0.5).contains(&sample[0]) && (-0.5..=0.5).contains(&sample[1]),
            "第 {index} 个抖动样本必须落在 (-0.5, 0.5] 像素内：{sample:?}"
        );
        for other in samples.iter().skip(index + 1) {
            assert_ne!(sample, other, "8 个抖动相位必须互不相同");
        }
    }
    // 8 相必须真的散开（低差异序列的意义）：x / y 的跨度都超过半个像素。
    for axis in 0..2 {
        let low = samples
            .iter()
            .map(|sample| sample[axis])
            .fold(f32::MAX, f32::min);
        let high = samples
            .iter()
            .map(|sample| sample[axis])
            .fold(f32::MIN, f32::max);
        assert!(
            high - low > 0.5,
            "第 {axis} 轴的抖动跨度必须超过半个像素：{low} .. {high}"
        );
    }
}

/// 像素偏移换算到裁剪空间是 2×，换算到纹理坐标时 y 要取反。
#[test]
fn jitter_units_follow_the_ndc_and_texture_conventions() {
    let ndc = jitter_to_ndc([0.5, -0.25], (8, 4));
    assert_eq!(ndc, [0.125, -0.125]);
    let uv = jitter_to_uv([0.5, -0.25], (8, 4));
    assert_eq!(uv, [0.0625, 0.0625]);
    assert_eq!(jitter_to_ndc([0.5, 0.5], (0, 0)), [0.0, 0.0]);
    assert_eq!(jitter_to_uv([0.5, 0.5], (0, 0)), [0.0, 0.0]);
}

/// 抖动投影等价于把裁剪空间平移 (ndc.x·w, ndc.y·w)：投影后的 ndc 正好偏移这么多。
#[test]
fn jitter_projection_shifts_the_clip_space() {
    let projection = crate::camera::perspective_rh_zo(1.0, 1.5, 0.1, 100.0);
    let point = [0.3, -0.4, 2.0, 1.0];
    let plain = crate::camera::transform_point(projection, point);
    let plain_ndc = [plain[0] / plain[3], plain[1] / plain[3]];
    let jitter = [0.25, -0.125];
    let jittered = jitter_projection(projection, jitter);
    let shifted = crate::camera::transform_point(jittered, point);
    let shifted_ndc = [shifted[0] / shifted[3], shifted[1] / shifted[3]];
    assert!(
        (shifted_ndc[0] - (plain_ndc[0] + jitter[0])).abs() < 1e-5
            && (shifted_ndc[1] - (plain_ndc[1] + jitter[1])).abs() < 1e-5,
        "抖动必须只平移裁剪空间的 x / y：{shifted_ndc:?} vs {:?}",
        [plain_ndc[0] + jitter[0], plain_ndc[1] + jitter[1]]
    );
    // z 不受影响（阴影 / 深度比较不能跟着抖）。
    assert!((shifted[2] / shifted[3] - plain[2] / plain[3]).abs() < 1e-6);
}

/// 容错口径与旧端一致：非有限权重落回缺省，合法值夹进 [0, 0.98]。
#[test]
fn settings_follow_the_old_engine_contract() {
    assert!(TaaSettings::default().is_default());
    assert!(!TaaSettings::default().is_enabled());
    assert_eq!(
        TaaSettings {
            enabled: true,
            history_weight: f32::NAN
        }
        .sanitized()
        .history_weight,
        DEFAULT_HISTORY_WEIGHT
    );
    assert_eq!(
        TaaSettings {
            enabled: true,
            history_weight: f32::INFINITY
        }
        .sanitized()
        .history_weight,
        DEFAULT_HISTORY_WEIGHT
    );
    assert_eq!(
        TaaSettings {
            enabled: true,
            history_weight: -1.0
        }
        .sanitized()
        .history_weight,
        0.0
    );
    assert_eq!(
        TaaSettings {
            enabled: true,
            history_weight: 4.0
        }
        .sanitized()
        .history_weight,
        MAX_HISTORY_WEIGHT
    );
}

#[test]
fn halton_sequence_matches_known_values() {
    assert_eq!(halton(1, 2), 0.5);
    assert_eq!(halton(2, 2), 0.25);
    assert_eq!(halton(3, 2), 0.75);
    assert_eq!(halton(4, 2), 0.125);
    let third = 1.0f32 / 3.0;
    assert!((halton(1, 3) - third).abs() < 1e-6);
    assert!((halton(2, 3) - 2.0 * third).abs() < 1e-6);
    // 8 相抖动必须两两不同（同一相位重复会让多帧累积不去噪）。
    for frame in 0..JITTER_SAMPLES {
        for other in (frame + 1)..JITTER_SAMPLES {
            assert_ne!(jitter_offset(frame), jitter_offset(other));
        }
    }
}

#[test]
fn zero_jitter_keeps_the_projection_bitwise() {
    // 含 -0.0 的矩阵：没有早退时 `+= 0.0 * w` 会把 -0.0 变成 +0.0。
    let projection = [
        -0.0, 0.0, -0.0, 1.0, 1.5, -0.0, 0.0, -0.0, 0.0, 2.5, -0.0, 0.0, 0.0, -0.0, 33.0, 0.0,
    ];
    let jittered = jitter_projection(projection, [0.0, 0.0]);
    for (index, (actual, expected)) in jittered.iter().zip(projection.iter()).enumerate() {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "零抖动必须逐位不变（第 {index} 项）"
        );
    }
}

#[test]
fn taa_params_matches_the_wgsl_layout() {
    let inv = [
        1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
    ];
    let prev = [
        17.0f32, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0, 25.0, 26.0, 27.0, 28.0, 29.0, 30.0,
        31.0, 32.0,
    ];
    let bytes = params_bytes(
        inv,
        prev,
        [0.5, 1.5, -2.5],
        [0.25, -0.75],
        [1.0 / 64.0, 1.0 / 32.0],
        0.9,
        1.0,
        [64.0, 32.0],
    );
    assert_eq!(bytes.len(), PARAMS_BYTES, "参数块必须恰好 176 字节");
    let read = |offset: usize| {
        let mut word = [0u8; 4];
        word.copy_from_slice(&bytes[offset..offset + 4]);
        f32::from_le_bytes(word)
    };
    let offsets = [
        (0usize, inv[0]),
        (60, inv[15]),
        (64, prev[0]),
        (124, prev[15]),
        (128, 0.5),
        (132, 1.5),
        (136, -2.5),
        (140, 1.0),
        (144, 0.25),
        (148, -0.75),
        (152, 1.0 / 64.0),
        (156, 1.0 / 32.0),
        (160, 0.9),
        (164, 1.0),
        (168, 64.0),
        (172, 32.0),
    ];
    for (offset, expected) in offsets {
        assert_eq!(
            read(offset).to_bits(),
            expected.to_bits(),
            "TaaParams 偏移 {offset} 必须与 WGSL 布局一致"
        );
    }
}
