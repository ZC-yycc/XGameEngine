//! Inspector 的**反射通道**：按 `TypeMeta` 画字段行，不写死任何具体组件。
//!
//! # 职责
//!
//! 所有组件段落都登记在本文件的 spec 表里，由同一个入口画分类条、处理菜单并
//! 收集编辑信号。字段型段落的控件形态由 [`ScalarKind`] / [`TypeRef`] 决定，
//! 范围 / 步长 / 小数位 / 说明文字 / 条件显示 / 同排分组由 [`FieldMeta`] 决定；
//! 版式本身承载信息的段落则显式选择专用渲染器。
//!
//! # 四种字段形状
//!
//! | 形状 | 渲染 | 例子 |
//! |---|---|---|
//! | 标量 | 一个控件 | `fov` / `looping` / `clip` |
//! | 无载荷枚举 | 下拉框 + 当前变体的一行说明 | `BodyKind` / `AudioBus` |
//! | 带载荷枚举 | 下拉框 + 载荷字段（当结构体画） | `CollisionShape` |
//! | 列表 | 每项一组行 + 删除 + 追加 | `LodGroup::levels` |
//!
//! 载荷与列表都没有另写一套控件分派：载荷被包成 [`EnumPayload`] 这个"借用适配器"
//! 当成结构体喂回 [`show_fields`]，列表的每一项同理。所以 `[f32; 3]` 这种字段
//! 无论是挂在 `PhysicsBody` 上还是挂在 `CollisionShape::Cuboid` 里，画出来一模一样。
//!
//! # 写入路径
//!
//! 控件作用在**局部副本**上，改动了才写回组件。原因不是性能，是借用：
//! `ValueRefMut` 同时只能存在一个，而"同排的两个数值框"（`min` / `max` 距离）
//! 天然要同时拿两个 `&mut`。先读后写还有第二个好处 —— 写回只发生在真的有改动
//! 的字段上，不会把没动过的字段重写一遍（那样会让脏检查误报）。

use xgame_core::reflect::{
    FieldMeta, MetaKind, Reflect, ReflectEnum, ScalarKind, ScalarValue, TypeMeta, TypeRef,
    ValueRef, ValueRefMut, Widget,
};
use xgame_core::{BuiltinComponent, ComponentAddress, Entity};

use super::inspector::{
    component_context_menu, row, ComponentAction, ComponentMenuTarget, FieldSignals,
    PLAY_STRUCTURE_HINT,
};
use super::{icons, theme};

/// 给响应挂上字段说明（`doc`），空的就不挂。
///
/// 单独一个函数是因为 `Response::on_hover_text` **消费** `self`：写成
/// `response.on_hover_text(...)` 而不重新绑定，后面就用不了 `response` 了。
fn with_doc(response: egui::Response, doc: &str) -> egui::Response {
    if doc.is_empty() {
        response
    } else {
        response.on_hover_text(doc)
    }
}

/// 一个内置组件在反射通道里需要的**全部**非通用信息。
///
/// 字段怎么画、画哪些，一概不在这里 —— 那是元数据的事。这里只有四类：
/// 文案、怎么拿到 `&mut dyn Reflect`、以及三处**用元数据表达不了**的派生逻辑。
///
/// 字段型组件只需一条声明式 spec，不含控件代码；需要专用版式的组件则在
/// `render` 里明确选择 [`Render::Custom`]。
pub struct ReflectComponentSpec {
    /// 分类条标题（`"Audio"` 与 `BuiltinComponent::name()` 的 `"Audio Source"`
    /// 历史上就不一致，这里以 Inspector 上的文案为准）。
    pub title: &'static str,
    /// `ui.indent` 的 id。它只影响 egui 记住的滚动/展开状态，不影响布局。
    pub indent: &'static str,
    /// "勾上才有"那个复选框的 `(文字, 悬浮提示)`。
    ///
    /// `None` = 这一段没有开关（`LOD` 的存不存在由 `levels` 是否为空决定，
    /// 画一个永远勾着的框只是噪声）。
    pub toggle: Option<(&'static str, &'static str)>,
    /// 分类条右键菜单。三选一，见 [`SectionMenu`]。
    pub menu: SectionMenu,
    /// 取组件的可写反射视图；组件不存在时返回 `None`。
    pub access: fn(&mut Entity) -> Option<&mut dyn Reflect>,
    /// 编辑后修复**跨字段**不变式（`far > near`）。
    ///
    /// 为什么放在这里而不是元数据里：这些规则是引擎语义（一个 `far <= near`
    /// 的相机整屏全黑），不是显示偏好。用元数据表达只能造一门 DSL
    /// （`max_from = "near" + 0.001`），而规则本身一共只有几条。
    ///
    /// 注意它**在任意字段改动后**都会跑，不只在那两个相关字段上 —— 于是
    /// "改 Volume 顺手把坏的 max ≤ min 修好"这种情况也会发生。这是有意的：
    /// 不变式成立比"只动我碰的那一格"更重要。
    pub normalize: Option<fn(&mut Entity)>,
    /// 枚举**切换变体**之后的修正（`Auto → Box` 时要按实体缩放给半边长一个初值）。
    ///
    /// 元数据给不出这个值：它要读**实体**的变换，而变体声明在 core 的纯数据类型上，
    /// 不知道实体存在。参数是 `(实体, 变体字段名, 新变体名)`。
    pub on_variant: Option<fn(&mut Entity, &str, &str)>,
    /// 段落末尾的**派生量**说明（`"胶囊中心 = 脚底 + 0.90 m"`）。
    ///
    /// 它是算出来的、不落盘、也不该被编辑，所以既不是字段也不是说明文字。
    pub footer: Option<fn(&Entity) -> String>,
    /// 一组"把若干字段填成某个值"的预设按钮（`Fire` / `Smoke` / `Sparks`）。
    ///
    /// 它做不到元数据里，因为**它不是字段**：点一下要改一整片字段，而且改哪些、
    /// 改成什么只有引擎自己知道（`ParticleEmitter::fire()` 里那十几个值）。
    /// 这与锚点预设是同一类东西 —— "操作"而不是"数据"。
    pub presets: &'static [Preset],
    /// 这一段怎么画。
    pub render: Render,
}

