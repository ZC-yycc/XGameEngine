//! 暴露给脚本的宿主 API —— Lua 转发壳。
//!
//! 每个函数体调 [`crate::GameCtx`] 的同名方法，**API 的真相在 `ctx.rs`**。
//! 本模块只负责 Lua 专属的翻译：把脚本的松散参数（`x, y, z`、整数 ID）
//! 收拢成引擎类型，以及把结果摊回 Lua 值（表 / 数字）。
//!
//! 两处 Lua 专属的形状（都是 Lua 的语言习惯，不是能力差异）：
//!
//! - **`ctx` 是 userdata**。`ctx` 统一作为每个全局函数的第一参数传入。
//! - **`Vec3` / `Vec2` 是 Lua 表**（`{x=, y=, z=}`），不是 userdata。这样
//!   `pos(ctx).x` 直接成立、零注册开销，也是 Lua 生态最普遍的表示。代价是
//!   没有 `a + b` 向量运算符 —— 常见写法是取分量做标量算术
//!   （`set_pos(ctx, p.x + 1.0, p.y, p.z)`）；向量运算走 `vadd` / `vsub` /
//!   `vmul` 等具名函数。
//! - **整数 ID 就是 Lua 整数**（`0` = 无效）。
//!
//! # 坐标约定
//!
//! 与 `ctx.rs` 完全一致（世界位置 / 局部旋转），见那里的表。

use mlua::{Error, FromLua, Lua, Table, Value};
use xgame_core::EntityId;
use xgame_math::glam::{Vec2, Vec3};

use crate::context::ScriptCtx;
use crate::ctx::RayHit;

/// Lua 侧的脚本上下文句柄。
///
/// 它是 [`ScriptCtx`]（`Copy` 地址束）的一层壳。**传给脚本时不再现造
/// userdata**（每次调用 `lua_newuserdata` + 元表挂接实测 ~284 ns/次，
/// 1000 实体 ×2 钩子 ≈568 µs/帧，比脚本本体还贵），改为传宿主常驻槽的
/// **轻量指针**（`LightUserData`，~3 ns）：[`LuaHost`] 持有一个
/// `Box<MaybeUninit<LuaCtx>>`，每次钩子调用前写入本次地址束，脚本侧
/// [`FromLua`] 把指针解回一份 `Copy`。
///
/// # 为什么这样仍然安全
///
/// - 指针**永远**指向宿主持有的槽：槽在 [`LuaHost`] 的堆上，任何脚本可见的
///   读取都发生在某次钩子调用内（脚本代码只在钩子调用窗口里跑，没有别的
///   宿主入口），而槽在每次调用入口就被写成本次 ctx —— 所以读到的一定是
///   **当前正在执行的这次调用**的地址束，被指向的对象（`GameCtx`）此刻由
///   调用栈上的 `&mut` 独占（调用窗口约定见 `context.rs` 模块文档）。
/// - 脚本把句柄存起来下帧再用（违约）时，读到的也是**下帧那次调用**的
///   有效 ctx —— 语义是"当前运行中的上下文"，不是悬垂地址。这比旧方案
///   （userdata 里存一份过期拷贝、下帧用 = 解引用已死栈帧）更不容易踩坑。
/// - 脚本没法**伪造**这种值：Lua 语言层没有构造 lightuserdata 的操作，
///   VM 里出现的 lightuserdata 只可能是我们放进去的槽指针。
///
/// 用指针而不是裸 `usize`：`Value::LightUserData` 仍带 tag，脚本传错第一参
/// （数字 / 表 / nil）时 [`FromLua`] 给出清晰报错，而不是把随机数当地址解引用。
#[derive(Clone, Copy)]
pub(crate) struct LuaCtx(ScriptCtx);

impl LuaCtx {
    /// 从脚本上下文地址束构造（宿主侧唯一入口）。
    pub(crate) fn from_script_ctx(ctx: ScriptCtx) -> Self {
        Self(ctx)
    }

    /// 把自己摊成传给脚本的轻量指针（指向宿主常驻槽，见类型文档）。
    ///
    /// 入参就是槽里那个 `LuaCtx` 的地址 —— 传引用而不是裸指针，让"指针必须
    /// 指向有效槽"这条责任留在安全 Rust 侧（槽地址由宿主的 `&mut` 保证）。
    pub(crate) fn as_light(ctx: &LuaCtx) -> mlua::LightUserData {
        mlua::LightUserData(ctx as *const LuaCtx as *mut _)
    }

    #[inline]
    pub(crate) fn game(&self) -> &mut crate::GameCtx<'static> {
        self.0.game()
    }

    #[inline]
    fn state(&self) -> Option<&mut Table> {
        self.0.state()
    }
}

