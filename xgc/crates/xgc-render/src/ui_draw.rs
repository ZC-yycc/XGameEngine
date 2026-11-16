//! UI 屏幕矩形 → NDC 模型矩阵；单位四边形覆盖 `rect`。

use super::camera::Mat4;

/// 输入是像素矩形 `[x, y, width, height]`，输出 [-1,1] NDC 的缩放 + 平移。
pub fn quad_matrix(rect: [f32; 4], viewport: [f32; 2]) -> Mat4 {
    let viewport_w = viewport[0].max(1.0);
    let viewport_h = viewport[1].max(1.0);
    let scale_x = rect[2] * 2.0 / viewport_w;
    let scale_y = -rect[3] * 2.0 / viewport_h;
    let center_x = rect[0] + rect[2] * 0.5;
    let center_y = rect[1] + rect[3] * 0.5;
    let translate_x = center_x * 2.0 / viewport_w - 1.0;
    let translate_y = 1.0 - center_y * 2.0 / viewport_h;
    [
        scale_x,
        0.0,
        0.0,
        0.0, //
        0.0,
        scale_y,
        0.0,
        0.0, //
        0.0,
        0.0,
        1.0,
        0.0, //
        translate_x,
        translate_y,
        0.0,
        1.0,
    ]
}
