//! 图像对照：尺寸、逐像素差异比例与平均绝对误差。

use super::image::ImageData;

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