/// 一段组件内容的画法。
///
/// 这是"统一"的边界，也是它唯一诚实的表述：**入口统一，画法可以不同**。
/// 与 UE 的 `Details` 面板 + `IDetailCustomization` 是同一个安排 ——
/// 反射负责"字段列表"这种形状（十个内置组件里有七个是），而版式本身承载
/// 信息的段落（UI 的锚点预设、九宫格切片）交回专用渲染器。
///
/// 关键是两者**登记在同一张表里、走同一个入口、用同一种信号**。这样
/// "这个组件的 Inspector 在哪"永远只有一个答案。
pub enum Render {
    /// 按 `TypeMeta` 逐字段画。
    Fields,
    /// 专用渲染：版式本身承载信息的段落（见上）。
    Custom(fn(&mut egui::Ui, &mut Entity, &mut SectionCx<'_>)),
}

/// 专用渲染器能用的一切（除了 `ui` 与实体本身）。
///
/// 组件结构动作统一从 [`SectionCx::request`] 走，编辑与拖拽信号统一进入
/// [`SectionCx::sig`]，与 `Fields` 那条路完全相同。
pub struct SectionCx<'a> {
    pub state: &'a mut crate::state::EditorState,
    pub sig: &'a mut FieldSignals,
    pub request: &'a mut Option<ComponentAction>,
    pub refresh_scripts: &'a mut bool,
}

/// 分类条右键菜单的三种形态。
///
/// 三者是**语义**差别，不是样式差别：能不能把整段当成一个组件实例来移除。
pub enum SectionMenu {
    /// 没有右键菜单（`Prefab`：这一段既不实例也不分组）。
    None,
    /// 标题正好对应一个可移除的组件实例（绝大多数）。
    Removable {
        address: ComponentAddress,
        label: &'static str,
    },
    /// 固有组件：真实存在，但不能删（`Transform`）。
    Locked {
        label: &'static str,
        reason: &'static str,
    },
    /// 实例集合：分组标题不能冒充成一个组件（`Scripts` / `Behaviors`）。
    Collection {
        label: &'static str,
        reason: &'static str,
    },
}

/// 一个预设按钮。
pub struct Preset {
    pub label: &'static str,
    pub tooltip: &'static str,
    /// 应用。拿整个实体是因为预设要动的是组件本身（`*p = …`），
    /// 而不是某个字段的某个值。
    pub apply: fn(&mut Entity),
}

/// Camera 的 spec。
pub static CAMERA: ReflectComponentSpec = ReflectComponentSpec {
    title: "Camera",
    indent: "camera",
    toggle: Some((
        "Has camera",
        "勾上之后这个实体就是一台相机：\nPlay 态视口 / 独立游戏 / 无头截图都从它取机位",
    )),
    menu: SectionMenu::Removable {
        address: ComponentAddress::Builtin(BuiltinComponent::Camera),
        label: "Remove Camera",
    },
    access: |entity| match entity.camera.as_mut() {
        Some(camera) => Some(camera),
        None => None,
    },
    normalize: Some(|entity| {
        // 复用引擎自己的钳制规则，不在面板里复制第二份边界。
        if let Some(camera) = entity.camera.as_mut() {
            *camera = camera.sanitized();
        }
    }),
    on_variant: None,
    footer: None,
    presets: &[],
    render: Render::Fields,
};

/// AudioSource 的 spec。
pub static AUDIO: ReflectComponentSpec = ReflectComponentSpec {
    title: "Audio",
    indent: "audio",
    toggle: Some((
        "Has audio source",
        "挂一个音频源：autoplay 进场景就响；\n非 autoplay 的剪辑由脚本 play_sound 触发",
    )),
    menu: SectionMenu::Removable {
        address: ComponentAddress::Builtin(BuiltinComponent::Audio),
        label: "Remove Audio Source",
    },
    access: |entity| match entity.audio.as_mut() {
        Some(audio) => Some(audio),
        None => None,
    },
    normalize: Some(|entity| {
        if let Some(audio) = entity.audio.as_mut() {
            // 引擎侧要求 max > min（否则衰减曲线退化），编辑器先把住门。
            if audio.max_distance <= audio.min_distance {
                audio.max_distance = audio.min_distance + 0.001;
            }
        }
    }),
    on_variant: None,
    footer: None,
    presets: &[],
    render: Render::Fields,
};

/// PhysicsBody 的 spec。
pub static PHYSICS: ReflectComponentSpec = ReflectComponentSpec {
    title: "Physics",
    indent: "physics",
    toggle: Some((
        "Participate in physics",
        "不勾选 = 纯视觉实体，不会被重力拽走",
    )),
    menu: SectionMenu::Removable {
        address: ComponentAddress::Builtin(BuiltinComponent::Physics),
        label: "Remove Physics",
    },
    access: |entity| match entity.physics.as_mut() {
        Some(body) => Some(body),
        None => None,
    },
    normalize: None,
    on_variant: Some(|entity, field, variant| {
        // `Auto` 是"按实体缩放推盒子"，切到 `Box` 时应该**接着**那个尺寸，
        // 而不是从 0 开始 —— 否则切完立刻得到一个零厚度的碰撞体。
        if field == "shape" && variant == "Cuboid" {
            let half =
                xgame_core::CollisionShape::auto_half_extents(entity.transform.scale.to_array());
            if let Some(body) = entity.physics.as_mut() {
                body.shape = xgame_core::CollisionShape::Cuboid {
                    half_extents: half,
                };
            }
        }
    }),
    footer: None,
    presets: &[],
    render: Render::Fields,
};

/// CharacterController 的 spec。
pub static CHARACTER: ReflectComponentSpec = ReflectComponentSpec {
    title: "Character",
    indent: "character",
    toggle: Some((
        "Character controller",
        "由角色控制器驱动：撞墙、自动上台阶、贴地、陡坡滑落",
    )),
    menu: SectionMenu::Removable {
        address: ComponentAddress::Builtin(BuiltinComponent::Character),
        label: "Remove Character",
    },
    access: |entity| match entity.controller.as_mut() {
        Some(controller) => Some(controller),
        None => None,
    },
    normalize: None,
    on_variant: None,
    footer: Some(|entity| {
        // 半高是从 `height` / `radius` 推的，改任一个都会动 —— 直接算出来给作者看，
        // 省得他拿计算器验证"我填的 1.8 到底是不是脚底在原点"。
        entity
            .controller
            .as_ref()
            .map(|c| format!("胶囊中心 = 脚底 + {:.2} m", c.center_height()))
            .unwrap_or_default()
    }),
    presets: &[],
    render: Render::Fields,
};

/// LodGroup 的 spec。
pub static LOD: ReflectComponentSpec = ReflectComponentSpec {
    title: "LOD",
    indent: "lod",
    // 没有开关：LOD 的存在与否完全由 `levels` 决定（空列表 = 没有 LOD），
    // 画一个勾选框只会让"取消勾选之后又自动勾上"变成谜。
    toggle: None,
    menu: SectionMenu::Removable {
        address: ComponentAddress::Builtin(BuiltinComponent::Lod),
        label: "Remove LOD",
    },
    access: |entity| Some(&mut entity.lod),
    normalize: Some(|entity| {
        // 新追加的级别沿用上一级的模型；只补空的，不覆盖用户填过的。
        let levels = &mut entity.lod.levels;
        for index in 1..levels.len() {
            if levels[index].model.is_empty() {
                levels[index].model = levels[index - 1].model.clone();
            }
        }
    }),
    on_variant: None,
    footer: None,
    presets: &[],
    render: Render::Fields,
};

/// 粒子发射器的预设。
///
/// 三个都是"一键把整块参数换成某个手感"，之后照常可改 —— 所以它们不是字段、
/// 也不落盘（落盘的是换完之后的那份值）。
pub static PARTICLE_PRESETS: [Preset; 3] = [
    Preset {
        label: "Fire",
        tooltip: "火焰：快、短命、加色、往上飘",
        apply: |entity| entity.particle = Some(xgame_core::ParticleEmitter::fire()),
    },
    Preset {
        label: "Smoke",
        tooltip: "烟尘：慢、长大、Alpha、继续上浮",
        apply: |entity| entity.particle = Some(xgame_core::ParticleEmitter::smoke()),
    },
    Preset {
        label: "Sparks",
        tooltip: "火花：小、快、强重力、加色",
        apply: |entity| entity.particle = Some(xgame_core::ParticleEmitter::sparks()),
    },
];

/// ParticleEmitter 的 spec。
pub static PARTICLE: ReflectComponentSpec = ReflectComponentSpec {
    title: "Particle",
    indent: "particle",
    toggle: Some((
        "Has particle emitter",
        "CPU 粒子（billboard）。进 Play 开始喷，\n场景存盘只存参数、不存粒子状态",
    )),
    menu: SectionMenu::Removable {
        address: ComponentAddress::Builtin(BuiltinComponent::Particle),
        label: "Remove Particle Emitter",
    },
    access: |entity| entity.particle.as_mut().map(|p| p as &mut dyn Reflect),
    normalize: Some(|entity| {
        // 复用引擎自己的钳制规则：`rate` / `max_particles` / `lifetime` 的上下界
        // 在加载与 Inspector 两处共用一份，手抄第二份就是漂移的来源。
        if let Some(particle) = entity.particle.as_mut() {
            particle.sanitized();
        }
    }),
    on_variant: None,
    footer: None,
    presets: &PARTICLE_PRESETS,
    render: Render::Fields,
};

/// UiElement 的 spec。
///
/// 它选择 `Render::Custom`，理由是**版式本身承载信息**：},{
/// 锚点预设（点一下改六个字段）、`Size` 派生读数、九宫格切片四连框、
/// "把后代裁到矩形内"这类开关都排在特定位置，靠元数据拼不出来 ——
/// 硬拼的结果是往 `FieldMeta` 里加六七个只有这一个组件用得上的字段。
///
/// 但它的**入口**与其余组件完全一样：同一张表、同一个 `show_section`、
/// 同一份 `FieldSignals`。所以"UI 的 Inspector 在哪"这个问题只有一个答案。
pub static UI: ReflectComponentSpec = ReflectComponentSpec {
    title: "UI",
    indent: "ui",
    // 段内那个 "UI Element" 勾选框由专用渲染器自己画（它是"组件存在与否"的开关，
    // 位置在缩进之内，而不是像其余组件那样紧跟分类条）。
    toggle: None,
    menu: SectionMenu::Removable {
        address: ComponentAddress::Builtin(BuiltinComponent::Ui),
        label: "Remove UI Element",
    },
    access: |entity| entity.ui.as_mut().map(|ui| ui as &mut dyn Reflect),
    normalize: None,
    on_variant: None,
    footer: None,
    presets: &[],
    render: Render::Custom(|ui, entity, cx| {
        if super::inspector_ui::show(ui, entity, cx.sig, cx.state.is_editing()) {
            *cx.request = Some(ComponentAction::Remove(ComponentAddress::Builtin(
                BuiltinComponent::Ui,
            )));
        }
    }),
};

/// `Transform` 段。
///
/// 走专用渲染：用户改的是**欧拉角**、存的是**四元数**，双向换算做不进字段表
/// （一个字段对应三个用户可见的输入框，而且是"改角度顺便规范化四元数"）。
pub static TRANSFORM: ReflectComponentSpec = ReflectComponentSpec {
    title: "Transform",
    indent: "transform",
    menu: SectionMenu::Locked {
        label: "Remove Transform",
        reason: super::inspector::TRANSFORM_REMOVE_REASON,
    },
    toggle: None,
    access: |_| None,
    normalize: None,
    on_variant: None,
    footer: None,
    presets: &[],
    render: Render::Custom(super::inspector_sections::show_transform),
};

/// `Mesh` 段。
///
/// 走专用渲染：`model` 与内置图元下拉要互相对照，Morph 目标的**名字来自渲染器**
/// （模型加载之后才知道有哪些），不是编译期常量。
pub static MESH: ReflectComponentSpec = ReflectComponentSpec {
    title: "Mesh",
    indent: "mesh",
    menu: SectionMenu::Removable {
        address: ComponentAddress::Builtin(BuiltinComponent::Mesh),
        label: "Remove Mesh",
    },
    toggle: None,
    access: |_| None,
    normalize: None,
    on_variant: None,
    footer: None,
    presets: &[],
    render: Render::Custom(super::inspector_sections::show_mesh),
};

/// `Animation` 段。
///
/// 走专用渲染：状态机是**一张图**（状态 + 转移 + 参数），不是字段树。
pub static ANIMATION: ReflectComponentSpec = ReflectComponentSpec {
    title: "Animation",
    indent: "animation",
    menu: SectionMenu::Removable {
        address: ComponentAddress::Builtin(BuiltinComponent::Animator),
        label: "Remove Animation",
    },
    toggle: None,
    access: |_| None,
    normalize: None,
    on_variant: None,
    footer: None,
    presets: &[],
    render: Render::Custom(super::inspector_sections::show_animation),
};

/// `Scripts` 段（Lua 实例集合）。
///
/// 没有对应的 `BuiltinComponent`：它不是"一个组件"，而是 `Entity::scripts`
/// 这个字段里的一组路径。所以菜单是 `Collection` —— 不能让分组标题冒充组件。
pub static SCRIPTS: ReflectComponentSpec = ReflectComponentSpec {
    title: "Scripts",
    indent: "scripts",
    menu: SectionMenu::Collection {
        label: "Remove Scripts Group",
        reason: "Scripts 是多个 Lua 组件实例的分组；请右键具体脚本行移除。",
    },
    toggle: None,
    access: |_| None,
    normalize: None,
    on_variant: None,
    footer: None,
    presets: &[],
    render: Render::Custom(super::inspector_sections::show_scripts),
};

/// `Behaviors` 段（Rust 实例集合）。理由同 [`SCRIPTS`]。
pub static BEHAVIORS: ReflectComponentSpec = ReflectComponentSpec {
    title: "Behaviors",
    indent: "behaviors",
    menu: SectionMenu::Collection {
        label: "Remove Behaviors Group",
        reason: "Behaviors 是多个 Rust 组件实例的分组；请右键具体组件行移除。",
    },
    toggle: None,
    access: |_| None,
    normalize: None,
    on_variant: None,
    footer: None,
    presets: &[],
    render: Render::Custom(super::inspector_sections::show_behaviors),
};

/// `Material` 段。
///
/// 走专用渲染：通道来自 `.mat` 资产（运行时才知道有哪些），而"资产 + 覆盖"
/// 是两层正交的数据，字段表只装得下其中一层。
pub static MATERIAL: ReflectComponentSpec = ReflectComponentSpec {
    title: "Material",
    indent: "material",
    menu: SectionMenu::Removable {
        address: ComponentAddress::Builtin(BuiltinComponent::Material),
        label: "Remove Material",
    },
    toggle: None,
    access: |_| None,
    normalize: None,
    on_variant: None,
    footer: None,
    presets: &[],
    render: Render::Custom(super::inspector_sections::show_material),
};

/// `Prefab` 段。
pub static PREFAB: ReflectComponentSpec = ReflectComponentSpec {
    title: "Prefab",
    indent: "prefab",
    // 这一段既不是组件实例也不是分组，右键里没有可移除的东西。
    menu: SectionMenu::None,
    toggle: None,
    access: |_| None,
    normalize: None,
    on_variant: None,
    footer: None,
    presets: &[],
    render: Render::Custom(super::inspector_sections::show_prefab),
};

/// 画一个内置组件的整段（分类条 + "勾上才有" + 全部字段 + 末尾说明）。
///
/// 调用方负责先判断组件是否存在（`BuiltinComponent::present`）：分类条与右键
/// 菜单由本函数绘制，没有组件的实体不应多出一个空标题。
pub(super) fn show_section(
    state: &mut crate::state::EditorState,
    ui: &mut egui::Ui,
    entity: &mut Entity,
    spec: &ReflectComponentSpec,
    sig: &mut FieldSignals,
    request: &mut Option<ComponentAction>,
    refresh_scripts: &mut bool,
) {
    let editing = state.is_editing();

    let header = theme::section_bar(ui, spec.title);
    let target = match spec.menu {
        SectionMenu::None => None,
        SectionMenu::Removable { address, label } => {
            Some(ComponentMenuTarget::Removable { address, label })
        }
        SectionMenu::Locked { label, reason } => {
            Some(ComponentMenuTarget::Locked { label, reason })
        }
        SectionMenu::Collection { label, reason } => Some(ComponentMenuTarget::Collection {
            label,
            reason,
        }),
    };
    if let Some(target) = target {
        component_context_menu(&header, state, entity, target, request, refresh_scripts);
    }

    // 专用渲染器自己负责缩进（它内部已经有 `ui.indent`），这里不能再包一层 ——
    // 两层缩进会让整段右移 15px，而段内的行距、控件宽度全都会跟着变。
    if let Render::Custom(render) = spec.render {
        let mut cx = SectionCx {
            state,
            sig,
            request,
            refresh_scripts,
        };
        render(ui, entity, &mut cx);
        return;
    }

    let mut switched: Option<(&'static str, &'static str)> = None;
    ui.indent(spec.indent, |ui| {
        // 取消勾选 = 移除组件。地址来自 spec 的菜单（`Removable` 才有地址）。
        let toggle_address = match spec.menu {
            SectionMenu::Removable { address, .. } => Some(address),
            _ => None,
        };
        if let Some((label, hint)) = spec.toggle {
            // 取消勾选 = 移除组件。勾选框的初值恒为 `true`：能走到这里就说明组件在。
            let mut enabled = true;
            if ui
                .add_enabled(editing, egui::Checkbox::new(&mut enabled, label))
                .on_hover_text(hint)
                .on_disabled_hover_text(PLAY_STRUCTURE_HINT)
                .changed()
            {
                if let Some(address) = toggle_address {
                    *request = Some(ComponentAction::Remove(address));
                }
            }
        }

        // 预设按钮：改的是一整片字段，所以只能放在拿到组件借用**之前**。
        let mut preset_changed = false;
        if !spec.presets.is_empty() {
            ui.horizontal(|ui| {
                ui.label(theme::dim_note("Preset:"));
                for preset in spec.presets {
                    if icons::chip(ui, preset.label, preset.tooltip) {
                        (preset.apply)(entity);
                        preset_changed = true;
                    }
                }
            });
        }

        let mut changed = preset_changed;
        {
            if let Some(component) = (spec.access)(entity) {
                let mut ctx = RenderCtx {
                    sig,
                    switch: &mut switched,
                    first_label: None,
                };
                changed = show_fields(ui, component, &mut ctx);
            }
        }
        // 借用必须在这里结束：下面两个钩子都要独占 `entity`。
        if let Some((field, variant)) = switched {
            if let Some(hook) = spec.on_variant {
                hook(entity, field, variant);
            }
            changed = true;
        }
        if changed {
            if let Some(normalize) = spec.normalize {
                normalize(entity);
            }
        }
        if let Some(footer) = spec.footer {
            theme::note(ui, footer(entity));
        }
    });
}

/// 一次字段渲染的共享状态。
struct RenderCtx<'a> {
    sig: &'a mut FieldSignals,
    /// 本帧被切换的枚举变体 `(字段名, 新变体)`；由 `show_section` 在借用结束后消费。
    switch: &'a mut Option<(&'static str, &'static str)>,
    /// 覆盖**第一个可见字段**的显示名（列表项用 `"L0"` / `"L1"`）。
    first_label: Option<String>,
}

