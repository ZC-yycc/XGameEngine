//! 图像对照：尺寸、逐像素差异比例与平均绝对误差。
//!
//! 除了总比例，另外给一份「余差归因」：把每个像素按最大通道差值分桶，并统计
//! 16×9 块的差异分布。视觉割接门槛上真正要回答的是"剩下的差异是舍入还是结构"，
//! 只看 32% 这个总数会把 ±1 的渲染舍入和阴影 / UI 的结构性差异混为一谈。

use super::image::ImageData;

/// 余差归因的块网格：16 列 × 9 行（16:9 画面下每块近似正方形）。
pub const BREAKDOWN_COLUMNS: usize = 16;
/// 见 [`BREAKDOWN_COLUMNS`]。
pub const BREAKDOWN_ROWS: usize = 9;
/// 报告里列出多少个最差块。
pub const WORST_BLOCKS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageDiff {
    pub width: u32,
    pub height: u32,
    pub pixels: u32,
    /// 任一通道差值 > threshold 的像素数。
    pub different: u32,
    /// 0..1。
    pub ratio: f32,
    /// 所有通道绝对差均值（0..255）。
    pub mean_abs: f32,
    pub max_delta: u8,
}

/// 按最大 RGB 通道差值分桶的像素数（alpha 不计：画面最终 alpha 恒为 1）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffBuckets {
    /// 逐位相同。
    pub identical: u32,
    /// 最大通道差恰为 1，通常是浮点 → 8 位编码的舍入。
    pub one: u32,
    /// 最大通道差 2..=3。
    pub two_to_three: u32,
    /// 最大通道差 4..=7。
    pub four_to_seven: u32,
    /// 最大通道差 >= 8：几何 / 阴影 / UI 这类结构性差异。
    pub structural: u32,
}

impl DiffBuckets {
    /// 结构性差异占全部像素的比例（0..1）。
    pub fn structural_ratio(&self, pixels: u32) -> f32 {
        self.structural as f32 / pixels.max(1) as f32
    }
}

/// 一个块的差异情况。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockDiff {
    pub row: usize,
    pub column: usize,
    /// 块内任一通道不同的像素比例（0..1）。
    pub ratio: f32,
}

/// `image-diff --report` 的完整报告。
#[derive(Clone, Debug, PartialEq)]
pub struct ImageDiffReport {
    pub width: u32,
    pub height: u32,
    pub pixels: u32,
    pub buckets: DiffBuckets,
    /// RGB 三通道各自的平均绝对差（0..255）。
    pub channel_mean_abs: [f32; 3],
    /// 行优先的块差异比例，长度 = [`BREAKDOWN_COLUMNS`] × [`BREAKDOWN_ROWS`]。
    pub block_ratios: Vec<f32>,
    /// 差异比例最高的若干块（比例降序，平手按行 / 列升序）。
    pub worst_blocks: Vec<BlockDiff>,
}

pub fn diff_images(a: &ImageData, b: &ImageData, threshold: u8) -> Result<ImageDiff, String> {
    if a.width != b.width || a.height != b.height {
        return Err(format!(
            "尺寸不同：{}x{} vs {}x{}",
            a.width, a.height, b.width, b.height
        ));
    }
    let pixels = a.width * a.height;
    let mut different = 0u32;
    let mut sum = 0u64;
    let mut max_delta = 0u8;
    for (left, right) in a.rgba.iter().zip(&b.rgba) {
        let mut changed = false;
        for channel in 0..4 {
            let delta = left[channel].abs_diff(right[channel]);
            sum += u64::from(delta);
            max_delta = max_delta.max(delta);
            if delta > threshold {
                changed = true;
            }
        }
        if changed {
            different += 1;
        }
    }
    let channels = u64::from(pixels.max(1)) * 4;
    Ok(ImageDiff {
        width: a.width,
        height: a.height,
        pixels,
        different,
        ratio: different as f32 / pixels.max(1) as f32,
        mean_abs: sum as f32 / channels as f32,
        max_delta,
    })
}

