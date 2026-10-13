//! 暴露给脚本的宿主 API。
//!
//! 所有函数都收一个 [`ScriptCtx`] 作为第一参数。Rhai 支持"首参数类型匹配即可
//! 当方法用"，所以脚本里两种写法都成立：
//!
//! ```rhai
//! fn update(ctx) {
//!     set_pos(ctx, 0.0, 1.0, 0.0);   // 函数式
//!     ctx.set_pos(0.0, 1.0, 0.0);    // 方法式
//! }
//! ```
//!
//! # 坐标约定
//!
//! | 函数 | 空间 |
//! |---|---|
//! | `pos` / `set_pos` / `translate` | **世界**（脚本作者最常想要的） |
//! | `local_pos` / `set_local_pos` | 局部（相对父节点） |
//! | `rot` / `set_rot` | **局部**欧拉角，单位**度**，顺序 Y-X-Z |
//! | `scale` / `set_scale` | 局部 |
//!
//! 位置用世界空间、旋转用局部角度，是因为位置几乎总在"摆到哪里"的语境里
//! （父节点已经摆好了），而旋转几乎总在"这个物体自己转多少"的语境里。

use rhai::{Dynamic, Engine, Map};
use xgame_core::{Entity, EntityId, PhysicsBody, PhysicsHandle, Scene};
use xgame_input::{KeyCode, MouseButton};
use xgame_math::glam::{EulerRot, Quat, Vec2, Vec3};
use xgame_physics::RaycastFilter;

use crate::context::{LogLevel, ScriptCommand, ScriptCtx, SpawnRequest};

// ---------------------------------------------------------------------------
// 注册入口
// ---------------------------------------------------------------------------

/// 把整套 API 装进 Rhai 引擎。
pub(crate) fn register(engine: &mut Engine) {
    register_math(engine);
    register_transform(engine);
    register_entity(engine);
    register_spawn(engine);
    register_physics(engine);
    register_input(engine);
    register_time_and_log(engine);
    register_animation(engine);
}

// ---------------------------------------------------------------------------
// 向量类型
// ---------------------------------------------------------------------------

fn register_math(engine: &mut Engine) {
    engine
        .register_type_with_name::<Vec3>("Vec3")
        .register_get_set("x", |v: &mut Vec3| v.x, |v: &mut Vec3, val: f32| v.x = val)
        .register_get_set("y", |v: &mut Vec3| v.y, |v: &mut Vec3, val: f32| v.y = val)
        .register_get_set("z", |v: &mut Vec3| v.z, |v: &mut Vec3, val: f32| v.z = val);

    engine
        .register_type_with_name::<Vec2>("Vec2")
        .register_get_set("x", |v: &mut Vec2| v.x, |v: &mut Vec2, val: f32| v.x = val)
        .register_get_set("y", |v: &mut Vec2| v.y, |v: &mut Vec2, val: f32| v.y = val);

    engine.register_fn("vec3", |x: f32, y: f32, z: f32| Vec3::new(x, y, z));
    engine.register_fn("vec2", |x: f32, y: f32| Vec2::new(x, y));

    // 运算：脚本里 `a + b`、`v * 2.0` 这种写法要能直接用。
    engine.register_fn("+", |a: Vec3, b: Vec3| a + b);
    engine.register_fn("-", |a: Vec3, b: Vec3| a - b);
    engine.register_fn("-", |a: Vec3| -a);
    engine.register_fn("*", |a: Vec3, s: f32| a * s);
    engine.register_fn("*", |s: f32, a: Vec3| a * s);
    engine.register_fn("/", |a: Vec3, s: f32| a / s);
    engine.register_fn("==", |a: Vec3, b: Vec3| a == b);
    engine.register_fn("!=", |a: Vec3, b: Vec3| a != b);

    // 常用成员。返回 `f32` 的用 `#[must_use]` 语义不适用于 Rhai，这里只求好用。
    engine.register_fn("length", |v: Vec3| v.length());
    engine.register_fn("length_squared", |v: Vec3| v.length_squared());
    engine.register_fn("normalized", |v: Vec3| {
        // 零向量归一化会得到 NaN，脚本作者很难自己发现 —— 这里退回零向量。
        let len = v.length();
        if len > 1e-6 { v / len } else { Vec3::ZERO }
    });
    engine.register_fn("dot", |a: Vec3, b: Vec3| a.dot(b));
    engine.register_fn("cross", |a: Vec3, b: Vec3| a.cross(b));
    engine.register_fn("distance_to", |a: Vec3, b: Vec3| a.distance(b));
    engine.register_fn("lerp", |a: Vec3, b: Vec3, t: f32| a.lerp(b, t.clamp(0.0, 1.0)));
    engine.register_fn("to_string", |v: Vec3| format!("({:.3}, {:.3}, {:.3})", v.x, v.y, v.z));

    engine.register_fn("to_string", |v: Vec2| format!("({:.3}, {:.3})", v.x, v.y));
}

