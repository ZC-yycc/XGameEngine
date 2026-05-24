//! Physics world — owns rapier3d internals, drives simulation steps,
//! and provides a simple interface for game code.

use std::collections::HashMap;
use std::sync::Mutex;

use glam::{Quat, Vec3};
use xgame_math::Transform;

use rapier3d::dynamics::{
    CCDSolver, ImpulseJointSet, IntegrationParameters, IslandManager,
    MultibodyJointSet, RigidBodyBuilder, RigidBodyHandle, RigidBodySet,
};
use rapier3d::geometry::{
    BroadPhaseMultiSap as DefaultBroadPhase, ColliderBuilder, ColliderHandle,
    ColliderSet, NarrowPhase,
};
use rapier3d::prelude::ActiveEvents;
use rapier3d::pipeline::{PhysicsPipeline, QueryPipeline};

use crate::collider::{Collider, ColliderShape};
use crate::rigid_body::{RigidBody, RigidBodyType};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Global physics settings.
#[derive(Debug, Clone)]
pub struct PhysicsConfig {
    /// Gravity vector (default: -Y, 9.81 m/s²).
    pub gravity: Vec3,
    /// Number of sub-steps per `step()` call. Higher = more stable but slower.
    pub num_sub_steps: u32,
    /// Maximum timestep per sub-step in seconds.
    pub timestep: f32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, -9.81, 0.0),
            num_sub_steps: 1,
            timestep: 1.0 / 60.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Collision event (our simplified view)
// ---------------------------------------------------------------------------

/// Simplified view of a contact event emitted by rapier3d.
#[derive(Debug, Clone)]
pub struct CollisionEvent {
    /// User-assigned ID of the first body involved.
    pub body_a: u64,
    /// User-assigned ID of the second body involved.
    pub body_b: u64,
    /// Whether the two bodies just started touching (`true`) or stopped (`false`).
    pub started: bool,
}

// ---------------------------------------------------------------------------
// Internal handle record
// ---------------------------------------------------------------------------

struct BodyRecord {
    rapier_handle: RigidBodyHandle,
}

struct ColliderRecord {
    rapier_handle: ColliderHandle,
    body_id: u64,
}

// ---------------------------------------------------------------------------
// Event collector
// ---------------------------------------------------------------------------

/// Implements rapier3d's `EventHandler` using a `Mutex<Vec<...>>` for
/// interior mutability.  `Mutex` is `Send + Sync` so the trait bound is met.
struct RapierEventCollector {
    events: Mutex<Vec<(ColliderHandle, ColliderHandle, bool)>>,
}

impl RapierEventCollector {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    fn drain(self) -> Vec<(ColliderHandle, ColliderHandle, bool)> {
        self.events.into_inner().unwrap()
    }
}

impl rapier3d::pipeline::EventHandler for RapierEventCollector {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        event: rapier3d::geometry::CollisionEvent,
        _contact_pair: Option<&rapier3d::geometry::ContactPair>,
    ) {
        self.events
            .lock()
            .unwrap()
            .push((event.collider1(), event.collider2(), event.started()));
    }

    fn handle_contact_force_event(
        &self,
        _dt: f32,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _contact_pair: &rapier3d::geometry::ContactPair,
        _total_impulse: f32,
    ) {
        // Not used for now
    }
}

// ---------------------------------------------------------------------------
// Physics world
// ---------------------------------------------------------------------------

pub struct PhysicsWorld {
    rigid_body_set: RigidBodySet,
    collider_set: ColliderSet,
    integration_params: IntegrationParameters,
    physics_pipeline: PhysicsPipeline,
    island_manager: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    impulse_joint_set: ImpulseJointSet,
    multibody_joint_set: MultibodyJointSet,
    ccd_solver: CCDSolver,
    query_pipeline: QueryPipeline,

    /// Pending collision events to drain.
    pending_events: Vec<CollisionEvent>,

    /// Config.
    config: PhysicsConfig,

    /// Maps user ID → rapier3d rigid-body handle.
    bodies: HashMap<u64, BodyRecord>,
    /// Maps user ID → rapier3d collider handle.
    colliders: HashMap<u64, ColliderRecord>,
    next_id: u64,
}

