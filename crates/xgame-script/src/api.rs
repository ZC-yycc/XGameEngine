//! 暴露给脚本的宿主 API —— Rhai 转发壳。
//!
//! 双轨计划 P0 起，这里的每个函数体都只剩一行：调 [`crate::GameCtx`] 的同名
//! 方法。**API 的真相在 `ctx.rs`**（Rust 组件与未来的 Lua 绑定调的是同一批方法），
//! 本模块只负责两件 Rhai 专属的事：把脚本的松散参数（`x, y, z`、`i64` ID）
//! 收拢成引擎类型，以及把结果摊回 Rhai 值（`Map` / `Dynamic`）。
//!
//! Rhai 支持"首参数类型匹配即可当方法用"，所以脚本里两种写法都成立：
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
//! （约定的实现在 `ctx.rs`，这里只登记名字。）

use rhai::{Dynamic, Engine, Map};
use xgame_core::EntityId;
use xgame_math::glam::{Vec2, Vec3};

use crate::context::ScriptCtx;
use crate::ctx::RayHit;

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
// 脚本 ↔ 引擎的参数翻译（本模块仅有的"逻辑"）
// ---------------------------------------------------------------------------

/// 脚本的整数 ID（`0` = 无效）→ 引擎 [`EntityId`]。
fn id_of(raw: i64) -> EntityId {
    EntityId::from_raw(raw as u64)
}

/// 同上，但把 `<= 0` 判成"没有目标"（`set_parent(ctx, child, 0)` = 挂根）。
fn opt_id(raw: i64) -> Option<EntityId> {
    (raw > 0).then(|| EntityId::from_raw(raw as u64))
}

/// 引擎 [`EntityId`] → 脚本整数。
fn raw_i64(id: EntityId) -> i64 {
    id.raw() as i64
}

/// 把射线命中结果摊进 Rhai 的 Map。
///
/// 用 Map 而不是自定义类型，是刻意的取舍：`raycast` 的返回值通常只被读一两个
/// 字段（`hit.point`、`hit.entity`），为它注册一个 Rhai 类型只会让 API 表面变大。
/// Map 的代价是字段名写错不会报错（拿到 `()`），所以这里把**所有字段都填上**，
/// 包括 `hit: false` 的分支 —— 脚本里 `r.point` 永远拿得到东西。
fn ray_hit_map(hit: RayHit) -> Map {
    let mut map = Map::new();
    map.insert("hit".into(), hit.hit.into());
    map.insert("point".into(), Dynamic::from(hit.point));
    map.insert("normal".into(), Dynamic::from(hit.normal));
    map.insert("distance".into(), hit.distance.into());
    map.insert("entity".into(), hit.entity.map_or(0, raw_i64).into());
    map.insert("is_sensor".into(), hit.is_sensor.into());
    map
}

// ---------------------------------------------------------------------------
// 向量类型（Rhai 专属：运算符与成员注册）
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
    engine.register_fn("pos", |ctx: &mut ScriptCtx| ctx.game().pos());
    engine.register_fn("local_pos", |ctx: &mut ScriptCtx| ctx.game().local_pos());
    engine.register_fn("set_pos", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        ctx.game().set_pos(Vec3::new(x, y, z))
    });
    engine.register_fn("set_local_pos", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        ctx.game().set_local_pos(Vec3::new(x, y, z))
    });
    engine.register_fn("translate", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        ctx.game().translate(Vec3::new(x, y, z))
    });
    engine.register_fn("rot", |ctx: &mut ScriptCtx| ctx.game().rot());
    engine.register_fn("set_rot", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        ctx.game().set_rot(Vec3::new(x, y, z))
    });
    engine.register_fn("rotate", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        ctx.game().rotate(Vec3::new(x, y, z))
    });
    engine.register_fn("scale", |ctx: &mut ScriptCtx| ctx.game().scale());
    engine.register_fn("set_scale", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        ctx.game().set_scale(Vec3::new(x, y, z))
    });

    // 世界朝向。
    engine.register_fn("forward", |ctx: &mut ScriptCtx| ctx.game().forward());
    engine.register_fn("right", |ctx: &mut ScriptCtx| ctx.game().right());
    engine.register_fn("up", |ctx: &mut ScriptCtx| ctx.game().up());
}

// ---------------------------------------------------------------------------
// 实体查询与操作
// ---------------------------------------------------------------------------