// ---------------------------------------------------------------------------
// 自身变换
// ---------------------------------------------------------------------------

fn register_transform(engine: &mut Engine) {
    engine.register_fn("pos", |ctx: &mut ScriptCtx| -> Vec3 {
        ctx.entity_index()
            .map(|i| world_position(ctx.scene(), i))
            .unwrap_or(Vec3::ZERO)
    });

    engine.register_fn("local_pos", |ctx: &mut ScriptCtx| -> Vec3 {
        ctx.entity_index()
            .and_then(|i| ctx.scene().entities.get(i).map(|e| e.transform.position))
            .unwrap_or(Vec3::ZERO)
    });

    engine.register_fn("set_pos", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        if let Some(i) = ctx.entity_index() {
            set_world_position(ctx.scene_mut(), i, Vec3::new(x, y, z));
        }
    });

    engine.register_fn("set_local_pos", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        if let Some(i) = ctx.entity_index() {
            if let Some(e) = ctx.scene_mut().entities.get_mut(i) {
                e.transform.position = Vec3::new(x, y, z);
            }
        }
    });

    engine.register_fn("translate", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        if let Some(i) = ctx.entity_index() {
            let p = world_position(ctx.scene(), i) + Vec3::new(x, y, z);
            set_world_position(ctx.scene_mut(), i, p);
        }
    });

    engine.register_fn("rot", |ctx: &mut ScriptCtx| -> Vec3 {
        ctx.entity_index()
            .and_then(|i| ctx.scene().entities.get(i).map(|e| euler_degrees(e.transform.rotation)))
            .unwrap_or(Vec3::ZERO)
    });

    engine.register_fn("set_rot", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        if let Some(i) = ctx.entity_index() {
            if let Some(e) = ctx.scene_mut().entities.get_mut(i) {
                e.transform.rotation = quat_from_degrees(Vec3::new(x, y, z));
            }
        }
    });

    engine.register_fn("rotate", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        if let Some(i) = ctx.entity_index() {
            let delta = quat_from_degrees(Vec3::new(x, y, z));
            if let Some(e) = ctx.scene_mut().entities.get_mut(i) {
                e.transform.rotation = (delta * e.transform.rotation).normalize();
            }
        }
    });

    engine.register_fn("scale", |ctx: &mut ScriptCtx| -> Vec3 {
        ctx.entity_index()
            .and_then(|i| ctx.scene().entities.get(i).map(|e| e.transform.scale))
            .unwrap_or(Vec3::ONE)
    });

    engine.register_fn("set_scale", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        if let Some(i) = ctx.entity_index() {
            if let Some(e) = ctx.scene_mut().entities.get_mut(i) {
                e.transform.scale = Vec3::new(x, y, z);
            }
        }
    });

    // 世界朝向：脚本算"往前飞"最常用的三个向量，省得自己拿四元数去乘。
    engine.register_fn("forward", |ctx: &mut ScriptCtx| -> Vec3 {
        ctx.entity_index()
            .map(|i| world_rotation(ctx.scene(), i).mul_vec3(Vec3::Z))
            .unwrap_or(Vec3::Z)
    });
    engine.register_fn("right", |ctx: &mut ScriptCtx| -> Vec3 {
        ctx.entity_index()
            .map(|i| world_rotation(ctx.scene(), i).mul_vec3(Vec3::X))
            .unwrap_or(Vec3::X)
    });
    engine.register_fn("up", |ctx: &mut ScriptCtx| -> Vec3 {
        ctx.entity_index()
            .map(|i| world_rotation(ctx.scene(), i).mul_vec3(Vec3::Y))
            .unwrap_or(Vec3::Y)
    });
}