/// 按元数据顺序画全部可见字段，返回"本帧是否有字段被改"。
fn show_fields(ui: &mut egui::Ui, host: &mut dyn Reflect, ctx: &mut RenderCtx) -> bool {
    let meta: &'static TypeMeta = host.type_meta();
    let mut changed = false;
    let mut i = 0usize;
    while i < meta.fields.len() {
        let field = &meta.fields[i];
        i += 1;
        if field.flags.hidden || !is_visible(&*host, field) {
            continue;
        }
        // 标签覆盖只作用于**第一个画出来的**字段，用完就清掉。
        let label_override = ctx.first_label.take();
        match field.row_group {
            Some(_group) => {
                // 只合并**连续**的同组字段：不连续还合并的话，渲染顺序会与声明
                // 顺序脱节，读代码的人得在元数据里来回跳才能预测界面。
                let mut members: Vec<&'static FieldMeta> = vec![field];
                while i < meta.fields.len() {
                    let next = &meta.fields[i];
                    if next.row_group != field.row_group {
                        break;
                    }
                    i += 1;
                    if next.flags.hidden || !is_visible(&*host, next) {
                        continue;
                    }
                    members.push(next);
                }
                changed |= show_group_row(ui, host, &members, label_override.as_deref(), ctx);
            }
            None => {
                changed |= show_field_row(ui, host, field, label_override.as_deref(), ctx);
            }
        }
    }
    changed
}

