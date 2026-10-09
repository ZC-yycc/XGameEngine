//! XGameEngine 物理层。
//!
//! 封装 rapier3d 用于刚体动力学和碰撞检测。

pub mod collider;
pub mod debug_draw;
pub mod rigid_body;
pub mod scene_body;
pub mod world;

pub use collider::{Collider, ColliderShape};
pub use debug_draw::DebugLine;
pub use rigid_body::{RigidBody, RigidBodyType};
pub use scene_body::body_from_scene;
pub use world::{CollisionEvent, PhysicsConfig, PhysicsRayHit, PhysicsWorld, RaycastFilter};