// ---------------------------------------------------------------------------
// 实体查询与操作
// ---------------------------------------------------------------------------

fn register_entity(engine: &mut Engine) {
    engine.register_fn("self_id", |ctx: &mut ScriptCtx| -> i64 {
        ctx.entity_id().map(|id| id.raw() as i64).unwrap_or(0)
    });

    engine.register_fn("self_name", |ctx: &mut ScriptCtx| -> String {
        ctx.entity_index()
            .and_then(|i| ctx.scene().entities.get(i).map(|e| e.name.clone()))
            .unwrap_or_default()
    });

    engine.register_fn("entity_count", |ctx: &mut ScriptCtx| -> i64 {
        ctx.scene().entities.len() as i64
    });

    engine.register_fn("entity_at", |ctx: &mut ScriptCtx, index: i64| -> i64 {
        ctx.scene()
            .entities
            .get(index.max(0) as usize)
            .map(|e| e.id.raw() as i64)
            .unwrap_or(0)
    });

    engine.register_fn("entity_id", |ctx: &mut ScriptCtx, name: &str| -> i64 {
        ctx.scene()
            .entities
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.id.raw() as i64)
            .unwrap_or(0)
    });

    engine.register_fn("entity_name", |ctx: &mut ScriptCtx, id: i64| -> String {
        index_of(ctx.scene(), id)
            .and_then(|i| ctx.scene().entities.get(i).map(|e| e.name.clone()))
            .unwrap_or_default()
    });

    engine.register_fn("entity_model", |ctx: &mut ScriptCtx, id: i64| -> String {
        index_of(ctx.scene(), id)
            .and_then(|i| ctx.scene().entities.get(i).map(|e| e.model.clone()))
            .unwrap_or_default()
    });

    engine.register_fn("set_entity_model", |ctx: &mut ScriptCtx, id: i64, model: &str| {
        if let Some(i) = index_of(ctx.scene(), id) {
            if let Some(e) = ctx.scene_mut().entities.get_mut(i) {
                e.model = model.to_owned();
            }
        }
    });

    engine.register_fn("entity_pos", |ctx: &mut ScriptCtx, id: i64| -> Vec3 {
        index_of(ctx.scene(), id)
            .map(|i| world_position(ctx.scene(), i))
            .unwrap_or(Vec3::ZERO)
    });

    engine.register_fn(
        "set_entity_pos",
        |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
            if let Some(i) = index_of(ctx.scene(), id) {
                set_world_position(ctx.scene_mut(), i, Vec3::new(x, y, z));
            }
        },
    );

    engine.register_fn("entity_scale", |ctx: &mut ScriptCtx, id: i64| -> Vec3 {
        index_of(ctx.scene(), id)
            .and_then(|i| ctx.scene().entities.get(i).map(|e| e.transform.scale))
            .unwrap_or(Vec3::ONE)
    });

    engine.register_fn(
        "set_entity_scale",
        |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
            if let Some(i) = index_of(ctx.scene(), id) {
                if let Some(e) = ctx.scene_mut().entities.get_mut(i) {
                    e.transform.scale = Vec3::new(x, y, z);
                }
            }
        },
    );

    engine.register_fn("entity_rot", |ctx: &mut ScriptCtx, id: i64| -> Vec3 {
        index_of(ctx.scene(), id)
            .and_then(|i| ctx.scene().entities.get(i).map(|e| euler_degrees(e.transform.rotation)))
            .unwrap_or(Vec3::ZERO)
    });

    engine.register_fn(
        "set_entity_rot",
        |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
            if let Some(i) = index_of(ctx.scene(), id) {
                if let Some(e) = ctx.scene_mut().entities.get_mut(i) {
                    e.transform.rotation = quat_from_degrees(Vec3::new(x, y, z));
                }
            }
        },
    );

    engine.register_fn("distance_to", |ctx: &mut ScriptCtx, id: i64| -> f32 {
        let Some(self_i) = ctx.entity_index() else { return 0.0 };
        let Some(other_i) = index_of(ctx.scene(), id) else { return 0.0 };
        world_position(ctx.scene(), self_i).distance(world_position(ctx.scene(), other_i))
    });

    engine.register_fn("destroy", |ctx: &mut ScriptCtx, id: i64| {
        let Some(index) = index_of(ctx.scene(), id) else {
            return;
        };
        // 立即删除（整棵子树），并把被删掉的 ID 报给运行时清刚体。
        let removed = ctx.scene_mut().remove_subtree(index);
        if removed.is_empty() {
            return;
        }
        let ids: Vec<EntityId> = removed.iter().map(|e| e.id).collect();
        ctx.push_command(ScriptCommand::EntitiesRemoved { ids });
    });

    engine.register_fn("set_parent", |ctx: &mut ScriptCtx, child: i64, parent: i64| {
        let Some(child_index) = index_of(ctx.scene(), child) else {
            return;
        };
        let parent_index = if parent == 0 {
            None
        } else {
            match index_of(ctx.scene(), parent) {
                Some(index) => Some(index),
                None => return,
            }
        };
        // `set_parent` 自己会拒绝成环，返回 false 时不要假装成功。
        if ctx.scene_mut().set_parent(child_index, parent_index) {
            ctx.push_command(ScriptCommand::Reparented {
                child: EntityId::from_raw(child as u64),
            });
        }
    });
}

