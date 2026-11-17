use std::collections::{HashMap, HashSet};

use xgc_base::EntityId;
use xgc_world::{
    Animator, CharacterController, Component, GlobalTransform, Parent, PhysicsBody, Slope,
    Transform, Value, Velocity, World,
};

use super::{System, SystemAccess};

#[derive(Clone, Copy, Debug)]
pub struct PhysicsSystem {
    pub gravity: [f32; 3],
}

impl Default for PhysicsSystem {
    fn default() -> Self {
        Self {
            gravity: [0.0, -9.81, 0.0],
        }
    }
}

impl System for PhysicsSystem {
    fn name(&self) -> &'static str {
        "physics"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![PhysicsBody::ID],
            writes: vec![Velocity::ID],
        }
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        let gravity = self.gravity;
        world.for_each2_mut::<Velocity, PhysicsBody>(|_, velocity, body| {
            if body.kind != "dynamic" {
                return;
            }
            for (axis, value) in gravity.iter().enumerate() {
                velocity.linear[axis] += value * body.gravity_scale * dt;
            }
        });
    }
}

/// 地面碰撞：dynamic 刚体落到 ground_y 时夹住位置并清掉向下速度。
#[derive(Clone, Copy, Debug)]
pub struct GroundCollisionSystem {
    pub ground_y: f32,
}

impl Default for GroundCollisionSystem {
    fn default() -> Self {
        Self { ground_y: 0.0 }
    }
}

impl System for GroundCollisionSystem {
    fn name(&self) -> &'static str {
        "ground"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![PhysicsBody::ID],
            writes: vec![Transform::ID, Velocity::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        let ground = self.ground_y;
        world.for_each3_mut::<Transform, Velocity, PhysicsBody>(|_, transform, velocity, body| {
            if body.kind != "dynamic" {
                return;
            }
            let half = support_half(body);
            if transform.position[1] - half < ground {
                transform.position[1] = ground + half;
                if velocity.linear[1] < 0.0 {
                    velocity.linear[1] = 0.0;
                }
            }
        });
    }
}

fn support_half(body: &PhysicsBody) -> f32 {
    match body.shape.as_str() {
        "ball" => body.radius,
        "capsule" => body.half_height + body.radius,
        _ => body.half_extents[1],
    }
}

/// 角色起跳：消费 `jump` 请求，踩地时写入竖直速度。
#[derive(Clone, Copy, Debug, Default)]
pub struct CharacterJumpSystem;

impl System for CharacterJumpSystem {
    fn name(&self) -> &'static str {
        "character_jump"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![CharacterController::ID],
            writes: vec![Velocity::ID, CharacterController::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        world.for_each2_mut::<Velocity, CharacterController>(|_, velocity, controller| {
            if controller.jump {
                if controller.grounded {
                    velocity.linear[1] = controller.jump_velocity;
                    controller.grounded = false;
                }
                controller.jump = false;
            }
        });
    }
}

/// 角色贴地：角色原点在脚底，落到可行走坡面 / y=0 地面时夹住并清掉向下速度；
/// 超过 `slope_limit` 的上升坡当墙挡在立面之前；下行离地不超过 `snap_to_ground` 时贴回地面。
#[derive(Clone, Copy, Debug, Default)]
pub struct CharacterSystem;

