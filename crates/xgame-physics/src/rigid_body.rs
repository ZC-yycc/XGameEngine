//! Rigid-body component types.

use glam::Vec3;
use serde::{Deserialize, Serialize};
use xgame_math::Transform;

/// The type of a rigid body determines how it interacts with forces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RigidBodyType {
    /// Immovable, infinite mass. Does not respond to forces.
    Static,
    /// Full dynamics simulation — affected by gravity, forces, collisions.
    Dynamic,
    /// User-controlled velocity. Affects dynamic bodies but is not
    /// affected by them. Typical for moving platforms or player characters.
    KinematicVelocity,
    /// Position-controlled. Like KinematicVelocity but set by direct
    /// position updates each frame.
    KinematicPosition,
}

/// Describes a rigid body's physical properties.
///
/// Stored on the CPU side; converted to a rapier3d `RigidBody` when
/// added to the physics world.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigidBody {
    pub body_type: RigidBodyType,
    /// Transform at time of creation or last sync.
    pub transform: Transform,
    /// Initial linear velocity (world-space). Used for Dynamic.
    pub linear_velocity: Vec3,
    /// Initial angular velocity. Used for Dynamic.
    pub angular_velocity: Vec3,
    /// Mass in kg. If `body_type` is Static, this is ignored.
    pub mass: f32,
    /// Angular inertia damping (0 = no damping, 1 = full stop).
    pub angular_damping: f32,
    /// Linear damping.
    pub linear_damping: f32,
    /// Whether gravity affects this body.
    pub gravity_enabled: bool,
    /// Whether this body can sleep (performance optimisation).
    pub can_sleep: bool,
}

impl Default for RigidBody {
    fn default() -> Self {
        Self {
            body_type: RigidBodyType::Dynamic,
            transform: Transform::IDENTITY,
            linear_velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            mass: 1.0,
            angular_damping: 0.1,
            linear_damping: 0.1,
            gravity_enabled: true,
            can_sleep: true,
        }
    }
}

impl RigidBody {
    /// Convenience: a static body at the given transform.
    pub fn static_at(transform: Transform) -> Self {
        Self {
            body_type: RigidBodyType::Static,
            transform,
            mass: 0.0,
            gravity_enabled: false,
            can_sleep: true,
            ..Default::default()
        }
    }

    /// Convenience: a dynamic body at the given transform.
    pub fn dynamic_at(transform: Transform) -> Self {
        Self {
            body_type: RigidBodyType::Dynamic,
            transform,
            ..Default::default()
        }
    }
}