/// `visible_when` 的判定：同级 `bool` 字段为 `true` 才显示。
fn is_visible(host: &dyn Reflect, field: &FieldMeta) -> bool {
    let sibling = field
        .visible_when
        .and_then(|name| host.get(name))
        .and_then(|value| value.as_bool());
    field.visible_given(sibling)
}

/// 行标签：列表项的第一行用 `"L0"` 这种下标标签，其余用字段自己的显示名。
fn label_of<'a>(field: &'a FieldMeta, override_label: Option<&'a str>) -> &'a str {
    override_label.unwrap_or_else(|| field.display_name())
}

/// 一个字段按类型分派到具体控件。
fn show_field_row(
    ui: &mut egui::Ui,
    host: &mut dyn Reflect,
    field: &'static FieldMeta,
    label_override: Option<&str>,
    ctx: &mut RenderCtx,
) -> bool {
    // `widget` 优先于类型推断：`[f32; 2]` 默认是两根轴，声明成区间才是 `a → b`。
    // 这一层刻意放在最前面 —— 否则"类型 → 控件"的推断会把意图盖掉。
    match field.widget {
        Widget::Color => return show_color_row(ui, host, field, label_override, ctx),
        Widget::Range => return show_range_row(ui, host, field, label_override, ctx),
        Widget::Auto => {}
    }
    match field.ty {
        TypeRef::Scalar(kind) => show_scalar_row(ui, host, field, kind, label_override, ctx),
        TypeRef::Named(meta) if meta().kind == MetaKind::Enum => {
            show_enum_row(ui, host, field, meta(), label_override, ctx)
        }
        TypeRef::Named(_) => {
            // 嵌套结构体：把它的字段平铺进来（`UiElement` 的样式子结构就是这种）。
            // 画成一个折叠块需要额外的展开状态，而现在的用途是"在 Physics 里
            // 看到形状参数"，平铺比折叠更直接。
            let Some(slot) = host.get_mut(field.name) else {
                return false;
            };
            match slot {
                ValueRefMut::Struct(nested) => show_fields(ui, nested, ctx),
                ValueRefMut::List(_) => show_list_row(ui, host, field, ctx),
                _ => false,
            }
        }
        TypeRef::List(element) | TypeRef::Array(element, _) => {
            // 定长 / 不定长的数值数组：长度 2..4 且元素是标量时当向量画
            // （一排轴标 + 数值框），其余走列表编辑器。
            if is_vector_element(element) && vector_len_of(&field.ty) {
                show_axis_row(ui, host, field, label_override, ctx)
            } else {
                show_list_row(ui, host, field, ctx)
            }
        }
        TypeRef::Option(_) => {
            // `Option<T>` 字段：`None` 时画一行"未设置"，`Some` 时按**实际值**
            // 的类型挑控件（内层类型不用元数据再分一次）。
            let kind = host.get(field.name).and_then(|v| v.scalar_kind());
            match kind {
                Some(kind) => show_scalar_row(ui, host, field, kind, label_override, ctx),
                None => {
                    if matches!(host.get(field.name), Some(ValueRef::None)) {
                        row(ui, label_of(field, label_override), |ui| {
                            ui.label(theme::dim_note("（未设置）"));
                        });
                    }
                    false
                }
            }
        }
        TypeRef::Map(_) => false,
    }
}

/// 标量控件。
fn show_scalar_row(
    ui: &mut egui::Ui,
    host: &mut dyn Reflect,
    field: &'static FieldMeta,
    kind: ScalarKind,
    label_override: Option<&str>,
    ctx: &mut RenderCtx,
) -> bool {
    let label = label_of(field, label_override);
    match kind {
        // 布尔走整行勾选框，不再重复一列字段标签。
        ScalarKind::Bool => {
            let mut value = host.get(field.name).and_then(|v| v.as_bool()).unwrap_or(false);
            let response = with_doc(ui.checkbox(&mut value, label), field.doc);
            ctx.sig.collect(&response);
            if response.changed() {
                write(host, field.name, ScalarValue::Bool(value));
                return true;
            }
            false
        }
        ScalarKind::String => {
            let mut text = host
                .get(field.name)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            let response = row(ui, label, |ui| {
                let mut edit = egui::TextEdit::singleline(&mut text);
                if let Some(hint) = field.hint {
                    edit = edit.hint_text(hint);
                }
                // `+ 2.0` 与 Name 那行一致：单行输入框比数值框矮 2px 会显得塌。
                ui.add_sized(
                    egui::vec2(ui.available_width(), theme::FIELD_HEIGHT + 2.0),
                    edit,
                )
            });
            let response = with_doc(response, field.doc);
            ctx.sig.collect(&response);
            if response.changed() {
                write(host, field.name, ScalarValue::Str(text));
                return true;
            }
            false
        }
        ScalarKind::Vec2 | ScalarKind::Vec3 | ScalarKind::Vec4 | ScalarKind::Quat => {
            show_axis_row(ui, host, field, label_override, ctx)
        }
        _ => show_number_row(ui, host, field, label_override, ctx),
    }
}

/// 单分量数值框。
///
/// 宏而不是六段复制：`DragValue` 是泛型的，六个数值类型各自要一个 `&mut T`，
/// 类型一换其余三行（speed / range / decimals）完全一样。
macro_rules! number_arm {
    ($ui:expr, $host:expr, $field:expr, $label:expr, $ctx:expr, $ref_variant:ident, $ty:ty, $scalar:ident, $kind:expr) => {{
        let field = $field;
        let mut value: $ty = match $host.get(field.name) {
            Some(ValueRef::$ref_variant(v)) => *v,
            _ => <$ty>::default(),
        };
        let response = row($ui, $label, |ui| {
            let mut drag = egui::DragValue::new(&mut value);
            drag = drag.speed(field.speed.map_or(default_speed($kind), |s| s as f64));
            if let Some((lo, hi)) = field.range {
                // 浮点 → 整数用 `as`：Rust 的浮点转整型是**饱和**的，
                // 负数变 0、超界变 MAX，正好是"钳住"而不是回绕成天文数字。
                drag = drag.range((lo as $ty)..=(hi as $ty));
            }
            if let Some(decimals) = field.decimals {
                drag = drag.fixed_decimals(decimals as usize);
            }
            let response =
                ui.add_sized(egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT), drag);
            if let Some(note) = field.note {
                ui.label(theme::dim_note(note));
            }
            response
        });
        let response = with_doc(response, field.doc);
        $ctx.sig.collect(&response);
        if response.changed() {
            write($host, field.name, ScalarValue::$scalar(value));
            return true;
        }
        return false;
    }};
}

fn show_number_row(
    ui: &mut egui::Ui,
    host: &mut dyn Reflect,
    field: &'static FieldMeta,
    label_override: Option<&str>,
    ctx: &mut RenderCtx,
) -> bool {
    let label = label_of(field, label_override);
    // 分派依据是**字段声明的类型**（`cur` 的变体只用来取值）：
    // 元数据说这是 `u32`，控件就按 `u32` 编辑，避免"显示是浮点、写回是整数"。
    match field.ty {
        TypeRef::Scalar(ScalarKind::F32) => {
            number_arm!(ui, host, field, label, ctx, F32, f32, F32, ScalarKind::F32)
        }
        TypeRef::Scalar(ScalarKind::F64) => {
            number_arm!(ui, host, field, label, ctx, F64, f64, F64, ScalarKind::F64)
        }
        TypeRef::Scalar(ScalarKind::I32) => {
            number_arm!(ui, host, field, label, ctx, I32, i32, I32, ScalarKind::I32)
        }
        TypeRef::Scalar(ScalarKind::I64) => {
            number_arm!(ui, host, field, label, ctx, I64, i64, I64, ScalarKind::I64)
        }
        TypeRef::Scalar(ScalarKind::U32) => {
            number_arm!(ui, host, field, label, ctx, U32, u32, U32, ScalarKind::U32)
        }
        TypeRef::Scalar(ScalarKind::U64) => {
            number_arm!(ui, host, field, label, ctx, U64, u64, U64, ScalarKind::U64)
        }
        _ => false,
    }
}

