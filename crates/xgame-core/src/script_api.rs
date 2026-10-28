//! 脚本 API 的机器可读清单（Agent-Native ③ 的另一半）。
//!
//! # 为什么需要它
//!
//! `Docs/脚本API.md` 是给人读的；智能体要写 `.lua` 时，需要一份能直接解析的
//! 契约：函数名、签名、参数、返回值、类别、生命周期钩子、值类型与全局约定。
//! `--export-script-api` 与 MCP `script_api_read` 都返回这份清单。
//!
//! # 真相源与防漂移
//!
//! 运行时注册名在 `xgame-script` 的 `lua_api.rs` / `ui_api.rs`。本目录是**手写**的
//! （Lua C 函数没有可反射的参数名），但有三层钉子：
//!
//! 1. `xgame-script` 测试从注册源码提取全部 `globals.set("…")`，与清单逐名比对；
//!    同时在真实 Lua VM 里确认每个名字都是函数。
//! 2. `xgame-editor` 测试从 `Docs/脚本API.md` 的 API 表提取函数名，与清单比对；
//!    文档加了函数而清单没加，测试红。
//! 3. 本模块测试检查类别引用、签名与函数名一致、名字唯一、函数 / 钩子数量。
//!
//! 参数、返回值与一句话语义仍然是手写的：它们描述的是**契约**，不是 Rust 闭包的
//! 类型推导结果。改注册时必须同步这里，否则第 1 条测试会红。
//!
//! # 确定性
//!
//! 没有时间戳、没有 HashMap 迭代序；函数与类别都按 `Docs/脚本API.md` 的顺序排列。
//! 同一份代码连续导出两次必须逐字节相同。

use serde::{Deserialize, Serialize};

/// 脚本 API 清单自身的格式版本。**加字段不涨，改语义 / 删字段 / 改返回值才涨。**
pub const SCRIPT_API_FORMAT_VERSION: u32 = 1;

/// 一份完整的脚本 API 说明（`--export-script-api` / MCP `script_api_read` 的返回体）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptApiDoc {
    /// 本说明书的格式版本（[`SCRIPT_API_FORMAT_VERSION`]）。
    pub format_version: u32,
    /// 生成它的 core crate 版本；只用于诊断，不参与兼容判断。
    pub engine_version: String,
    /// 跨函数的全局约定（必读）。
    pub conventions: Vec<String>,
    /// 生命周期钩子：不是全局函数，由脚本按需定义。
    pub hooks: Vec<ScriptHook>,
    /// Lua 里会出现的值类型 / 形状。
    pub value_types: Vec<ScriptValueType>,
    /// 按文档章节分组的全局函数。
    pub categories: Vec<ScriptCategory>,
}

impl ScriptApiDoc {
    /// 按名字查函数（测试与工具用；序列化时仍然只输出 `categories` 数组）。
    pub fn function(&self, name: &str) -> Option<&ScriptFunction> {
        self.categories
            .iter()
            .flat_map(|category| category.functions.iter())
            .find(|function| function.name.as_str() == name)
    }

    /// 遍历全部函数名（不保证字母序，按文档顺序）。
    pub fn function_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.categories
            .iter()
            .flat_map(|category| category.functions.iter())
            .map(|function| function.name.as_str())
    }

    /// 函数总数。
    pub fn function_count(&self) -> usize {
        self.categories
            .iter()
            .map(|category| category.functions.len())
            .sum()
    }
}

/// 一个生命周期钩子。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptHook {
    /// 钩子名（脚本里定义的全局函数名）。
    pub name: String,
    /// 签名，例如 `on_collision_enter(ctx, other)`。
    pub signature: String,
    /// 什么时候调用。
    pub timing: String,
    /// 一句话用途。
    pub summary: String,
}

/// Lua 里会出现的值类型 / 形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptValueType {
    /// 类型名（返回值里引用的名字）。
    pub name: String,
    /// 在 Lua 里的形状。
    pub lua_shape: String,
    /// 语义与易错点。
    pub note: String,
}

/// 一个文档章节 / API 类别。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptCategory {
    /// 稳定的 ASCII 标识（给工具做过滤 / 分组用）。
    pub id: String,
    /// 中文类别名（与 `Docs/脚本API.md` 的章节标题一致）。
    pub name: String,
    /// 这一组函数的共同语义。
    pub summary: String,
    /// 组内函数，按文档顺序。
    pub functions: Vec<ScriptFunction>,
}

/// 一个全局函数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptFunction {
    /// 全局函数名。
    pub name: String,
    /// 完整签名，例如 `set_pos(ctx, x, y, z)`。
    pub signature: String,
    /// 参数名（从签名解析；数学函数没有 `ctx`）。
    pub params: Vec<String>,
    /// 返回值类型；没有返回值时是 `"nil"`。
    pub returns: String,
    /// 一句话语义。
    pub summary: String,
}

/// 生成当前引擎版本的脚本 API 清单。
///
/// 这是 `--export-script-api` 与 MCP `script_api_read` 的唯一入口；不要在别处再拼一份。
pub fn script_api() -> ScriptApiDoc {
    let categories = CATEGORIES
        .iter()
        .map(|category| ScriptCategory {
            id: category.id.to_string(),
            name: category.name.to_string(),
            summary: category.summary.to_string(),
            functions: FUNCTIONS
                .iter()
                .filter(|function| function.category == category.id)
                .map(|function| ScriptFunction {
                    name: function.name.to_string(),
                    signature: function.signature.to_string(),
                    params: params_of(function.signature),
                    returns: function.returns.to_string(),
                    summary: function.summary.to_string(),
                })
                .collect(),
        })
        .collect();

    ScriptApiDoc {
        format_version: SCRIPT_API_FORMAT_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").to_string(),
        conventions: conventions(),
        hooks: hooks(),
        value_types: value_types(),
        categories,
    }
}