impl PhysicsWorld {
    pub fn new(config: PhysicsConfig) -> Self {
        Self {
            rigid_body_set: RigidBodySet::new(),
            collider_set: ColliderSet::new(),
            integration_params: IntegrationParameters::default(),
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            pending_events: Vec::new(),

            config,

            bodies: HashMap::new(),
            colliders: HashMap::new(),
            next_id: 1,
        }
    }

    // -----------------------------------------------------------------------
    // Body / collider management
    // -----------------------------------------------------------------------

    /// Add a rigid body and return its user ID.
    pub fn add_body(&mut self, body: RigidBody) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let rb_type = match body.body_type {
            RigidBodyType::Static => rapier3d::dynamics::RigidBodyType::Fixed,
            RigidBodyType::Dynamic => rapier3d::dynamics::RigidBodyType::Dynamic,
            RigidBodyType::KinematicVelocity => {
                rapier3d::dynamics::RigidBodyType::KinematicVelocityBased
            }
            RigidBodyType::KinematicPosition => {
                rapier3d::dynamics::RigidBodyType::KinematicPositionBased
            }
        };

        let translation = glam_to_na_vec3(body.transform.translation);
        let rotation = glam_quat_to_na_rotation(body.transform.rotation);

        let mut rb = RigidBodyBuilder::new(rb_type)
            .translation(translation)
            .linvel(glam_to_na_vec3(body.linear_velocity))
            .angvel(glam_to_na_vec3(body.angular_velocity))
            .gravity_scale(if body.gravity_enabled { 1.0 } else { 0.0 })
            .linear_damping(body.linear_damping)
            .angular_damping(body.angular_damping)
            .can_sleep(body.can_sleep)
            .build();

        // Set initial rotation via the position
        let mut pos = *rb.position();
        pos.rotation = rotation;
        rb.set_position(pos, true);

        if body.body_type == RigidBodyType::Dynamic && body.mass > 0.0 {
            rb.set_additional_mass(body.mass, true);
        }

        let rapier_handle = self.rigid_body_set.insert(rb);

        self.bodies.insert(id, BodyRecord { rapier_handle });