impl System for CharacterSystem {
    fn name(&self) -> &'static str {
        "character"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![CharacterController::ID, Slope::ID],
            writes: vec![Transform::ID, Velocity::ID, CharacterController::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        let mut slopes = Vec::new();
        world.for_each::<Slope>(|_, slope| {
            slopes.push(*slope);
        });
        world.for_each3_mut::<Transform, Velocity, CharacterController>(
            |_, transform, velocity, controller| {
                // NaN 上限会让 `degrees() <= limit` 恒假、连平地都被当墙；
                // 落回旧端 `CharacterController::sanitized()` 的缺省 45°。
                let limit = if controller.slope_limit.is_finite() {
                    controller.slope_limit.clamp(0.0, 89.0)
                } else {
                    45.0
                };
                let radius = controller.radius.max(0.0);
                let snap = controller.snap_to_ground.max(0.0);
                // 继续把 y=0 当兜底地面参与取大值（旧实现如此）：坡面低于 0 时不该把角色带下去。
                // 注意 `ground` 只由坡面与 y=0 组成，**不含**台阶 / 箱子的顶面：那些由
                // `CharacterStepSystem` 在本系统之后抬上去，本系统不该替它算。
                let mut ground = 0.0_f32;
                // 爬不上去的陡坡当墙：脚底低于坡面就是"陷进斜面里"，把角色推到最近的一侧。
                //
                // 旧实现把任何坡面都当可站立地面，`physics.character.slope_limit` 因此是
                // 个死字段——45° 上限的角色能一路走上 60° 的斜坡。这里按旧引擎同一档位语义
                // （超过上限就不可攀爬）把它分成"能站的地面"和"要挡的墙"。
                // 推的时候必须两侧都算：只按坡底（−X 侧）推，会把从高端（+X）走进去的角色
                // 一帧传送过整个斜坡（`CharacterBlockSystem` 也是按最近出口推的）。
                for slope in &slopes {
                    // 每根坡都用推进后的位置重新取值：重叠坡时不能拿已经离开的位置算。
                    let x = transform.position[0];
                    let feet = transform.position[1];
                    let Some(height) = slope.height_at(x, transform.position[2]) else {
                        continue;
                    };
                    // 下坡（rise <= 0，含平地坡）没有能"卡住"人的立面，一律当地面；只有上升坡才可能是墙。
                    if slope.rise <= 0.0 || slope.degrees() <= limit {
                        ground = ground.max(height);
                        continue;
                    }
                    // 已站在坡面之上（例如从侧面沿平台走上坡顶）就不推。
                    if feet >= height {
                        continue;
                    }
                    let Some(base) = slope.base_x_at(feet) else {
                        continue;
                    };
                    let front = base - radius;
                    let back = slope.origin[0] + slope.size[0] + radius;
                    // 最近出口按两侧墙的中点为界：`front` 会随 `feet` 沿坡面右移，
                    // 用坡面几何中点选边会在角色陷在坡体高处（`feet > origin[1]`，
                    // 正常行走到不了、但 spawn / 脚本改位置能到）时推到远的那侧。
                    if x < (front + back) * 0.5 {
                        if x > front {
                            transform.position[0] = front;
                            if velocity.linear[0] > 0.0 {
                                velocity.linear[0] = 0.0;
                            }
                        }
                    } else if x < back {
                        transform.position[0] = back;
                        if velocity.linear[0] < 0.0 {
                            velocity.linear[0] = 0.0;
                        }
                    }
                }
                if transform.position[1] < ground {
                    transform.position[1] = ground;
                    if velocity.linear[1] < 0.0 {
                        velocity.linear[1] = 0.0;
                    }
                } else if velocity.linear[1] <= 0.0 && transform.position[1] - ground <= snap {
                    // 下行贴地：坡面 / 下一级台阶在吸附距离内时按回地面。
                    // 不吸附的话每帧都"先离地再落地"，`grounded` 跟着抖，跳跃判定
                    // 变成看运气——旧引擎的 `snap_to_ground` 就是干这件事。
                    // 只在向下运动时吸：起跳当帧速度为正，不能被拉回地面。
                    transform.position[1] = ground;
                    velocity.linear[1] = 0.0;
                }
                controller.grounded = transform.position[1] <= ground + 1e-4;
            },
        );
    }
}

/// 动画时钟：自动播放的 Animator 按 speed 推进，duration 有效时循环 / 截断。
#[derive(Clone, Copy, Debug, Default)]
pub struct AnimatorSystem;

impl System for AnimatorSystem {
    fn name(&self) -> &'static str {
        "animation"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: Vec::new(),
            writes: vec![Animator::ID],
        }
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        world.for_each_mut::<Animator>(|_, animator| {
            if !animator.autoplay {
                return;
            }
            animator.time += animator.speed * dt;
            if animator.blend_duration > 0.0 {
                animator.blend_elapsed += dt;
                if animator.blend_elapsed >= animator.blend_duration {
                    animator.blend_duration = 0.0;
                    animator.blend_elapsed = 0.0;
                    animator.blend_from.clear();
                }
            }
            if animator.duration > 0.0 {
                if animator.looping {
                    animator.time = animator.time.rem_euclid(animator.duration);
                } else {
                    animator.time = animator.time.min(animator.duration);
                }
            }
        });
    }
}

/// 第一个内建系统：`position += linear * dt`。
#[derive(Clone, Copy, Debug)]
pub struct VelocitySystem {
    pub threads: usize,
}

impl Default for VelocitySystem {
    fn default() -> Self {
        Self { threads: 1 }
    }
}