// ---------------------------------------------------------------------------
// 生成实体
// ---------------------------------------------------------------------------

fn register_spawn(engine: &mut Engine) {
    // 名字里必须带 `_entity`：Rhai 把 `spawn` 当**保留关键字**（它自己的
    // 多线程 `spawn` 语法用掉了这个词），注册成 `spawn` 的脚本会直接编译失败。
    engine.register_fn("spawn_entity", |ctx: &mut ScriptCtx, name: &str, model: &str| -> i64 {
        push_spawn(ctx, SpawnRequest {
            name: name.to_owned(),
            model: model.to_owned(),
            ..SpawnRequest::new(name)
        })
    });

    engine.register_fn(
        "spawn_entity_at",
        |ctx: &mut ScriptCtx, name: &str, model: &str, x: f32, y: f32, z: f32| -> i64 {
            push_spawn(ctx, SpawnRequest {
                name: name.to_owned(),
                model: model.to_owned(),
                position: Vec3::new(x, y, z),
                ..SpawnRequest::new(name)
            })
        },
    );

    engine.register_fn(
        "spawn_ball",
        |ctx: &mut ScriptCtx, name: &str, radius: f32, x: f32, y: f32, z: f32| -> i64 {
            push_spawn(ctx, SpawnRequest {
                name: name.to_owned(),
                model: "builtin://sphere".to_owned(),
                position: Vec3::new(x, y, z),
                scale: Vec3::splat(radius * 2.0),
                physics: Some(PhysicsBody::dynamic_ball(radius)),
                ..SpawnRequest::new(name)
            })
        },
    );

    engine.register_fn(
        "spawn_static_box",
        |ctx: &mut ScriptCtx, name: &str, hx: f32, hy: f32, hz: f32, x: f32, y: f32, z: f32| -> i64 {
            push_spawn(ctx, SpawnRequest {
                name: name.to_owned(),
                model: "builtin://cube".to_owned(),
                position: Vec3::new(x, y, z),
                scale: Vec3::new(hx * 2.0, hy * 2.0, hz * 2.0),
                physics: Some(PhysicsBody::static_box([hx, hy, hz])),
                ..SpawnRequest::new(name)
            })
        },
    );
}

/// 按请求生成实体，**立即**放进场景，返回它的 ID。
///
/// 立即（而不是攒成命令延后执行）是刻意的：脚本里
/// `let id = spawn_entity(...); set_entity_pos(id, ...)` 是最自然的写法，
/// 延后执行会让第二句对着一个还不存在的实体静默失效。
/// 安全性依据见 [`ScriptCommand`] 的说明。
fn push_spawn(ctx: &mut ScriptCtx, request: SpawnRequest) -> i64 {
    let id = EntityId::new();
    let mut entity = Entity::new(&request.name);
    entity.id = id;
    entity.model = request.model;
    entity.transform.position = request.position;
    entity.transform.rotation = request.rotation;
    entity.transform.scale = request.scale;
    entity.physics = request.physics;

    ctx.scene_mut().entities.push(entity);
    ctx.push_command(ScriptCommand::EntitySpawned {
        id,
        physics: request.physics,
    });
    id.raw() as i64
}