        id
    }

    /// Remove a rigid body and all its attached colliders.
    pub fn remove_body(&mut self, body_id: u64) {
        if let Some(record) = self.bodies.remove(&body_id) {
            self.colliders.retain(|_, cr| {
                if cr.body_id == body_id {
                    self.collider_set.remove(
                        cr.rapier_handle,
                        &mut self.island_manager,
                        &mut self.rigid_body_set,
                        false,
                    );
                    false
                } else {
                    true
                }
            });
            self.rigid_body_set.remove(
                record.rapier_handle,
                &mut self.island_manager,
                &mut self.collider_set,
                &mut self.impulse_joint_set,
                &mut self.multibody_joint_set,
                true,
            );
        }
    }

    /// Attach a collider to an existing body. Returns the collider's user ID.
    pub fn add_collider(&mut self, body_id: u64, collider: Collider) -> u64 {
        let cid = self.next_id;
        self.next_id += 1;

        let body_record = self
            .bodies
            .get(&body_id)
            .expect("attach collider to unknown body");

        let shape = collider_to_shared_shape(&collider.shape);
        let cb = ColliderBuilder::new(shape)
            .friction(collider.friction)
            .restitution(collider.restitution)
            .sensor(collider.is_sensor)
            .density(collider.density)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .translation(glam_to_na_vec3(collider.offset));

        let rapier_handle = self.collider_set.insert_with_parent(
            cb,
            body_record.rapier_handle,
            &mut self.rigid_body_set,
        );

        self.colliders.insert(
            cid,
            ColliderRecord {
                rapier_handle,
                body_id,
            },
        );

        cid
    }

    /// Remove a single collider.
    pub fn remove_collider(&mut self, collider_id: u64) {
        if let Some(record) = self.colliders.remove(&collider_id) {
            self.collider_set.remove(
                record.rapier_handle,
                &mut self.island_manager,
                &mut self.rigid_body_set,
                false,
            );
        }
    }

    // -----------------------------------------------------------------------
    // Simulation step
    // -----------------------------------------------------------------------

    /// Advance the simulation by `dt` seconds using a fixed internal timestep.
    pub fn step(&mut self, _dt: f32) {
        self.integration_params.dt = self.config.timestep;

        let gravity = glam_to_na_vec3(self.config.gravity);

        let event_collector = RapierEventCollector::new();

        let num_steps = self.config.num_sub_steps.max(1);
        for _ in 0..num_steps {
            self.physics_pipeline.step(
                &gravity,
                &self.integration_params,
                &mut self.island_manager,
                &mut self.broad_phase,
                &mut self.narrow_phase,
                &mut self.rigid_body_set,
                &mut self.collider_set,
                &mut self.impulse_joint_set,
                &mut self.multibody_joint_set,
                &mut self.ccd_solver,
                Some(&mut self.query_pipeline),
                &(),
                &event_collector,
            );
        }

        // Resolve events to body IDs
        for (c1, c2, started) in event_collector.drain() {
            let body_a = self.resolve_collider_body(c1);
            let body_b = self.resolve_collider_body(c2);
            if let (Some(a), Some(b)) = (body_a, body_b) {
                self.pending_events.push(CollisionEvent {
                    body_a: a,
                    body_b: b,
                    started,
                });
            }
        }
    }

    // -----------------------------------------------------------------------
    // Query
    // -----------------------------------------------------------------------

    /// Get the current world-space transform of a body.
    pub fn body_transform(&self, body_id: u64) -> Option<Transform> {
        let record = self.bodies.get(&body_id)?;
        let rb = self.rigid_body_set.get(record.rapier_handle)?;
        let pos = rb.position();
        let t = pos.translation.vector;
        let rot = pos.rotation;
        Some(Transform {
            translation: Vec3::new(t.x, t.y, t.z),
            rotation: Quat::from_xyzw(
                rot.coords.x, rot.coords.y, rot.coords.z, rot.coords.w,
            ),
            scale: Vec3::ONE,
        })
    }

    /// Get the current linear velocity of a body.
    pub fn body_velocity(&self, body_id: u64) -> Option<Vec3> {
        let record = self.bodies.get(&body_id)?;
        let rb = self.rigid_body_set.get(record.rapier_handle)?;
        let v = *rb.linvel();
        Some(Vec3::new(v.x, v.y, v.z))
    }

    /// Apply a world-space force to a dynamic body's centre of mass.
    pub fn apply_force(&mut self, body_id: u64, force: Vec3) {
        if let Some(record) = self.bodies.get(&body_id) {
            if let Some(rb) = self.rigid_body_set.get_mut(record.rapier_handle) {
                rb.add_force(na_vector(force.x, force.y, force.z), true);
            }
        }
    }

    /// Apply a world-space impulse to a dynamic body's centre of mass.
    pub fn apply_impulse(&mut self, body_id: u64, impulse: Vec3) {
        if let Some(record) = self.bodies.get(&body_id) {
            if let Some(rb) = self.rigid_body_set.get_mut(record.rapier_handle) {
                rb.apply_impulse(na_vector(impulse.x, impulse.y, impulse.z), true);
            }
        }
    }

    /// Apply a torque to a dynamic body.
    pub fn apply_torque(&mut self, body_id: u64, torque: Vec3) {
        if let Some(record) = self.bodies.get(&body_id) {
            if let Some(rb) = self.rigid_body_set.get_mut(record.rapier_handle) {
                rb.add_torque(na_vector(torque.x, torque.y, torque.z), true);
            }
        }
    }

    /// Set gravity after construction.
    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.config.gravity = gravity;
    }

    // -----------------------------------------------------------------------
    // Events
    // -----------------------------------------------------------------------

    /// Drain all pending collision events.
    ///
    /// Call once per frame after `step()`.
    pub fn drain_events(&mut self) -> impl Iterator<Item = CollisionEvent> + '_ {
        self.pending_events.drain(..)
    }

    /// Return the number of active bodies and colliders.
    pub fn stats(&self) -> (usize, usize) {
        (self.bodies.len(), self.colliders.len())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn resolve_collider_body(&self, collider: ColliderHandle) -> Option<u64> {
        let c = self.collider_set.get(collider)?;
        let body_handle = c.parent()?;
        self.bodies
            .iter()
            .find(|(_, r)| r.rapier_handle == body_handle)
            .map(|(id, _)| *id)
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn na_vector(x: f32, y: f32, z: f32) -> rapier3d::na::Vector3<f32> {
    rapier3d::na::Vector3::new(x, y, z)
}

fn glam_to_na_vec3(v: glam::Vec3) -> rapier3d::na::Vector3<f32> {
    na_vector(v.x, v.y, v.z)
}

fn glam_quat_to_na_rotation(q: glam::Quat) -> rapier3d::na::UnitQuaternion<f32> {
    rapier3d::na::UnitQuaternion::from_quaternion(rapier3d::na::Quaternion::new(
        q.w, q.x, q.y, q.z,
    ))
}

fn collider_to_shared_shape(shape: &ColliderShape) -> rapier3d::geometry::SharedShape {
    match shape {
        ColliderShape::Cuboid { half_extents } => {
            rapier3d::geometry::SharedShape::cuboid(half_extents.x, half_extents.y, half_extents.z)
        }
        ColliderShape::Ball { radius } => rapier3d::geometry::SharedShape::ball(*radius),
        ColliderShape::Capsule { half_height, radius } => {
            rapier3d::geometry::SharedShape::capsule_y(*half_height, *radius)
        }
        ColliderShape::Cylinder { half_height, radius } => {
            rapier3d::geometry::SharedShape::cylinder(*half_height, *radius)
        }
        ColliderShape::Trimesh { vertices, indices } => {
            rapier3d::geometry::SharedShape::trimesh(
                vertices
                    .iter()
                    .map(|v| rapier3d::na::Point3::new(v[0], v[1], v[2]))
                    .collect(),
                indices.iter().map(|t| [t[0], t[1], t[2]]).collect(),
            )
        }
        ColliderShape::HeightField {
            heights,
            nx,
            nz,
            scale,
        } => {
            let data =
                rapier3d::na::DMatrix::from_row_slice(*nx as usize, *nz as usize, heights);
            rapier3d::geometry::SharedShape::heightfield(
                data,
                na_vector(scale.x, scale.y, scale.z),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn drop_body_frees_colliders() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let body = world.add_body(RigidBody::dynamic_at(Transform::IDENTITY));
        world.add_collider(body, Collider::default());
        assert_eq!(world.stats(), (1, 1));
        world.remove_body(body);
        assert_eq!(world.stats(), (0, 0));
    }

    #[test]
    fn gravity_pulls_downward() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let body = world.add_body(RigidBody::dynamic_at(Transform {
            translation: Vec3::new(0.0, 5.0, 0.0),
            ..Transform::IDENTITY
        }));
        world.add_collider(body, Collider::default());
        for _ in 0..10 {
            world.step(1.0 / 60.0);
        }
        let t = world.body_transform(body).unwrap();
        assert!(t.translation.y < 5.0, "body should fall due to gravity");
    }

    #[test]
    fn static_body_does_not_move() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let body = world.add_body(RigidBody::static_at(Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            ..Transform::IDENTITY
        }));
        world.add_collider(body, Collider::default());
        for _ in 0..10 {
            world.step(1.0 / 60.0);
        }
        let t = world.body_transform(body).unwrap();
        assert_eq!(t.translation, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn ball_collides_with_floor() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let floor = world.add_body(RigidBody::static_at(Transform {
            translation: Vec3::new(0.0, -1.0, 0.0),
            ..Transform::IDENTITY
        }));
        world.add_collider(floor, Collider::cuboid(Vec3::new(10.0, 0.5, 10.0)));

        let ball = world.add_body(RigidBody::dynamic_at(Transform {
            translation: Vec3::new(0.0, 2.0, 0.0),
            ..Transform::IDENTITY
        }));
        world.add_collider(ball, Collider::ball(1.0));

        // Simulate 2 seconds
        for _ in 0..120 {
            world.step(1.0 / 60.0);
        }

        // The ball should have stopped on the floor
        let ball_pos = world.body_transform(ball).unwrap();
        assert!(
            ball_pos.translation.y > -0.5,
            "ball y {} should be above floor",
            ball_pos.translation.y
        );

        // Collision events should have been emitted
        let events: Vec<_> = world.drain_events().collect();
        assert!(!events.is_empty(), "collision events should be non-empty");
    }
}