/// 全局约定：写脚本前先读这一组。
fn conventions() -> Vec<String> {
    vec![
        "钩子函数是可选的；脚本只需定义用到的那些，缺哪个就跳过哪个。".into(),
        "除数学函数（vec3 / vec2 / v* / clamp / lerpf / deg_to_rad / rad_to_deg）外，所有 API 的第一个参数都是 ctx。".into(),
        "实体 id 是整数，0 = 无效 / 找不到；只有文档明确写 nil 的接口才返回 nil。".into(),
        "Vec3 / Vec2 是普通 Lua 表，不是 userdata；没有 a + b 运算符重载，向量运算用 vadd / vsub / vmul / vdot 等。".into(),
        "资产路径一律是项目相对路径（正斜杠），不接受绝对路径或 ..。".into(),
        "顶层 local 是脚本级共享；每实体状态用 state_get / state_set（跨帧、跨热重载保留）。".into(),
        "脚本错误只影响本次钩子调用；单次调用有 20 万条指令预算，死循环会被中断，游戏继续。".into(),
        "函数名大小写敏感，全部是全局函数；调用时不要加模块前缀。".into(),
        "时间缩放（set_time_scale）影响物理固定步 / delta / time / 粒子 / 动画；音频与 UI 不受影响。".into(),
    ]
}

/// 六个生命周期钩子。
fn hooks() -> Vec<ScriptHook> {
    vec![
        hook(
            "start",
            "start(ctx)",
            "实体第一次进入运行时，一次",
            "初始化状态、查找其它实体。",
        ),
        hook(
            "update",
            "update(ctx)",
            "每个渲染帧一次",
            "视觉、UI、非物理逻辑。",
        ),
        hook(
            "fixed_update",
            "fixed_update(ctx)",
            "每个物理固定步长一次（默认 60 Hz）",
            "与物理交互的逻辑。",
        ),
        hook(
            "on_collision_enter",
            "on_collision_enter(ctx, other)",
            "与另一个实体开始接触时一次",
            "拾取、伤害、触发器。",
        ),
        hook(
            "on_collision_exit",
            "on_collision_exit(ctx, other)",
            "结束接触时一次",
            "离开触发区、停止持续伤害。",
        ),
        hook(
            "on_destroy",
            "on_destroy(ctx)",
            "实体被销毁之前一次",
            "清理、生成特效。",
        ),
    ]
}

fn hook(name: &str, signature: &str, timing: &str, summary: &str) -> ScriptHook {
    ScriptHook {
        name: name.to_string(),
        signature: signature.to_string(),
        timing: timing.to_string(),
        summary: summary.to_string(),
    }
}

/// Lua 里会出现的值类型。
fn value_types() -> Vec<ScriptValueType> {
    vec![
        value_type(
            "ctx",
            "userdata",
            "脚本上下文；只能由宿主作为钩子参数传入，不要自己构造。",
        ),
        value_type(
            "Vec3",
            "{ x: number, y: number, z: number }",
            "普通 Lua 表；没有运算符重载，用 vadd / vsub / vmul 等。",
        ),
        value_type(
            "Vec2",
            "{ x: number, y: number }",
            "普通 Lua 表；鼠标 / 滚动等二维值。",
        ),
        value_type(
            "RayHit",
            "{ hit: boolean, point: Vec3, normal: Vec3, distance: number, entity: EntityId, is_sensor: boolean }",
            "raycast 的返回表；永远有全部字段，没打中时 hit=false、其余为零值。",
        ),
        value_type(
            "EntityId",
            "integer",
            "实体 id；0 表示无效 / 找不到。",
        ),
        value_type(
            "AssetPath",
            "string",
            "项目相对路径（如 Resources/Audio/Beep.wav）；不接受绝对路径或 ..。",
        ),
        value_type(
            "LuaValue",
            "any Lua value",
            "state_* 可存任意 Lua 值；跨脚本边界不要放 userdata。",
        ),
    ]
}

fn value_type(name: &str, lua_shape: &str, note: &str) -> ScriptValueType {
    ScriptValueType {
        name: name.to_string(),
        lua_shape: lua_shape.to_string(),
        note: note.to_string(),
    }
}

/// 类别表。顺序 = `Docs/脚本API.md` 的章节顺序。
struct CategoryDef {
    id: &'static str,
    name: &'static str,
    summary: &'static str,
}

const CATEGORIES: &[CategoryDef] = &[
    CategoryDef {
        id: "transform",
        name: "自身变换",
        summary: "读取 / 修改调用者自己的世界位置、局部旋转与缩放。",
    },
    CategoryDef {
        id: "entity",
        name: "其它实体",
        summary: "按名字或索引查找实体，读写任意实体的变换，维护父子关系与销毁。",
    },
    CategoryDef {
        id: "spawn",
        name: "生成实体",
        summary: "运行时创建纯视觉实体、物理球 / 盒子，或实例化预制体。",
    },
    CategoryDef {
        id: "physics",
        name: "物理",
        summary: "刚体查询、射线检测、力 / 冲量 / 速度 / 重力。",
    },
    CategoryDef {
        id: "character",
        name: "角色控制器",
        summary: "驱动 CharacterController：移动、跳跃、速度与着地状态。",
    },
    CategoryDef {
        id: "input",
        name: "输入",
        summary: "查询 input.ron 里的动作 / 轴，或直接查物理键位与鼠标。",
    },
    CategoryDef {
        id: "time",
        name: "时间与日志",
        summary: "帧间隔、游戏时间、时间缩放，以及 Console 日志。",
    },
    CategoryDef {
        id: "state",
        name: "每实体状态",
        summary: "按 (实体, 脚本) 保存持久键值；跨帧 / 跨热重载保留。",
    },
    CategoryDef {
        id: "math",
        name: "数学",
        summary: "Lua 表形式的向量构造 / 运算与标量工具（不接 ctx）。",
    },
    CategoryDef {
        id: "animation",
        name: "动画",
        summary: "基础层剪辑 / 状态机参数 / 叠加层控制（只作用于调用者自己）。",
    },
    CategoryDef {
        id: "animation_layers",
        name: "混合层栈",
        summary: "按层名控制动画混合层栈的权重与剪辑。",
    },
    CategoryDef {
        id: "audio",
        name: "音频",
        summary: "播放 / 停止音效，设置听者与主音量。",
    },
    CategoryDef {
        id: "material",
        name: "材质",
        summary: "共享 .mat 资产引用与逐实体内联覆盖。",
    },
    CategoryDef {
        id: "particle",
        name: "粒子",
        summary: "开关发射、瞬时爆发与速率调整。",
    },
    CategoryDef {
        id: "ui",
        name: "游戏 UI",
        summary: "按实体 id 操作屏幕空间 UI（文本 / 颜色 / 布局 / 交互）。",
    },
    CategoryDef {
        id: "camera",
        name: "相机与朝向",
        summary: "朝向、主相机选择与 FOV。",
    },
    CategoryDef {
        id: "scene",
        name: "场景切换",
        summary: "读取当前场景、请求切换、按发布清单索引切关。",
    },
];

