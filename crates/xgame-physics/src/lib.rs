//! XGameEngine physics layer.
//!
//! Wraps rapier3d for rigid-body dynamics and collision detection.

pub mod collider;
pub mod debug_draw;
pub mod rigid_body;
pub mod world;

pub use collider::{Collider, ColliderShape};
pub use debug_draw::DebugLine;
pub use rigid_body::{RigidBody, RigidBodyType};
pub use world::{CollisionEvent, PhysicsConfig, PhysicsWorld};
