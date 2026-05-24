//! Collider component types.

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// Supported collider shapes.
///
/// All sizes use half-extents (e.g. a unit cube has `half_extents = (0.5, 0.5, 0.5)`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColliderShape {
    /// Axis-aligned box with half-extents.
    Cuboid { half_extents: Vec3 },
    /// Sphere defined by radius.
    Ball { radius: f32 },
    /// Capsule aligned with the Y axis.
    Capsule { half_height: f32, radius: f32 },
    /// Cylinder aligned with the Y axis.
    Cylinder { half_height: f32, radius: f32 },
    /// Arbitrary triangle mesh (must be non-self-intersecting).
    Trimesh {
        vertices: Vec<[f32; 3]>,
        indices: Vec<[u32; 3]>,
    },
    /// Height-field collider (for terrain).
    HeightField {
        heights: Vec<f32>,
        nx: u32,
        nz: u32,
        scale: Vec3,
    },
}

impl ColliderShape {
    pub fn cuboid(half_extents: Vec3) -> Self {
        Self::Cuboid { half_extents }
    }

    pub fn ball(radius: f32) -> Self {
        Self::Ball { radius }
    }

    pub fn capsule(half_height: f32, radius: f32) -> Self {
        Self::Capsule { half_height, radius }
    }

    pub fn cylinder(half_height: f32, radius: f32) -> Self {
        Self::Cylinder { half_height, radius }
    }
}

/// Describes a collision shape attached to a rigid body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collider {
    pub shape: ColliderShape,
    /// Coulomb friction coefficient (0 = frictionless, 1 = high friction).
    pub friction: f32,
    /// Restitution (bounciness). 0 = inelastic, 1 = perfectly elastic.
    pub restitution: f32,
    /// Whether this collider is a sensor (detects overlaps without
    /// producing contact forces).
    pub is_sensor: bool,
    /// Density for automatic mass computation. Ignored when the parent
    /// rigid body has a manually-specified mass > 0.
    pub density: f32,
    /// Local offset from the parent rigid body's origin.
    pub offset: Vec3,
}

impl Default for Collider {
    fn default() -> Self {
        Self {
            shape: ColliderShape::Cuboid {
                half_extents: Vec3::splat(0.5),
            },
            friction: 0.5,
            restitution: 0.0,
            is_sensor: false,
            density: 1.0,
            offset: Vec3::ZERO,
        }
    }
}

impl Collider {
    pub fn cuboid(half_extents: Vec3) -> Self {
        Self {
            shape: ColliderShape::Cuboid { half_extents },
            ..Default::default()
        }
    }

    pub fn ball(radius: f32) -> Self {
        Self {
            shape: ColliderShape::Ball { radius },
            ..Default::default()
        }
    }

    pub fn capsule(half_height: f32, radius: f32) -> Self {
        Self {
            shape: ColliderShape::Capsule { half_height, radius },
            ..Default::default()
        }
    }
}