/// 数值框在元数据没写 `speed` 时的兜底灵敏度。
///
/// 分两类而不是一律 0.1：`f32` 拖一下想动 0.1，而整数拖一下动 0.1 只会
/// **一格不动**（向下取整），看起来像控件坏了。
fn default_speed(kind: ScalarKind) -> f64 {
    match kind {
        ScalarKind::I32 | ScalarKind::I64 | ScalarKind::U32 | ScalarKind::U64 => 1.0,
        _ => 0.1,
    }
}

/// 这个字段能不能当"一排轴标"来画：`glam::Vec2/3/4` 或长度 2..4 的定长浮点数组。
fn vector_len_of(ty: &TypeRef) -> bool {
    matches!(
        ty,
        TypeRef::Scalar(ScalarKind::Vec2 | ScalarKind::Vec3 | ScalarKind::Vec4 | ScalarKind::Quat)
            | TypeRef::Array(_, 2..=4)
    )
}

/// 元素类型能不能参与"一排轴标"的排版（只有单分量浮点可以）。
fn is_vector_element(element: &TypeRef) -> bool {
    matches!(
        element,
        TypeRef::Scalar(
            ScalarKind::F32 | ScalarKind::F64 | ScalarKind::I32 | ScalarKind::U32
        )
    )
}

/// 多分量向量 / 定长数值数组：一排轴标 + 每轴一个数值框。
///
/// 排版与 Transform 的三行同款（`axis_field_width` 先算一次、彩色轴标、
/// 宽度按面板可用宽度三等分）。`PhysicsBody` 的 `half_extents` 走的就是这条路径 ——
/// 它在 RON 里是 `[f32; 3]`，在界面上必须跟 `Transform.scale` 长得一样。
fn show_axis_row(
    ui: &mut egui::Ui,
    host: &mut dyn Reflect,
    field: &'static FieldMeta,
    label_override: Option<&str>,
    ctx: &mut RenderCtx,
) -> bool {
    let label = label_of(field, label_override);
    // 先读成一排 `f32`：`Vec3` 与 `[f32; 3]` 在反射里有两种变体，读到本地就统一了。
    let Some((mut axes, len)) = read_components(&*host, field) else {
        return false;
    };
    if len < 2 {
        return false;
    }

    // 必须在 `row()` **之前**算：`row` 内部已经扣掉标签列并开了子 ui，
    // 那时候的 `available_width` 只剩控件区，再除以 3 会算窄。
    let axis_width = theme::axis_field_width(ui);
    let speed = field.speed.map_or(0.1, |s| s as f64);
    let mut local_changed = false;
    row(ui, label, |ui| {
        for (index, axis) in ["X", "Y", "Z", "W"].iter().take(len).enumerate() {
            theme::axis_label(ui, axis);
            let response = match field.range {
                Some((lo, hi)) => {
                    theme::drag_field_range_w(ui, axis_width, &mut axes[index], speed, lo..=hi)
                }
                None => theme::drag_field_w(ui, axis_width, &mut axes[index], speed, ""),
            };
            ctx.sig.collect(&response);
            local_changed |= response.changed();
        }
        if let Some(note) = field.note {
            ui.label(theme::dim_note(note));
        }
    });
    if !local_changed {
        return false;
    }
    write_components(host, field, &axes[..len]);
    true
}

/// 把多分量字段读成 4 个槽，返回 `(槽, 实际分量数)`。
///
/// `Vec2/3/4` 与 `[f32; N]` 在反射里是两种变体（标量向量 / 列表），读到本地就
/// 统一了 —— 上游于是只需要"一排数"，不用关心底层表示。
fn read_components(host: &dyn Reflect, field: &FieldMeta) -> Option<([f32; 4], usize)> {
    let mut slots = [0.0f32; 4];
    let len = match host.get(field.name)? {
        ValueRef::Vec2(v) => {
            slots[..2].copy_from_slice(&[v.x, v.y]);
            2
        }
        ValueRef::Vec3(v) => {
            slots[..3].copy_from_slice(&[v.x, v.y, v.z]);
            3
        }
        ValueRef::Vec4(v) => {
            slots.copy_from_slice(&[v.x, v.y, v.z, v.w]);
            4
        }
        ValueRef::Quat(v) => {
            slots.copy_from_slice(&[v.x, v.y, v.z, v.w]);
            4
        }
        ValueRef::List(list) => {
            let len = list.len().min(4);
            for (index, slot) in slots.iter_mut().enumerate().take(len) {
                *slot = list.get(index).and_then(|v| v.as_f32()).unwrap_or_default();
            }
            len
        }
        _ => return None,
    };
    Some((slots, len))
}

/// 把一排数写回多分量字段。
fn write_components(host: &mut dyn Reflect, field: &FieldMeta, values: &[f32]) {
    let mut slots = [0.0f32; 4];
    slots[..values.len()].copy_from_slice(values);
    match &field.ty {
        TypeRef::Scalar(ScalarKind::Vec2) => write(
            host,
            field.name,
            ScalarValue::Vec2(xgame_math::glam::Vec2::new(slots[0], slots[1])),
        ),
        TypeRef::Scalar(ScalarKind::Vec4) => write(
            host,
            field.name,
            ScalarValue::Vec4(xgame_math::glam::Vec4::new(
                slots[0], slots[1], slots[2], slots[3],
            )),
        ),
        TypeRef::Scalar(ScalarKind::Quat) => write(
            host,
            field.name,
            ScalarValue::Quat(xgame_math::glam::Quat::from_xyzw(
                slots[0], slots[1], slots[2], slots[3],
            )),
        ),
        TypeRef::Scalar(_) => write(
            host,
            field.name,
            ScalarValue::Vec3(xgame_math::glam::Vec3::new(slots[0], slots[1], slots[2])),
        ),
        // 定长数组：逐个元素写回（`ReflectList` 没有"整体替换"的接口）。
        _ => write_list_scalars(host, field.name, values),
    }
}

/// 取色块。
///
/// 直接把 `&mut [f32; 4]` 交给 egui 会让借用活到绘制结束，于是先读到局部副本、
/// 画完再写回 —— 与数值行同一条规则（见模块文档的"写入路径"）。
fn show_color_row(
    ui: &mut egui::Ui,
    host: &mut dyn Reflect,
    field: &'static FieldMeta,
    label_override: Option<&str>,
    ctx: &mut RenderCtx,
) -> bool {
    let Some((slots, len)) = read_components(&*host, field) else {
        return false;
    };
    if len < 3 {
        // `widget = "color"` 配到短数组上：derive 已经拦了，走到这里是手写元数据。
        log::warn!("字段 {} 声明了取色块但不是 3/4 分量", field.name);
        return false;
    }
    let mut rgba = slots;
    let label = label_of(field, label_override);
    let response = row(ui, label, |ui| {
        let response = ui.color_edit_button_rgba_unmultiplied(&mut rgba);
        if let Some(note) = field.note {
            ui.label(theme::dim_note(note));
        }
        response
    });
    let response = with_doc(response, field.doc);
    ctx.sig.collect(&response);
    if !response.changed() {
        return false;
    }
    write_components(host, field, &rgba[..len]);
    true
}

/// 区间对：两个数值框之间一个 `→`（`[最短, 最长]`）。
///
/// 与 [`show_axis_row`] 的区别是语义而不是形状 —— 但作者看到的差别很大：
/// 轴标写 `X / Y` 会让人以为在填一个二维坐标，`→` 才读得出"从下界到上界"。
fn show_range_row(
    ui: &mut egui::Ui,
    host: &mut dyn Reflect,
    field: &'static FieldMeta,
    label_override: Option<&str>,
    ctx: &mut RenderCtx,
) -> bool {
    let Some((mut slots, len)) = read_components(&*host, field) else {
        return false;
    };
    if len != 2 {
        log::warn!("字段 {} 声明了区间但对数不是 2", field.name);
        return false;
    }
    let label = label_of(field, label_override);
    let speed = field.speed.map_or(default_speed(ScalarKind::F32), |s| s as f64);
    let mut changed = false;
    let mut first: Option<egui::Response> = None;
    row(ui, label, |ui| {
        for (index, slot) in slots[..2].iter_mut().enumerate() {
            if index > 0 {
                ui.label(theme::dim_note("→"));
            }
            let mut drag = egui::DragValue::new(slot).speed(speed);
            if let Some((lo, hi)) = field.range {
                // 区间两端常常要允许相等（`[0.5, 0.5]` = 固定值），
                // `DragValue` 的闭区间正好允许。
                drag = drag.range(lo..=hi);
            }
            if let Some(decimals) = field.decimals {
                drag = drag.fixed_decimals(decimals as usize);
            }
            let response =
                ui.add_sized(egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT), drag);
            changed |= response.changed();
            if first.is_none() {
                first = Some(response.clone());
            }
        }
        if let Some(note) = field.note {
            ui.label(theme::dim_note(note));
        }
    });
    // 悬浮说明挂在第一个数值框上：整行没有"自己的"响应，而作者悬停时
    // 十有八九指在第一个框上。
    if let Some(response) = first {
        ctx.sig.collect(&with_doc(response, field.doc));
    }
    if !changed {
        return false;
    }
    write_components(host, field, &slots[..2]);
    true
}