/// 一条函数定义。`params` 不单独存，构造时从签名解析，避免两处漂移。
struct ApiDef {
    category: &'static str,
    name: &'static str,
    signature: &'static str,
    returns: &'static str,
    summary: &'static str,
}

const fn api(
    category: &'static str,
    name: &'static str,
    signature: &'static str,
    returns: &'static str,
    summary: &'static str,
) -> ApiDef {
    ApiDef {
        category,
        name,
        signature,
        returns,
        summary,
    }
}

/// 全部 160 个注册名。顺序 = `Docs/脚本API.md` 的表格顺序。
const FUNCTIONS: &[ApiDef] = &[
    api(
        "transform",
        "pos",
        "pos(ctx)",
        "Vec3",
        r#"自身**世界**坐标 → `{x, y, z}` 表"#,
    ),
    api(
        "transform",
        "set_pos",
        "set_pos(ctx, x, y, z)",
        "nil",
        r#"设置**世界**坐标（有父节点时自动换算回局部）"#,
    ),
    api(
        "transform",
        "local_pos",
        "local_pos(ctx)",
        "Vec3",
        r#"自身局部坐标 → Vec3"#,
    ),
    api(
        "transform",
        "set_local_pos",
        "set_local_pos(ctx, x, y, z)",
        "nil",
        r#"设置自身局部坐标（有父节点时相对父节点）"#,
    ),
    api(
        "transform",
        "translate",
        "translate(ctx, dx, dy, dz)",
        "nil",
        r#"世界空间平移增量"#,
    ),
    api(
        "transform",
        "rot",
        "rot(ctx)",
        "Vec3",
        r#"自身**局部**欧拉角（度），顺序 Y-X-Z → `{x, y, z}` 表"#,
    ),
    api(
        "transform",
        "set_rot",
        "set_rot(ctx, x, y, z)",
        "nil",
        r#"设置局部欧拉角（度，顺序同 `rot`：Y-X-Z；与 Inspector / MCP 同一约定）"#,
    ),
    api(
        "transform",
        "rotate",
        "rotate(ctx, dx, dy, dz)",
        "nil",
        r#"叠加旋转增量（度）"#,
    ),
    api(
        "transform",
        "scale",
        "scale(ctx)",
        "Vec3",
        r#"自身局部缩放 → Vec3"#,
    ),
    api(
        "transform",
        "set_scale",
        "set_scale(ctx, x, y, z)",
        "nil",
        r#"设置自身局部缩放"#,
    ),
    api(
        "transform",
        "forward",
        "forward(ctx)",
        "Vec3",
        r#"自身世界朝向 +Z → Vec3"#,
    ),
    api(
        "transform",
        "right",
        "right(ctx)",
        "Vec3",
        r#"自身世界朝向 +X → Vec3"#,
    ),
    api(
        "transform",
        "up",
        "up(ctx)",
        "Vec3",
        r#"自身世界朝向 +Y → Vec3"#,
    ),
    api(
        "transform",
        "self_id",
        "self_id(ctx)",
        "EntityId",
        r#"调用者自己的实体 id"#,
    ),
    api(
        "transform",
        "self_name",
        "self_name(ctx)",
        "string",
        r#"调用者自己的实体名字"#,
    ),
    api(
        "entity",
        "entity_count",
        "entity_count(ctx)",
        "integer",
        r#"场景实体数"#,
    ),
    api(
        "entity",
        "entity_at",
        "entity_at(ctx, index)",
        "EntityId",
        r#"索引 → 实体 ID"#,
    ),
    api(
        "entity",
        "entity_id",
        "entity_id(ctx, name)",
        "EntityId",
        r#"名字 → 实体 ID（找不到返回 `0`）"#,
    ),
    api(
        "entity",
        "entity_name",
        "entity_name(ctx, id)",
        "string",
        r#"实体名字（找不到返回空串）"#,
    ),
    api(
        "entity",
        "entity_model",
        "entity_model(ctx, id)",
        "string",
        r#"实体模型路径（没有返回空串）"#,
    ),
    api(
        "entity",
        "set_entity_model",
        "set_entity_model(ctx, id, path)",
        "nil",
        r#"换模型（`"builtin://cube"` 或项目相对路径）"#,
    ),
    api(
        "entity",
        "entity_pos",
        "entity_pos(ctx, id)",
        "Vec3",
        r#"任意实体的世界坐标 → Vec3"#,
    ),
    api(
        "entity",
        "set_entity_pos",
        "set_entity_pos(ctx, id, x, y, z)",
        "nil",
        r#"设置任意实体的世界坐标"#,
    ),
    api(
        "entity",
        "entity_rot",
        "entity_rot(ctx, id)",
        "Vec3",
        r#"任意实体的局部欧拉角（度）→ Vec3"#,
    ),
    api(
        "entity",
        "set_entity_rot",
        "set_entity_rot(ctx, id, x, y, z)",
        "nil",
        r#"设置任意实体的局部欧拉角（度）"#,
    ),
    api(
        "entity",
        "entity_forward",
        "entity_forward(ctx, id)",
        "Vec3",
        r#"该实体世界朝向的 +Z（绕到背后 / 判断"它背没背对我"）"#,
    ),
    api(
        "entity",
        "entity_scale",
        "entity_scale(ctx, id)",
        "Vec3",
        r#"任意实体的局部缩放 → Vec3"#,
    ),
    api(
        "entity",
        "set_entity_scale",
        "set_entity_scale(ctx, id, x, y, z)",
        "nil",
        r#"设置任意实体的局部缩放"#,
    ),
    api(
        "entity",
        "distance_to",
        "distance_to(ctx, id)",
        "number",
        r#"与目标的距离"#,
    ),
    api(
        "entity",
        "set_parent",
        "set_parent(ctx, child, parent)",
        "nil",
        r#"重挂父节点（`parent = 0` 表示回到根）"#,
    ),
    api(
        "entity",
        "destroy",
        "destroy(ctx, id)",
        "nil",
        r#"销毁实体**及其整棵子树**"#,
    ),
    api(
        "spawn",
        "spawn_entity",
        "spawn_entity(ctx, name, model)",
        "EntityId",
        r#"生成一个纯视觉实体，返回它的 ID"#,
    ),
    api(
        "spawn",
        "spawn_entity_at",
        "spawn_entity_at(ctx, name, model, x, y, z)",
        "EntityId",
        r#"同上，指定位置"#,
    ),
    api(
        "spawn",
        "spawn_ball",
        "spawn_ball(ctx, name, radius, x, y, z)",
        "EntityId",
        r#"生成一个动态球（带刚体与球碰撞体）"#,
    ),
    api(
        "spawn",
        "spawn_static_box",
        "spawn_static_box(ctx, name, hx, hy, hz, x, y, z)",
        "EntityId",
        r#"生成一个静态盒子"#,
    ),
    api(
        "spawn",
        "spawn_prefab",
        "spawn_prefab(ctx, path, x, y, z)",
        "EntityId | nil",
        r#"**实例化一个预制体资产**，返回实例根 ID（失败返回 `nil`）"#,
    ),
    api(
        "spawn",
        "spawn_prefab_under",
        "spawn_prefab_under(ctx, path, parent, x, y, z)",
        "EntityId | nil",
        r#"同上，但把实例根挂到 `parent` 底下，坐标为**局部**"#,
    ),
    api(
        "physics",
        "has_physics",
        "has_physics(ctx, id)",
        "boolean",
        r#"这个实体有没有刚体"#,
    ),
    api(
        "physics",
        "raycast",
        "raycast(ctx, ox, oy, oz, dx, dy, dz, max_dist)",
        "RayHit",
        r#"射线检测，返回一个表"#,
    ),
    api(
        "physics",
        "raycast_masked",
        "raycast_masked(ctx, ox, oy, oz, dx, dy, dz, max_dist, layer_mask)",
        "RayHit",
        r#"只命中 `layer_mask` 里那些层的射线（第 N 位 = 第 N 层；不看碰撞矩阵）"#,
    ),
    api(
        "physics",
        "set_layer",
        "set_layer(ctx, id, layer)",
        "boolean",
        r#"把实体换到某一层（0–31，越界取模）；碰撞组由运行时按项目矩阵立刻改写"#,
    ),
    api(
        "physics",
        "layer",
        "layer(ctx, id)",
        "integer | nil",
        r#"实体所在的层；实体不存在返回 nil"#,
    ),
    api(
        "physics",
        "apply_force",
        "apply_force(ctx, id, x, y, z)",
        "nil",
        r#"施力（持续，下一帧清零）"#,
    ),
    api(
        "physics",
        "apply_impulse",
        "apply_impulse(ctx, id, x, y, z)",
        "nil",
        r#"施冲量（瞬间改速度）"#,
    ),
    api(
        "physics",
        "add_torque",
        "add_torque(ctx, id, x, y, z)",
        "nil",
        r#"施加力矩"#,
    ),
    api(
        "physics",
        "velocity",
        "velocity(ctx, id)",
        "Vec3",
        r#"刚体线速度 → Vec3"#,
    ),
    api(
        "physics",
        "set_velocity",
        "set_velocity(ctx, id, x, y, z)",
        "nil",
        r#"设置刚体线速度"#,
    ),
    api(
        "physics",
        "gravity",
        "gravity(ctx)",
        "Vec3",
        r#"全局重力 → Vec3"#,
    ),
    api(
        "physics",
        "set_gravity",
        "set_gravity(ctx, x, y, z)",
        "nil",
        r#"设置全局重力"#,
    ),
    api(
        "character",
        "character_simple_move",
        "character_simple_move(ctx, vx, vy, vz)",
        "nil",
        r#"给水平速度（米/秒），重力 / 坡面 / 陡坡滑落由引擎处理"#,
    ),
    api(
        "character",
        "character_move",
        "character_move(ctx, dx, dy, dz)",
        "nil",
        r#"按位移移动（米），不施加重力"#,
    ),
    api(
        "character",
        "character_jump",
        "character_jump(ctx, speed)",
        "boolean",
        r#"着地时起跳（米/秒），返回是否成功"#,
    ),
    api(
        "character",
        "character_velocity",
        "character_velocity(ctx)",
        "Vec3",
        r#"角色实际速度 → Vec3（撞墙后水平分量归零）"#,
    ),
    api(
        "character",
        "character_set_velocity",
        "character_set_velocity(ctx, x, y, z)",
        "nil",
        r#"直接设置角色速度"#,
    ),
    api(
        "character",
        "character_is_grounded",
        "character_is_grounded(ctx)",
        "boolean",
        r#"是否着地 → boolean"#,
    ),
    api(
        "character",
        "character_ground_normal",
        "character_ground_normal(ctx)",
        "Vec3",
        r#"脚下法线 → Vec3（腾空时 {0,1,0}）"#,
    ),
    api(
        "character",
        "character_is_sliding",
        "character_is_sliding(ctx)",
        "boolean",
        r#"是否正在陡坡上自动下滑 → boolean"#,
    ),
    api(
        "input",
        "action_down",
        "action_down(ctx, action)",
        "boolean",
        r#"动作是否按住"#,
    ),
    api(
        "input",
        "action_pressed",
        "action_pressed(ctx, action)",
        "boolean",
        r#"动作是否**本帧刚按下**"#,
    ),
    api(
        "input",
        "action_released",
        "action_released(ctx, action)",
        "boolean",
        r#"动作是否**本帧刚松开**"#,
    ),
    api(
        "input",
        "axis",
        "axis(ctx, axis)",
        "number",
        r#"轴的值（-1 … 1）"#,
    ),
    api(
        "input",
        "key_down",
        "key_down(ctx, key)",
        "boolean",
        r#"直接按物理键位查询"#,
    ),
    api(
        "input",
        "key_pressed",
        "key_pressed(ctx, key)",
        "boolean",
        r#"直接按物理键位查询"#,
    ),
    api(
        "input",
        "mouse_down",
        "mouse_down(ctx, button)",
        "boolean",
        r#"鼠标按键"#,
    ),
    api(
        "input",
        "mouse_pressed",
        "mouse_pressed(ctx, button)",
        "boolean",
        r#"鼠标按键"#,
    ),
    api(
        "input",
        "mouse_pos",
        "mouse_pos(ctx)",
        "Vec2",
        r#"指针位置 → Vec2"#,
    ),
    api(
        "input",
        "mouse_delta",
        "mouse_delta(ctx)",
        "Vec2",
        r#"本帧指针位移 → Vec2"#,
    ),
    api(
        "input",
        "scroll",
        "scroll(ctx)",
        "Vec2",
        r#"本帧滚轮增量 → Vec2"#,
    ),
    api(
        "time",
        "delta",
        "delta(ctx)",
        "number",
        r#"到上一帧的时间（秒，**已按时间缩放**；暂停时恒为 0）"#,
    ),
    api(
        "time",
        "time",
        "time(ctx)",
        "number",
        r#"游戏已运行的总时长（秒，同样已缩放；暂停时不增长）"#,
    ),
    api(
        "time",
        "time_scale",
        "time_scale(ctx)",
        "number",
        r#"当前时间缩放系数（默认 `1.0`）"#,
    ),
    api(
        "time",
        "set_time_scale",
        "set_time_scale(ctx, scale)",
        "nil",
        r#"设置时间缩放；`0` = 暂停模拟，`0.3` = 慢动作，`2.0` = 二倍速"#,
    ),
    api(
        "time",
        "log",
        "log(ctx, msg)",
        "nil",
        r#"写一条 info 日志到编辑器 Console"#,
    ),
    api(
        "time",
        "warn",
        "warn(ctx, msg)",
        "nil",
        r#"写一条 warning 日志到编辑器 Console"#,
    ),
    api(
        "time",
        "error",
        "error(ctx, msg)",
        "nil",
        r#"写一条 error 日志到编辑器 Console"#,
    ),
    api(
        "state",
        "state_get",
        "state_get(ctx, key)",
        "any | nil",
        r#"取值；没设过返回 `nil`（用 `== nil` 判空）"#,
    ),
    api(
        "state",
        "state_get_or",
        "state_get_or(ctx, key, default)",
        "any",
        r#"取值，缺省时返回 `default`"#,
    ),
    api(
        "state",
        "state_set",
        "state_set(ctx, key, value)",
        "nil",
        r#"写入"#,
    ),
    api(
        "state",
        "state_has",
        "state_has(ctx, key)",
        "boolean",
        r#"键是否存在 → boolean"#,
    ),
    api(
        "state",
        "state_remove",
        "state_remove(ctx, key)",
        "nil",
        r#"删除一个键（不存在也不报错）"#,
    ),
    api(
        "state",
        "state_clear",
        "state_clear(ctx)",
        "nil",
        r#"清空当前 (实体, 脚本) 的全部状态"#,
    ),
    api(
        "math",
        "vec3",
        "vec3(x, y, z)",
        "Vec3",
        r#"构造向量（Lua 表 `{x=, y=, z=}`）"#,
    ),
    api(
        "math",
        "vec2",
        "vec2(x, y)",
        "Vec2",
        r#"构造 Vec2（Lua 表 `{x=, y=}`）"#,
    ),
    api("math", "vlength", "vlength(v)", "number", r#"向量长度"#),
    api(
        "math",
        "vlength_squared",
        "vlength_squared(v)",
        "number",
        r#"向量长度平方（省一次开方）"#,
    ),
    api(
        "math",
        "vnormalized",
        "vnormalized(v)",
        "Vec3",
        r#"归一化向量（零向量返回零向量）"#,
    ),
    api("math", "vdot", "vdot(a, b)", "number", r#"点积"#),
    api("math", "vcross", "vcross(a, b)", "Vec3", r#"叉积 → Vec3"#),
    api(
        "math",
        "vdistance_to",
        "vdistance_to(a, b)",
        "number",
        r#"两点距离"#,
    ),
    api("math", "vadd", "vadd(a, b)", "Vec3", r#"向量相加 → Vec3"#),
    api("math", "vsub", "vsub(a, b)", "Vec3", r#"向量相减 → Vec3"#),
    api("math", "vmul", "vmul(v, s)", "Vec3", r#"向量乘标量 → Vec3"#),
    api(
        "math",
        "vlerp",
        "vlerp(a, b, t)",
        "Vec3",
        r#"向量插值（t 钳在 0..1）→ Vec3"#,
    ),
    api(
        "math",
        "vtostring",
        "vtostring(v)",
        "string",
        r#"格式化成 (x, y, z) 字符串（调试用）"#,
    ),
    api(
        "math",
        "clamp",
        "clamp(v, lo, hi)",
        "number",
        r#"把 v 钳在 lo..hi"#,
    ),
    api(
        "math",
        "lerpf",
        "lerpf(a, b, t)",
        "number",
        r#"标量线性插值"#,
    ),
    api(
        "math",
        "deg_to_rad",
        "deg_to_rad(d)",
        "number",
        r#"角度转弧度"#,
    ),
    api(
        "math",
        "rad_to_deg",
        "rad_to_deg(r)",
        "number",
        r#"弧度转角度"#,
    ),
    api(
        "animation",
        "set_anim_clip",
        "set_anim_clip(ctx, clip)",
        "nil",
        r#"直接切**基础层**剪辑（走与 Inspector / RON 相同的交叉淡入路径）"#,
    ),
    api(
        "animation",
        "set_anim_speed",
        "set_anim_speed(ctx, speed)",
        "nil",
        r#"基础层倍率（钳在 0~100；0 = 冻结；状态机模式下由状态表覆盖）"#,
    ),
    api(
        "animation",
        "set_anim_looping",
        "set_anim_looping(ctx, looping)",
        "nil",
        r#"基础层是否循环（状态机模式下由状态表覆盖）"#,
    ),
    api(
        "animation",
        "set_anim_fade_time",
        "set_anim_fade_time(ctx, seconds)",
        "nil",
        r#"换剪辑 / 换状态的淡入时长（钳在 0~10；0 = 硬切）"#,
    ),
    api(
        "animation",
        "set_anim_param",
        "set_anim_param(ctx, name, value)",
        "nil",
        r#"写**自己实体**的动画状态机参数（键不存在就创建）"#,
    ),
    api(
        "animation",
        "set_anim_additive_clip",
        "set_anim_additive_clip(ctx, clip)",
        "nil",
        r#"切叠加层剪辑（硬切；没配叠加层 = no-op）"#,
    ),
    api(
        "animation",
        "set_anim_additive_weight",
        "set_anim_additive_weight(ctx, value)",
        "nil",
        r#"写**自己实体**叠加层的权重（自动钳在 0~1）"#,
    ),
    api(
        "animation_layers",
        "set_anim_layer_weight",
        "set_anim_layer_weight(ctx, name, value)",
        "nil",
        r#"写**自己实体**层栈中按层名匹配的层的权重（自动钳在 0~1）"#,
    ),
    api(
        "animation_layers",
        "set_anim_layer_clip",
        "set_anim_layer_clip(ctx, name, clip)",
        "nil",
        r#"按层名切该层剪辑（硬切；层名不存在 = no-op）"#,
    ),
    api(
        "animation",
        "morph",
        "morph(ctx, id, target)",
        "number",
        r#"读实体第 `target` 个形变目标（morph target / blend shape）的权重，没有则 0"#,
    ),
    api(
        "animation",
        "set_morph",
        "set_morph(ctx, id, target, value)",
        "boolean",
        r#"写形变目标权重（钳在 0~1，允许稀疏写；剪辑带权重轨道时被动画覆盖）"#,
    ),
    api(
        "audio",
        "play_sound",
        "play_sound(ctx, clip, volume, pitch)",
        "nil",
        r#"播一次**非空间化**音效（UI 音、提示音）"#,
    ),
    api(
        "audio",
        "play_sound_at",
        "play_sound_at(ctx, clip, x, y, z, volume)",
        "nil",
        r#"播一次**空间化**音效（3D 定位，随距离衰减）"#,
    ),
    api(
        "audio",
        "stop_sound",
        "stop_sound(ctx, clip)",
        "nil",
        r#"停止某个 clip 的**全部**在播实例"#,
    ),
    api(
        "audio",
        "stop_all_sounds",
        "stop_all_sounds(ctx)",
        "nil",
        r#"停止所有声音（脚本播的 + 组件 autoplay 的）"#,
    ),
    api(
        "audio",
        "set_listener",
        "set_listener(ctx, x, y, z, fx, fy, fz)",
        "nil",
        r#"设置听者位置与朝向（`fx/fy/fz` 是朝前向量）"#,
    ),
    api(
        "audio",
        "set_master_volume",
        "set_master_volume(ctx, v)",
        "nil",
        r#"主音量（`0.0` 静音，`1.0` 原音量）"#,
    ),
    api(
        "material",
        "material_asset",
        "material_asset(ctx, id)",
        "string",
        r#"读实体引用的材质资产路径（没有 → 空串）"#,
    ),
    api(
        "material",
        "set_material_asset",
        "set_material_asset(ctx, id, path)",
        "nil",
        r#"指向一份 `.mat`（项目相对路径）；传 `""` 恢复模型自带材质"#,
    ),
    api(
        "material",
        "set_material",
        "set_material(ctx, id, r, g, b, a, metallic, roughness)",
        "nil",
        r#"**一次性**写内联覆盖（颜色是线性 RGBA；金属度 / 粗糙度自动钳到 0..1）"#,
    ),
    api(
        "material",
        "clear_material",
        "clear_material(ctx, id)",
        "nil",
        r#"清掉内联覆盖（不影响共享资产）"#,
    ),
    api(
        "particle",
        "particle_emitting",
        "particle_emitting(ctx, id)",
        "boolean",
        r#"读开关（没挂发射器 → false）"#,
    ),
    api(
        "particle",
        "set_particle_emitting",
        "set_particle_emitting(ctx, id, enabled)",
        "nil",
        r#"开 / 关持续发射"#,
    ),
    api(
        "particle",
        "particle_burst",
        "particle_burst(ctx, id, n)",
        "nil",
        r#"立刻炸出 n 颗（可累积：连按两帧 = 两次相加）"#,
    ),
    api(
        "particle",
        "set_particle_rate",
        "set_particle_rate(ctx, id, r)",
        "nil",
        r#"改发射速率（颗/秒；非有限值钳 0）"#,
    ),
    api(
        "ui",
        "ui_set_text",
        "ui_set_text(ctx, id, text)",
        "nil",
        r#"改文本"#,
    ),
    api(
        "ui",
        "ui_text",
        "ui_text(ctx, id)",
        "string",
        r#"读文本（不存在返回空串）"#,
    ),
    api(
        "ui",
        "ui_set_text_size",
        "ui_set_text_size(ctx, id, size)",
        "nil",
        r#"字号（像素）"#,
    ),
    api(
        "ui",
        "ui_set_text_color",
        "ui_set_text_color(ctx, id, r, g, b, a)",
        "nil",
        r#"文字颜色"#,
    ),
    api(
        "ui",
        "ui_set_text_align",
        "ui_set_text_align(ctx, id, align)",
        "nil",
        r#"水平对齐；align 取 "left" / "center" / "right""#,
    ),
    api(
        "ui",
        "ui_set_bg_color",
        "ui_set_bg_color(ctx, id, r, g, b, a)",
        "nil",
        r#"背景色"#,
    ),
    api(
        "ui",
        "ui_set_radius",
        "ui_set_radius(ctx, id, radius)",
        "nil",
        r#"圆角半径"#,
    ),
    api(
        "ui",
        "ui_set_visible",
        "ui_set_visible(ctx, id, visible)",
        "nil",
        r#"设置显隐（连带整棵子树）"#,
    ),
    api(
        "ui",
        "ui_visible",
        "ui_visible(ctx, id)",
        "boolean",
        r#"是否可见（连带整棵子树）→ boolean"#,
    ),
    api(
        "ui",
        "ui_set_enabled",
        "ui_set_enabled(ctx, id, enabled)",
        "nil",
        r#"设置是否接受点击（按钮禁用态半透明）"#,
    ),
    api(
        "ui",
        "ui_enabled",
        "ui_enabled(ctx, id)",
        "boolean",
        r#"是否接受点击 → boolean"#,
    ),
    api(
        "ui",
        "ui_has",
        "ui_has(ctx, id)",
        "boolean",
        r#"这个实体是不是 UI 元素"#,
    ),
    api(
        "ui",
        "ui_clicked",
        "ui_clicked(ctx, id)",
        "boolean",
        r#"**本帧**完成了一次"在元素上按下并在元素上松开""#,
    ),
    api(
        "ui",
        "ui_hovered",
        "ui_hovered(ctx, id)",
        "boolean",
        r#"本帧指针停在元素上"#,
    ),
    api(
        "ui",
        "ui_set_rect",
        "ui_set_rect(ctx, id, ax0, ay0, ax1, ay1, ox0, oy0, ox1, oy1)",
        "nil",
        r#"一次设定锚点 + 偏移"#,
    ),
    api(
        "ui",
        "ui_set_anchor",
        "ui_set_anchor(ctx, id, ax0, ay0, ax1, ay1)",
        "nil",
        r#"只改锚点"#,
    ),
    api(
        "ui",
        "ui_set_offset",
        "ui_set_offset(ctx, id, ox0, oy0, ox1, oy1)",
        "nil",
        r#"只改偏移"#,
    ),
    api(
        "ui",
        "ui_set_size",
        "ui_set_size(ctx, id, w, h)",
        "nil",
        r#"改尺寸（左上角不动）"#,
    ),
    api(
        "ui",
        "ui_set_pos",
        "ui_set_pos(ctx, id, x, y)",
        "nil",
        r#"改位置（相对父矩形左上角）"#,
    ),
    api(
        "ui",
        "ui_set_image",
        "ui_set_image(ctx, id, path)",
        "nil",
        r#"换图片"#,
    ),
    api(
        "ui",
        "ui_set_image_tint",
        "ui_set_image_tint(ctx, id, r, g, b, a)",
        "nil",
        r#"图片着色"#,
    ),
    api(
        "ui",
        "ui_set_fill",
        "ui_set_fill(ctx, id, t)",
        "nil",
        r#"填充比例 `0..1`（血条：从左往右裁剪）"#,
    ),
    api(
        "ui",
        "ui_spawn_label",
        "ui_spawn_label(ctx, text, x, y, size)",
        "EntityId",
        r#"生成文本，返回 id（父 = 屏幕根）"#,
    ),
    api(
        "ui",
        "ui_spawn_button",
        "ui_spawn_button(ctx, text, x, y, w, h)",
        "EntityId",
        r#"生成按钮，返回 id"#,
    ),
    api(
        "ui",
        "ui_spawn_panel",
        "ui_spawn_panel(ctx, x, y, w, h)",
        "EntityId",
        r#"生成面板，返回 id"#,
    ),
    api(
        "ui",
        "ui_spawn_image",
        "ui_spawn_image(ctx, path, x, y, w, h)",
        "EntityId",
        r#"生成图片元素，返回 id"#,
    ),
    api(
        "camera",
        "look_at",
        "look_at(ctx, x, y, z)",
        "nil",
        r#"让调用者的 +Z 指向世界坐标点（只改朝向）"#,
    ),
    api(
        "camera",
        "look_at_entity",
        "look_at_entity(ctx, id)",
        "nil",
        r#"让调用者看向另一个实体（目标不存在则不动）"#,
    ),
    api(
        "camera",
        "active_camera",
        "active_camera(ctx)",
        "EntityId",
        r#"当前主相机实体 id（0 = 没有）"#,
    ),
    api(
        "camera",
        "set_active_camera",
        "set_active_camera(ctx, id)",
        "nil",
        r#"把 id 设为唯一主相机（其它相机全部关掉）"#,
    ),
    api(
        "camera",
        "camera_fov",
        "camera_fov(ctx, id)",
        "number | nil",
        r#"垂直视场角（度）；不是相机 → nil"#,
    ),
    api(
        "camera",
        "set_camera_fov",
        "set_camera_fov(ctx, id, deg)",
        "nil",
        r#"设置视场角（钳在 1..179）"#,
    ),
    api(
        "scene",
        "current_scene",
        "current_scene(ctx)",
        "string",
        r#"当前场景的项目相对路径（空串 = 宿主没告诉运行时）"#,
    ),
    api(
        "scene",
        "load_scene",
        "load_scene(ctx, path)",
        "nil",
        r#"请求切换到 path（项目相对路径），本帧末尾生效"#,
    ),
    api(
        "scene",
        "scene_count",
        "scene_count(ctx)",
        "integer",
        r#"发布清单里的关卡数（0 = 没配清单）"#,
    ),
    api(
        "scene",
        "scene_index",
        "scene_index(ctx)",
        "integer | nil",
        r#"当前场景在清单里的下标（0 起；nil = 没配清单 / 不在清单里）"#,
    ),
    api(
        "scene",
        "load_scene_at",
        "load_scene_at(ctx, i)",
        "nil",
        r#"切到清单里的第 i 关（0 起）；越界记 error、场景不动"#,
    ),
];

/// 从 `name(ctx, x, y)` 里取出 `["ctx", "x", "y"]`。
fn params_of(signature: &str) -> Vec<String> {
    let Some((_, rest)) = signature.split_once('(') else {
        return Vec::new();
    };
    let Some(inner) = rest.strip_suffix(')') else {
        return Vec::new();
    };
    if inner.trim().is_empty() {
        Vec::new()
    } else {
        inner
            .split(',')
            .map(str::trim)
            .map(str::to_string)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn catalog_has_the_expected_shape() {
        let doc = script_api();
        assert_eq!(doc.format_version, SCRIPT_API_FORMAT_VERSION);
        assert_eq!(doc.categories.len(), 17);
        assert_eq!(doc.function_count(), 160);
        assert_eq!(doc.hooks.len(), 6);
        assert_eq!(doc.value_types.len(), 7);
        assert!(!doc.conventions.is_empty());
        assert_eq!(doc.engine_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn names_are_unique_and_signatures_match() {
        let doc = script_api();
        let mut names = BTreeSet::new();
        for function in doc
            .categories
            .iter()
            .flat_map(|category| &category.functions)
        {
            assert!(
                names.insert(function.name.clone()),
                "重复函数名: {}",
                function.name
            );
            assert!(
                function
                    .signature
                    .starts_with(&format!("{}(", function.name)),
                "签名与函数名不一致: {}",
                function.signature
            );
            assert!(
                function.signature.ends_with(')'),
                "签名缺右括号: {}",
                function.signature
            );
            assert!(!function.returns.is_empty(), "{} 缺返回值", function.name);
            assert!(!function.summary.is_empty(), "{} 缺说明", function.name);
            assert_eq!(
                params_of(&function.signature),
                function.params,
                "{} 的参数解析不一致",
                function.name
            );
            assert!(!function.params.is_empty(), "{} 没有参数", function.name);
        }
        assert_eq!(names.len(), 160);
    }

    #[test]
    fn every_function_belongs_to_a_declared_category() {
        let doc = script_api();
        let ids: BTreeSet<&str> = doc
            .categories
            .iter()
            .map(|category| category.id.as_str())
            .collect();
        for category in &doc.categories {
            assert!(
                !category.functions.is_empty(),
                "类别 {} 没有函数",
                category.id
            );
            assert!(ids.contains(category.id.as_str()));
            assert!(!category.name.is_empty());
            assert!(!category.summary.is_empty());
        }
        // 所有 FUNCTIONS 都必须被某个类别消费；漏写 category 会在这里掉数量。
        assert_eq!(doc.function_count(), FUNCTIONS.len());
    }

    #[test]
    fn lookups_work() {
        let doc = script_api();
        let set_pos = doc.function("set_pos").expect("set_pos 应在清单里");
        assert_eq!(set_pos.signature, "set_pos(ctx, x, y, z)");
        assert_eq!(set_pos.params.join(", "), "ctx, x, y, z");
        assert_eq!(set_pos.returns, "nil");
        assert!(doc.function("no_such_function").is_none());
        assert!(doc.function_names().any(|name| name == "spawn_prefab"));
    }

    #[test]
    fn hooks_are_the_six_lifecycle_entry_points() {
        let doc = script_api();
        let names: Vec<&str> = doc.hooks.iter().map(|hook| hook.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "start",
                "update",
                "fixed_update",
                "on_collision_enter",
                "on_collision_exit",
                "on_destroy",
            ]
        );
    }

    #[test]
    fn value_types_cover_the_shapes_used_by_returns() {
        let doc = script_api();
        let names: BTreeSet<&str> = doc
            .value_types
            .iter()
            .map(|value_type| value_type.name.as_str())
            .collect();
        for expected in [
            "ctx",
            "Vec3",
            "Vec2",
            "RayHit",
            "EntityId",
            "AssetPath",
            "LuaValue",
        ] {
            assert!(names.contains(expected), "缺值类型 {expected}");
        }
        // 返回值里出现的自定义类型必须能查到；基元 / nil / any 不要求。
        for function in doc
            .categories
            .iter()
            .flat_map(|category| &category.functions)
        {
            for ty in function.returns.split('|').map(str::trim) {
                if matches!(
                    ty,
                    "nil" | "number" | "integer" | "boolean" | "string" | "any"
                ) {
                    continue;
                }
                assert!(names.contains(ty), "{} 返回未知类型 {ty}", function.name);
            }
        }
    }
}
