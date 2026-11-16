//! 视口投影与 Gizmo 命中辅助。

use eframe::egui;

/// 列主序 MVP 把世界点投到图像像素坐标；裁剪外返回 None。
pub(crate) fn project(mvp: [f32; 16], point: [f32; 3], size: [f32; 2]) -> Option<egui::Pos2> {
    let x = mvp[0] * point[0] + mvp[4] * point[1] + mvp[8] * point[2] + mvp[12];
    let y = mvp[1] * point[0] + mvp[5] * point[1] + mvp[9] * point[2] + mvp[13];
    let z = mvp[2] * point[0] + mvp[6] * point[1] + mvp[10] * point[2] + mvp[14];
    let w = mvp[3] * point[0] + mvp[7] * point[1] + mvp[11] * point[2] + mvp[15];
    if w <= 1e-6 || z < 0.0 || z > w {
        return None;
    }
    Some(egui::pos2(
        (x / w * 0.5 + 0.5) * size[0],
        (1.0 - (y / w * 0.5 + 0.5)) * size[1],
    ))
}

pub(crate) fn distance_to_segment(point: egui::Pos2, start: egui::Pos2, end: egui::Pos2) -> f32 {
    let segment = end - start;
    let length_sq = segment.length_sq();
    if length_sq <= f32::EPSILON {
        return (point - start).length();
    }
    let t = ((point - start).dot(segment) / length_sq).clamp(0.0, 1.0);
    (point - (start + segment * t)).length()
}