// ---------------------------------------------------------------------------
// 物理
// ---------------------------------------------------------------------------

fn register_physics(engine: &mut Engine) {
    engine.register_fn("has_physics", |ctx: &mut ScriptCtx, id: i64| -> bool {
        body_of(ctx, id).is_some()
    });

    engine.register_fn(
        "raycast",
        |ctx: &mut ScriptCtx,
         ox: f32, oy: f32, oz: f32,
         dx: f32, dy: f32, dz: f32,
         max_dist: f32|
         -> Map {
            let mut map = Map::new();
            let Some(physics) = ctx.physics() else {
                map.insert("hit".into(), false.into());
                return map;
            };
            let filter = RaycastFilter {
                ignore_sensors: true,
                exclude_body: ctx.entity_id().and_then(|id| body_of(ctx, id.raw() as i64)),
                ..RaycastFilter::default()
            };
            let hit = physics.raycast_with_filter(
                Vec3::new(ox, oy, oz),
                Vec3::new(dx, dy, dz),
                max_dist,
                true,
                filter,
            );
            // 命中结果里的 body 句柄对脚本没意义，换成实体 ID 才可用。
            let entity = hit
                .and_then(|h| h.body_id)
                .and_then(|body| entity_of_body(ctx, body));
            insert_ray_hit(&mut map, hit, entity);
            map
        },
    );

    engine.register_fn("apply_force", |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
        let Some(body) = body_of(ctx, id) else { return };
        if let Some(physics) = ctx.physics_mut() {
            physics.apply_force(body, Vec3::new(x, y, z));
        }
    });

    engine.register_fn("apply_impulse", |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
        let Some(body) = body_of(ctx, id) else { return };
        if let Some(physics) = ctx.physics_mut() {
            physics.apply_impulse(body, Vec3::new(x, y, z));
        }
    });

    engine.register_fn("add_torque", |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
        let Some(body) = body_of(ctx, id) else { return };
        if let Some(physics) = ctx.physics_mut() {
            physics.apply_torque(body, Vec3::new(x, y, z));
        }
    });

    engine.register_fn("velocity", |ctx: &mut ScriptCtx, id: i64| -> Vec3 {
        let Some(body) = body_of(ctx, id) else { return Vec3::ZERO };
        ctx.physics()
            .and_then(|p| p.body_velocity(body))
            .unwrap_or(Vec3::ZERO)
    });

    engine.register_fn("set_velocity", |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
        let Some(body) = body_of(ctx, id) else { return };
        if let Some(physics) = ctx.physics_mut() {
            physics.set_velocity(body, Vec3::new(x, y, z));
        }
    });

    engine.register_fn("gravity", |ctx: &mut ScriptCtx| -> Vec3 {
        ctx.physics().map(|p| p.gravity()).unwrap_or(Vec3::ZERO)
    });

    engine.register_fn("set_gravity", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        if let Some(physics) = ctx.physics_mut() {
            physics.set_gravity(Vec3::new(x, y, z));
        }
    });
}

/// 把命中结果摊进 Rhai 的 Map。
///
/// 用 Map 而不是自定义类型，是刻意的取舍：`raycast` 的返回值通常只被读一两个
/// 字段（`hit.point`、`hit.entity`），为它注册一个 Rhai 类型只会让 API 表面变大。
/// Map 的代价是字段名写错不会报错（拿到 `()`），所以这里把**所有字段都填上**，
/// 包括 `hit: false` 的分支 —— 脚本里 `r.point` 永远拿得到东西。
fn insert_ray_hit(
    map: &mut Map,
    hit: Option<xgame_physics::PhysicsRayHit>,
    entity: Option<EntityId>,
) {
    match hit {
        Some(h) => {
            map.insert("hit".into(), true.into());
            map.insert("point".into(), Dynamic::from(h.point));
            map.insert("normal".into(), Dynamic::from(h.normal));
            map.insert("distance".into(), h.distance.into());
            map.insert(
                "entity".into(),
                entity.map(|e| e.raw() as i64).unwrap_or(0).into(),
            );
            map.insert("is_sensor".into(), h.is_sensor.into());
        }
        None => {
            map.insert("hit".into(), false.into());
            map.insert("point".into(), Dynamic::from(Vec3::ZERO));
            map.insert("normal".into(), Dynamic::from(Vec3::Y));
            map.insert("distance".into(), 0.0f32.into());
            map.insert("entity".into(), 0i64.into());
            map.insert("is_sensor".into(), false.into());
        }
    }
}