fn register_entity(engine: &mut Engine) {
    engine.register_fn("self_id", |ctx: &mut ScriptCtx| raw_i64(ctx.game().self_id()));
    engine.register_fn("self_name", |ctx: &mut ScriptCtx| ctx.game().self_name());
    engine.register_fn("entity_count", |ctx: &mut ScriptCtx| ctx.game().entity_count() as i64);
    engine.register_fn("entity_at", |ctx: &mut ScriptCtx, index: i64| {
        ctx.game()
            .entity_at(index.max(0) as usize)
            .map_or(0, raw_i64)
    });
    engine.register_fn("entity_id", |ctx: &mut ScriptCtx, name: &str| {
        ctx.game().find_entity_id(name).map_or(0, raw_i64)
    });
    engine.register_fn("entity_name", |ctx: &mut ScriptCtx, id: i64| {
        ctx.game().entity_name(id_of(id))
    });
    engine.register_fn("entity_model", |ctx: &mut ScriptCtx, id: i64| {
        ctx.game().entity_model(id_of(id))
    });
    engine.register_fn("set_entity_model", |ctx: &mut ScriptCtx, id: i64, model: &str| {
        ctx.game().set_entity_model(id_of(id), model)
    });
    engine.register_fn("entity_pos", |ctx: &mut ScriptCtx, id: i64| {
        ctx.game().entity_pos(id_of(id))
    });
    engine.register_fn("set_entity_pos", |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
        ctx.game().set_entity_pos(id_of(id), Vec3::new(x, y, z))
    });
    engine.register_fn("entity_scale", |ctx: &mut ScriptCtx, id: i64| {
        ctx.game().entity_scale(id_of(id))
    });
    engine.register_fn(
        "set_entity_scale",
        |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
            ctx.game().set_entity_scale(id_of(id), Vec3::new(x, y, z))
        },
    );
    engine.register_fn("entity_rot", |ctx: &mut ScriptCtx, id: i64| {
        ctx.game().entity_rot(id_of(id))
    });
    engine.register_fn(
        "set_entity_rot",
        |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
            ctx.game().set_entity_rot(id_of(id), Vec3::new(x, y, z))
        },
    );
    engine.register_fn("distance_to", |ctx: &mut ScriptCtx, id: i64| {
        ctx.game().distance_to(id_of(id))
    });
    engine.register_fn("destroy", |ctx: &mut ScriptCtx, id: i64| {
        ctx.game().destroy(id_of(id))
    });
    engine.register_fn("set_parent", |ctx: &mut ScriptCtx, child: i64, parent: i64| {
        ctx.game().set_parent(id_of(child), opt_id(parent))
    });
}

// ---------------------------------------------------------------------------
// 生成实体
// ---------------------------------------------------------------------------

fn register_spawn(engine: &mut Engine) {
    // 名字里必须带 `_entity`：Rhai 把 `spawn` 当**保留关键字**（它自己的
    // 多线程 `spawn` 语法用掉了这个词），注册成 `spawn` 的脚本会直接编译失败。
    engine.register_fn("spawn_entity", |ctx: &mut ScriptCtx, name: &str, model: &str| {
        raw_i64(ctx.game().spawn_entity(name, model))
    });
    engine.register_fn(
        "spawn_entity_at",
        |ctx: &mut ScriptCtx, name: &str, model: &str, x: f32, y: f32, z: f32| {
            raw_i64(ctx.game().spawn_entity_at(name, model, Vec3::new(x, y, z)))
        },
    );
    engine.register_fn(
        "spawn_ball",
        |ctx: &mut ScriptCtx, name: &str, radius: f32, x: f32, y: f32, z: f32| {
            raw_i64(ctx.game().spawn_ball(name, radius, Vec3::new(x, y, z)))
        },
    );
    engine.register_fn(
        "spawn_static_box",
        |ctx: &mut ScriptCtx, name: &str, hx: f32, hy: f32, hz: f32, x: f32, y: f32, z: f32| {
            raw_i64(ctx.game().spawn_static_box(name, Vec3::new(hx, hy, hz), Vec3::new(x, y, z)))
        },
    );
}

// ---------------------------------------------------------------------------
// 物理
// ---------------------------------------------------------------------------