// mlua 只为 `T: UserData` 提供 `IntoLua`，没有对应的 `FromLua`（见 conversion.rs）。
// 这里手写：lightuserdata → 槽地址 → 解出一份 `Copy`（`ScriptCtx: Copy`）。
// 参数校验仍由类型层做：不是 lightuserdata（脚本传了数字 / 表 / nil）直接报错，
// 不会把随机值当地址解引用 —— 伪造 lightuserdata 需要 Lua 语言层没有的操作。
impl FromLua for LuaCtx {
    fn from_lua(value: Value, _lua: &Lua) -> mlua::Result<Self> {
        let mlua::LightUserData(ptr) = value.as_light_userdata().ok_or_else(|| {
            Error::FromLuaConversionError {
                from: value.type_name(),
                to: "ScriptCtx".to_owned(),
                message: Some("期望脚本上下文句柄（第一个参数）".to_owned()),
            }
        })?;
        // SAFETY: 槽由 LuaHost 独占持有，任何脚本可见的执行都发生在钩子调用
        // 窗口内（见 `LuaCtx` 类型文档的安全论证），窗口内槽已写入本次 ctx。
        Ok(unsafe { *(ptr as *const LuaCtx) })
    }
}

// ---------------------------------------------------------------------------
// 注册入口
// ---------------------------------------------------------------------------

/// 把整套 API 装进 Lua 虚拟机的全局表。
pub(crate) fn register(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    register_math(lua, &globals)?;
    register_transform(lua, &globals)?;
    register_camera(lua, &globals)?;
    register_entity(lua, &globals)?;
    register_scene(lua, &globals)?;
    register_spawn(lua, &globals)?;
    register_physics(lua, &globals)?;
    register_input(lua, &globals)?;
    register_time_and_log(lua, &globals)?;
    register_state(lua, &globals)?;
    register_animation(lua, &globals)?;
    register_audio(lua, &globals)?;
    crate::ui_api::register_ui(lua, &globals)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 脚本 ↔ 引擎的参数翻译
// ---------------------------------------------------------------------------

/// 脚本的整数 ID（`0` = 无效）→ 引擎 [`EntityId`]。
#[inline]
pub(crate) fn id_of(raw: i64) -> EntityId {
    EntityId::from_raw(raw as u64)
}

/// 同上，但把 `<= 0` 判成"没有目标"（`set_parent(ctx, child, 0)` = 挂根）。
#[inline]
fn opt_id(raw: i64) -> Option<EntityId> {
    (raw > 0).then(|| EntityId::from_raw(raw as u64))
}

/// 引擎 [`EntityId`] → 脚本整数。
#[inline]
fn raw_i64(id: EntityId) -> i64 {
    id.raw() as i64
}

/// 引擎 `Vec3` → Lua 表 `{x=, y=, z=}`。
fn vec3_to_lua(lua: &Lua, v: Vec3) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("x", v.x)?;
    t.set("y", v.y)?;
    t.set("z", v.z)?;
    Ok(t)
}

/// 引擎 `Vec2` → Lua 表 `{x=, y=}`。
fn vec2_to_lua(lua: &Lua, v: Vec2) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("x", v.x)?;
    t.set("y", v.y)?;
    Ok(t)
}

/// 把射线命中结果摊进 Lua 表。
///
/// 用表而不是自定义类型，且**所有字段都填上**（含 `hit=false`
/// 的分支），脚本里 `hit.point` 永远拿得到东西。`point` / `normal` 是嵌套的向量表。
fn ray_hit_table(lua: &Lua, hit: RayHit) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("hit", hit.hit)?;
    t.set("point", vec3_to_lua(lua, hit.point)?)?;
    t.set("normal", vec3_to_lua(lua, hit.normal)?)?;
    t.set("distance", hit.distance)?;
    t.set("entity", hit.entity.map_or(0, raw_i64))?;
    t.set("is_sensor", hit.is_sensor)?;
    Ok(t)
}

// ---------------------------------------------------------------------------
// 数学（Lua 专属：Vec3/Vec2 是表）
// ---------------------------------------------------------------------------

fn register_math(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    globals.set(
        "vec3",
        lua.create_function(|lua, (x, y, z): (f32, f32, f32)| vec3_to_lua(lua, Vec3::new(x, y, z)))?,
    )?;
    globals.set(
        "vec2",
        lua.create_function(|lua, (x, y): (f32, f32)| vec2_to_lua(lua, Vec2::new(x, y)))?,
    )?;

    // 向量工具（Lua 无运算符重载，走具名函数）。
    globals.set(
        "vlength",
        lua.create_function(|_, v: Table| Ok(vec3_of(&v).length()))?,
    )?;
    globals.set(
        "vlength_squared",
        lua.create_function(|_, v: Table| Ok(vec3_of(&v).length_squared()))?,
    )?;
    globals.set(
        "vnormalized",
        lua.create_function(|lua, v: Table| {
            // 零向量归一化会得到 NaN，脚本作者很难自己发现 —— 这里退回零向量。
            let v = vec3_of(&v);
            let len = v.length();
            let out = if len > 1e-6 { v / len } else { Vec3::ZERO };
            vec3_to_lua(lua, out)
        })?,
    )?;
    globals.set(
        "vdot",
        lua.create_function(|_, (a, b): (Table, Table)| Ok(vec3_of(&a).dot(vec3_of(&b))))?,
    )?;
    globals.set(
        "vcross",
        lua.create_function(|lua, (a, b): (Table, Table)| {
            vec3_to_lua(lua, vec3_of(&a).cross(vec3_of(&b)))
        })?,
    )?;
    globals.set(
        "vadd",
        lua.create_function(|lua, (a, b): (Table, Table)| {
            vec3_to_lua(lua, vec3_of(&a) + vec3_of(&b))
        })?,
    )?;
    globals.set(
        "vsub",
        lua.create_function(|lua, (a, b): (Table, Table)| {
            vec3_to_lua(lua, vec3_of(&a) - vec3_of(&b))
        })?,
    )?;
    globals.set(
        "vmul",
        lua.create_function(|lua, (a, s): (Table, f32)| vec3_to_lua(lua, vec3_of(&a) * s))?,
    )?;
    globals.set(
        "vlerp",
        lua.create_function(|lua, (a, b, t): (Table, Table, f32)| {
            vec3_to_lua(lua, vec3_of(&a).lerp(vec3_of(&b), t.clamp(0.0, 1.0)))
        })?,
    )?;
    globals.set(
        "vdistance_to",
        lua.create_function(|_, (a, b): (Table, Table)| Ok(vec3_of(&a).distance(vec3_of(&b))))?,
    )?;
    globals.set(
        "vtostring",
        lua.create_function(|_, v: Table| {
            let v = vec3_of(&v);
            Ok(format!("({:.3}, {:.3}, {:.3})", v.x, v.y, v.z))
        })?,
    )?;
    Ok(())
}