/// 实体 ID → 物理刚体句柄。
fn body_of(ctx: &ScriptCtx, id: i64) -> Option<u64> {
    if id == 0 {
        return None;
    }
    let index = index_of(ctx.scene(), id)?;
    ctx.scene()
        .entities
        .get(index)?
        .component::<PhysicsHandle>()
        .map(|h| h.0)
}

/// 刚体句柄 → 实体 ID。
///
/// 线性扫场景：射线不是每帧每实体都发，O(n) 足够了；真要成了热点，
/// 再让运行时在 `ScriptCtx` 里带一张反查表。
fn entity_of_body(ctx: &ScriptCtx, body: u64) -> Option<EntityId> {
    ctx.scene()
        .entities
        .iter()
        .find(|e| e.component::<PhysicsHandle>().map(|h| h.0) == Some(body))
        .map(|e| e.id)
}

// ---------------------------------------------------------------------------
// 输入
// ---------------------------------------------------------------------------

fn register_input(engine: &mut Engine) {
    engine.register_fn("action_down", |ctx: &mut ScriptCtx, action: &str| -> bool {
        ctx.input().map(|i| i.action_down(action)).unwrap_or(false)
    });

    engine.register_fn("action_pressed", |ctx: &mut ScriptCtx, action: &str| -> bool {
        ctx.input().map(|i| i.action_pressed(action)).unwrap_or(false)
    });

    engine.register_fn("action_released", |ctx: &mut ScriptCtx, action: &str| -> bool {
        ctx.input().map(|i| i.action_released(action)).unwrap_or(false)
    });

    engine.register_fn("axis", |ctx: &mut ScriptCtx, axis: &str| -> f32 {
        ctx.input().map(|i| i.axis(axis)).unwrap_or(0.0)
    });

    engine.register_fn("key_down", |ctx: &mut ScriptCtx, key: &str| -> bool {
        let Some(code) = KeyCode::from_name(key) else {
            ctx.push_log(LogLevel::Warn, format!("未知按键 '{key}'"));
            return false;
        };
        ctx.input().map(|i| i.key_down(code)).unwrap_or(false)
    });

    engine.register_fn("key_pressed", |ctx: &mut ScriptCtx, key: &str| -> bool {
        let Some(code) = KeyCode::from_name(key) else {
            return false;
        };
        ctx.input().map(|i| i.key_pressed(code)).unwrap_or(false)
    });

    engine.register_fn("mouse_down", |ctx: &mut ScriptCtx, button: &str| -> bool {
        let Some(b) = MouseButton::from_name(button) else {
            return false;
        };
        ctx.input().map(|i| i.mouse_down(b)).unwrap_or(false)
    });

    engine.register_fn("mouse_pressed", |ctx: &mut ScriptCtx, button: &str| -> bool {
        let Some(b) = MouseButton::from_name(button) else {
            return false;
        };
        ctx.input().map(|i| i.mouse_pressed(b)).unwrap_or(false)
    });

    engine.register_fn("mouse_pos", |ctx: &mut ScriptCtx| -> Vec2 {
        ctx.input().map(|i| i.mouse_pos()).unwrap_or(Vec2::ZERO)
    });

    engine.register_fn("mouse_delta", |ctx: &mut ScriptCtx| -> Vec2 {
        ctx.input().map(|i| i.mouse_delta()).unwrap_or(Vec2::ZERO)
    });

    engine.register_fn("scroll", |ctx: &mut ScriptCtx| -> Vec2 {
        ctx.input().map(|i| i.scroll()).unwrap_or(Vec2::ZERO)
    });
}

// ---------------------------------------------------------------------------
// 时间与日志
// ---------------------------------------------------------------------------