/// 把差异拆成「舍入 / 结构性」两半，并给出最差的块。
///
/// 与 `diff_images` 的 `threshold` 无关：分桶看的是最大通道差，阈值只影响
/// "算不算不同"的计数。两者并排看，才能判断余差是加一道 `round` 能解决的，
/// 还是必须改渲染路径。
pub fn diff_breakdown(a: &ImageData, b: &ImageData) -> Result<ImageDiffReport, String> {
    if a.width != b.width || a.height != b.height {
        return Err(format!(
            "尺寸不同：{}x{} vs {}x{}",
            a.width, a.height, b.width, b.height
        ));
    }
    let width = a.width as usize;
    let height = a.height as usize;
    let pixels = (width * height) as u32;
    let column_spans = spans(width, BREAKDOWN_COLUMNS);
    let row_spans = spans(height, BREAKDOWN_ROWS);
    let mut buckets = DiffBuckets::default();
    let mut channel_sums = [0u64; 3];
    let mut block_changed = vec![0u32; BREAKDOWN_COLUMNS * BREAKDOWN_ROWS];
    let mut block_totals = vec![0u32; BREAKDOWN_COLUMNS * BREAKDOWN_ROWS];
    for (row, &(start, end)) in row_spans.iter().enumerate() {
        for y in start..end {
            for (column, &(left, right)) in column_spans.iter().enumerate() {
                let block = row * BREAKDOWN_COLUMNS + column;
                for x in left..right {
                    let index = y * width + x;
                    let ours = a.rgba[index];
                    let theirs = b.rgba[index];
                    let mut max_delta = 0u8;
                    for channel in 0..3 {
                        let delta = ours[channel].abs_diff(theirs[channel]);
                        channel_sums[channel] += u64::from(delta);
                        max_delta = max_delta.max(delta);
                    }
                    match max_delta {
                        0 => buckets.identical += 1,
                        1 => buckets.one += 1,
                        2..=3 => buckets.two_to_three += 1,
                        4..=7 => buckets.four_to_seven += 1,
                        _ => buckets.structural += 1,
                    }
                    if max_delta > 0 {
                        block_changed[block] += 1;
                    }
                    block_totals[block] += 1;
                }
            }
        }
    }
    let block_ratios = block_changed
        .iter()
        .zip(&block_totals)
        .map(|(&changed, &total)| changed as f32 / total.max(1) as f32)
        .collect::<Vec<_>>();
    let mut worst_blocks = (0..block_ratios.len())
        .filter(|&block| block_ratios[block] > 0.0)
        .map(|block| BlockDiff {
            row: block / BREAKDOWN_COLUMNS,
            column: block % BREAKDOWN_COLUMNS,
            ratio: block_ratios[block],
        })
        .collect::<Vec<_>>();
    // 平手按行 / 列升序：`sort_by` 是稳定排序，而 `worst_blocks` 的收集顺序
    // 已经是块号（行优先）升序，所以比例相同时自然保持行 / 列升序。
    // 注意不要换成 `sort_unstable_by` —— 那会让平手顺序依赖实现，报告就不再逐字节确定。
    worst_blocks.sort_by(|left, right| {
        right
            .ratio
            .partial_cmp(&left.ratio)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    worst_blocks.truncate(WORST_BLOCKS);
    let channels = u64::from(pixels.max(1));
    Ok(ImageDiffReport {
        width: a.width,
        height: a.height,
        pixels,
        buckets,
        channel_mean_abs: [
            channel_sums[0] as f32 / channels as f32,
            channel_sums[1] as f32 / channels as f32,
            channel_sums[2] as f32 / channels as f32,
        ],
        block_ratios,
        worst_blocks,
    })
}

/// 轮廓带判定的邻域色差阈值：中心像素与 8 邻域任一分量差超过它就当轮廓像素。
///
/// 与「结构性差异」的 8 不是同一个口径：前者回答"这里是不是几何轮廓"（AA 覆盖差
/// 只会落在轮廓两侧 1px），后者回答"差得够不够多"。两个常量分开写，改一个不会
/// 悄悄改另一个。
pub const PARITY_EDGE_THRESHOLD: u8 = 40;

/// 视觉对等判定报告：把结构性差异拆成「轮廓 1px 带的 AA 覆盖差」与「内部差」。
///
/// 两个独立渲染器在同一台机器上也做不到逐字节一致：±1 LSB 是浮点 → 8 位编码的
/// 舍入，轮廓附近的 ≥8 差是 MSAA 覆盖差（三角边落在采样点两侧）。真正该追问的是
/// **内部**（不在任一图轮廓 1px 带内）的结构性差异——那是着色 / 几何 / 阴影的语义差。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParityReport {
    pub width: u32,
    pub height: u32,
    pub pixels: u32,
    /// 所有通道绝对差均值（0..255，RGBA 四通道，与 [`ImageDiff::mean_abs`] 同口径）。
    pub mean_abs: f32,
    pub max_delta: u8,
    /// 最大 RGB 通道差 >= 8 的像素数。
    pub structural: u32,
    /// 结构性像素占比（0..1）。
    pub structural_ratio: f32,
    /// 落在任一图像自身轮廓 1px 带内的结构性像素。
    pub edge_structural: u32,
    /// 不在轮廓带内的结构性像素——视觉对等真正要审的那部分。
    pub interior_structural: u32,
    /// 内部结构性像素占全部像素的比例（0..1）。
    pub interior_ratio: f32,
}

/// 逐像素判断「是否在轮廓 1px 带内」；轮廓在两张图里各判一次，取并集。
fn edge_mask(image: &ImageData) -> Vec<bool> {
    let width = image.width as usize;
    let height = image.height as usize;
    let mut mask = vec![false; width * height];
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let center = image.rgba[index];
            'neighbor: for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    let other = image.rgba[ny as usize * width + nx as usize];
                    for channel in 0..3 {
                        if center[channel].abs_diff(other[channel]) > PARITY_EDGE_THRESHOLD {
                            mask[index] = true;
                            break 'neighbor;
                        }
                    }
                }
            }
        }
    }
    mask
}

