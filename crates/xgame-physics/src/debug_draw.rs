//! Debug-draw lines for physics visualisation.
//!
//! The physics engine can emit line segments representing collider outlines
//! and contact points. The renderer can consume these to draw wireframe
//! overlays.

use glam::Vec3;
use xgame_core::Color;

/// A single debug line segment with start / end points and a colour.
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

/// Collects debug-draw primitives each frame.
///
/// Call `clear()` at the start of a frame, then `emit_*` methods
/// to record lines. The renderer can then consume `lines()`.
#[derive(Debug, Clone, Default)]
pub struct DebugDrawBuffer {
    lines: Vec<DebugLine>,
}

impl DebugDrawBuffer {
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    /// Clear all lines — call at the start of each frame before emitting.
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Emit a line segment.
    pub fn add_line(&mut self, start: Vec3, end: Vec3, color: Color) {
        self.lines.push(DebugLine::new(start, end, color));
    }

    /// Emit an axis-aligned box outline.
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

    /// Emit a cross at a point (useful for contact points).
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