/// 从 Lua 表读出 `Vec3`（缺分量按 0 处理 —— 检视是旁路，不该因脚本少写一个
/// 分量就报错拖垮这一帧）。
fn vec3_of(t: &Table) -> Vec3 {
    Vec3::new(
        t.get::<f32>("x").unwrap_or(0.0),
        t.get::<f32>("y").unwrap_or(0.0),
        t.get::<f32>("z").unwrap_or(0.0),
    )
}

// ---------------------------------------------------------------------------
// 自身变换
// ---------------------------------------------------------------------------

fn register_transform(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    globals.set(
        "pos",
        lua.create_function(|lua, ctx: LuaCtx| vec3_to_lua(lua, ctx.game().pos()))?,
    )?;
    globals.set(
        "local_pos",
        lua.create_function(|lua, ctx: LuaCtx| vec3_to_lua(lua, ctx.game().local_pos()))?,
    )?;
    globals.set(
        "set_pos",
        lua.create_function(|_, (ctx, x, y, z): (LuaCtx, f32, f32, f32)| {
            ctx.game().set_pos(Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "set_local_pos",
        lua.create_function(|_, (ctx, x, y, z): (LuaCtx, f32, f32, f32)| {
            ctx.game().set_local_pos(Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "translate",
        lua.create_function(|_, (ctx, x, y, z): (LuaCtx, f32, f32, f32)| {
            ctx.game().translate(Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "rot",
        lua.create_function(|lua, ctx: LuaCtx| vec3_to_lua(lua, ctx.game().rot()))?,
    )?;
    globals.set(
        "set_rot",
        lua.create_function(|_, (ctx, x, y, z): (LuaCtx, f32, f32, f32)| {
            ctx.game().set_rot(Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "rotate",
        lua.create_function(|_, (ctx, x, y, z): (LuaCtx, f32, f32, f32)| {
            ctx.game().rotate(Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "scale",
        lua.create_function(|lua, ctx: LuaCtx| vec3_to_lua(lua, ctx.game().scale()))?,
    )?;
    globals.set(
        "set_scale",
        lua.create_function(|_, (ctx, x, y, z): (LuaCtx, f32, f32, f32)| {
            ctx.game().set_scale(Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;

    // 世界朝向。
    globals.set(
        "forward",
        lua.create_function(|lua, ctx: LuaCtx| vec3_to_lua(lua, ctx.game().forward()))?,
    )?;
    globals.set(
        "right",
        lua.create_function(|lua, ctx: LuaCtx| vec3_to_lua(lua, ctx.game().right()))?,
    )?;
    globals.set(
        "up",
        lua.create_function(|lua, ctx: LuaCtx| vec3_to_lua(lua, ctx.game().up()))?,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 相机
// ---------------------------------------------------------------------------

fn register_camera(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    // `look_at` / `look_at_entity` 是"朝向"而不是"相机专属"：任何物体都能用
    // （炮塔转向玩家、NPC 盯人）。放在相机这一节只是因为它们最常见的用途是
    // 推跟随相机 —— 名字不带 `camera_` 前缀，免得让人以为只能给相机用。
    globals.set(
        "look_at",
        lua.create_function(|_, (ctx, x, y, z): (LuaCtx, f32, f32, f32)| {
            ctx.game().look_at(Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "look_at_entity",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| {
            if let Some(id) = opt_id(id) {
                ctx.game().look_at_entity(id);
            }
            Ok(())
        })?,
    )?;
    // `active_camera` 返回 0 表示"场景里没有相机实体"（与 `entity_id` 找不到
    // 返回 0 同一个哨兵值，脚本不必处理 nil）。
    globals.set(
        "active_camera",
        lua.create_function(|_, ctx: LuaCtx| {
            Ok(ctx.game().active_camera().map_or(0, raw_i64))
        })?,
    )?;
    globals.set(
        "set_active_camera",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| {
            if let Some(id) = opt_id(id) {
                ctx.game().set_active_camera(id);
            }
            Ok(())
        })?,
    )?;
    globals.set(
        "camera_fov",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| {
            Ok(opt_id(id).and_then(|id| ctx.game().camera_fov(id)))
        })?,
    )?;
    globals.set(
        "set_camera_fov",
        lua.create_function(|_, (ctx, id, degrees): (LuaCtx, i64, f32)| {
            if let Some(id) = opt_id(id) {
                ctx.game().set_camera_fov(id, degrees);
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 实体查询与操作
// ---------------------------------------------------------------------------

fn register_entity(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    globals.set(
        "self_id",
        lua.create_function(|_, ctx: LuaCtx| Ok(raw_i64(ctx.game().self_id())))?,
    )?;
    globals.set(
        "self_name",
        lua.create_function(|_, ctx: LuaCtx| Ok(ctx.game().self_name()))?,
    )?;
    globals.set(
        "entity_count",
        lua.create_function(|_, ctx: LuaCtx| Ok(ctx.game().entity_count() as i64))?,
    )?;
    globals.set(
        "entity_at",
        lua.create_function(|_, (ctx, index): (LuaCtx, i64)| {
            Ok(ctx
                .game()
                .entity_at(index.max(0) as usize)
                .map_or(0, raw_i64))
        })?,
    )?;
    globals.set(
        "entity_id",
        lua.create_function(|_, (ctx, name): (LuaCtx, String)| {
            Ok(ctx.game().find_entity_id(&name).map_or(0, raw_i64))
        })?,
    )?;
    globals.set(
        "entity_name",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| Ok(ctx.game().entity_name(id_of(id))))?,
    )?;
    globals.set(
        "entity_model",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| {
            Ok(ctx.game().entity_model(id_of(id)))
        })?,
    )?;
    globals.set(
        "set_entity_model",
        lua.create_function(|_, (ctx, id, model): (LuaCtx, i64, String)| {
            ctx.game().set_entity_model(id_of(id), &model);
            Ok(())
        })?,
    )?;
    globals.set(
        "material_asset",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| {
            Ok(ctx.game().entity_material_asset(id_of(id)))
        })?,
    )?;
    globals.set(
        "set_material_asset",
        lua.create_function(|_, (ctx, id, path): (LuaCtx, i64, String)| {
            ctx.game().set_entity_material_asset(id_of(id), &path);
            Ok(())
        })?,
    )?;
    globals.set(
        "set_material",
        lua.create_function(
            |_, (ctx, id, r, g, b, a, metallic, roughness): (
                LuaCtx,
                i64,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
            )| {
                ctx.game()
                    .set_entity_material(id_of(id), [r, g, b, a], metallic, roughness);
                Ok(())
            },
        )?,
    )?;
    globals.set(
        "clear_material",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| {
            ctx.game().clear_entity_material(id_of(id));
            Ok(())
        })?,
    )?;
    // ── 粒子（运行态动作；参数配置在场景 / Inspector）──
    globals.set(
        "particle_emitting",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| {
            Ok(ctx.game().entity_particle_emitting(id_of(id)))
        })?,
    )?;
    globals.set(
        "set_particle_emitting",
        lua.create_function(|_, (ctx, id, on): (LuaCtx, i64, bool)| {
            ctx.game().set_entity_particle_emitting(id_of(id), on);
            Ok(())
        })?,
    )?;
    globals.set(
        "particle_burst",
        lua.create_function(|_, (ctx, id, count): (LuaCtx, i64, u32)| {
            ctx.game().entity_particle_burst(id_of(id), count);
            Ok(())
        })?,
    )?;
    globals.set(
        "set_particle_rate",
        lua.create_function(|_, (ctx, id, rate): (LuaCtx, i64, f32)| {
            ctx.game().set_entity_particle_rate(id_of(id), rate);
            Ok(())
        })?,
    )?;
    globals.set(
        "entity_pos",
        lua.create_function(|lua, (ctx, id): (LuaCtx, i64)| {
            vec3_to_lua(lua, ctx.game().entity_pos(id_of(id)))
        })?,
    )?;
    globals.set(
        "set_entity_pos",
        lua.create_function(|_, (ctx, id, x, y, z): (LuaCtx, i64, f32, f32, f32)| {
            ctx.game().set_entity_pos(id_of(id), Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "entity_scale",
        lua.create_function(|lua, (ctx, id): (LuaCtx, i64)| {
            vec3_to_lua(lua, ctx.game().entity_scale(id_of(id)))
        })?,
    )?;
    globals.set(
        "set_entity_scale",
        lua.create_function(|_, (ctx, id, x, y, z): (LuaCtx, i64, f32, f32, f32)| {
            ctx.game().set_entity_scale(id_of(id), Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "entity_rot",
        lua.create_function(|lua, (ctx, id): (LuaCtx, i64)| {
            vec3_to_lua(lua, ctx.game().entity_rot(id_of(id)))
        })?,
    )?;
    globals.set(
        "set_entity_rot",
        lua.create_function(|_, (ctx, id, x, y, z): (LuaCtx, i64, f32, f32, f32)| {
            ctx.game().set_entity_rot(id_of(id), Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "entity_forward",
        lua.create_function(|lua, (ctx, id): (LuaCtx, i64)| {
            vec3_to_lua(lua, ctx.game().entity_forward(id_of(id)))
        })?,
    )?;
    globals.set(
        "distance_to",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| Ok(ctx.game().distance_to(id_of(id))))?,
    )?;
    globals.set(
        "destroy",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| {
            ctx.game().destroy(id_of(id));
            Ok(())
        })?,
    )?;
    globals.set(
        "set_parent",
        lua.create_function(|_, (ctx, child, parent): (LuaCtx, i64, i64)| {
            ctx.game().set_parent(id_of(child), opt_id(parent));
            Ok(())
        })?,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 生成实体
// ---------------------------------------------------------------------------

fn register_spawn(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    // 函数名沿用历史命名 `spawn_entity`（早期 Rhai 轨把 `spawn` 当保留字，
    // 后缀是那时留下的；删 Rhai 后保持不变以免破坏既有脚本与文档）。
    globals.set(
        "spawn_entity",
        lua.create_function(|_, (ctx, name, model): (LuaCtx, String, String)| {
            Ok(raw_i64(ctx.game().spawn_entity(&name, &model)))
        })?,
    )?;
    globals.set(
        "spawn_entity_at",
        lua.create_function(
            |_, (ctx, name, model, x, y, z): (LuaCtx, String, String, f32, f32, f32)| {
                Ok(raw_i64(ctx.game().spawn_entity_at(&name, &model, Vec3::new(x, y, z))))
            },
        )?,
    )?;
    globals.set(
        "spawn_ball",
        lua.create_function(|_, (ctx, name, radius, x, y, z): (LuaCtx, String, f32, f32, f32, f32)| {
            Ok(raw_i64(ctx.game().spawn_ball(&name, radius, Vec3::new(x, y, z))))
        })?,
    )?;
    globals.set(
        "spawn_static_box",
        lua.create_function(
            |_, (ctx, name, hx, hy, hz, x, y, z): (LuaCtx, String, f32, f32, f32, f32, f32, f32)| {
                Ok(raw_i64(
                    ctx.game()
                        .spawn_static_box(&name, Vec3::new(hx, hy, hz), Vec3::new(x, y, z)),
                ))
            },
        )?,
    )?;

    // 预制体实例化：返回实例根的 id，失败返回 **nil**（另一类生成函数永远
    // 成功，所以直接给 id；预制体要读盘，失败是常态，脚本必须能判）。
    globals.set(
        "spawn_prefab",
        lua.create_function(
            |_, (ctx, path, x, y, z): (LuaCtx, String, f32, f32, f32)| {
                Ok(ctx
                    .game()
                    .spawn_prefab(&path, Vec3::new(x, y, z))
                    .map(raw_i64))
            },
        )?,
    )?;
    globals.set(
        "spawn_prefab_under",
        lua.create_function(
            |_, (ctx, path, parent, x, y, z): (LuaCtx, String, i64, f32, f32, f32)| {
                Ok(ctx
                    .game()
                    .spawn_prefab_under(&path, id_of(parent), Vec3::new(x, y, z))
                    .map(raw_i64))
            },
        )?,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 场景
// ---------------------------------------------------------------------------

fn register_scene(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    globals.set(
        "current_scene",
        lua.create_function(|_, ctx: LuaCtx| Ok(ctx.game().current_scene().to_owned()))?,
    )?;

    // 只登记请求，真正装载在 `World::update` 末尾 —— 钩子执行期间场景正被
    // 可变借用，中途换掉它等于把脚下那份抽走（见 `ScriptCommand::LoadScene`）。
    globals.set(
        "load_scene",
        lua.create_function(|_, (ctx, path): (LuaCtx, String)| {
            ctx.game().load_scene(&path);
            Ok(())
        })?,
    )?;

    // -- 关卡顺序（project.ron 的 build.scenes）----------------------------
    //
    // 三者下标都是 **0 起**（与 `load_scene_at` 直接配对）。Lua 惯例是 1 起，
    // 但这里刻意的：`load_scene_at(scene_index(ctx) + 1)` 读起来最顺，
    // 若折成 1 起，每个调用点都要 `+1 -1` 往返，错一次就是串关。
    globals.set(
        "scene_count",
        lua.create_function(|_, ctx: LuaCtx| Ok(ctx.game().scene_count() as i64))?,
    )?;

    globals.set(
        "scene_index",
        lua.create_function(|_, ctx: LuaCtx| Ok(ctx.game().scene_index().map(|i| i as i64)))?,
    )?;

    globals.set(
        "load_scene_at",
        lua.create_function(|_, (ctx, index): (LuaCtx, i64)| {
            // 负数在 Lua 里可能来自 `index - 1` 算错：按越界处理（记 error），
            // 不当成 usize 溢出后变成天文数字。
            if index < 0 {
                ctx.game().error(format!("load_scene_at({index}) 失败：关卡下标不能为负"));
                return Ok(());
            }
            ctx.game().load_scene_at(index as usize);
            Ok(())
        })?,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 物理
// ---------------------------------------------------------------------------

fn register_physics(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    globals.set(
        "has_physics",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| Ok(ctx.game().has_physics(id_of(id))))?,
    )?;

    globals.set(
        "raycast",
        lua.create_function(
            |lua,
             (ctx, ox, oy, oz, dx, dy, dz, max_dist): (
                LuaCtx,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
            )| {
                let hit = ctx
                    .game()
                    .raycast(Vec3::new(ox, oy, oz), Vec3::new(dx, dy, dz), max_dist);
                ray_hit_table(lua, hit)
            },
        )?,
    )?;

    globals.set(
        "raycast_masked",
        lua.create_function(
            |lua,
             (ctx, ox, oy, oz, dx, dy, dz, max_dist, mask): (
                LuaCtx,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                u32,
            )| {
                let hit = ctx.game().raycast_masked(
                    Vec3::new(ox, oy, oz),
                    Vec3::new(dx, dy, dz),
                    max_dist,
                    mask,
                );
                ray_hit_table(lua, hit)
            },
        )?,
    )?;

    globals.set(
        "set_layer",
        lua.create_function(|_, (ctx, id, layer): (LuaCtx, i64, i64)| {
            // 负 / 越界的层号按取模收敛（与 Rust 侧 `sanitize_layer` 同一条规则），
            // 不 panic 也不静默改成 0 —— 脚本传 -1 的意图通常是"最后一层"。
            Ok(ctx.game().set_layer(id_of(id), layer as u8))
        })?,
    )?;
    globals.set(
        "layer",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| Ok(ctx.game().layer(id_of(id))))?,
    )?;

    globals.set(
        "apply_force",
        lua.create_function(|_, (ctx, id, x, y, z): (LuaCtx, i64, f32, f32, f32)| {
            ctx.game().apply_force(id_of(id), Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "apply_impulse",
        lua.create_function(|_, (ctx, id, x, y, z): (LuaCtx, i64, f32, f32, f32)| {
            ctx.game().apply_impulse(id_of(id), Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "add_torque",
        lua.create_function(|_, (ctx, id, x, y, z): (LuaCtx, i64, f32, f32, f32)| {
            ctx.game().add_torque(id_of(id), Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "velocity",
        lua.create_function(|lua, (ctx, id): (LuaCtx, i64)| {
            vec3_to_lua(lua, ctx.game().velocity(id_of(id)))
        })?,
    )?;
    globals.set(
        "set_velocity",
        lua.create_function(|_, (ctx, id, x, y, z): (LuaCtx, i64, f32, f32, f32)| {
            ctx.game().set_velocity(id_of(id), Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "gravity",
        lua.create_function(|lua, ctx: LuaCtx| vec3_to_lua(lua, ctx.game().gravity()))?,
    )?;
    globals.set(
        "set_gravity",
        lua.create_function(|_, (ctx, x, y, z): (LuaCtx, f32, f32, f32)| {
            ctx.game().set_gravity(Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;

    // -- 角色控制器 --------------------------------------------------------
    globals.set(
        "character_move",
        lua.create_function(|_, (ctx, x, y, z): (LuaCtx, f32, f32, f32)| {
            ctx.game().character_move(Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "character_simple_move",
        lua.create_function(|_, (ctx, x, y, z): (LuaCtx, f32, f32, f32)| {
            ctx.game().character_simple_move(Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "character_jump",
        lua.create_function(|_, (ctx, speed): (LuaCtx, f32)| {
            Ok(ctx.game().character_jump(speed))
        })?,
    )?;
    globals.set(
        "character_velocity",
        lua.create_function(|lua, ctx: LuaCtx| vec3_to_lua(lua, ctx.game().character_velocity()))?,
    )?;
    globals.set(
        "character_set_velocity",
        lua.create_function(|_, (ctx, x, y, z): (LuaCtx, f32, f32, f32)| {
            ctx.game().character_set_velocity(Vec3::new(x, y, z));
            Ok(())
        })?,
    )?;
    globals.set(
        "character_is_grounded",
        lua.create_function(|_, ctx: LuaCtx| Ok(ctx.game().character_is_grounded()))?,
    )?;
    globals.set(
        "character_ground_normal",
        lua.create_function(|lua, ctx: LuaCtx| {
            vec3_to_lua(lua, ctx.game().character_ground_normal())
        })?,
    )?;
    globals.set(
        "character_is_sliding",
        lua.create_function(|_, ctx: LuaCtx| Ok(ctx.game().character_is_sliding()))?,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 输入
// ---------------------------------------------------------------------------

fn register_input(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    globals.set(
        "action_down",
        lua.create_function(|_, (ctx, action): (LuaCtx, String)| {
            Ok(ctx.game().action_down(&action))
        })?,
    )?;
    globals.set(
        "action_pressed",
        lua.create_function(|_, (ctx, action): (LuaCtx, String)| {
            Ok(ctx.game().action_pressed(&action))
        })?,
    )?;
    globals.set(
        "action_released",
        lua.create_function(|_, (ctx, action): (LuaCtx, String)| {
            Ok(ctx.game().action_released(&action))
        })?,
    )?;
    globals.set(
        "axis",
        lua.create_function(|_, (ctx, axis): (LuaCtx, String)| Ok(ctx.game().axis(&axis)))?,
    )?;

    globals.set(
        "key_down",
        lua.create_function(|_, (ctx, key): (LuaCtx, String)| Ok(ctx.game().key_down(&key)))?,
    )?;
    globals.set(
        "key_pressed",
        lua.create_function(|_, (ctx, key): (LuaCtx, String)| Ok(ctx.game().key_pressed(&key)))?,
    )?;
    globals.set(
        "mouse_down",
        lua.create_function(|_, (ctx, button): (LuaCtx, String)| {
            Ok(ctx.game().mouse_down(&button))
        })?,
    )?;
    globals.set(
        "mouse_pressed",
        lua.create_function(|_, (ctx, button): (LuaCtx, String)| {
            Ok(ctx.game().mouse_pressed(&button))
        })?,
    )?;
    globals.set(
        "mouse_pos",
        lua.create_function(|lua, ctx: LuaCtx| vec2_to_lua(lua, ctx.game().mouse_pos()))?,
    )?;
    globals.set(
        "mouse_delta",
        lua.create_function(|lua, ctx: LuaCtx| vec2_to_lua(lua, ctx.game().mouse_delta()))?,
    )?;
    globals.set(
        "scroll",
        lua.create_function(|lua, ctx: LuaCtx| vec2_to_lua(lua, ctx.game().scroll()))?,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 时间与日志
// ---------------------------------------------------------------------------

fn register_time_and_log(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    globals.set(
        "delta",
        lua.create_function(|_, ctx: LuaCtx| Ok(ctx.game().delta()))?,
    )?;
    globals.set(
        "time",
        lua.create_function(|_, ctx: LuaCtx| Ok(ctx.game().time()))?,
    )?;
    // 时间缩放：`set_time_scale(ctx, 0)` 就是暂停（Unity `Time.timeScale` 语义）。
    globals.set(
        "time_scale",
        lua.create_function(|_, ctx: LuaCtx| Ok(ctx.game().time_scale()))?,
    )?;
    globals.set(
        "set_time_scale",
        lua.create_function(|_, (ctx, scale): (LuaCtx, f32)| {
            ctx.game().set_time_scale(scale);
            Ok(())
        })?,
    )?;

    globals.set(
        "log",
        lua.create_function(|_, (ctx, msg): (LuaCtx, String)| {
            ctx.game().log(msg);
            Ok(())
        })?,
    )?;
    globals.set(
        "warn",
        lua.create_function(|_, (ctx, msg): (LuaCtx, String)| {
            ctx.game().warn(msg);
            Ok(())
        })?,
    )?;
    globals.set(
        "error",
        lua.create_function(|_, (ctx, msg): (LuaCtx, String)| {
            ctx.game().error(msg);
            Ok(())
        })?,
    )?;

    // 数值工具：脚本里算插值/角度非常常用。
    globals.set(
        "clamp",
        lua.create_function(|_, (v, lo, hi): (f32, f32, f32)| Ok(v.clamp(lo, hi)))?,
    )?;
    globals.set(
        "lerpf",
        lua.create_function(|_, (a, b, t): (f32, f32, f32)| Ok(a + (b - a) * t.clamp(0.0, 1.0)))?,
    )?;
    globals.set(
        "deg_to_rad",
        lua.create_function(|_, d: f32| Ok(d.to_radians()))?,
    )?;
    globals.set(
        "rad_to_deg",
        lua.create_function(|_, r: f32| Ok(r.to_degrees()))?,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 每 (实体, 脚本) 一份的持久状态
// ---------------------------------------------------------------------------

/// Lua 的状态表就是 [`ScriptCtx`] 携带的那张 `mlua::Table`。
///
/// Lua 的函数**能**写外层 upvalue，但 upvalue 是**脚本级共享**的
/// （同一脚本的所有实体看到同一个变量），而 `state_*` 按 `(实体, 脚本)`
/// 分开 —— "每个实体一份血量"这类需求只能走这组 API。
fn register_state(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    globals.set(
        "state_get",
        lua.create_function(|_, (ctx, key): (LuaCtx, String)| {
            // 没设过的键返回 `nil`，脚本里用 `if x == nil` 判空即可。
            let v = ctx
                .state()
                .and_then(|t| t.get::<Value>(key.as_str()).ok())
                .unwrap_or(Value::Nil);
            Ok(v)
        })?,
    )?;

    globals.set(
        "state_get_or",
        lua.create_function(|_, (ctx, key, default): (LuaCtx, String, Value)| {
            let v = ctx
                .state()
                .and_then(|t| t.get::<Value>(key.as_str()).ok())
                .unwrap_or(Value::Nil);
            Ok(if v == Value::Nil { default } else { v })
        })?,
    )?;

    globals.set(
        "state_set",
        lua.create_function(|_, (ctx, key, value): (LuaCtx, String, Value)| {
            if let Some(t) = ctx.state() {
                // 状态表写失败只可能是 Lua 内存耗尽，不该拖垮这一帧。
                let _ = t.set(key, value);
            }
            Ok(())
        })?,
    )?;

    globals.set(
        "state_has",
        lua.create_function(|_, (ctx, key): (LuaCtx, String)| {
            Ok(ctx
                .state()
                .map(|t| t.contains_key(key.as_str()).unwrap_or(false))
                .unwrap_or(false))
        })?,
    )?;

    globals.set(
        "state_remove",
        lua.create_function(|_, (ctx, key): (LuaCtx, String)| {
            if let Some(t) = ctx.state() {
                // Lua 里把键置 nil 即删除。
                let _ = t.set(key, Value::Nil);
            }
            Ok(())
        })?,
    )?;

    globals.set(
        "state_clear",
        lua.create_function(|_, ctx: LuaCtx| {
            if let Some(t) = ctx.state() {
                // mlua 的 Table 没有 clear()，遍历键逐个置 nil。
                let keys: Vec<Value> = match t
                    .pairs::<Value, Value>()
                    .collect::<mlua::Result<Vec<_>>>()
                {
                    Ok(pairs) => pairs.into_iter().map(|(k, _)| k).collect(),
                    Err(_) => return Ok(()),
                };
                for k in keys {
                    let _ = t.set(k, Value::Nil);
                }
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 动画状态机
// ---------------------------------------------------------------------------

fn register_animation(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    globals.set(
        "set_anim_clip",
        lua.create_function(|_, (ctx, clip): (LuaCtx, String)| {
            ctx.game().set_anim_clip(&clip);
            Ok(())
        })?,
    )?;
    globals.set(
        "set_anim_speed",
        lua.create_function(|_, (ctx, speed): (LuaCtx, f32)| {
            ctx.game().set_anim_speed(speed);
            Ok(())
        })?,
    )?;
    globals.set(
        "set_anim_looping",
        lua.create_function(|_, (ctx, looping): (LuaCtx, bool)| {
            ctx.game().set_anim_looping(looping);
            Ok(())
        })?,
    )?;
    globals.set(
        "set_anim_fade_time",
        lua.create_function(|_, (ctx, seconds): (LuaCtx, f32)| {
            ctx.game().set_anim_fade_time(seconds);
            Ok(())
        })?,
    )?;
    globals.set(
        "set_anim_additive_clip",
        lua.create_function(|_, (ctx, clip): (LuaCtx, String)| {
            ctx.game().set_anim_additive_clip(&clip);
            Ok(())
        })?,
    )?;
    globals.set(
        "set_anim_layer_clip",
        lua.create_function(|_, (ctx, layer, clip): (LuaCtx, String, String)| {
            ctx.game().set_anim_layer_clip(&layer, &clip);
            Ok(())
        })?,
    )?;
    globals.set(
        "set_anim_param",
        lua.create_function(|_, (ctx, name, value): (LuaCtx, String, f32)| {
            ctx.game().set_anim_param(&name, value);
            Ok(())
        })?,
    )?;
    globals.set(
        "set_anim_additive_weight",
        lua.create_function(|_, (ctx, value): (LuaCtx, f32)| {
            ctx.game().set_anim_additive_weight(value);
            Ok(())
        })?,
    )?;
    globals.set(
        "set_anim_layer_weight",
        lua.create_function(|_, (ctx, layer, value): (LuaCtx, String, f32)| {
            ctx.game().set_anim_layer_weight(&layer, value);
            Ok(())
        })?,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 音频
// ---------------------------------------------------------------------------

/// 音频 API：全部固定参数（不用 Variadic）——参数个数固定换来的是
/// 脚本里写错参数个数时**当场报错**，而不是静默吞掉一个音效。
fn register_audio(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    globals.set(
        "play_sound",
        lua.create_function(|_, (ctx, clip, volume, pitch): (LuaCtx, String, f32, f32)| {
            ctx.game().play_sound(&clip, volume, pitch);
            Ok(())
        })?,
    )?;
    globals.set(
        "play_sound_at",
        lua.create_function(
            |_, (ctx, clip, x, y, z, volume): (LuaCtx, String, f32, f32, f32, f32)| {
                ctx.game().play_sound_at(&clip, x, y, z, volume);
                Ok(())
            },
        )?,
    )?;
    globals.set(
        "stop_sound",
        lua.create_function(|_, (ctx, clip): (LuaCtx, String)| {
            ctx.game().stop_sound(&clip);
            Ok(())
        })?,
    )?;
    globals.set(
        "stop_all_sounds",
        lua.create_function(|_, ctx: LuaCtx| {
            ctx.game().stop_all_sounds();
            Ok(())
        })?,
    )?;
    globals.set(
        "set_listener",
        lua.create_function(
            |_, (ctx, x, y, z, fx, fy, fz): (LuaCtx, f32, f32, f32, f32, f32, f32)| {
                ctx.game().set_listener(x, y, z, fx, fy, fz);
                Ok(())
            },
        )?,
    )?;
    globals.set(
        "set_master_volume",
        lua.create_function(|_, (ctx, volume): (LuaCtx, f32)| {
            ctx.game().set_master_volume(volume);
            Ok(())
        })?,
    )?;
    Ok(())
}
