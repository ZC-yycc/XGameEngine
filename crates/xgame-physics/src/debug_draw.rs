//! 物理调试绘制线条。
//!
//! 物理引擎可输出表示碰撞体轮廓和接触点的线段。渲染器可消费这些数据绘制线框叠加层。

use glam::Vec3;
use xgame_core::Color;

/// 单个调试线段，包含起点、终点和颜色。
#[derive(Debug, Clone, Copy)]
pub struct DebugLine {
    pub start: Vec3,
    pub end: Vec3,
    pub color: Color,
}

impl DebugLine {
    pub fn new(start: Vec3, end: Vec3, color: Color) -> Self {
        Self { start, end, color }
    }
}

/// 每帧收集调试绘制图元。
///
/// 在每帧开始时调用 `clear()`，然后使用 `emit_*` 方法记录线条。渲染器通过 `lines()` 消费。
#[derive(Debug, Clone, Default)]
pub struct DebugDrawBuffer {
    lines: Vec<DebugLine>,
}

impl DebugDrawBuffer {
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    /// 清除所有线条——在每帧输出前调用。
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// 输出一条线段。
    pub fn add_line(&mut self, start: Vec3, end: Vec3, color: Color) {
        self.lines.push(DebugLine::new(start, end, color));
    }

    /// 输出一个轴对齐盒体的轮廓线。
    pub fn add_cuboid(&mut self, center: Vec3, half_extents: Vec3, color: Color) {
        let min = center - half_extents;
        let max = center + half_extents;

        let corners = [
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, min.y, min.z),
            Vec3::new(max.x, max.y, min.z),
            Vec3::new(min.x, max.y, min.z),
            Vec3::new(min.x, min.y, max.z),
            Vec3::new(max.x, min.y, max.z),
            Vec3::new(max.x, max.y, max.z),
            Vec3::new(min.x, max.y, max.z),
        ];

        let edges = [
            (0, 1), (1, 2), (2, 3), (3, 0), // front
            (4, 5), (5, 6), (6, 7), (7, 4), // back
            (0, 4), (1, 5), (2, 6), (3, 7), // connects
        ];

        for (a, b) in edges {
            self.add_line(corners[a], corners[b], color);
        }
    }

    /// 在某个点输出十字标记（用于接触点）。
    pub fn add_cross(&mut self, position: Vec3, size: f32, color: Color) {
        let s = size * 0.5;
        self.add_line(
            position - Vec3::new(s, 0.0, 0.0),
            position + Vec3::new(s, 0.0, 0.0),
            color,
        );
        self.add_line(
            position - Vec3::new(0.0, s, 0.0),
            position + Vec3::new(0.0, s, 0.0),
            color,
        );
        self.add_line(
            position - Vec3::new(0.0, 0.0, s),
            position + Vec3::new(0.0, 0.0, s),
            color,
        );
    }

    /// Return all recorded lines.
    pub fn lines(&self) -> &[DebugLine] {
        &self.lines
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}