/// 枚举：下拉框 + 载荷。
///
/// 载荷不另写控件分派 —— 它被包成 [`EnumPayload`] 当结构体喂回 `show_fields`，
/// 所以 `CollisionShape::Cuboid.half_extents` 与 `PhysicsBody.mass` 走的是同一段代码。
fn show_enum_row(
    ui: &mut egui::Ui,
    host: &mut dyn Reflect,
    field: &'static FieldMeta,
    meta: &'static TypeMeta,
    label_override: Option<&str>,
    ctx: &mut RenderCtx,
) -> bool {
    let label = label_of(field, label_override);
    let current = match host.get(field.name) {
        Some(ValueRef::Enum(v)) => v.variant(),
        _ => return false,
    };
    // 悬浮提示用**当前变体**的说明，文字直接来自变体的文档注释。
    let variant_doc = meta
        .variant(current)
        .map(|v| v.doc)
        .filter(|doc| !doc.is_empty());

    let mut picked: Option<&'static str> = None;
    let response = row(ui, label, |ui| {
        egui::ComboBox::from_id_salt(format!("reflect_{}_{}", meta.name, field.name))
            .selected_text(variant_label(meta, current))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for variant in meta.variants {
                    // 变体说明挂在每一项的悬浮上：下拉框收起时显示的是短标签，
                    // 长解释只能在展开时才有地方放 —— 而它恰恰是展开着才需要看的。
                    let item = ui.selectable_label(variant.name == current, variant.display_name());
                    let item = if variant.doc.is_empty() {
                        item
                    } else {
                        item.on_hover_text(variant.doc)
                    };
                    if item.clicked() {
                        picked = Some(variant.name);
                    }
                }
            })
            .response
    });
    let _ = with_doc(response, variant_doc.unwrap_or(field.doc));
    if let Some(picked) = picked {
        if let Some(ValueRefMut::Enum(value)) = host.get_mut(field.name) {
            if value.set_variant(picked) {
                // 下拉框的改动不进 `Response::changed`，见 `FieldSignals::mark_changed`。
                ctx.sig.mark_changed();
                *ctx.switch = Some((field.name, picked));
                return true;
            }
        }
        return false;
    }

    // -- 载荷 --
    let payload_meta = match host.get(field.name) {
        Some(ValueRef::Enum(value)) => value.payload_meta(),
        _ => None,
    };
    if payload_meta.is_none() {
        // 无载荷变体：**只在字段声明了 `variant_note` 时**把变体说明画成一行小字
        // （`CollisionShape::Auto` 的"按实体缩放的绝对值生成盒子"）。
        // 默认不画 —— 自解释的变体（`sfx` / `dynamic`）多一行只是噪声，而且
        // 多出来的那一行会把下面的内容整体推下去。
        if field.variant_note {
            if let Some(doc) = variant_doc {
                ui.label(theme::dim_note(doc));
            }
        }
        return false;
    }

    let Some(ValueRefMut::Enum(value)) = host.get_mut(field.name) else {
        return false;
    };
    let mut payload = EnumPayload(value);
    show_fields(ui, &mut payload, ctx)
}

/// 把"枚举当前变体的载荷"包装成一个可读写的结构体。
///
/// 载荷不是独立的 Rust 值 —— 它散在枚举变体里，没有一块连续内存可以假装成结构体
/// 借出去。这个适配器让载荷与真结构体走**同一个** `show_fields`，
/// 代价是每次取字段多一次 match。
struct EnumPayload<'a>(&'a mut dyn ReflectEnum);

impl Reflect for EnumPayload<'_> {
    fn type_meta(&self) -> &'static TypeMeta {
        self.0.payload_meta().unwrap_or(&EMPTY_META)
    }

    fn get(&self, field: &str) -> Option<ValueRef<'_>> {
        self.0.payload_get(field)
    }

    fn get_mut(&mut self, field: &str) -> Option<ValueRefMut<'_>> {
        self.0.payload_get_mut(field)
    }
}

/// 载荷缺失时的占位元数据（字段表为空，于是 `show_fields` 什么都不画）。
static EMPTY_META: TypeMeta = TypeMeta {
    name: "()",
    doc: "",
    kind: MetaKind::Struct,
    fields: &[],
    variants: &[],
};

/// 变体的显示名。找不到时退回寻址名 —— 那说明 `ReflectEnum` 与元数据脱节，
/// 界面上显示 `Bgm` 也比显示空白强。
fn variant_label(meta: &'static TypeMeta, name: &'static str) -> &'static str {
    meta.variant(name)
        .map_or(name, |variant| variant.display_name())
}

/// 列表：每一项按结构体画一组行，行尾一个删除按钮；全部项之后是"加一项"。
///
/// 元素不是结构体时（`Vec<f32>` 这种）画成一行数值框 + 删除按钮。
fn show_list_row(
    ui: &mut egui::Ui,
    host: &mut dyn Reflect,
    field: &'static FieldMeta,
    ctx: &mut RenderCtx,
) -> bool {
    let Some(ValueRefMut::List(items)) = host.get_mut(field.name) else {
        return false;
    };
    let count = items.len();
    let mut changed = false;
    let mut remove_at: Option<usize> = None;

    for index in 0..count {
        let Some(item) = items.get_mut(index) else {
            continue;
        };
        // 第一行的标签是下标（`L0`），其余字段用各自的显示名。
        let mut item_ctx = RenderCtx {
            sig: &mut *ctx.sig,
            switch: &mut *ctx.switch,
            first_label: field.item_prefix.map(|prefix| format!("{prefix}{index}")),
        };
        match item {
            ValueRefMut::Struct(inner) => {
                changed |= show_fields(ui, inner, &mut item_ctx);
            }
            other => {
                // 标量元素：标签用下标，控件占满一行。
                let label = field
                    .item_prefix
                    .map_or_else(|| index.to_string(), |prefix| format!("{prefix}{index}"));
                changed |= show_list_scalar(ui, other, &label, item_ctx.sig);
            }
        }
        // 删除按钮单独一行、靠右：`row()` 的标签列留空，控件区右对齐。
        row(ui, "", |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::close_button(ui, "删掉这一项") {
                    remove_at = Some(index);
                }
            });
        });
    }

    if let Some(index) = remove_at {
        items.remove(index);
        // 增删也必须标脏：否则删掉一项之后 Ctrl+S 不会保存，"改完没生效"。
        ctx.sig.mark_changed();
        changed = true;
    }
    if let Some(add_label) = field.add_label {
        if icons::add_row(ui, add_label, "追加一项").clicked() {
            // 追加在末尾：`insert_default` 允许 `index == len`。
            items.insert_default(items.len());
            ctx.sig.mark_changed();
            changed = true;
        }
    }
    if let Some(note) = field.footnote {
        theme::note(ui, note);
    }
    changed
}

/// 列表里的标量元素：一个数值框，写在当前行。
fn show_list_scalar(
    ui: &mut egui::Ui,
    slot: ValueRefMut<'_>,
    label: &str,
    sig: &mut FieldSignals,
) -> bool {
    let mut slot = slot;
    let mut value = slot.as_ref().as_f32().unwrap_or(0.0);
    let response = row(ui, label, |ui| {
        ui.add_sized(
            egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT),
            egui::DragValue::new(&mut value).speed(0.1),
        )
    });
    sig.collect(&response);
    if !response.changed() {
        return false;
    }
    match slot.scalar_kind() {
        Some(ScalarKind::F32) => slot.set(&ScalarValue::F32(value)),
        Some(ScalarKind::F64) => slot.set(&ScalarValue::F64(value as f64)),
        _ => Ok(()),
    }
    .is_ok()
}

/// 同排一行里的一格。
///
/// 组里允许混排是 S3 才出现的需求：`Duration` 后面紧跟 `Loop` 勾选框、
/// 出生色后面跟死亡色。"一行两个数值框"是最早的形态，不是唯一形态。
enum Cell {
    Number(f32),
    Bool(bool),
    Color([f32; 4]),
}

