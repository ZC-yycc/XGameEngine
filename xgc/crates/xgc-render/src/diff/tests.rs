//! `diff_breakdown` 的纯 CPU 回归：分桶、通道均值、块网格与最差块排序。
//!
//! 这些数字会进割接文档，算错一个桶就会把"舍入余差"说成"结构性差异"，
//! 所以用手工构造的小图逐项钉住。

use super::*;

fn image(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> ImageData {
    let mut rgba = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            rgba.push(pixel(x, y));
        }
    }
    ImageData {
        width,
        height,
        rgba,
    }
}

#[test]
fn buckets_split_rounding_from_structural_differences() {
    let base = image(32, 18, |_, _| [10, 20, 30, 255]);
    let mut noise = base.clone();
    noise.rgba[0] = [10, 20, 30, 255]; // 相同
    noise.rgba[1] = [11, 20, 30, 255]; // ±1
    noise.rgba[2] = [12, 20, 30, 255]; // 2
    noise.rgba[3] = [13, 20, 30, 255]; // 3：2..=3 的上边界
    noise.rgba[4] = [10, 24, 30, 255]; // G 差 4：4..=7 的下边界
    noise.rgba[5] = [17, 20, 30, 255]; // 7：4..=7 的上边界
    noise.rgba[6] = [20, 20, 30, 255]; // >= 8：结构性
    noise.rgba[7] = [10, 28, 30, 255]; // G 差 8：结构性的下边界
    let report = diff_breakdown(&base, &noise).expect("同尺寸");
    assert_eq!(report.pixels, 32 * 18);
    assert_eq!(report.buckets.identical, 32 * 18 - 7);
    assert_eq!(report.buckets.one, 1);
    assert_eq!(report.buckets.two_to_three, 2);
    assert_eq!(report.buckets.four_to_seven, 2);
    assert_eq!(report.buckets.structural, 2);
    // 结构性占比按像素算，不是按通道算。
    let expected = (2.0 / (32.0 * 18.0)) as f32;
    assert!((report.buckets.structural_ratio(report.pixels) - expected).abs() < 1e-6);
    // 通道均值只算 RGB：R = (1 + 2 + 3 + 7 + 10) / 像素数，
    // G = (4 + 8) / 像素数，B = 0。分母是像素数而不是差异像素数。
    let pixels = (32 * 18) as f32;
    assert!((report.channel_mean_abs[0] - 23.0 / pixels).abs() < 1e-6);
    assert!((report.channel_mean_abs[1] - 12.0 / pixels).abs() < 1e-6);
    assert_eq!(report.channel_mean_abs[2], 0.0);
}

#[test]
fn block_grid_and_worst_blocks_follow_the_pixels() {
    // 32x18 正好被 16x9 均匀切成 2x2 的块。
    let base = image(32, 18, |_, _| [0, 0, 0, 255]);
    let mut noise = base.clone();
    noise.rgba[2] = [9, 0, 0, 255]; // 行 0 列 1 的块：1/4
    noise.rgba[3] = [9, 0, 0, 255]; // 同上：2/4
    noise.rgba[17 * 32 + 31] = [9, 0, 0, 255]; // 行 8 的两个像素行（y 16..17）里 x=31：1/4
    let report = diff_breakdown(&base, &noise).expect("同尺寸");
    assert_eq!(
        report.block_ratios.len(),
        BREAKDOWN_COLUMNS * BREAKDOWN_ROWS
    );
    assert!((report.block_ratios[1] - 0.5).abs() < 1e-6);
    assert!((report.block_ratios[8 * BREAKDOWN_COLUMNS + 15] - 0.25).abs() < 1e-6);
    assert_eq!(report.worst_blocks[0].row, 0);
    assert_eq!(report.worst_blocks[0].column, 1);
    assert!((report.worst_blocks[0].ratio - 0.5).abs() < 1e-6);
    // 只改了块 (0,1) 与 (8,15)：零差异块不进榜，剩下按比例降序排列。
    assert_eq!(report.worst_blocks[1].row, 8);
    assert_eq!(report.worst_blocks[1].column, 15);
    assert_eq!(report.worst_blocks.len(), 2);
}

#[test]
fn spans_keep_every_pixel_when_the_axis_does_not_divide_evenly() {
    // 33 列切成 16 段：前 1 段 3 列、其余 2 列，首尾相接且不越界。
    let even = spans(33, 16);
    assert_eq!(even.len(), 16);
    assert_eq!(even[0], (0, 3));
    assert_eq!(even[1], (3, 5));
    assert_eq!(even[15].1, 33);
    for pair in even.windows(2) {
        assert_eq!(pair[0].1, pair[1].0, "相邻段必须首尾相接");
    }
    // 比段数还短的一轴也必须每段可处理（部分段为空）。
    let tiny = spans(3, 16);
    assert_eq!(tiny.iter().map(|(a, b)| b - a).sum::<usize>(), 3);
    assert!(tiny.iter().all(|(a, b)| a <= b));
}

#[test]
fn mismatched_sizes_are_rejected() {
    let left = image(4, 2, |_, _| [0, 0, 0, 255]);
    let right = image(4, 3, |_, _| [0, 0, 0, 255]);
    assert!(diff_breakdown(&left, &right).is_err());
}