impl VelocitySystem {
    pub fn new(threads: usize) -> Self {
        Self {
            threads: threads.max(1),
        }
    }
}

impl System for VelocitySystem {
    fn name(&self) -> &'static str {
        "velocity"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![Velocity::ID],
            writes: vec![Transform::ID],
        }
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        world.par_for_each2_mut::<Transform, Velocity>(self.threads, |_, transform, velocity| {
            for axis in 0..3 {
                transform.position[axis] += velocity.linear[axis] * dt;
            }
        });
    }
}

/// 第二个内建系统：给速度加固定重力；只写 Velocity，不读其它组件。
#[derive(Clone, Copy, Debug)]
pub struct GravitySystem {
    pub gravity: [f32; 3],
}

impl Default for GravitySystem {
    fn default() -> Self {
        Self {
            gravity: [0.0, -9.81, 0.0],
        }
    }
}

impl GravitySystem {
    pub fn new(gravity: [f32; 3]) -> Self {
        Self { gravity }
    }
}

impl System for GravitySystem {
    fn name(&self) -> &'static str {
        "gravity"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: Vec::new(),
            writes: vec![Velocity::ID],
        }
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        let gravity = self.gravity;
        world.for_each_mut::<Velocity>(|_, velocity| {
            for (axis, value) in gravity.iter().enumerate() {
                velocity.linear[axis] += *value * dt;
            }
        });
    }
}

/// 层级系统：按 `Parent` 链把局部 `Transform` 合成 `GlobalTransform`。
/// 层级系统：按 `Parent` 链把局部 `Transform` 合成 `GlobalTransform`。
///
/// 没有 `Parent`、或父节点不带 `GlobalTransform` 的实体按根处理；环里的实体
/// 保持原值，不阻塞其它节点。节点 / 存在集 / 结果表跨帧复用，稳态不再分配。
#[derive(Default)]
pub struct HierarchySystem {
    nodes: Vec<HierarchyNode>,
    present: HashSet<EntityId>,
    globals: HashMap<EntityId, GlobalTransform>,
}

struct HierarchyNode {
    entity: EntityId,
    parent: Option<EntityId>,
    local: Transform,
}

impl System for HierarchySystem {
    fn name(&self) -> &'static str {
        "hierarchy"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![Parent::ID, Transform::ID],
            writes: vec![GlobalTransform::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        self.nodes.clear();
        self.present.clear();
        self.globals.clear();

        world.for_each::<GlobalTransform>(|key, _| {
            let Some(entity) = world.entity(key) else {
                return;
            };
            self.nodes.push(HierarchyNode {
                entity,
                parent: world.get::<Parent>(key).map(|parent| parent.entity),
                local: world.get::<Transform>(key).copied().unwrap_or_default(),
            });
        });

        for node in &self.nodes {
            self.present.insert(node.entity);
        }
        for node in &self.nodes {
            if node
                .parent
                .is_none_or(|parent| !self.present.contains(&parent))
            {
                self.globals
                    .insert(node.entity, GlobalTransform::from_local(&node.local));
            }
        }

        let mut changed = true;
        while changed {
            changed = false;
            for node in &self.nodes {
                if self.globals.contains_key(&node.entity) {
                    continue;
                }
                let Some(parent) = node.parent else {
                    continue;
                };
                let Some(parent_global) = self.globals.get(&parent) else {
                    continue;
                };
                self.globals.insert(
                    node.entity,
                    GlobalTransform::compose(parent_global, &node.local),
                );
                changed = true;
            }
        }

        for (entity, global) in &self.globals {
            let _ = world.set_field(
                *entity,
                GlobalTransform::ID,
                "position",
                &Value::Vec3([
                    f64::from(global.position[0]),
                    f64::from(global.position[1]),
                    f64::from(global.position[2]),
                ]),
            );
            let _ = world.set_field(
                *entity,
                GlobalTransform::ID,
                "rotation",
                &Value::Quat([
                    f64::from(global.rotation[0]),
                    f64::from(global.rotation[1]),
                    f64::from(global.rotation[2]),
                    f64::from(global.rotation[3]),
                ]),
            );
            let _ = world.set_field(
                *entity,
                GlobalTransform::ID,
                "scale",
                &Value::Vec3([
                    f64::from(global.scale[0]),
                    f64::from(global.scale[1]),
                    f64::from(global.scale[2]),
                ]),
            );
        }
    }
}