/// 同排的一组字段（`min` → `max` 那种成对区间）。
///
/// 组内字段先读到局部副本、画完再逐个写回：同时持有两个 `ValueRefMut` 是
/// `ValueRefMut` 的借用模型不允许的（见 `reflect::value` 的模块文档）。
fn show_group_row(
    ui: &mut egui::Ui,
    host: &mut dyn Reflect,
    members: &[&'static FieldMeta],
    label_override: Option<&str>,
    ctx: &mut RenderCtx,
) -> bool {
    let cells = read_cells(&*host, members);
    let Some(mut cells) = cells else {
        // 组里有画不到一行里的类型：退回逐行。宁可不合并，也不画错 ——
        // 合并是排版偏好，画错是信息错误。
        let mut changed = false;
        for (index, field) in members.iter().copied().enumerate() {
            let override_label = if index == 0 { label_override } else { None };
            changed |= show_field_row(ui, host, field, override_label, ctx);
        }
        return changed;
    };

    let label = label_of(members[0], label_override);
    let mut touched = vec![false; members.len()];

    row(ui, label, |ui| {
        for (index, field) in members.iter().enumerate() {
            // `→` 只夹在两个可比较的量之间：`Duration → ☑ Loop` 里的箭头
            // 会把"一个是数、一个是开关"读成一条连续刻度，那是误导。
            if index > 0
                && !matches!(cells[index], Cell::Bool(_))
                && !matches!(cells[index - 1], Cell::Bool(_))
            {
                ui.label(theme::dim_note("→"));
            }
            let response = match &mut cells[index] {
                Cell::Number(value) => {
                    let mut drag = egui::DragValue::new(value)
                        .speed(field.speed.map_or(default_speed(ScalarKind::F32), |s| s as f64));
                    if let Some((lo, hi)) = field.range {
                        drag = drag.range(lo..=hi);
                    }
                    if let Some(decimals) = field.decimals {
                        drag = drag.fixed_decimals(decimals as usize);
                    }
                    ui.add_sized(egui::vec2(theme::FIELD_WIDTH, theme::FIELD_HEIGHT), drag)
                }
                Cell::Bool(value) => ui.checkbox(value, field.display_name()),
                Cell::Color(rgba) => ui.color_edit_button_rgba_unmultiplied(rgba),
            };
            let response = with_doc(response, field.doc);
            ctx.sig.collect(&response);
            touched[index] = response.changed();
            if let Some(note) = field.note {
                ui.label(theme::dim_note(note));
            }
        }
    });

    if let Some(note) = members.last().and_then(|field| field.footnote) {
        theme::note(ui, note);
    }

    let mut changed = false;
    for (index, field) in members.iter().enumerate() {
        if !touched[index] {
            continue;
        }
        match &cells[index] {
            Cell::Number(value) => {
                let scalar = match field.ty {
                    TypeRef::Scalar(ScalarKind::F64) => ScalarValue::F64(*value as f64),
                    _ => ScalarValue::F32(*value),
                };
                write(host, field.name, scalar);
            }
            Cell::Bool(value) => write(host, field.name, ScalarValue::Bool(*value)),
            Cell::Color(rgba) => write_components(host, field, rgba),
        }
        changed = true;
    }
    changed
}

/// 把一组成员读成一排 [`Cell`]；有任何一个读不出来就返回 `None`。
fn read_cells(host: &dyn Reflect, members: &[&'static FieldMeta]) -> Option<Vec<Cell>> {
    if members.len() < 2 {
        return None;
    }
    let mut cells = Vec::with_capacity(members.len());
    for field in members {
        let cell = match (field.widget, field.ty) {
            (Widget::Color, _) => {
                let (slots, len) = read_components(host, field)?;
                if len < 3 {
                    return None;
                }
                Cell::Color(slots)
            }
            (_, TypeRef::Scalar(ScalarKind::Bool)) => {
                Cell::Bool(host.get(field.name)?.as_bool()?)
            }
            // 只有浮点能同排：整型塞进 `f32` 会丢精度（2^24 以上），
            // 而成对区间在引擎里全是浮点。
            (_, TypeRef::Scalar(ScalarKind::F32 | ScalarKind::F64)) => {
                Cell::Number(host.get(field.name)?.as_f32()?)
            }
            _ => return None,
        };
        cells.push(cell);
    }
    Some(cells)
}

/// 逐个元素写回定长数值数组。
fn write_list_scalars(host: &mut dyn Reflect, field: &str, values: &[f32]) {
    let Some(ValueRefMut::List(items)) = host.get_mut(field) else {
        return;
    };
    for (index, value) in values.iter().enumerate() {
        if let Some(mut slot) = items.get_mut(index) {
            if slot.scalar_kind() == Some(ScalarKind::F32) {
                let _ = slot.set(&ScalarValue::F32(*value));
            }
        }
    }
}

/// 把值写回组件。类型不匹配只可能来自"元数据与 `Reflect` 实现脱节"，
/// 那是编程错误而不是用户输入错误 —— 记一条日志，不打断编辑。
fn write(host: &mut dyn Reflect, field: &str, value: ScalarValue) {
    match host.get_mut_checked(field) {
        Ok(mut slot) => {
            if let Err(err) = slot.set(&value) {
                log::warn!("反射写回失败：{field} —— {err}");
            }
        }
        Err(err) => log::warn!("反射取不到可写字段：{field} —— {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_core::reflect::ReflectMeta;
    use xgame_core::{Camera, CollisionShape, Entity};

    /// 两个 spec 的 `access` 必须真的拿到组件、且拿到的是**同一个**值 ——
    /// 写错了 `Some(...)` 的分支会让 Inspector 画出一个永远改不动的副本。
    #[test]
    fn spec_access_reads_and_writes_the_real_component() {
        let mut entity = Entity::new("probe");
        entity.camera = Some(Camera::default());
        {
            let view = (CAMERA.access)(&mut entity).expect("camera 应在");
            assert_eq!(view.type_meta().name, "Camera");
            write(view, "fov", ScalarValue::F32(45.0));
        }
        assert_eq!(entity.camera.as_ref().unwrap().fov, 45.0);

        // 组件不存在时必须返回 `None`（否则会画出空 section）。
        let mut bare = Entity::new("bare");
        assert!((CAMERA.access)(&mut bare).is_none());
        assert!((AUDIO.access)(&mut bare).is_none());
        assert!((PHYSICS.access)(&mut bare).is_none());
        assert!((CHARACTER.access)(&mut bare).is_none());
    }

    #[test]
    fn camera_normalize_keeps_far_beyond_near() {
        let mut entity = Entity::new("probe");
        entity.camera = Some(Camera {
            near: 10.0,
            far: 1.0,
            ..Camera::default()
        });
        (CAMERA.normalize.unwrap())(&mut entity);
        let camera = entity.camera.unwrap();
        assert!(camera.far > camera.near, "far = {}", camera.far);
    }

    #[test]
    fn audio_normalize_keeps_max_above_min() {
        let mut entity = Entity::new("probe");
        entity.audio = Some(xgame_core::AudioSource {
            min_distance: 20.0,
            max_distance: 5.0,
            ..xgame_core::AudioSource::default()
        });
        (AUDIO.normalize.unwrap())(&mut entity);
        let audio = entity.audio.unwrap();
        assert!(audio.max_distance > audio.min_distance);
    }

    /// `visible_when` 必须让距离区间跟着 `spatial` 走。
    #[test]
    fn distance_fields_hide_when_not_spatial() {
        use xgame_core::AudioSource;
        let min = AudioSource::meta().field("min_distance").unwrap();
        let off = AudioSource {
            spatial: false,
            ..AudioSource::default()
        };
        let on = AudioSource {
            spatial: true,
            ..AudioSource::default()
        };
        assert!(!is_visible(&off, min));
        assert!(is_visible(&on, min));
    }

    /// 组标签取组内第一个字段的显示名，组尾取最后一个字段的 `footnote`。
    #[test]
    fn distance_group_is_a_pair_with_one_caption() {
        use xgame_core::AudioSource;
        let meta = AudioSource::meta();
        let min = meta.field("min_distance").unwrap();
        let max = meta.field("max_distance").unwrap();
        assert_eq!(min.row_group, max.row_group);
        assert_eq!(min.display_name(), "Distance");
        assert!(min.footnote.is_none(), "说明只该挂一个字，否则会画两遍");
        assert!(max.footnote.is_some());
    }

    /// 切到 `Box` 时必须按实体缩放给半边长，而不是留一个零厚度碰撞体。
    #[test]
    fn switching_to_box_seeds_half_extents_from_scale() {
        let mut entity = Entity::new("probe");
        entity.transform.scale = xgame_math::glam::Vec3::new(4.0, 0.2, 4.0);
        entity.physics = Some(xgame_core::PhysicsBody::default());
        (PHYSICS.on_variant.unwrap())(&mut entity, "shape", "Cuboid");
        match entity.physics.unwrap().shape {
            // 半边长 = |缩放| × 0.5：实体是 4×0.2×4 的"平台"，盒子就是 2×0.1×2。
            CollisionShape::Cuboid { half_extents } => assert_eq!(half_extents, [2.0, 0.1, 2.0]),
            other => panic!("应该是 Cuboid，实际 {other:?}"),
        }
    }

    /// 角色段末尾那行派生量必须跟着 `height` / `radius` 变。
    #[test]
    fn character_footer_reports_the_derived_center() {
        let mut entity = Entity::new("probe");
        entity.controller = Some(xgame_core::CharacterController {
            height: 1.8,
            radius: 0.3,
            ..xgame_core::CharacterController::default()
        });
        let text = (CHARACTER.footer.unwrap())(&entity);
        assert!(text.contains("0.90"), "实际 = {text}");
    }

    /// LOD 新加的级别要沿用上一级的模型，否则"加一级"得到的是一行空模型，
    /// 而空模型的 LOD 不为 `present()` —— 段会直接消失。
    #[test]
    fn lod_normalize_seeds_new_levels_from_the_previous_one() {
        let mut entity = Entity::new("probe");
        entity.lod.levels = vec![
            xgame_core::LodLevel::new("Resources/Fox.gltf", 20.0),
            xgame_core::LodLevel::new("", 0.0),
        ];
        (LOD.normalize.unwrap())(&mut entity);
        assert_eq!(entity.lod.levels[1].model, "Resources/Fox.gltf");
        // 用户填过的不能被覆盖。
        entity.lod.levels[1].model = "Resources/Box.glb".into();
        (LOD.normalize.unwrap())(&mut entity);
        assert_eq!(entity.lod.levels[1].model, "Resources/Box.glb");
    }

    /// 粒子段的 spec：预设改的是整块组件、钳制走引擎自己的规则。
    #[test]
    fn particle_spec_applies_presets_and_clamps() {
        let mut entity = Entity::new("probe");
        entity.particle = Some(xgame_core::ParticleEmitter::default());
        {
            let view = (PARTICLE.access)(&mut entity).expect("particle 应在");
            assert_eq!(view.type_meta().name, "ParticleEmitter");
            write(view, "rate", ScalarValue::F32(12.0));
        }
        assert_eq!(entity.particle.as_ref().unwrap().rate, 12.0);

        // 预设：一键换成火焰那套参数。
        (PARTICLE.presets[0].apply)(&mut entity);
        let fire = entity.particle.unwrap();
        assert!(fire.speed[1] > fire.speed[0], "火焰应当比默认更快");
        assert_eq!(fire.blend, xgame_core::particle::ParticleBlend::Additive);

        // 钳制：`max_particles` 超上限、`lifetime` 反序都要被修好。
        entity.particle = Some(xgame_core::ParticleEmitter {
            max_particles: u32::MAX,
            lifetime: [5.0, 1.0],
            ..xgame_core::ParticleEmitter::default()
        });
        (PARTICLE.normalize.unwrap())(&mut entity);
        let fixed = entity.particle.unwrap();
        assert_eq!(
            fixed.max_particles,
            xgame_core::particle::MAX_PARTICLES_LIMIT
        );
        assert!(fixed.lifetime[0] <= fixed.lifetime[1], "{:?}", fixed.lifetime);
    }

    /// UI 段走专用渲染器，但仍然登记在同一张表里 —— 这条守住"入口统一"：
    /// spec 得能拿到组件、`render` 得是 `Custom`。
    #[test]
    fn ui_spec_is_registered_with_a_custom_renderer() {
        assert!(matches!(UI.render, Render::Custom(_)));
        assert!(matches!(
            UI.menu,
            SectionMenu::Removable {
                address: ComponentAddress::Builtin(BuiltinComponent::Ui),
                ..
            }
        ));
        assert_eq!(UI.title, "UI");
        let mut entity = Entity::new("probe");
        assert!((UI.access)(&mut entity).is_none());
        entity.ui = Some(xgame_core::UiElement::default());
        let view = (UI.access)(&mut entity).expect("ui 应在");
        assert_eq!(view.type_meta().name, "UiElement");
    }

    /// 混合类型的行组（数值 + 勾选框）不能插 `→`：那会把"一个数、一个开关"
    /// 读成一条连续刻度。这里用 `Duration` + `Loop` 那对守住。
    #[test]
    fn mixed_group_skips_the_arrow_before_a_toggle() {
        use xgame_core::reflect::ReflectMeta;
        let meta = <xgame_core::ParticleEmitter as ReflectMeta>::meta();
        let duration = meta.field("duration").unwrap();
        let looping = meta.field("looping").unwrap();
        assert_eq!(duration.row_group, looping.row_group);
        assert_eq!(duration.row_group, Some("duration"));
        // 一个数值、一个布尔 —— 渲染器见到 `Cell::Bool` 就不画箭头。
        assert!(matches!(duration.ty, TypeRef::Scalar(ScalarKind::F32)));
        assert!(matches!(looping.ty, TypeRef::Scalar(ScalarKind::Bool)));
    }

    /// 无头跑一遍全部字段，抓"画的时候才炸"的问题：id 冲突、借用顺序违规、
    /// 元数据与 `Reflect` 实现不匹配导致的分支走错。
    ///
    /// 断言形状数量而不是截图：这里要守的是"渲染能跑通且条件显示真的少画了东西"，
    /// 这里验证渲染路径与条件显示；视觉排版由截图验收单独负责（需要 GPU）。
    #[test]
    fn renders_headlessly_and_hides_gated_rows() {
        let ctx = egui::Context::default();
        crate::panels::theme::install_fonts(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 900.0),
            )),
            ..Default::default()
        };

        let make = |spatial: bool| xgame_core::AudioSource {
            clip: "Resources/Audio/Beep.wav".into(),
            spatial,
            ..xgame_core::AudioSource::default()
        };
        let count = |component: &mut dyn Reflect| -> usize {
            let mut shapes = 0usize;
            // 跑两帧：egui 第一帧分配 id、第二帧才稳定，只跑一帧会漏掉
            // "第二帧才出现的 id 冲突"。
            for _ in 0..2 {
                let output = ctx.run(input.clone(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let mut sig = FieldSignals::default();
                        let mut switch = None;
                        let mut render_ctx = RenderCtx {
                            sig: &mut sig,
                            switch: &mut switch,
                            first_label: None,
                        };
                        show_fields(ui, component, &mut render_ctx);
                    });
                });
                shapes = output.shapes.len();
            }
            shapes
        };

        let with_distance = count(&mut make(true));
        let without_distance = count(&mut make(false));
        assert!(
            without_distance < with_distance,
            "关掉 spatial 后必须少画两行（实际 {without_distance} vs {with_distance}）"
        );

        assert!(count(&mut Camera::default()) > 0);
        assert!(count(&mut xgame_core::ParticleEmitter::default()) > 0);
        assert!(count(&mut xgame_core::PhysicsBody::default()) > 0);
        assert!(count(&mut xgame_core::CharacterController::default()) > 0);
        // 列表要有内容才会走"每项一组行 + 删除按钮"那条路；空列表只画 Add。
        let mut lod = xgame_core::LodGroup::default();
        assert!(count(&mut lod) > 0, "空列表也要能画（Add Level 行）");
        lod.levels = vec![
            xgame_core::LodLevel::new("Resources/Fox.gltf", 20.0),
            xgame_core::LodLevel::new("Resources/Box.glb", 0.0),
        ];
        let two_levels = count(&mut lod);
        assert!(
            two_levels > count(&mut xgame_core::LodGroup::default()),
            "两个级别的列表必须比空列表画得多"
        );
    }

    /// 带载荷枚举的载荷走的是"包装成结构体"那条路 —— 这条测试守住包装本身：
    /// 字段表、读、写都要通到真正的变体上。
    #[test]
    fn enum_payload_round_trips_through_the_adapter() {
        let mut shape = CollisionShape::Cuboid {
            half_extents: [1.0, 2.0, 3.0],
        };
        {
            let mut payload = EnumPayload(&mut shape);
            let meta = payload.type_meta();
            assert_eq!(meta.name, "Cuboid");
            let names: Vec<&str> = meta.fields.iter().map(|f| f.name).collect();
            assert_eq!(names, ["half_extents"]);
            // 读：数组在反射里是 List。
            assert!(matches!(
                payload.get("half_extents"),
                Some(ValueRef::List(_))
            ));
            // 写：逐个元素。
            write_list_scalars(&mut payload, "half_extents", &[9.0, 8.0, 7.0]);
            assert!(payload.get("nope").is_none());
        }
        assert_eq!(
            shape,
            CollisionShape::Cuboid {
                half_extents: [9.0, 8.0, 7.0]
            }
        );
    }
}