#[test]
fn worst_blocks_break_ties_by_row_then_column() {
    // 32x18 被切成 16x9 个 2x2 块。四个块各差 1 个像素 ⇒ 比例全是 1/4：
    // 排序必须是行升序、同排再列升序（报告必须逐字节确定）。
    let base = image(32, 18, |_, _| [0, 0, 0, 255]);
    let mut noise = base.clone();
    noise.rgba[4] = [9, 0, 0, 255]; // 行 0 列 2
    noise.rgba[0] = [9, 0, 0, 255]; // 行 0 列 0
    noise.rgba[2 * 32] = [9, 0, 0, 255]; // 行 1（y = 2）列 0 ⇒ 必须排在所有行 0 块之后
    noise.rgba[2] = [9, 0, 0, 255]; // 行 0 列 1
    let report = diff_breakdown(&base, &noise).expect("同尺寸");
    assert_eq!(report.worst_blocks.len(), 4);
    let order = report
        .worst_blocks
        .iter()
        .map(|block| (block.row, block.column))
        .collect::<Vec<_>>();
    assert_eq!(order, vec![(0, 0), (0, 1), (0, 2), (1, 0)]);
    for block in &report.worst_blocks {
        assert!((block.ratio - 0.25).abs() < 1e-6);
    }
}

#[test]
fn worst_blocks_cap_at_five() {
    // 同一排 6 个等比例块：只保留前 5 块（`WORST_BLOCKS`），且列序升序。
    let base = image(32, 18, |_, _| [0, 0, 0, 255]);
    let mut noise = base.clone();
    for column in 0..6 {
        noise.rgba[column * 2] = [9, 0, 0, 255];
    }
    let report = diff_breakdown(&base, &noise).expect("同尺寸");
    // 写成字面 5 而不是只比 `WORST_BLOCKS`：否则把常量改成 6 时这条仍绿，
    // 而文档里「上限从 5 放宽到 6 会让用例红」的变异声明就不成立。
    assert_eq!(WORST_BLOCKS, 5);
    assert_eq!(report.worst_blocks.len(), WORST_BLOCKS);
    for (index, block) in report.worst_blocks.iter().enumerate() {
        assert_eq!(block.row, 0);
        assert_eq!(block.column, index);
    }
}

#[test]
fn alpha_only_differences_are_not_counted() {
    // alpha 不参与差异：只改 alpha 时必须全部落在"相同"桶，块比例也全 0。
    let base = image(32, 18, |_, _| [10, 20, 30, 255]);
    let mut noise = base.clone();
    noise.rgba[0] = [10, 20, 30, 0];
    noise.rgba[4 * 32 + 4] = [10, 20, 30, 128];
    let report = diff_breakdown(&base, &noise).expect("同尺寸");
    assert_eq!(report.buckets.identical, report.pixels);
    assert_eq!(report.buckets.structural, 0);
    assert_eq!(report.buckets.structural_ratio(report.pixels), 0.0);
    assert!(report.block_ratios.iter().all(|&ratio| ratio == 0.0));
    assert!(report.worst_blocks.is_empty());
    assert_eq!(report.channel_mean_abs, [0.0, 0.0, 0.0]);
}

#[test]
fn parity_classifies_a_one_pixel_edge_shift_as_edge() {
    // 黑底白条：把白条整体右移 1 列，差异像素都贴着轮廓。
    let base = image(8, 8, |x, _| {
        if x == 4 {
            [255, 255, 255, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    let shifted = image(8, 8, |x, _| {
        if x == 5 {
            [255, 255, 255, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    let report = parity_report(&base, &shifted).expect("同尺寸");
    // 每行 x=4 / x=5 两个像素不一致，都在轮廓 1px 带内。
    assert_eq!(report.structural, 16);
    assert_eq!(report.edge_structural, 16);
    assert_eq!(report.interior_structural, 0);
    assert_eq!(report.interior_ratio, 0.0);
}

#[test]
fn parity_keeps_an_interior_blot_interior() {
    // 纯色底上一个 +20 的孤立像素：邻域色差都只有 20（<40），不算轮廓。
    let base = image(8, 8, |_, _| [100, 100, 100, 255]);
    let mut blot = base.clone();
    blot.rgba[3 * 8 + 3] = [130, 100, 100, 255];
    let report = parity_report(&base, &blot).expect("同尺寸");
    assert_eq!(report.structural, 1);
    assert_eq!(report.edge_structural, 0);
    assert_eq!(report.interior_structural, 1);
    assert!((report.interior_ratio - 1.0 / 64.0).abs() < 1e-6);
}

#[test]
fn parity_ignores_one_lsb_noise_but_keeps_the_mean() {
    let base = image(8, 8, |_, _| [100, 100, 100, 255]);
    let mut noise = base.clone();
    for pixel in noise.rgba.iter_mut() {
        pixel[0] += 1;
    }
    let report = parity_report(&base, &noise).expect("同尺寸");
    assert_eq!(report.structural, 0);
    assert_eq!(report.edge_structural, 0);
    assert_eq!(report.interior_structural, 0);
    assert_eq!(report.max_delta, 1);
    assert!((report.mean_abs - 0.25).abs() < 1e-6);
}

#[test]
fn parity_rejects_mismatched_sizes() {
    let left = image(4, 4, |_, _| [0, 0, 0, 255]);
    let right = image(5, 4, |_, _| [0, 0, 0, 255]);
    assert!(parity_report(&left, &right).is_err());
}