/// 视觉对等判定：把结构性差异拆成轮廓带与内部两部分。
///
/// 判定门槛由调用方决定（CLI `xgc parity` / 割接文档）；这里只负责分类与计数，
/// 保证同一份图像永远得到同一组数字。
pub fn parity_report(a: &ImageData, b: &ImageData) -> Result<ParityReport, String> {
    if a.width != b.width || a.height != b.height {
        return Err(format!(
            "尺寸不同：{}x{} vs {}x{}",
            a.width, a.height, b.width, b.height
        ));
    }
    let pixels = a.width * a.height;
    let edges_a = edge_mask(a);
    let edges_b = edge_mask(b);
    let mut sum = 0u64;
    let mut max_delta = 0u8;
    let mut structural = 0u32;
    let mut edge_structural = 0u32;
    for (index, (left, right)) in a.rgba.iter().zip(&b.rgba).enumerate() {
        let mut max_rgb = 0u8;
        for channel in 0..3 {
            let delta = left[channel].abs_diff(right[channel]);
            max_delta = max_delta.max(delta);
            max_rgb = max_rgb.max(delta);
        }
        // 均值与 `diff_images` 同口径：alpha 也计入（画面最终 alpha 恒为 1，
        // 但这里不假设调用方）。
        for channel in 3..4 {
            sum += u64::from(left[channel].abs_diff(right[channel]));
            max_delta = max_delta.max(left[channel].abs_diff(right[channel]));
        }
        sum += u64::from(left[0].abs_diff(right[0]))
            + u64::from(left[1].abs_diff(right[1]))
            + u64::from(left[2].abs_diff(right[2]));
        if max_rgb >= 8 {
            structural += 1;
            if edges_a[index] || edges_b[index] {
                edge_structural += 1;
            }
        }
    }
    let interior_structural = structural - edge_structural;
    let channels = u64::from(pixels.max(1)) * 4;
    Ok(ParityReport {
        width: a.width,
        height: a.height,
        pixels,
        mean_abs: sum as f32 / channels as f32,
        max_delta,
        structural,
        structural_ratio: structural as f32 / pixels.max(1) as f32,
        edge_structural,
        interior_structural,
        interior_ratio: interior_structural as f32 / pixels.max(1) as f32,
    })
}

/// 把一条轴按 `count` 段切分；除不尽的余数平均分给前面的段（不丢像素）。
fn spans(length: usize, count: usize) -> Vec<(usize, usize)> {
    let mut spans = Vec::with_capacity(count);
    let base = length / count;
    let extra = length % count;
    let mut start = 0;
    for segment in 0..count {
        let size = base + usize::from(segment < extra);
        spans.push((start, start + size));
        start += size;
    }
    spans
}

#[cfg(test)]
mod tests;