fn register_time_and_log(engine: &mut Engine) {
    engine.register_fn("delta", |ctx: &mut ScriptCtx| -> f32 { ctx.delta() });
    engine.register_fn("time", |ctx: &mut ScriptCtx| -> f32 { ctx.time() });

    register_state(engine);

    engine.register_fn("log", |ctx: &mut ScriptCtx, msg: &str| {
        ctx.push_log(LogLevel::Info, msg.to_owned());
    });
    engine.register_fn("warn", |ctx: &mut ScriptCtx, msg: &str| {
        ctx.push_log(LogLevel::Warn, msg.to_owned());
    });
    engine.register_fn("error", |ctx: &mut ScriptCtx, msg: &str| {
        ctx.push_log(LogLevel::Error, msg.to_owned());
    });

    // 数值工具：脚本里算插值/角度非常常用，Rhai 自带的不够顺手。
    engine.register_fn("clamp", |v: f32, lo: f32, hi: f32| v.clamp(lo, hi));
    engine.register_fn("lerpf", |a: f32, b: f32, t: f32| a + (b - a) * t.clamp(0.0, 1.0));
    engine.register_fn("deg_to_rad", |d: f32| d.to_radians());
    engine.register_fn("rad_to_deg", |r: f32| r.to_degrees());
}

/// 每 (实体, 脚本) 一份的持久状态。
///
/// Rhai 的函数不能写外层作用域，所以"给自己记一个计时器"这类需求在脚本里
/// 没有别的落地方式。这组函数把状态表交给引擎侧托管，键是 `(实体, 脚本)`：
///
/// ```rhai
/// fn update(ctx) {
///     let t = state_get_or(ctx, "cooldown", 0.0) - delta(ctx);
///     state_set(ctx, "cooldown", t);
///     if t <= 0.0 && action_pressed(ctx, "Fire") {
///         log(ctx, "fire!");
///         state_set(ctx, "cooldown", 0.5);
///     }
/// }
/// ```
fn register_state(engine: &mut Engine) {
    engine.register_fn("state_get", |ctx: &mut ScriptCtx, key: &str| -> Dynamic {
        ctx.state()
            .and_then(|map| map.get(key).cloned())
            // 没设过的键返回 `()`，脚本里用 `if x == ()` 判空即可。
            .unwrap_or(Dynamic::UNIT)
    });

    engine.register_fn(
        "state_get_or",
        |ctx: &mut ScriptCtx, key: &str, default: Dynamic| -> Dynamic {
            ctx.state()
                .and_then(|map| map.get(key).cloned())
                .unwrap_or(default)
        },
    );

    engine.register_fn("state_set", |ctx: &mut ScriptCtx, key: &str, value: Dynamic| {
        if let Some(map) = ctx.state() {
            map.insert(key.into(), value);
        }
    });

    engine.register_fn("state_has", |ctx: &mut ScriptCtx, key: &str| -> bool {
        ctx.state().map(|map| map.contains_key(key)).unwrap_or(false)
    });

    engine.register_fn("state_remove", |ctx: &mut ScriptCtx, key: &str| {
        if let Some(map) = ctx.state() {
            map.remove(key);
        }
    });

    engine.register_fn("state_clear", |ctx: &mut ScriptCtx| {
        if let Some(map) = ctx.state() {
            map.clear();
        }
    });
}

// ---------------------------------------------------------------------------
// 动画状态机
// ---------------------------------------------------------------------------