fn register_physics(engine: &mut Engine) {
    engine.register_fn("has_physics", |ctx: &mut ScriptCtx, id: i64| {
        ctx.game().has_physics(id_of(id))
    });

    engine.register_fn(
        "raycast",
        |ctx: &mut ScriptCtx,
         ox: f32, oy: f32, oz: f32,
         dx: f32, dy: f32, dz: f32,
         max_dist: f32|
         -> Map {
            ray_hit_map(ctx.game().raycast(Vec3::new(ox, oy, oz), Vec3::new(dx, dy, dz), max_dist))
        },
    );

    engine.register_fn("apply_force", |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
        ctx.game().apply_force(id_of(id), Vec3::new(x, y, z))
    });
    engine.register_fn("apply_impulse", |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
        ctx.game().apply_impulse(id_of(id), Vec3::new(x, y, z))
    });
    engine.register_fn("add_torque", |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
        ctx.game().add_torque(id_of(id), Vec3::new(x, y, z))
    });
    engine.register_fn("velocity", |ctx: &mut ScriptCtx, id: i64| {
        ctx.game().velocity(id_of(id))
    });
    engine.register_fn("set_velocity", |ctx: &mut ScriptCtx, id: i64, x: f32, y: f32, z: f32| {
        ctx.game().set_velocity(id_of(id), Vec3::new(x, y, z))
    });
    engine.register_fn("gravity", |ctx: &mut ScriptCtx| ctx.game().gravity());
    engine.register_fn("set_gravity", |ctx: &mut ScriptCtx, x: f32, y: f32, z: f32| {
        ctx.game().set_gravity(Vec3::new(x, y, z))
    });
}

// ---------------------------------------------------------------------------
// 输入
// ---------------------------------------------------------------------------

fn register_input(engine: &mut Engine) {
    engine.register_fn("action_down", |ctx: &mut ScriptCtx, action: &str| {
        ctx.game().action_down(action)
    });
    engine.register_fn("action_pressed", |ctx: &mut ScriptCtx, action: &str| {
        ctx.game().action_pressed(action)
    });
    engine.register_fn("action_released", |ctx: &mut ScriptCtx, action: &str| {
        ctx.game().action_released(action)
    });
    engine.register_fn("axis", |ctx: &mut ScriptCtx, axis: &str| ctx.game().axis(axis));

    engine.register_fn("key_down", |ctx: &mut ScriptCtx, key: &str| ctx.game().key_down(key));
    engine.register_fn("key_pressed", |ctx: &mut ScriptCtx, key: &str| {
        ctx.game().key_pressed(key)
    });
    engine.register_fn("mouse_down", |ctx: &mut ScriptCtx, button: &str| {
        ctx.game().mouse_down(button)
    });
    engine.register_fn("mouse_pressed", |ctx: &mut ScriptCtx, button: &str| {
        ctx.game().mouse_pressed(button)
    });
    engine.register_fn("mouse_pos", |ctx: &mut ScriptCtx| ctx.game().mouse_pos());
    engine.register_fn("mouse_delta", |ctx: &mut ScriptCtx| ctx.game().mouse_delta());
    engine.register_fn("scroll", |ctx: &mut ScriptCtx| ctx.game().scroll());
}

// ---------------------------------------------------------------------------
// 时间与日志
// ---------------------------------------------------------------------------

fn register_time_and_log(engine: &mut Engine) {
    engine.register_fn("delta", |ctx: &mut ScriptCtx| ctx.game().delta());
    engine.register_fn("time", |ctx: &mut ScriptCtx| ctx.game().time());

    register_state(engine);

    engine.register_fn("log", |ctx: &mut ScriptCtx, msg: &str| ctx.game().log(msg));
    engine.register_fn("warn", |ctx: &mut ScriptCtx, msg: &str| ctx.game().warn(msg));
    engine.register_fn("error", |ctx: &mut ScriptCtx, msg: &str| ctx.game().error(msg));

    // 数值工具：脚本里算插值/角度非常常用，Rhai 自带的不够顺手。
    engine.register_fn("clamp", |v: f32, lo: f32, hi: f32| v.clamp(lo, hi));
    engine.register_fn("lerpf", |a: f32, b: f32, t: f32| a + (b - a) * t.clamp(0.0, 1.0));
    engine.register_fn("deg_to_rad", |d: f32| d.to_radians());
    engine.register_fn("rad_to_deg", |r: f32| r.to_degrees());
}

/// 每 (实体, 脚本) 一份的持久状态 —— Rhai 专属，不进 `GameCtx`
/// （表的值是 `rhai::Dynamic`，引擎无关的方法层不该认识它）。
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
    engine.register_fn("set_anim_param", |ctx: &mut ScriptCtx, name: &str, value: f32| {
        ctx.game().set_anim_param(name, value)
    });
    engine.register_fn("set_anim_additive_weight", |ctx: &mut ScriptCtx, value: f32| {
        ctx.game().set_anim_additive_weight(value)
    });
    engine.register_fn("set_anim_layer_weight", |ctx: &mut ScriptCtx, layer: &str, value: f32| {
        ctx.game().set_anim_layer_weight(layer, value)
    });
}