/// 动画状态机的脚本接口。
///
/// v1 只给**写**：状态机的转移求值与叠加层的增量合成都在引擎侧（驱动每 tick
/// 跑），脚本只负责把"现在多快 / 要不要攻击"这类**信号**推进参数表、把
/// "晃动多重"推进叠加层权重。没有读取、没有 id 变体 —— 真实需求出现再加（YAGNI）。
fn register_animation(engine: &mut Engine) {
    // 写本实体的状态机参数（键不存在就创建）。
    //
    // 没挂 animator / 没配机器 → **静默 no-op**：脚本照常跑，只是机器不动。
    // 这让"通用移动脚本挂到任意实体上"成为可能，不需要脚本自己先探测组件。
    engine.register_fn(
        "set_anim_param",
        |ctx: &mut ScriptCtx, name: &str, value: f32| {
            if let Some(i) = ctx.entity_index() {
                if let Some(animator) = ctx.scene_mut().entities.get_mut(i).and_then(|e| e.animator.as_mut())
                {
                    if let Some(machine) = animator.machine.as_mut() {
                        machine.parameters.insert(name.to_owned(), value);
                    }
                }
            }
        },
    );

    // 写本实体叠加层的权重 ∈ [0,1]（引擎侧钳位）。
    //
    // 脚本的经典用法：受击晃动"淡入再淡出"——平时 weight=0（零成本 no-op），
    // 挨打瞬间拉到 1 再随时间衰减回去。没配叠加层 → 静默 no-op，同 set_anim_param。
    engine.register_fn("set_anim_additive_weight", |ctx: &mut ScriptCtx, value: f32| {
        if let Some(i) = ctx.entity_index() {
            if let Some(add) = ctx
                .scene_mut()
                .entities
                .get_mut(i)
                .and_then(|e| e.animator.as_mut())
                .and_then(|a| a.additive.as_mut())
            {
                add.weight = value.clamp(0.0, 1.0);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// 场景辅助
// ---------------------------------------------------------------------------

fn index_of(scene: &Scene, id: i64) -> Option<usize> {
    if id <= 0 {
        return None;
    }
    scene.find_entity(EntityId::from_raw(id as u64))
}

/// 实体的世界坐标（取世界矩阵的平移分量）。
fn world_position(scene: &Scene, index: usize) -> Vec3 {
    scene.world_matrix(index).w_axis.truncate()
}

/// 实体的世界旋转。
fn world_rotation(scene: &Scene, index: usize) -> Quat {
    let (_, rotation, _) = scene.world_matrix(index).to_scale_rotation_translation();
    rotation
}

/// 设置实体的**世界**位置：有父节点时换算回局部。
///
/// 直接写 `transform.position` 会把世界坐标当成局部坐标用 —— 子物体一动就飞。
fn set_world_position(scene: &mut Scene, index: usize, world_pos: Vec3) {
    let local = match scene.parent_index(index) {
        Some(parent) => {
            let parent_world = scene.world_matrix(parent);
            if parent_world.determinant().abs() > 1e-12 {
                (parent_world.inverse() * world_pos.extend(1.0)).truncate()
            } else {
                world_pos
            }
        }
        None => world_pos,
    };
    if let Some(entity) = scene.entities.get_mut(index) {
        entity.transform.position = local;
    }
}

fn euler_degrees(q: Quat) -> Vec3 {
    let (y, x, z) = q.to_euler(EulerRot::YXZ);
    Vec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees())
}

fn quat_from_degrees(v: Vec3) -> Quat {
    Quat::from_euler(
        EulerRot::YXZ,
        v.y.to_radians(),
        v.x.to_radians(),
        v.z.to_radians(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euler_roundtrip_is_stable() {
        let original = Vec3::new(12.0, 45.0, -30.0);
        let back = euler_degrees(quat_from_degrees(original));
        // YXZ 分解在同一象限内应当还原；浮点误差留一点余量。
        assert!((back.x - original.x).abs() < 1e-2, "{back:?}");
        assert!((back.y - original.y).abs() < 1e-2, "{back:?}");
        assert!((back.z - original.z).abs() < 1e-2, "{back:?}");
    }

    #[test]
    fn setting_world_position_respects_parent() {
        let mut scene = Scene::new("t");
        let parent_id = scene.add_entity(xgame_core::Entity::new("parent"));
        let parent = scene.find_entity(parent_id).unwrap();
        scene.entities[parent].transform.position = Vec3::new(10.0, 0.0, 0.0);

        let child_id = scene.add_entity(xgame_core::Entity::new("child"));
        let child = scene.find_entity(child_id).unwrap();
        scene.set_parent(child, Some(parent));

        set_world_position(&mut scene, child, Vec3::new(12.0, 5.0, 0.0));

        assert_eq!(scene.entities[child].transform.position, Vec3::new(2.0, 5.0, 0.0));
        let world = scene.world_matrix(child).w_axis.truncate();
        assert!((world - Vec3::new(12.0, 5.0, 0.0)).length() < 1e-4);
    }
}
