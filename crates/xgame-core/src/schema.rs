//! 场景格式的**机器可读说明书**（Agent-Native ③）。
//!
//! # 为什么需要它
//!
//! 场景是 RON 文本，智能体可以直接读写；但"直接读写"不等于"知道怎么写"——
//! 字段名、可选组件、默认值、枚举变体、资产引用规则分散在十几个 Rust 类型里，
//! 只靠 `--check` 的报错反推，试错成本极高（一次坏 RON 就是一次加载失败）。
//!
//! 本模块把**真正参与落盘的类型**整理成一份结构化目录：每个类型有字段、字段类型、
//! 是否必填、省略时的默认 RON、可直接解析的示例；每种资产引用有磁盘 / 内存两种
//! 形态。导出成 JSON 后，智能体在写第一个实体之前就能拿到完整契约。
//!
//! # 真相源与防漂移
//!
//! 目录是**手写**的（Rust 的 serde 元数据在运行期不可反射），但测试会从三个方向
//! 钉住它：
//!
//! 1. **穷举解构**每个结构体 / 枚举 —— 新增字段或变体时测试编译不过，必须同步目录；
//! 2. **默认值与示例全部解析回真实类型** —— 写错一个字段名或类型，测试立刻红；
//! 3. **字段类型引用必须能在目录或基元表里找到** —— 防止出现"文档里有个类型，
//!    代码里没有"的悬空引用。
//!
//! 运行时字段被刻意排除：`Entity.components`、`UiElement.state`、
//! `ParticleEmitter.burst` 都是 `#[serde(skip)]`，写进 RON 也不会生效。
//!
//! # 确定性
//!
//! 输出里没有时间戳、没有随机 ID、没有 HashMap 迭代序（实体示例的 ID 固定为 1）。
//! 同一份代码连续导出两次必须逐字节相同 —— 这样 schema 才能进版本库做 diff，
//! 智能体也能用哈希判断"引擎格式有没有变"。

use std::collections::BTreeMap;

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::animation::{
    AnimAdditive, AnimCondition, AnimLayer, AnimLayerMode, AnimMachine, AnimState, AnimTransition,
    Animator,
};
use crate::audio::AudioSource;
use crate::camera::Camera;
use crate::character::CharacterController;
use crate::id::EntityId;
use crate::inspect::{FieldValue, FieldValues};
use crate::light::{Light, LightKind};
use crate::material::{MaterialAsset, MaterialOverride};
use crate::particle::{EmissionShape, ParticleBlend, ParticleEmitter};
use crate::physics::{BodyKind, CollisionShape, PhysicsBody};
use crate::prefab::{Prefab, PrefabInstanceRef};
use crate::scene::{Entity, Scene};
use crate::transform::Transform;
use crate::ui::{UiAlign, UiElement, UiImageStyle, UiKind, UiStyle, UiTextStyle, UiVAlign};

/// Schema 自身的格式版本。**加字段不涨，改语义 / 删字段 / 改默认值才涨。**
///
/// 与项目清单的 `PROJECT_FORMAT_VERSION` 是两条独立的线：schema 描述的是
/// "智能体读得懂的那份说明书"，场景文件本身仍由 serde 的向后兼容规则兜底。
pub const SCHEMA_FORMAT_VERSION: u32 = 1;

/// 一份完整的场景格式说明（`--export-schema` / MCP `schema_read` 的返回体）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaDoc {
    /// 本说明书的格式版本（[`SCHEMA_FORMAT_VERSION`]）。
    pub format_version: u32,
    /// 生成它的 core crate 版本；只用于诊断，不参与兼容判断。
    pub engine_version: String,
    /// 可以独立落盘的根文件（场景 / 预制体）。
    pub roots: Vec<RootSchema>,
    /// 跨类型的约定（必读，很多字段的语义靠它）。
    pub conventions: Vec<String>,
    /// 标量 / 向量等基元在 RON 里的写法。
    pub primitives: Vec<PrimitiveSchema>,
    /// 资产引用字段在磁盘与内存里的两种形态。
    pub asset_references: Vec<AssetReferenceSchema>,
    /// 全部落盘类型。根类型在前，其余按"场景 → 动画 → 物理 → …"分组。
    pub types: Vec<TypeSchema>,
}

impl SchemaDoc {
    /// 按名字查类型（测试与工具用；序列化时仍然只输出 `types` 数组）。
    pub fn type_schema(&self, name: &str) -> Option<&TypeSchema> {
        self.types.iter().find(|ty| ty.name == name)
    }
}

/// 一个可以独立落盘的根文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootSchema {
    /// 文件种类：`scene` / `prefab`。
    pub kind: String,
    /// 文件后缀（含点）。
    pub file_suffix: String,
    /// 根类型名（指向 [`SchemaDoc::types`]）。
    pub root_type: String,
    /// 推荐存放位置。
    pub path_note: String,
    /// 一句话说明。
    pub summary: String,
    /// 一份可解析的最小示例。
    pub example_ron: String,
}

/// 基元类型在 RON 里的写法。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimitiveSchema {
    /// 类型名（字段 `ty` 里出现的叶名）。
    pub name: String,
    /// RON 示例。
    pub ron_example: String,
    /// 语义 / 易错点。
    pub note: String,
}

/// 一个资产引用字段的两种形态。
///
/// 磁盘上的 GUID 由 `ProjectHandle` 的读写边界自动编解码，**不要手工写**；
/// 智能体经 MCP 拿到的场景文本已经是内存形态（项目相对路径）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetReferenceSchema {
    /// 字段路径，例如 `Entity.model`。
    pub field: String,
    /// 磁盘文件里的写法。
    pub disk_format: String,
    /// 内存 / MCP 里的写法。
    pub memory_format: String,
    /// 语义与边界。
    pub note: String,
}

/// 一个落盘类型的完整说明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeSchema {
    /// Rust 类型名，也是字段 `ty` 里引用的名字。
    pub name: String,
    /// 结构体 / 枚举 / newtype。
    pub kind: TypeKind,
    /// 一句话说明。
    pub summary: String,
    /// 整个类型省略（或写成 `()`）时的等价 RON；`None` = 没有固定默认值
    /// （必须写全字段，或省略后的值不是固定字面量，见 `notes`）。
    pub default_ron: Option<String>,
    /// 一份可解析的示例。
    pub example_ron: Option<String>,
    /// 结构体字段（枚举为空）。
    pub fields: Vec<FieldSchema>,
    /// 枚举变体（结构体 / newtype 为空）。
    pub variants: Vec<VariantSchema>,
    /// 语义补充与易错点。
    pub notes: Vec<String>,
}

/// 类型种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeKind {
    /// 普通结构体，RON 形态是 `(field: value, ...)`。
    Struct,
    /// 枚举，RON 形态是 `Variant` 或 `Variant(field: value, ...)`。
    Enum,
    /// 单字段包装 / 类型别名，RON 形态就是内部值本身。
    Newtype,
}

/// 一个结构体字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSchema {
    /// 字段名（RON 里的键）。
    pub name: String,
    /// 类型名或容器写法（`Option<T>` / `Vec<T>` / `[T; N]` / `BTreeMap<K, V>`）。
    pub ty: String,
    /// `true` = RON 里必须出现，省略会导致解析失败。
    pub required: bool,
    /// 省略时的等价 RON；`None` = 必填，或省略后的值不是固定字面量。
    pub default_ron: Option<String>,
    /// 语义 / 单位 / 易错点。
    pub note: String,
}

/// 一个枚举变体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantSchema {
    /// 变体名（RON 里的标签）。
    pub name: String,
    /// 单元 / 结构体 / 单值。
    pub kind: VariantKind,
    /// newtype 变体的载荷类型（其余为 `None`）。
    pub payload: Option<String>,
    /// 结构体变体的字段（其余为空）。
    pub fields: Vec<FieldSchema>,
    /// 一份可解析的 RON 片段（顶层枚举值）。
    pub example_ron: String,
    /// 语义 / 易错点。
    pub note: String,
}

/// 枚举变体种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VariantKind {
    /// `Variant`
    Unit,
    /// `Variant { field: T }`
    Struct,
    /// `Variant(T)`
    Newtype,
}

/// 生成当前引擎版本的场景格式说明。
///
/// 这是 `--export-schema` 与 MCP `schema_read` 的唯一入口；不要在别处再拼一份。
pub fn scene_schema() -> SchemaDoc {
    SchemaDoc {
        format_version: SCHEMA_FORMAT_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").to_string(),
        roots: vec![
            RootSchema {
                kind: "scene".into(),
                file_suffix: ".ron".into(),
                root_type: "Scene".into(),
                path_note: "项目内相对路径，通常放在 Scenes/ 下（清单 entry_scene 指向的也是这种路径）。".into(),
                summary: "一张场景：实体平铺列表 + 场景级灯光 + 环境光。".into(),
                example_ron: pretty(&scene_example()),
            },
            RootSchema {
                kind: "prefab".into(),
                file_suffix: ".prefab.ron".into(),
                root_type: "Prefab".into(),
                path_note: "项目内相对路径，通常放在 Prefabs/ 下；双后缀用来和场景区分。".into(),
                summary: "一份可复用实体子树；与场景同构，但没有 lights / ambient。".into(),
                example_ron: pretty(&prefab_example()),
            },
        ],
        conventions: conventions(),
        primitives: primitives(),
        asset_references: asset_references(),
        types: types(),
    }
}

fn conventions() -> Vec<String> {
    vec![
        "文件是 RON 文本；根结构见 roots。路径一律是项目相对路径（正斜杠），不要写绝对路径。".into(),
        "字段缺省值见类型的 default_ron 与字段的 default_ron；required=true 表示必须写。未知字段会被忽略（不报错），但拼错字段名等于没写。".into(),
        "Entity.id 是场景内稳定标识；parent 与 PrefabInstanceRef.source_id 都按它引用。省略 id 会分配新的进程内 ID（不可复现），手写场景必须显式给唯一 ID。".into(),
        "Transform 是局部空间（相对 parent）；旋转是四元数 (x, y, z, w)，不是欧拉角；矩阵语义是 T * R * S。".into(),
        "可选组件字段省略 = None（实体没有这个组件）；Vec / map 省略 = 空。没有 physics 的实体不参与物理，没有 camera 的实体不是相机。".into(),
        "运行时字段永不落盘：Entity.components、UiElement.state、ParticleEmitter.burst。写进 RON 也会被忽略。".into(),
        "颜色一律线性空间 RGB / RGBA，不是 sRGB；长度单位是米，角度单位是度（字段说明另有标注的除外）。".into(),
        "资产引用在磁盘文件里是 guid://<32 位十六进制>，内存 / MCP 里是项目相对路径；转换只发生在 ProjectHandle 读写边界，不要手工编 GUID。builtin:// 是程序化几何，不是资产。".into(),
        "scripts / behaviors 的列表顺序就是执行顺序；behavior_params 外层键是注册的 Rust 行为名，内层是 Inspect 字段名。".into(),
        "同一个场景内 id 必须唯一；parent 必须指向同一张场景 / 同一份预制体里存在的实体，且不能形成环。".into(),
        "预制体实例的实体带 prefab 字段，通常由引擎维护（Apply / Revert 靠它）；手写场景不要伪造，除非确实要接一个已有预制体。".into(),
    ]
}

fn primitives() -> Vec<PrimitiveSchema> {
    vec![
        primitive("f32", "1.5", "32 位浮点；建议带小数点，和现有场景文件保持一致。"),
        primitive("f64", "1.5", "64 位浮点（目前只出现在 FieldValue::F64）。"),
        primitive("u8", "0", "无符号 8 位整数（目前只用于 Entity.layer：0–31）。"),
        primitive("u32", "512", "无符号 32 位整数。"),
        primitive("i32", "-1", "有符号 32 位整数（FieldValue::I32）。"),
        primitive("i64", "-1", "有符号 64 位整数（FieldValue::I64）。"),
        primitive("u64", "0", "无符号 64 位整数（实体 ID 的底层是 u64；FieldValue 没有 U64 变体）。"),
        primitive("bool", "true", "true / false。"),
        primitive("String", "\"text\"", "双引号字符串；允许 // 行注释。"),
        primitive("Vec2", "(0.0, 0.0)", "固定长度元组，不是 [0.0, 0.0]。"),
        primitive("Vec3", "(0.0, 1.0, 0.0)", "固定长度元组，不是 [0.0, 1.0, 0.0]。"),
        primitive("Vec4", "(1.0, 0.0, 0.0, 1.0)", "固定长度元组；颜色按 RGBA 顺序。"),
        primitive("Quat", "(0.0, 0.0, 0.0, 1.0)", "四元数，分量顺序 x, y, z, w；单位四元数 = 无旋转。"),
    ]
}

fn primitive(name: &str, ron_example: &str, note: &str) -> PrimitiveSchema {
    PrimitiveSchema {
        name: name.into(),
        ron_example: ron_example.into(),
        note: note.into(),
    }
}

fn asset_references() -> Vec<AssetReferenceSchema> {
    vec![
        asset_ref(
            "Entity.model",
            "模型路径：模型资产经 .meta 登记后写成 guid://<32 hex>；builtin://… 与未登记路径保持原样",
            "项目相对路径，例如 Resources/Box.glb；程序化几何用 builtin://cube / builtin://sphere",
            "空串 = 没有模型（纯逻辑 / UI 实体）。同一字段在磁盘与内存里形态不同，唯一转换点是 ProjectHandle::load_scene / write_scene。",
        ),
        asset_ref(
            "Entity.material_asset",
            "guid://<32 hex>",
            "项目相对路径，例如 Resources/Materials/Wood.mat",
            "共享材质资产；None = 用模型自带材质。逐实体微调走 Entity.material（MaterialOverride），两者正交。",
        ),
        asset_ref(
            "Entity.scripts",
            "列表里每一项都是 guid://<32 hex>",
            "项目相对路径，例如 Scripts/Player.lua",
            "只支持 .lua；顺序即执行顺序。",
        ),
        asset_ref(
            "AudioSource.clip",
            "guid://<32 hex>",
            "项目相对路径，例如 Resources/Audio/Beep.wav",
            "音频解码在 xgame-audio；场景文件只存引用。",
        ),
        asset_ref(
            "Entity.prefab.source",
            "guid://<32 hex>",
            "项目相对路径，例如 Prefabs/Enemy.prefab.ron",
            "预制体实例来源；通常由引擎实例化 / Apply / Revert 维护，手写场景不要伪造。",
        ),
        asset_ref(
            "UiImageStyle.path",
            "guid://<32 hex>",
            "项目相对路径，例如 Resources/UI/Icon.png",
            "空串 = 不画图片；图片与模型 / 音频走同一套 GUID 管线。",
        ),
        asset_ref(
            "MaterialAsset.albedo",
            "guid://<32 hex>",
            "项目相对路径，例如 Resources/Textures/Wood.png",
            "空串 = 纯色（1×1 白色），不是保留模型贴图 —— 材质资产是一份完整材质。",
        ),
    ]
}

fn asset_ref(
    field: &str,
    disk_format: &str,
    memory_format: &str,
    note: &str,
) -> AssetReferenceSchema {
    AssetReferenceSchema {
        field: field.into(),
        disk_format: disk_format.into(),
        memory_format: memory_format.into(),
        note: note.into(),
    }
}

// ---------------------------------------------------------------------------
// 类型目录
// ---------------------------------------------------------------------------

fn types() -> Vec<TypeSchema> {
    let mut types = Vec::new();
    types.extend(scene_types());
    types.extend(animation_types());
    types.extend(physics_types());
    types.extend(audio_material_types());
    types.extend(character_camera_types());
    types.extend(ui_types());
    types.extend(particle_types());
    types.extend(inspect_types());
    types
}

// ---------------------------------------------------------------------------
// 构造辅助
// ---------------------------------------------------------------------------

/// 必填字段。
fn req(name: &str, ty: &str, note: &str) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        ty: ty.into(),
        required: true,
        default_ron: None,
        note: note.into(),
    }
}

/// 省略时走 serde 默认值的字段；`default` 必须是**真实的默认值**。
fn def<T: Serialize>(name: &str, ty: &str, default: &T, note: &str) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        ty: ty.into(),
        required: false,
        default_ron: Some(ron_text(default)),
        note: note.into(),
    }
}

/// `Option<T>` 字段：省略 = `None`。
fn opt(name: &str, ty: &str, note: &str) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        ty: format!("Option<{ty}>"),
        required: false,
        default_ron: Some("None".into()),
        note: note.into(),
    }
}

/// 字段可省略，但省略后的值不是固定字面量（例如 `Entity.id` 会分配新 ID）。
fn def_dynamic(name: &str, ty: &str, note: &str) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        ty: ty.into(),
        required: false,
        default_ron: None,
        note: note.into(),
    }
}

fn struct_schema(
    name: &str,
    summary: &str,
    default_ron: Option<String>,
    example_ron: Option<String>,
    fields: Vec<FieldSchema>,
    notes: Vec<&str>,
) -> TypeSchema {
    TypeSchema {
        name: name.into(),
        kind: TypeKind::Struct,
        summary: summary.into(),
        default_ron,
        example_ron,
        fields,
        variants: Vec::new(),
        notes: notes.into_iter().map(str::to_string).collect(),
    }
}

fn enum_schema(
    name: &str,
    summary: &str,
    default_ron: Option<String>,
    example_ron: Option<String>,
    variants: Vec<VariantSchema>,
    notes: Vec<&str>,
) -> TypeSchema {
    TypeSchema {
        name: name.into(),
        kind: TypeKind::Enum,
        summary: summary.into(),
        default_ron,
        example_ron,
        fields: Vec::new(),
        variants,
        notes: notes.into_iter().map(str::to_string).collect(),
    }
}

fn newtype_schema(
    name: &str,
    summary: &str,
    default_ron: Option<String>,
    example_ron: String,
    notes: Vec<&str>,
) -> TypeSchema {
    TypeSchema {
        name: name.into(),
        kind: TypeKind::Newtype,
        summary: summary.into(),
        default_ron,
        example_ron: Some(example_ron),
        fields: Vec::new(),
        variants: Vec::new(),
        notes: notes.into_iter().map(str::to_string).collect(),
    }
}

fn unit_variant(name: &str, note: &str) -> VariantSchema {
    VariantSchema {
        name: name.into(),
        kind: VariantKind::Unit,
        payload: None,
        fields: Vec::new(),
        example_ron: name.into(),
        note: note.into(),
    }
}

fn struct_variant(
    name: &str,
    example_ron: &str,
    fields: Vec<FieldSchema>,
    note: &str,
) -> VariantSchema {
    VariantSchema {
        name: name.into(),
        kind: VariantKind::Struct,
        payload: None,
        fields,
        example_ron: example_ron.into(),
        note: note.into(),
    }
}

fn newtype_variant(name: &str, payload: &str, example_ron: &str, note: &str) -> VariantSchema {
    VariantSchema {
        name: name.into(),
        kind: VariantKind::Newtype,
        payload: Some(payload.into()),
        fields: Vec::new(),
        example_ron: example_ron.into(),
        note: note.into(),
    }
}

/// 紧凑 RON（字段默认值用）。
fn ron_text<T: Serialize + ?Sized>(value: &T) -> String {
    ron::to_string(value).expect("schema 默认值必须可序列化为 RON")
}

/// 多行 RON（示例用，与 `ProjectHandle::write_scene` 同一套 PrettyConfig）。
fn pretty<T: Serialize + ?Sized>(value: &T) -> String {
    ron::ser::to_string_pretty(value, ron::ser::PrettyConfig::default())
        .expect("schema 示例必须可序列化为 RON")
}

/// 类型自带 `Default` 时的默认 RON。
fn default_of<T: Default + Serialize>() -> String {
    ron_text(&T::default())
}

fn entity_example() -> Entity {
    let mut entity = Entity::new("Entity");
    // 固定 ID：默认的 EntityId::new() 会读原子计数器，导出两次就不同了。
    entity.id = EntityId::from_raw(1);
    entity.model = "builtin://cube".into();
    entity
}

fn scene_example() -> Scene {
    let mut scene = Scene::new("Scene");
    scene.entities.push(entity_example());
    scene
}

fn prefab_example() -> Prefab {
    let mut prefab = Prefab::new("Prefab");
    prefab.entities.push(entity_example());
    prefab
}

// ---------------------------------------------------------------------------
// 场景 / 实体
// ---------------------------------------------------------------------------

fn scene_types() -> Vec<TypeSchema> {
    vec![
        struct_schema(
            "Scene",
            "一张场景：实体平铺列表 + 场景级灯光 + 环境光。",
            None,
            Some(pretty(&scene_example())),
            vec![
                req("name", "String", "场景名（显示用，不参与路径解析）。"),
                req(
                    "entities",
                    "Vec<Entity>",
                    "实体平铺列表；层级由每个实体的 parent 表达，不是嵌套结构。",
                ),
                def(
                    "lights",
                    "Vec<Light>",
                    &vec![Light::default()],
                    "省略时补一盏默认方向光（避免旧场景打开全黑）；写 [] 才是真的没有灯。",
                ),
                def(
                    "ambient",
                    "[f32; 3]",
                    &[0.06, 0.06, 0.08],
                    "线性 RGB 环境光；所有表面都能收到的最低亮度。",
                ),
            ],
            vec![
                "旧场景文件缺 lights / ambient 时按默认值补齐；新字段一律走 serde default，保证向后兼容。",
                "保存时只改 GUID 引用，不改字段结构；手写后建议跑 xgame-editor --check 验证。",
            ],
        ),
        struct_schema(
            "Entity",
            "场景里的一个对象：变换 + 模型 + 可选组件。",
            None,
            Some(pretty(&entity_example())),
            vec![
                req("name", "String", "显示名；编辑器按名字寻址，同一场景建议唯一。"),
                def(
                    "transform",
                    "Transform",
                    &Transform::default(),
                    "局部空间变换（相对 parent）；省略 = 单位变换。纯 UI 实体可以整段省略。",
                ),
                def(
                    "layer",
                    "u8",
                    &0u8,
                    "层索引 0..=31（决定与哪些层碰撞 / 触发）；省略 = 0。矩阵在 project.ron 的 layers 节，默认全通。越界值由 --check 报告。",
                ),
                def(
                    "model",
                    "String",
                    &"",
                    "模型路径（资产引用）或 builtin://cube / builtin://sphere 等程序化几何；空串 = 没有模型。",
                ),
                opt(
                    "material_asset",
                    "String",
                    "共享材质资产路径；None = 用模型自带材质。与 material（逐实体覆盖）正交。",
                ),
                def_dynamic(
                    "id",
                    "EntityId",
                    "稳定标识；省略会分配新的进程内 ID（不可复现）。手写场景必须显式给唯一 ID —— parent / source_id 都按它引用。",
                ),
                opt("parent", "EntityId", "父实体 ID；None = 场景根节点。层级按 ID 单向引用，没有内嵌子列表。"),
                opt(
                    "prefab",
                    "PrefabInstanceRef",
                    "预制体实例来源；普通实体省略。通常由引擎维护，手写场景不要伪造。",
                ),
                def(
                    "scripts",
                    "Vec<String>",
                    &Vec::<String>::new(),
                    "Lua 脚本路径列表（资产引用）；顺序即执行顺序。",
                ),
                def(
                    "behaviors",
                    "Vec<String>",
                    &Vec::<String>::new(),
                    "Rust 行为组件的注册名；顺序即执行顺序。",
                ),
                def(
                    "behavior_params",
                    "BTreeMap<String, FieldValues>",
                    &BTreeMap::<String, FieldValues>::new(),
                    "行为字段存盘值：外层键 = 注册名，内层键 = Inspect 字段名（用注册名而不是列表下标，增删行为不会错位）。",
                ),
                opt("physics", "PhysicsBody", "物理属性；None = 不参与物理（纯视觉 / 纯逻辑）。"),
                opt("animator", "Animator", "动画播放器；None = 不播动画（绑定姿态 / 静态网格）。"),
                opt("audio", "AudioSource", "音频源；None = 静默实体。"),
                opt(
                    "material",
                    "MaterialOverride",
                    "逐实体材质覆盖（字段级补丁）；None = 纯用模型 / 材质资产。",
                ),
                opt(
                    "controller",
                    "CharacterController",
                    "角色控制器参数；None = 不做角色移动。与 physics 正交：physics 描述碰撞体，controller 描述怎么移动它。",
                ),
                opt("ui", "UiElement", "屏幕空间 UI 元素；None = 不参与 UI。位置由锚点决定，transform 不参与布局。"),
                opt(
                    "particle",
                    "ParticleEmitter",
                    "粒子发射器（纯声明）；None = 零成本不发射。",
                ),
                opt("camera", "Camera", "相机组件；None = 这个实体不是相机。多相机选择规则见 Camera 的说明。"),
            ],
            vec![
                "components（运行时组件容器，#[serde(skip)]）不是落盘字段，刻意不在目录里。",
                "layer 省略 = 0（Default 层）；默认碰撞矩阵全通，所以老场景不写层时物理行为与引入层之前完全一致。",
                "transform 是局部的；世界变换由 parent 链现算，场景文件不存世界变换。",
                "scripts / behaviors / behavior_params 都按名字寻址：改名等于换目标，改列表顺序只影响执行顺序。",
                "可选组件字段省略 = None；Vec / map 省略 = 空。没有 physics 的实体不会突然被重力拽走。",
            ],
        ),
        struct_schema(
            "Transform",
            "局部空间变换（位置 / 旋转 / 缩放），矩阵语义 T * R * S。",
            Some(default_of::<Transform>()),
            Some(pretty(&Transform::from_translation(Vec3::new(
                0.0, 1.0, 0.0,
            )))),
            vec![
                req("position", "Vec3", "局部位置（米）。"),
                req("rotation", "Quat", "局部旋转，四元数 (x, y, z, w)；不是欧拉角。"),
                req("scale", "Vec3", "局部缩放；1.0 = 原尺寸。"),
            ],
            vec![
                "三个字段都没有 serde default —— 写 Transform 时必须写全；但 Entity.transform 整体可以省略（省略 = Transform::default()）。",
                "相机朝向约定本地方向 +Z；模型正面也是 +Z。",
            ],
        ),
        struct_schema(
            "Light",
            "一盏场景级光源（不属于任何实体，不能跟随 Transform 层级）。",
            Some(default_of::<Light>()),
            Some(pretty(&Light::point(
                "Torch",
                [0.0, 2.0, 0.0],
                [1.0, 0.6, 0.3],
                8.0,
                6.0,
            ))),
            vec![
                req("name", "String", "显示名；编辑器里用来区分多盏灯。"),
                def(
                    "kind",
                    "LightKind",
                    &LightKind::Directional,
                    "方向光 / 点光；点光用 position + range，方向光只用 direction。",
                ),
                def(
                    "direction",
                    "[f32; 3]",
                    &[0.5, -1.0, 0.5],
                    "方向光的**光线传播方向**（从光源射向场景），不是指向光源的方向；点光忽略。",
                ),
                def("position", "[f32; 3]", &[0.0, 0.0, 0.0], "点光的世界坐标；方向光忽略。"),
                def("color", "[f32; 3]", &[1.0, 0.98, 0.95], "线性 RGB 光色。"),
                def("intensity", "f32", &3.0, "强度倍率，直接乘到漫反射与高光项。"),
                def("range", "f32", &10.0, "点光影响半径（世界单位）；方向光忽略。"),
                def(
                    "casts_shadow",
                    "bool",
                    &true,
                    "是否投射阴影；点光阴影是 6 面立方体贴图，默认只在主方向光上开。",
                ),
                def(
                    "shadow_resolution",
                    "u32",
                    &0,
                    "阴影贴图边长（像素）；0 = 用引擎默认值。",
                ),
            ],
            vec![
                "Scene.lights 省略时补一盏 Light::default()；空数组才表示没有灯。",
                "Light 是场景级环境数据，不是 Entity 组件 —— 点光位置只能手填，不能挂层级。",
            ],
        ),
        enum_schema(
            "LightKind",
            "光源类型。",
            Some(ron_text(&LightKind::default())),
            Some("Point".into()),
            vec![
                unit_variant("Directional", "方向光：只有方向没有位置，模拟太阳。"),
                unit_variant("Point", "点光：从一点向外发光，按距离衰减。"),
            ],
            vec!["默认 Directional；方向光忽略 position / range，点光忽略 direction。"],
        ),
        newtype_schema(
            "EntityId",
            "实体的稳定标识符（u64 包装），RON 写作 (1)。",
            None,
            "(1)".into(),
            vec![
                "newtype 序列化是透明的：场景文件里就是 (1)，不是 (0: 1)。",
                "默认值不是固定字面量：EntityId::default() 会分配新的进程内 ID，所以 default_ron = None；手写场景请显式给唯一 ID。",
                "同一场景 / 同一预制体内必须唯一；删除后的 ID 不会复用。",
            ],
        ),
        struct_schema(
            "PrefabInstanceRef",
            "实体与预制体资产之间的对应关系（只有预制体实例才带）。",
            None,
            Some(pretty(&PrefabInstanceRef {
                source: "Prefabs/Enemy.prefab.ron".into(),
                source_id: EntityId::from_raw(1),
                is_root: true,
            })),
            vec![
                req("source", "String", "预制体资产路径（资产引用；磁盘 GUID / 内存项目相对路径）。"),
                req(
                    "source_id",
                    "EntityId",
                    "对应资产文件内部那个实体的 ID；Apply / Revert 靠它精确落位。",
                ),
                req(
                    "is_root",
                    "bool",
                    "本实体是不是实例根；只有根会被整体操作（Apply / Revert / 删除连坐）。",
                ),
            ],
            vec![
                "三个字段都没有默认值 —— 写 prefab 就必须写全。",
                "普通实体省略 prefab 字段即可；手写场景一般不要伪造这个结构。",
            ],
        ),
        struct_schema(
            "Prefab",
            "一份可复用的实体子树模板（.prefab.ron）。",
            None,
            Some(pretty(&prefab_example())),
            vec![
                req("name", "String", "预制体名（显示用，通常取资产文件名）。"),
                req(
                    "entities",
                    "Vec<Entity>",
                    "预制体内的实体；层级同样由 parent 表达。",
                ),
            ],
            vec![
                "与 Scene 同构，但没有 lights / ambient —— 那些属于场景，不属于可复用对象。",
                "实例化时每个实体分配新的场景 ID，并贴上 PrefabInstanceRef；资产里的 ID 只作为 source_id 保留。",
            ],
        ),
    ]
}

// ---------------------------------------------------------------------------
// 动画
// ---------------------------------------------------------------------------

fn animation_types() -> Vec<TypeSchema> {
    let machine = AnimMachine {
        entry: "Idle".into(),
        parameters: BTreeMap::from([("speed".to_string(), 0.0)]),
        states: vec![
            AnimState {
                name: "Idle".into(),
                clip: "Idle".into(),
                ..AnimState::default()
            },
            AnimState {
                name: "Run".into(),
                clip: "Run".into(),
                ..AnimState::default()
            },
        ],
        transitions: vec![
            AnimTransition {
                from: "Idle".into(),
                to: "Run".into(),
                when: AnimCondition::AtLeast {
                    param: "speed".into(),
                    value: 0.5,
                },
            },
            AnimTransition {
                from: "Run".into(),
                to: "Idle".into(),
                when: AnimCondition::Below {
                    param: "speed".into(),
                    value: 0.1,
                },
            },
        ],
    };

    vec![
        struct_schema(
            "AnimState",
            "状态机里的一个状态：绑一个剪辑 + 自己的播放参数。",
            Some(default_of::<AnimState>()),
            Some(pretty(&AnimState {
                name: "Run".into(),
                clip: "Run".into(),
                speed: 1.0,
                looping: true,
            })),
            vec![
                def("name", "String", &"", "状态名；同一机器内唯一，转移表按名字引用。"),
                def("clip", "String", &"", "要播的剪辑名（glTF animations[].name）；空串 = 不播。"),
                def("speed", "f32", &1.0, "本状态的播放速度，覆盖 Animator.speed。"),
                def("looping", "bool", &true, "本状态是否循环，覆盖 Animator.looping。"),
            ],
            vec!["机器模式下 speed / looping 由状态表决定；autoplay / fade_time 仍取 Animator。"],
        ),
        enum_schema(
            "AnimCondition",
            "转移条件：盯一个浮点参数与阈值比大小。",
            None,
            Some("AtLeast(param:\"speed\",value:0.5)".into()),
            vec![
                struct_variant(
                    "AtLeast",
                    "AtLeast(param:\"speed\",value:0.5)",
                    vec![
                        req("param", "String", "参数名（必须在 AnimMachine.parameters 里）。"),
                        req("value", "f32", "阈值；参数 ≥ 阈值时成立。"),
                    ],
                    "参数不存在时条件永不满足（打错名字不会静默当 0.0）。",
                ),
                struct_variant(
                    "Below",
                    "Below(param:\"speed\",value:0.1)",
                    vec![
                        req("param", "String", "参数名。"),
                        req("value", "f32", "阈值；参数 < 阈值时成立（严格小于）。"),
                    ],
                    "与同阈值的 AtLeast 互补；要做滞回就给出两个不同阈值。",
                ),
            ],
            vec!["布尔语义用 0.0 / 1.0 + 阈值表达，没有单独的 bool 参数类型。"],
        ),
        struct_schema(
            "AnimTransition",
            "一条转移：from 状态下条件成立 → 切到 to。",
            None,
            Some(pretty(&AnimTransition {
                from: "Idle".into(),
                to: "Run".into(),
                when: AnimCondition::AtLeast {
                    param: "speed".into(),
                    value: 0.5,
                },
            })),
            vec![
                req("from", "String", "源状态名。"),
                req("to", "String", "目标状态名；目标不存在时这条转移被跳过。"),
                req("when", "AnimCondition", "触发条件。"),
            ],
            vec!["转移按书写顺序求值，第一条满足条件的生效；坏一条不会卡死整台机器。"],
        ),
        struct_schema(
            "AnimMachine",
            "声明式动画状态机：参数表 + 状态表 + 转移表。",
            Some(default_of::<AnimMachine>()),
            Some(pretty(&machine)),
            vec![
                def("entry", "String", &"", "起始状态名；指不到真实状态时整台机器按不可用处理，退回单剪辑模式。"),
                def(
                    "parameters",
                    "BTreeMap<String, f32>",
                    &BTreeMap::<String, f32>::new(),
                    "参数初值表；随场景落盘，保存时的当前值就是下次运行的初值。",
                ),
                def("states", "Vec<AnimState>", &Vec::<AnimState>::new(), "状态表。"),
                def(
                    "transitions",
                    "Vec<AnimTransition>",
                    &Vec::<AnimTransition>::new(),
                    "转移表；按书写顺序求值。",
                ),
            ],
            vec![
                "可用性只看 entry 是否命中一个状态；配置写坏时引擎照常跑，只是不动。",
                "v1 只有浮点参数 + 阈值条件，没有 trigger / 分层转移；过渡时长统一用 Animator.fade_time。",
            ],
        ),
        struct_schema(
            "AnimAdditive",
            "单层叠加（additive）：在基础姿势之上加一层增量。",
            Some(default_of::<AnimAdditive>()),
            Some(pretty(&AnimAdditive {
                clip: "Breath".into(),
                weight: 0.5,
                speed: 1.0,
                looping: true,
            })),
            vec![
                def("clip", "String", &"", "叠加剪辑名；空串 / 模型里没有 = 这层不生效。"),
                def(
                    "weight",
                    "f32",
                    &1.0,
                    "权重 0..=1（引擎侧钳位）；0 = 完全不叠加，1 = 全额增量。",
                ),
                def("speed", "f32", &1.0, "叠加层自己的播放速度。"),
                def("looping", "bool", &true, "叠加层是否循环。"),
            ],
            vec![
                "数学语义：最终姿势 = 基础姿势 + weight × (叠加剪辑当前帧 − 叠加剪辑第 0 帧)。",
                "v1 是全骨架单层；换叠加剪辑是硬切，想平滑就先脚本把 weight 拉到 0。",
            ],
        ),
        enum_schema(
            "AnimLayerMode",
            "混合层模式：加增量（Additive）或整段覆盖（Override）。",
            Some(ron_text(&AnimLayerMode::default())),
            Some("Override".into()),
            vec![
                unit_variant("Additive", "加增量（当前帧 − 第 0 帧参考姿势），不替换基础姿势。"),
                unit_variant("Override", "向该层姿势整段 lerp；配骨骼遮罩就是分区域重定向。"),
            ],
            vec!["默认 Additive：呼吸 / 弹跳 / 晃动这类增量动作挂错模式会变成硬切。"],
        ),
        struct_schema(
            "AnimLayer",
            "混合层栈里的一层：模式 + 剪辑 + 权重 + 独立时钟 + 骨骼遮罩。",
            Some(default_of::<AnimLayer>()),
            Some(pretty(&AnimLayer {
                name: "Aim".into(),
                mode: AnimLayerMode::Override,
                clip: "Aim".into(),
                weight: 0.8,
                speed: 1.0,
                looping: true,
                mask: vec!["Spine1".into()],
            })),
            vec![
                def("name", "String", &"", "层名；脚本按名推权重，空名 / 重名按栈内顺序取第一个。"),
                def("mode", "AnimLayerMode", &AnimLayerMode::Additive, "求值模式。"),
                def("clip", "String", &"", "这层播的剪辑；空串 = 不生效。"),
                def("weight", "f32", &1.0, "权重 0..=1（引擎侧钳位）。"),
                def("speed", "f32", &1.0, "这层自己的播放速度。"),
                def("looping", "bool", &true, "这层是否循环。"),
                def(
                    "mask",
                    "Vec<String>",
                    &Vec::<String>::new(),
                    "骨骼遮罩 = 子树根名单；空名单 = 整副骨架。",
                ),
            ],
            vec![
                "mask 是二值遮罩：节点名等于名单里任一项、或某个祖先等于，这层才作用于它。",
                "求值次序：基础 → Animator.additive → layers[0..]（后层盖前层）。",
            ],
        ),
        struct_schema(
            "Animator",
            "实体上的动画播放器配置（单剪辑 / 状态机 / 叠加层 / 混合层栈）。",
            Some(default_of::<Animator>()),
            Some(pretty(&Animator {
                clip: "Idle".into(),
                ..Animator::default()
            })),
            vec![
                def("clip", "String", &"", "要播的剪辑名；空串 / 找不到 = 绑定姿态。"),
                def("speed", "f32", &1.0, "播放速度倍率；0 = 冻结，负值 v1 停在第 0 帧。"),
                def("looping", "bool", &true, "是否循环；false 播到末尾停在最后一帧。"),
                def("autoplay", "bool", &true, "是否自动开始；false = 静止在第 0 帧。"),
                def(
                    "fade_time",
                    "f32",
                    &0.25,
                    "换剪辑 / 换状态的交叉淡入时长（秒）；0 = 硬切。",
                ),
                opt("machine", "AnimMachine", "状态机；Some 且 entry 可用时进入机器模式，clip / speed / looping 被状态表覆盖。"),
                opt("additive", "AnimAdditive", "单层叠加快捷方式；Some 时叠在基础姿势之上。"),
                def(
                    "layers",
                    "Vec<AnimLayer>",
                    &Vec::<AnimLayer>::new(),
                    "混合层栈；按数组顺序依次合到当前姿势上。",
                ),
            ],
            vec![
                "机器模式与叠加层正交：状态机换基础层，叠加层按自己的时钟独立播放。",
                "autoplay / fade_time 在机器模式下仍然生效；is_active 还要看是否存在速度为正的状态。",
            ],
        ),
    ]
}

// ---------------------------------------------------------------------------
// 物理
// ---------------------------------------------------------------------------

fn physics_types() -> Vec<TypeSchema> {
    vec![
        struct_schema(
            "PhysicsBody",
            "一个实体的物理属性（声明式；rapier 适配在 xgame-physics）。",
            Some(default_of::<PhysicsBody>()),
            Some(pretty(&PhysicsBody::static_box([12.0, 0.5, 12.0]))),
            vec![
                def("kind", "BodyKind", &BodyKind::Dynamic, "运动学类型。"),
                def("shape", "CollisionShape", &CollisionShape::Auto, "碰撞体形状。"),
                def(
                    "mass",
                    "f32",
                    &0.0,
                    "质量（千克）；0 = 按碰撞体密度自动推算。",
                ),
                def("friction", "f32", &0.5, "库仑摩擦（0 无摩擦，1 高摩擦）。"),
                def("restitution", "f32", &0.0, "弹性（0 完全非弹性，1 完全弹性）。"),
                def(
                    "gravity_scale",
                    "f32",
                    &1.0,
                    "重力缩放；1 = 正常，0 = 失重，负数 = 反重力。",
                ),
                def(
                    "is_sensor",
                    "bool",
                    &false,
                    "传感器：检测重叠但不产生接触力（触发器）。",
                ),
            ],
            vec![
                "Entity.physics 省略 = 不参与物理；旧场景因此不会突然被重力拽走。",
                "挂了 controller 的实体会被运行时接管成位置驱动的运动学胶囊，kind 此时不再作数。",
                "Auto 形状用实体缩放绝对值的一半当盒子半边长（视觉外观即碰撞形状）。",
            ],
        ),
        enum_schema(
            "BodyKind",
            "刚体的运动学类型。",
            Some(ron_text(&BodyKind::default())),
            Some("Static".into()),
            vec![
                unit_variant("Static", "不动，无限质量；地面、墙壁、平台。"),
                unit_variant("Dynamic", "全动力学模拟：受重力、力、碰撞影响。"),
                unit_variant(
                    "Kinematic",
                    "由脚本 / 代码直接驱动，本身不受力，但会推动别人；移动平台、电梯。",
                ),
            ],
            vec!["默认 Dynamic：新实体挂上 physics 就会受重力，这是最容易理解的默认。"],
        ),
        enum_schema(
            "CollisionShape",
            "场景可编写的碰撞体形状子集。",
            Some(ron_text(&CollisionShape::default())),
            Some("Cuboid(half_extents:(12.0, 0.5, 12.0))".into()),
            vec![
                struct_variant(
                    "Cuboid",
                    "Cuboid(half_extents:(1.0, 1.0, 0.5))",
                    vec![req(
                        "half_extents",
                        "[f32; 3]",
                        "轴对齐盒体的**半边长**（不是全尺寸）。",
                    )],
                    "最常见的静态地面 / 箱子。",
                ),
                struct_variant(
                    "Ball",
                    "Ball(radius:0.5)",
                    vec![req("radius", "f32", "球半径。")],
                    "球体碰撞。",
                ),
                struct_variant(
                    "Capsule",
                    "Capsule(half_height:0.6,radius:0.3)",
                    vec![
                        req("half_height", "f32", "圆柱段半高（不含两端半球）。"),
                        req("radius", "f32", "半球半径。"),
                    ],
                    "沿 Y 轴的胶囊；角色控制器常用。",
                ),
                struct_variant(
                    "Cylinder",
                    "Cylinder(half_height:1.0,radius:0.5)",
                    vec![
                        req("half_height", "f32", "圆柱半高。"),
                        req("radius", "f32", "圆柱半径。"),
                    ],
                    "沿 Y 轴的圆柱。",
                ),
                unit_variant(
                    "Auto",
                    "按实体变换的缩放自动生成盒子（缩放的绝对值 / 2 作为半边长）。",
                ),
            ],
            vec![
                "默认 Auto：作者摆一个缩放成 4×0.2×4 的方块当平台，不用再手填一遍尺寸。",
                "三角网格 / 高度场由导入器生成，不进场景文件。",
            ],
        ),
    ]
}

// ---------------------------------------------------------------------------
// 音频 / 材质
// ---------------------------------------------------------------------------

fn audio_material_types() -> Vec<TypeSchema> {
    vec![
        struct_schema(
            "AudioSource",
            "实体上的音频源组件：一个可循环、可空间化的音效声明。",
            Some(default_of::<AudioSource>()),
            Some(pretty(&AudioSource::new("Resources/Audio/Beep.wav"))),
            vec![
                def(
                    "clip",
                    "String",
                    &"",
                    "音频文件路径（资产引用）；空串 = 静默。",
                ),
                def("volume", "f32", &1.0, "线性音量；1.0 = 原音量，0.0 = 静音。"),
                def(
                    "pitch",
                    "f32",
                    &1.0,
                    "播放速率；同时改变速度与音高（kira 语义）。",
                ),
                def("looping", "bool", &false, "是否循环。"),
                def("autoplay", "bool", &true, "进场景后自动播放一次。"),
                def(
                    "spatial",
                    "bool",
                    &false,
                    "是否空间化（3D 定位，随实体移动更新发射位置）。",
                ),
                def("min_distance", "f32", &1.0, "空间化：满音量的最近距离。"),
                def("max_distance", "f32", &50.0, "空间化：听不见的最远距离。"),
            ],
            vec![
                "Entity.audio 省略 = 静默实体；autoplay 默认 true 是刻意的（挂上音频源通常就是希望进场景就响）。",
                "引擎侧封装在 xgame-audio（kira）；场景文件从不引用 kira 类型。",
            ],
        ),
        struct_schema(
            "MaterialOverride",
            "逐实体材质覆盖：只写想改的 PBR 字段，None = 用模型 / 材质资产的值。",
            Some(default_of::<MaterialOverride>()),
            Some(pretty(&MaterialOverride {
                base_color: Some([0.8, 0.25, 0.2, 1.0]),
                metallic: None,
                roughness: Some(0.6),
            })),
            vec![
                opt(
                    "base_color",
                    "[f32; 4]",
                    "线性 RGBA **乘子**，与模型自带基础色相乘；None = 不改。",
                ),
                opt("metallic", "f32", "金属度覆盖（0 = 绝缘体，1 = 金属）；None = 用模型值。"),
                opt(
                    "roughness",
                    "f32",
                    "粗糙度覆盖（0 = 镜面，1 = 完全漫反射）；None = 用模型值。",
                ),
            ],
            vec![
                "空覆盖 () 合法：全 None = 什么都不改。",
                "与 material_asset 正交：资产负责「这是什么材质」，覆盖负责「这个实例想怎么微调」。",
            ],
        ),
        struct_schema(
            "MaterialAsset",
            "一份可复用的材质资产（.mat 文件）；引用了它就是一份完整材质。",
            Some(default_of::<MaterialAsset>()),
            Some(pretty(&MaterialAsset {
                base_color: [0.62, 0.45, 0.28, 1.0],
                metallic: 0.0,
                roughness: 0.55,
                albedo: "Resources/Textures/Wood.png".into(),
            })),
            vec![
                def("base_color", "[f32; 4]", &[1.0, 1.0, 1.0, 1.0], "线性 RGBA **乘子**；默认白色 = 不改模型原色。"),
                def("metallic", "f32", &0.0, "金属度（替换模型的对应参数）。"),
                def("roughness", "f32", &0.45, "粗糙度（替换模型的对应参数）。"),
                def(
                    "albedo",
                    "String",
                    &"",
                    "漫反射贴图路径（资产引用）；空串 = 纯色（1×1 白色），不是保留模型贴图。",
                ),
            ],
            vec![
                "MaterialAsset 不是 Entity 的字段；Entity.material_asset 存的是它的路径。",
                "优先级：模型自带 → 材质资产（替换 metallic / roughness，base_color 相乘，albedo 替换贴图）→ MaterialOverride。",
                "渲染侧使用的 MIN_ROUGHNESS = 0.04；存盘保持用户原值。",
            ],
        ),
    ]
}

// ---------------------------------------------------------------------------
// 角色 / 相机
// ---------------------------------------------------------------------------

fn character_camera_types() -> Vec<TypeSchema> {
    vec![
        struct_schema(
            "CharacterController",
            "角色控制器参数（怎么移动碰撞体，不是碰撞体本身）。",
            Some(default_of::<CharacterController>()),
            Some(pretty(&CharacterController {
                radius: 0.35,
                height: 1.75,
                ..CharacterController::default()
            })),
            vec![
                def("radius", "f32", &0.3, "胶囊体半径（米）。"),
                def(
                    "height",
                    "f32",
                    &1.8,
                    "角色总高度（米，含两端半球）；胶囊半高由 (height − 2·radius)/2 推导，不单独存。",
                ),
                def("step_offset", "f32", &0.35, "自动台阶高度（米）；0 = 不上任何台阶。"),
                def("slope_limit", "f32", &45.0, "可攀爬的最大坡度（**度**）；0 = 任何斜面都上不去。"),
                def(
                    "skin_width",
                    "f32",
                    &0.02,
                    "皮肤宽度（米）：胶囊与障碍物保持的微小间隙，避免贴墙抖动。",
                ),
                def(
                    "snap_to_ground",
                    "f32",
                    &0.3,
                    "贴地吸附距离（米）；0 = 不吸附（走下台阶会腾空）。",
                ),
            ],
            vec![
                "与 physics 正交：physics 描述碰撞体，controller 描述怎么移动它；挂了 controller 后刚体被接管成位置驱动。",
                "Entity.transform.position 是角色**脚底**（Unity 同款），胶囊中心由 center_height() 现算。",
                "加载后按 sanitized() 钳制；手写极端值不会让物理世界吃进 NaN。",
            ],
        ),
        struct_schema(
            "Camera",
            "透视相机参数 + 主相机标记；挂在实体上就表示「这台相机在这份场景里的机位」。",
            Some(default_of::<Camera>()),
            Some(pretty(&Camera::default())),
            vec![
                def("fov", "f32", &60.0, "垂直视场角（**度**）；水平视角由宽高比推出。"),
                def("near", "f32", &0.1, "近裁剪面（米）。"),
                def("far", "f32", &1000.0, "远裁剪面（米）。"),
                def(
                    "active",
                    "bool",
                    &true,
                    "是否作为主相机候选；多机位切换时脚本只翻这一个开关。",
                ),
            ],
            vec![
                "宽高比不在组件里 —— 它属于视口，不属于场景。",
                "选择规则：第一个 active=true 的相机；没有 active 就退回第一个相机；一台都没有 = None。",
                "朝向约定：本地方向 +Z 是镜头朝向（rotation 为单位四元数时看向世界 +Z）。",
                "只有透视投影；sanitized() 会把 fov 钳到 1..179°、near ≥ 0.001、far ≥ near + 0.001。",
            ],
        ),
    ]
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// `UiElement` 的**反序列化默认值**（不是 `UiElement::default()` 的面板预设）。
///
/// `#[serde(default)]` 在字段级时，缺字段用字段类型自己的 Default：
/// style 是透明样式、interactable 是 true。构造器默认值（面板）是另一回事，
/// schema 必须描述"省略字段后真正得到什么"，否则智能体会写出看不见的按钮。
fn ui_element_serde_default() -> UiElement {
    UiElement {
        style: UiStyle::default(),
        text: UiTextStyle::default(),
        interactable: true,
        ..UiElement::default()
    }
}

fn ui_types() -> Vec<TypeSchema> {
    vec![
        struct_schema(
            "UiElement",
            "屏幕空间 UI 元素（锚点布局）；位置由锚点决定，transform 不参与布局。",
            Some(ron_text(&ui_element_serde_default())),
            Some(pretty(&UiElement {
                kind: UiKind::Button,
                offset_max: [180.0, 44.0],
                style: UiStyle::panel(),
                text: UiTextStyle {
                    content: "Play".into(),
                    size: 18.0,
                    ..UiTextStyle::default()
                },
                interactable: true,
                ..UiElement::default()
            })),
            vec![
                def("kind", "UiKind", &UiKind::Panel, "控件类型；只影响 Inspector 默认值与语义，渲染按字段独立判断。"),
                def("anchor_min", "[f32; 2]", &[0.0, 0.0], "锚点左下（父矩形的归一化坐标，0..1 之外合法）。"),
                def("anchor_max", "[f32; 2]", &[0.0, 0.0], "锚点右上。"),
                def("offset_min", "[f32; 2]", &[0.0, 0.0], "相对 anchor_min 的像素偏移。"),
                def(
                    "offset_max",
                    "[f32; 2]",
                    &[200.0, 100.0],
                    "相对 anchor_max 的像素偏移；offset_max = offset_min + size 即固定尺寸。",
                ),
                def(
                    "style",
                    "UiStyle",
                    &UiStyle::default(),
                    "背景 / 边框 / 圆角 / 内边距；默认透明，省略不写。",
                ),
                def(
                    "text",
                    "UiTextStyle",
                    &UiTextStyle::default(),
                    "文本样式与内容；默认样式省略不写。",
                ),
                opt("image", "UiImageStyle", "图片；None = 不画图片。"),
                def("visible", "bool", &true, "自身是否可见；隐藏时子树一并隐藏。"),
                def(
                    "interactable",
                    "bool",
                    &true,
                    "是否接受指针事件；false = 纯装饰（点击穿透）。注意：省略时的 serde 默认是 true，不是 UiElement::default() 面板预设的 false。",
                ),
            ],
            vec![
                "绘制顺序 = 实体层级的深度优先先序遍历（同层按场景顺序），后面的盖住前面的；没有 z 序字段。",
                "父矩形来自最近的、同样挂了 UiElement 的祖先；没有这样的祖先时 = 整个视口。",
                "state（悬停 / 按下 / 本帧点击，#[serde(skip)]）是运行时字段，不落盘。",
                "默认构造 UiElement::default() 是面板预设（interactable=false）；但**省略字段反序列化**得到的是透明样式 + interactable=true。",
            ],
        ),
        struct_schema(
            "UiStyle",
            "背景与边框样式（线性颜色，像素尺寸）。",
            Some(default_of::<UiStyle>()),
            Some(pretty(&UiStyle::panel())),
            vec![
                def("background", "[f32; 4]", &[0.0, 0.0, 0.0, 0.0], "线性 RGBA 背景色；a = 0 = 不画背景。"),
                def(
                    "border_color",
                    "[f32; 4]",
                    &UiStyle::default().border_color,
                    "线性 RGBA 边框色；border_width = 0 时无效。",
                ),
                def("border_width", "f32", &0.0, "边框宽度（像素，画在矩形内侧）。"),
                def("corner_radius", "f32", &4.0, "圆角半径（像素）；SDF 抗锯齿，0 = 直角。"),
                def("padding", "f32", &0.0, "内边距（像素）：文本与图片在内容矩形内再缩进这么多。"),
            ],
            vec![
                "默认样式 = 无背景、无边框、无内边距（所以场景文件里常常整段省略 style）。",
                "面板 / 按钮的底色由 UiStyle::panel() 显式给出，不用默认值。",
            ],
        ),
        struct_schema(
            "UiTextStyle",
            "文本样式与内容。",
            Some(default_of::<UiTextStyle>()),
            Some(pretty(&UiTextStyle {
                content: "Hello".into(),
                size: 24.0,
                ..UiTextStyle::default()
            })),
            vec![
                def("content", "String", &"", "文本内容；支持 \\n 硬换行。"),
                def("size", "f32", &20.0, "字号（像素）。"),
                def("color", "[f32; 4]", &[1.0, 1.0, 1.0, 1.0], "线性 RGBA 颜色。"),
                def("align", "UiAlign", &UiAlign::Left, "水平对齐。"),
                def("valign", "UiVAlign", &UiVAlign::Middle, "垂直对齐。"),
                def("wrap", "bool", &true, "超出宽度时自动换行；false = 单行溢出（不裁剪）。"),
                def("bold", "bool", &false, "使用粗体字重；系统粗体缺失时静默退回常规字重。"),
            ],
            vec!["默认样式省略不写；只写 kind: Text + text.content 也能显示。"],
        ),
        struct_schema(
            "UiImageStyle",
            "图片元素。",
            Some(default_of::<UiImageStyle>()),
            Some(pretty(&UiImageStyle {
                path: "Resources/UI/Icon.png".into(),
                fill: 0.75,
                ..UiImageStyle::default()
            })),
            vec![
                def("path", "String", &"", "图片路径（资产引用）；空串 = 不画。"),
                def("tint", "[f32; 4]", &[1.0, 1.0, 1.0, 1.0], "着色乘子；白色 = 原图。"),
                def("fill", "f32", &1.0, "填充比例 0..=1：从左往右裁剪，做血条 / 进度条。"),
                def("keep_aspect", "bool", &true, "保持宽高比（按内容矩形做最大内接，居中）。"),
            ],
            vec!["fill 在加载 / Inspector 边界被钳到 0..=1。"],
        ),
        enum_schema(
            "UiKind",
            "控件类型（只影响语义与 Inspector 默认值）。",
            Some(ron_text(&UiKind::default())),
            Some("Button".into()),
            vec![
                unit_variant("Panel", "纯色 / 圆角矩形块。"),
                unit_variant("Text", "文本。"),
                unit_variant("Image", "图片。"),
                unit_variant("Button", "按钮（背景 + 居中文案），默认可交互。"),
            ],
            vec!["渲染没有类型分支：一个 Panel 填了 text 也会画字，组合控件（血条 = 图片 + 填充）不用新增类型。"],
        ),
        enum_schema(
            "UiAlign",
            "水平对齐。",
            Some(ron_text(&UiAlign::default())),
            Some("Center".into()),
            vec![
                unit_variant("Left", "左对齐。"),
                unit_variant("Center", "居中。"),
                unit_variant("Right", "右对齐。"),
            ],
            vec![],
        ),
        enum_schema(
            "UiVAlign",
            "垂直对齐。",
            Some(ron_text(&UiVAlign::default())),
            Some("Middle".into()),
            vec![
                unit_variant("Top", "顶部。"),
                unit_variant("Middle", "居中（默认：一行文案在按钮里看起来最自然）。"),
                unit_variant("Bottom", "底部。"),
            ],
            vec![],
        ),
    ]
}

// ---------------------------------------------------------------------------
// 粒子
// ---------------------------------------------------------------------------

fn particle_types() -> Vec<TypeSchema> {
    vec![
        struct_schema(
            "ParticleEmitter",
            "粒子发射器：纯声明（模拟在 xgame-particle，绘制在 xgame-render）。",
            Some(default_of::<ParticleEmitter>()),
            Some(pretty(&ParticleEmitter::smoke())),
            vec![
                def("emitting", "bool", &true, "是否持续发射；关掉后已出生的粒子照常走完寿命。"),
                def("rate", "f32", &60.0, "每秒发射数；0 = 只有 burst 才出粒子。"),
                def("max_particles", "u32", &512, "同时存在的粒子上限（硬顶，满了不再生成）。"),
                def("duration", "f32", &0.0, "发射时长（秒）；<= 0 = 无限。"),
                def("looping", "bool", &true, "到时后是否重来（只在 duration > 0 时有意义）。"),
                def("lifetime", "[f32; 2]", &[0.6, 1.2], "寿命区间 [最短, 最长]（秒）。"),
                def("speed", "[f32; 2]", &[1.0, 2.2], "初速度区间 [最慢, 最快]（世界单位/秒）。"),
                def("direction", "[f32; 3]", &[0.0, 1.0, 0.0], "发射方向（实体局部空间，会随实体朝向转）。"),
                def("spread", "f32", &18.0, "方向锥半角（度）；0 = 笔直一条线，180 = 全向球。"),
                def(
                    "shape",
                    "EmissionShape",
                    &EmissionShape::Sphere { radius: 0.15 },
                    "发射体积（出生位置分布）；与 direction / spread 正交。",
                ),
                def("gravity", "[f32; 3]", &[0.0, -1.2, 0.0], "粒子自身受到的重力（世界空间），与物理世界无关。"),
                def("drag", "f32", &0.6, "线性阻尼（每秒衰减比例，0 = 无阻尼）。"),
                def("start_size", "f32", &0.16, "出生尺寸（世界单位，四边形半边长）。"),
                def("end_size", "f32", &0.03, "死亡尺寸。"),
                def("start_color", "[f32; 4]", &[1.0, 0.55, 0.12, 0.9], "出生颜色（线性 RGBA，允许 > 1 配合 Bloom）。"),
                def("end_color", "[f32; 4]", &[0.35, 0.05, 0.0, 0.0], "死亡颜色。"),
                def("blend", "ParticleBlend", &ParticleBlend::Additive, "混合方式。"),
                def("seed", "u64", &0, "随机种子；同种子 + 同 dt 序列 ⇒ 逐位相同（无头截图依赖这条）。"),
            ],
            vec![
                "burst（运行时 burst 请求，#[serde(skip)]）不是落盘字段；脚本用 particle_burst 之类的 API 触发。",
                "默认值是一份能直接看见的火焰；挂上组件不改字段就应当有东西。",
                "粒子出生即独立（世界空间），实体移动 / 消亡不追着粒子跑；发射位置每帧按实体当前世界变换取。",
            ],
        ),
        enum_schema(
            "EmissionShape",
            "发射体积：出生位置在实体局部空间里的分布。",
            Some(ron_text(&EmissionShape::default())),
            Some("Sphere(radius:0.25)".into()),
            vec![
                unit_variant("Point", "一个点（实体原点）；最便宜的形态。"),
                struct_variant(
                    "Sphere",
                    "Sphere(radius:0.25)",
                    vec![req("radius", "f32", "球半径（世界单位）。")],
                    "实心球内均匀分布。",
                ),
                struct_variant(
                    "Box",
                    "Box(half_extents:(0.5, 0.2, 0.5))",
                    vec![req("half_extents", "[f32; 3]", "三个半边长。")],
                    "实心盒内均匀分布。",
                ),
            ],
            vec!["形状只管「从哪儿冒出来」；方向由 direction / spread 决定。"],
        ),
        enum_schema(
            "ParticleBlend",
            "粒子混合方式。",
            Some(ron_text(&ParticleBlend::default())),
            Some("Additive".into()),
            vec![
                unit_variant("Alpha", "直通 alpha 混合：烟雾、尘土、落叶。"),
                unit_variant("Additive", "加色混合：火焰、火花、能量；亮部叠起来自然过曝。"),
            ],
            vec!["默认 Alpha；加色模式挂错会把画面点成过曝（默认发射器自己显式用了 Additive）。"],
        ),
    ]
}

// ---------------------------------------------------------------------------
// 行为参数
// ---------------------------------------------------------------------------

fn inspect_types() -> Vec<TypeSchema> {
    let mut values = BTreeMap::new();
    values.insert("speed".to_string(), FieldValue::F32(5.0));

    vec![
        enum_schema(
            "FieldValue",
            "组件字段的运行期值（也是 behavior_params 的值类型）。",
            None,
            Some("F32(5.0)".into()),
            vec![
                newtype_variant("F32", "f32", "F32(5.0)", "32 位浮点。"),
                newtype_variant("F64", "f64", "F64(5.0)", "64 位浮点。"),
                newtype_variant("I32", "i32", "I32(-1)", "有符号 32 位整数。"),
                newtype_variant("I64", "i64", "I64(-1)", "有符号 64 位整数。"),
                newtype_variant("U32", "u32", "U32(1)", "无符号 32 位整数。"),
                newtype_variant("Bool", "bool", "Bool(true)", "布尔值。"),
                newtype_variant("String", "String", "String(\"text\")", "字符串。"),
                newtype_variant("Vec2", "Vec2", "Vec2((0.0, 0.0))", "二维向量。"),
                newtype_variant("Vec3", "Vec3", "Vec3((0.0, 1.0, 0.0))", "三维向量。"),
                newtype_variant("Vec4", "Vec4", "Vec4((1.0, 1.0, 1.0, 1.0))", "四维向量 / 颜色。"),
            ],
            vec!["带标签形式：变体名是磁盘格式的一部分，改名会破坏已有场景文件。"],
        ),
        newtype_schema(
            "FieldValues",
            "一组「Inspect 字段名 → 值」；类型别名，RON 形态就是内部 map。",
            Some(ron_text(&BTreeMap::<String, FieldValue>::new())),
            ron_text(&values),
            vec![
                "键是 Inspect 表里的寻址名（不是显示名）；改名等于换字段。",
                "BTreeMap 保证同一份参数每次序列化逐字节相同，场景可 diff。",
            ],
        ),
    ]
}

// ---------------------------------------------------------------------------
// 测试：目录的完整性 / 示例可解析 / 与 Rust 定义不漂移
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;

    use super::*;

    /// 字段 `ty` 里允许直接出现的叶类型。
    const PRIMITIVES: &[&str] = &[
        "f32", "f64", "u8", "u32", "i32", "i64", "u64", "bool", "String", "Vec2", "Vec3", "Vec4",
        "Quat",
    ];

    fn parse<T: DeserializeOwned>(text: &str) -> T {
        ron::from_str(text).unwrap_or_else(|e| panic!("RON 解析失败: {e}\n{text}"))
    }

    fn schema_of<'a>(doc: &'a SchemaDoc, name: &str) -> &'a TypeSchema {
        doc.type_schema(name)
            .unwrap_or_else(|| panic!("目录里没有类型 {name}"))
    }

    fn default_of<'a>(doc: &'a SchemaDoc, name: &str) -> &'a str {
        schema_of(doc, name)
            .default_ron
            .as_deref()
            .unwrap_or_else(|| panic!("{name} 没有 default_ron"))
    }

    fn example_of<'a>(doc: &'a SchemaDoc, name: &str) -> &'a str {
        schema_of(doc, name)
            .example_ron
            .as_deref()
            .unwrap_or_else(|| panic!("{name} 没有 example_ron"))
    }

    fn field_names<'a>(doc: &'a SchemaDoc, name: &str) -> Vec<&'a str> {
        schema_of(doc, name)
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect()
    }

    fn assert_fields(doc: &SchemaDoc, name: &str, expected: &[&str]) {
        assert_eq!(
            field_names(doc, name),
            expected,
            "{name} 的字段目录与 Rust 定义不一致"
        );
        let mut seen = std::collections::BTreeSet::new();
        for field in &schema_of(doc, name).fields {
            assert!(
                seen.insert(field.name.clone()),
                "{name} 的字段 {} 重复",
                field.name
            );
        }
    }

    fn assert_variants(doc: &SchemaDoc, name: &str, expected: &[&str]) {
        let got: Vec<&str> = schema_of(doc, name)
            .variants
            .iter()
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(got, expected, "{name} 的变体目录与 Rust 定义不一致");
        let mut seen = std::collections::BTreeSet::new();
        for variant in &schema_of(doc, name).variants {
            assert!(
                seen.insert(variant.name.clone()),
                "{name} 的变体 {} 重复",
                variant.name
            );
        }
    }

    fn assert_default_round_trip<T>(doc: &SchemaDoc, name: &str)
    where
        T: Default + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let parsed: T = parse(default_of(doc, name));
        assert_eq!(parsed, T::default(), "{name} 的 default_ron 与 Default 不一致");
    }

    fn assert_variant_examples<Enum, F>(doc: &SchemaDoc, name: &str, variant_name: F)
    where
        Enum: DeserializeOwned,
        F: Fn(&Enum) -> &'static str,
    {
        for variant in &schema_of(doc, name).variants {
            let parsed: Enum = parse(&variant.example_ron);
            assert_eq!(
                variant_name(&parsed),
                variant.name,
                "{name}.{} 的示例解析成了别的变体: {}",
                variant.name,
                variant.example_ron
            );
        }
    }

    fn split_top_level(text: &str) -> Vec<&str> {
        let mut depth = 0usize;
        let mut start = 0usize;
        let mut out = Vec::new();
        for (index, ch) in text.char_indices() {
            match ch {
                '<' | '[' | '(' => depth += 1,
                '>' | ']' | ')' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    out.push(&text[start..index]);
                    start = index + 1;
                }
                _ => {}
            }
        }
        out.push(&text[start..]);
        out
    }

    fn check_type_ref(doc: &SchemaDoc, raw: &str, owner: &str) {
        let ty = raw.trim();
        if let Some(inner) = ty.strip_prefix("Option<").and_then(|s| s.strip_suffix('>')) {
            check_type_ref(doc, inner, owner);
            return;
        }
        if let Some(inner) = ty.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
            check_type_ref(doc, inner, owner);
            return;
        }
        if let Some(inner) = ty
            .strip_prefix("BTreeMap<")
            .and_then(|s| s.strip_suffix('>'))
        {
            let parts = split_top_level(inner);
            assert_eq!(parts.len(), 2, "{owner} 的 map 类型写法不合法: {raw}");
            for part in parts {
                check_type_ref(doc, part, owner);
            }
            return;
        }
        if let Some(inner) = ty.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let (element, count) = inner
                .split_once(';')
                .unwrap_or_else(|| panic!("{owner} 的数组类型缺少长度: {raw}"));
            count
                .trim()
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("{owner} 的数组长度不是整数: {raw}"));
            check_type_ref(doc, element, owner);
            return;
        }
        assert!(
            doc.type_schema(ty).is_some() || PRIMITIVES.contains(&ty),
            "{owner} 引用了未知类型 {ty}"
        );
    }

    fn assert_type_refs_resolve(doc: &SchemaDoc) {
        for ty in &doc.types {
            for field in &ty.fields {
                check_type_ref(doc, &field.ty, &format!("{}.{}", ty.name, field.name));
            }
            for variant in &ty.variants {
                if let Some(payload) = &variant.payload {
                    check_type_ref(doc, payload, &format!("{}.{}", ty.name, variant.name));
                }
                for field in &variant.fields {
                    check_type_ref(
                        doc,
                        &field.ty,
                        &format!("{}.{}.{}", ty.name, variant.name, field.name),
                    );
                }
            }
        }
    }

    #[test]
    fn catalog_is_deterministic_and_well_formed() {
        let doc = scene_schema();
        assert_eq!(doc, scene_schema(), "同一份代码导出两次必须完全相同");
        assert_eq!(doc.format_version, SCHEMA_FORMAT_VERSION);
        assert_eq!(doc.format_version, 1);
        assert_eq!(doc.engine_version, env!("CARGO_PKG_VERSION"));

        let expected = [
            "Scene",
            "Entity",
            "Transform",
            "Light",
            "LightKind",
            "EntityId",
            "PrefabInstanceRef",
            "Prefab",
            "AnimState",
            "AnimCondition",
            "AnimTransition",
            "AnimMachine",
            "AnimAdditive",
            "AnimLayerMode",
            "AnimLayer",
            "Animator",
            "PhysicsBody",
            "BodyKind",
            "CollisionShape",
            "AudioSource",
            "MaterialOverride",
            "MaterialAsset",
            "CharacterController",
            "Camera",
            "UiElement",
            "UiStyle",
            "UiTextStyle",
            "UiImageStyle",
            "UiKind",
            "UiAlign",
            "UiVAlign",
            "ParticleEmitter",
            "EmissionShape",
            "ParticleBlend",
            "FieldValue",
            "FieldValues",
        ];
        let got: Vec<&str> = doc.types.iter().map(|ty| ty.name.as_str()).collect();
        assert_eq!(got, expected, "类型目录少了 / 多了 / 顺序变了");

        for ty in &doc.types {
            assert!(!ty.summary.is_empty(), "{} 缺 summary", ty.name);
            assert!(
                ty.example_ron.is_some(),
                "{} 必须有可解析示例",
                ty.name
            );
            match ty.kind {
                TypeKind::Struct => {
                    assert!(!ty.fields.is_empty(), "{} 是结构体但没有字段", ty.name);
                    assert!(ty.variants.is_empty(), "{} 是结构体但有变体", ty.name);
                }
                TypeKind::Enum => {
                    assert!(ty.fields.is_empty(), "{} 是枚举但有字段", ty.name);
                    assert!(!ty.variants.is_empty(), "{} 是枚举但没有变体", ty.name);
                }
                TypeKind::Newtype => {
                    assert!(ty.fields.is_empty(), "{} 是 newtype 但有字段", ty.name);
                    assert!(ty.variants.is_empty(), "{} 是 newtype 但有变体", ty.name);
                }
            }
        }

        assert_eq!(doc.primitives.len(), PRIMITIVES.len());
        let primitive_names: Vec<&str> = doc.primitives.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(primitive_names, PRIMITIVES, "基元表与校验表不一致");

        assert_type_refs_resolve(&doc);
    }

    #[test]
    fn roots_and_asset_reference_contract_are_complete() {
        let doc = scene_schema();
        assert_eq!(doc.roots.len(), 2);
        assert_eq!(doc.roots[0].kind, "scene");
        assert_eq!(doc.roots[0].root_type, "Scene");
        assert_eq!(doc.roots[0].file_suffix, ".ron");
        assert_eq!(doc.roots[1].kind, "prefab");
        assert_eq!(doc.roots[1].root_type, "Prefab");
        assert_eq!(doc.roots[1].file_suffix, ".prefab.ron");

        // 根文件的示例也必须能直接解析成根类型（智能体复制的就是这两段）。
        let scene: Scene = parse(&doc.roots[0].example_ron);
        assert_eq!(scene.name, "Scene");
        let prefab: Prefab = parse(&doc.roots[1].example_ron);
        assert_eq!(prefab.name, "Prefab");

        let fields: Vec<&str> = doc
            .asset_references
            .iter()
            .map(|r| r.field.as_str())
            .collect();
        assert_eq!(
            fields,
            [
                "Entity.model",
                "Entity.material_asset",
                "Entity.scripts",
                "AudioSource.clip",
                "Entity.prefab.source",
                "UiImageStyle.path",
                "MaterialAsset.albedo",
            ],
            "资产引用表漏了字段，或顺序漂移"
        );
        for reference in &doc.asset_references {
            assert!(
                reference.disk_format.contains("guid://"),
                "{} 的磁盘形态必须说明 GUID",
                reference.field
            );
            assert!(
                !reference.memory_format.is_empty() && !reference.note.is_empty(),
                "{} 的说明不完整",
                reference.field
            );
        }
    }

    #[test]
    fn runtime_only_fields_never_leak_into_the_catalog() {
        let doc = scene_schema();
        for (type_name, runtime_field) in [
            ("Entity", "components"),
            ("UiElement", "state"),
            ("ParticleEmitter", "burst"),
        ] {
            assert!(
                !field_names(&doc, type_name).contains(&runtime_field),
                "{type_name}.{runtime_field} 是运行时字段，不应出现在目录里"
            );
        }
        for ty in &doc.types {
            for field in &ty.fields {
                assert!(
                    !matches!(field.name.as_str(), "components" | "state" | "burst"),
                    "{}.{} 看起来是运行时字段",
                    ty.name,
                    field.name
                );
            }
        }
    }

    #[test]
    fn defaults_round_trip_and_ui_element_uses_serde_defaults() {
        let doc = scene_schema();

        assert_default_round_trip::<Transform>(&doc, "Transform");
        assert_default_round_trip::<Light>(&doc, "Light");
        assert_default_round_trip::<AnimState>(&doc, "AnimState");
        assert_default_round_trip::<AnimMachine>(&doc, "AnimMachine");
        assert_default_round_trip::<AnimAdditive>(&doc, "AnimAdditive");
        assert_default_round_trip::<AnimLayer>(&doc, "AnimLayer");
        assert_default_round_trip::<Animator>(&doc, "Animator");
        assert_default_round_trip::<PhysicsBody>(&doc, "PhysicsBody");
        assert_default_round_trip::<AudioSource>(&doc, "AudioSource");
        assert_default_round_trip::<MaterialOverride>(&doc, "MaterialOverride");
        assert_default_round_trip::<MaterialAsset>(&doc, "MaterialAsset");
        assert_default_round_trip::<CharacterController>(&doc, "CharacterController");
        assert_default_round_trip::<Camera>(&doc, "Camera");
        assert_default_round_trip::<UiStyle>(&doc, "UiStyle");
        assert_default_round_trip::<UiTextStyle>(&doc, "UiTextStyle");
        assert_default_round_trip::<UiImageStyle>(&doc, "UiImageStyle");
        assert_default_round_trip::<ParticleEmitter>(&doc, "ParticleEmitter");
        assert_default_round_trip::<FieldValues>(&doc, "FieldValues");
        assert_default_round_trip::<LightKind>(&doc, "LightKind");
        assert_default_round_trip::<BodyKind>(&doc, "BodyKind");
        assert_default_round_trip::<CollisionShape>(&doc, "CollisionShape");
        assert_default_round_trip::<AnimLayerMode>(&doc, "AnimLayerMode");
        assert_default_round_trip::<UiKind>(&doc, "UiKind");
        assert_default_round_trip::<UiAlign>(&doc, "UiAlign");
        assert_default_round_trip::<UiVAlign>(&doc, "UiVAlign");
        assert_default_round_trip::<EmissionShape>(&doc, "EmissionShape");
        assert_default_round_trip::<ParticleBlend>(&doc, "ParticleBlend");

        // 没有固定默认值的类型：EntityId 省略会分配新 ID；其余没有 Default。
        for name in [
            "Scene",
            "Entity",
            "EntityId",
            "PrefabInstanceRef",
            "Prefab",
            "AnimCondition",
            "AnimTransition",
            "FieldValue",
        ] {
            assert!(
                schema_of(&doc, name).default_ron.is_none(),
                "{name} 不该声称有固定默认值"
            );
        }

        // UiElement 的坑：Default::default() 是面板预设（interactable=false），
        // 但省略字段反序列化得到的是字段级默认值（透明样式 + interactable=true）。
        let effective: UiElement = parse(default_of(&doc, "UiElement"));
        assert_eq!(effective.style, UiStyle::default());
        assert!(effective.interactable);
        assert_eq!(effective.offset_max, [200.0, 100.0]);
        assert!(
            !UiElement::default().interactable,
            "构造器预设仍是面板语义；如果这条变了，schema 的说明要同步"
        );
        let empty: UiElement = parse("()");
        assert_eq!(empty, effective, "default_ron 必须等于省略全部字段的结果");
    }

    #[test]
    fn examples_round_trip_into_real_types() {
        let doc = scene_schema();

        let scene: Scene = parse(example_of(&doc, "Scene"));
        assert_eq!(scene.name, "Scene");
        assert_eq!(scene.entities.len(), 1);
        assert_eq!(scene.entities[0].id, EntityId::from_raw(1));
        assert_eq!(scene.lights, vec![Light::default()]);

        let prefab: Prefab = parse(example_of(&doc, "Prefab"));
        assert_eq!(prefab.name, "Prefab");
        assert_eq!(prefab.entities.len(), 1);

        let entity: Entity = parse(example_of(&doc, "Entity"));
        assert_eq!(entity.name, "Entity");
        assert_eq!(entity.id, EntityId::from_raw(1));
        assert_eq!(entity.model, "builtin://cube");
        assert!(entity.transform == Transform::IDENTITY);

        let transform: Transform = parse(example_of(&doc, "Transform"));
        assert_eq!(transform.position, Vec3::new(0.0, 1.0, 0.0));

        let light: Light = parse(example_of(&doc, "Light"));
        assert_eq!(light.name, "Torch");
        assert_eq!(light.kind, LightKind::Point);

        let id: EntityId = parse(example_of(&doc, "EntityId"));
        assert_eq!(id, EntityId::from_raw(1));

        let prefab_ref: PrefabInstanceRef = parse(example_of(&doc, "PrefabInstanceRef"));
        assert_eq!(prefab_ref.source, "Prefabs/Enemy.prefab.ron");
        assert!(prefab_ref.is_root);

        let state: AnimState = parse(example_of(&doc, "AnimState"));
        assert_eq!(state.name, "Run");
        assert_eq!(state.clip, "Run");

        let condition: AnimCondition = parse(example_of(&doc, "AnimCondition"));
        assert!(matches!(
            condition,
            AnimCondition::AtLeast { ref param, value } if param == "speed" && value == 0.5
        ));

        let transition: AnimTransition = parse(example_of(&doc, "AnimTransition"));
        assert_eq!(transition.from, "Idle");
        assert_eq!(transition.to, "Run");

        let machine: AnimMachine = parse(example_of(&doc, "AnimMachine"));
        assert_eq!(machine.entry, "Idle");
        assert_eq!(machine.states.len(), 2);
        assert_eq!(machine.transitions.len(), 2);
        assert!(machine.is_usable());

        let additive: AnimAdditive = parse(example_of(&doc, "AnimAdditive"));
        assert_eq!(additive.clip, "Breath");
        assert_eq!(additive.weight, 0.5);

        let mode: AnimLayerMode = parse(example_of(&doc, "AnimLayerMode"));
        assert_eq!(mode, AnimLayerMode::Override);

        let layer: AnimLayer = parse(example_of(&doc, "AnimLayer"));
        assert_eq!(layer.name, "Aim");
        assert_eq!(layer.mode, AnimLayerMode::Override);
        assert_eq!(layer.mask, vec!["Spine1".to_string()]);

        let animator: Animator = parse(example_of(&doc, "Animator"));
        assert_eq!(animator.clip, "Idle");
        assert!(animator.autoplay);

        let body: PhysicsBody = parse(example_of(&doc, "PhysicsBody"));
        assert_eq!(body.kind, BodyKind::Static);
        assert!(matches!(
            body.shape,
            CollisionShape::Cuboid { half_extents } if half_extents == [12.0, 0.5, 12.0]
        ));

        let body_kind: BodyKind = parse(example_of(&doc, "BodyKind"));
        assert_eq!(body_kind, BodyKind::Static);

        let shape: CollisionShape = parse(example_of(&doc, "CollisionShape"));
        assert!(matches!(
            shape,
            CollisionShape::Cuboid { half_extents } if half_extents == [12.0, 0.5, 12.0]
        ));

        let audio: AudioSource = parse(example_of(&doc, "AudioSource"));
        assert_eq!(audio.clip, "Resources/Audio/Beep.wav");

        let over: MaterialOverride = parse(example_of(&doc, "MaterialOverride"));
        assert_eq!(over.base_color, Some([0.8, 0.25, 0.2, 1.0]));
        assert_eq!(over.metallic, None);
        assert_eq!(over.roughness, Some(0.6));

        let material: MaterialAsset = parse(example_of(&doc, "MaterialAsset"));
        assert_eq!(material.albedo, "Resources/Textures/Wood.png");
        assert_eq!(material.roughness, 0.55);

        let controller: CharacterController = parse(example_of(&doc, "CharacterController"));
        assert_eq!(controller.radius, 0.35);
        assert_eq!(controller.height, 1.75);
        assert_eq!(controller.slope_limit, 45.0);

        let camera: Camera = parse(example_of(&doc, "Camera"));
        assert_eq!(camera, Camera::default());

        let ui: UiElement = parse(example_of(&doc, "UiElement"));
        assert_eq!(ui.kind, UiKind::Button);
        assert!(ui.interactable);
        assert_eq!(ui.text.content, "Play");
        assert_eq!(ui.style, UiStyle::panel());

        let style: UiStyle = parse(example_of(&doc, "UiStyle"));
        assert_eq!(style, UiStyle::panel());

        let text_style: UiTextStyle = parse(example_of(&doc, "UiTextStyle"));
        assert_eq!(text_style.content, "Hello");
        assert_eq!(text_style.size, 24.0);

        let image: UiImageStyle = parse(example_of(&doc, "UiImageStyle"));
        assert_eq!(image.path, "Resources/UI/Icon.png");
        assert_eq!(image.fill, 0.75);

        let ui_kind: UiKind = parse(example_of(&doc, "UiKind"));
        assert_eq!(ui_kind, UiKind::Button);
        let align: UiAlign = parse(example_of(&doc, "UiAlign"));
        assert_eq!(align, UiAlign::Center);
        let valign: UiVAlign = parse(example_of(&doc, "UiVAlign"));
        assert_eq!(valign, UiVAlign::Middle);

        let emitter: ParticleEmitter = parse(example_of(&doc, "ParticleEmitter"));
        assert_eq!(emitter, ParticleEmitter::smoke());
        assert_eq!(emitter.burst, 0, "burst 是运行时字段，示例不应带出非零值");

        let emission: EmissionShape = parse(example_of(&doc, "EmissionShape"));
        assert!(matches!(emission, EmissionShape::Sphere { radius } if radius == 0.25));

        let blend: ParticleBlend = parse(example_of(&doc, "ParticleBlend"));
        assert_eq!(blend, ParticleBlend::Additive);

        let value: FieldValue = parse(example_of(&doc, "FieldValue"));
        assert_eq!(value, FieldValue::F32(5.0));

        let values: FieldValues = parse(example_of(&doc, "FieldValues"));
        assert_eq!(values.get("speed"), Some(&FieldValue::F32(5.0)));

        let light_kind: LightKind = parse(example_of(&doc, "LightKind"));
        assert_eq!(light_kind, LightKind::Point);
    }

    /// 示例不仅要能 `ron::from_str`，还得能走一遍**真实的项目读写边界**：
    /// 写盘时资产引用变 GUID、读回时变路径。只测解析会漏掉这层——
    /// 智能体照着 schema 写出来的场景，保存 / 重开必须还是同一份内容。
    #[test]
    fn schema_examples_survive_project_handle_round_trip() {
        use crate::project::ProjectHandle;

        let doc = scene_schema();
        let base = std::env::temp_dir().join(format!(
            "xgame_schema_roundtrip_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let project = ProjectHandle::create(&base, "SchemaRoundTrip").expect("创建临时项目");
        let root = project.root().to_path_buf();

        for (relative, bytes) in [
            ("Resources/Models/Box.glb", b"glb".as_slice()),
            ("Scripts/Player.lua", b"-- lua".as_slice()),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }

        let mut scene: Scene = parse(example_of(&doc, "Scene"));
        scene.entities[0].model = "Resources/Models/Box.glb".into();
        scene.entities[0].scripts.push("Scripts/Player.lua".into());

        project
            .write_scene("Scenes/Schema.ron", &scene)
            .expect("示例场景应当能写盘");
        let disk = std::fs::read_to_string(root.join("Scenes/Schema.ron")).unwrap();
        assert!(disk.contains("guid://"), "资产引用落盘必须是 GUID:\n{disk}");
        assert!(
            !disk.contains("Resources/Models/Box.glb") && !disk.contains("Scripts/Player.lua"),
            "落盘不应保留资产路径:\n{disk}"
        );

        let loaded = project.load_scene("Scenes/Schema.ron").expect("示例场景应当能读回");
        assert_eq!(loaded, scene, "示例场景经 ProjectHandle 往返后必须逐字段一致");

        // 同一份内存场景再写一次：GUID 不能重新分配，字段顺序也必须稳定。
        project
            .write_scene("Scenes/SchemaAgain.ron", &loaded)
            .expect("读回的场景应当能再写一次");
        let disk_again = std::fs::read_to_string(root.join("Scenes/SchemaAgain.ron")).unwrap();
        assert_eq!(disk, disk_again, "GUID 引用往返后再写，文本必须稳定");

        // 预制体与场景共用同一套引用 walk，示例同样要能落盘 / 读回。
        let mut prefab: Prefab = parse(example_of(&doc, "Prefab"));
        prefab.entities[0].model = "Resources/Models/Box.glb".into();
        project
            .write_prefab("Prefabs/Schema.prefab.ron", &prefab)
            .expect("示例预制体应当能写盘");
        let prefab_disk =
            std::fs::read_to_string(root.join("Prefabs/Schema.prefab.ron")).unwrap();
        assert!(prefab_disk.contains("guid://"), "预制体资产引用也必须是 GUID");
        let prefab_back = project
            .load_prefab("Prefabs/Schema.prefab.ron")
            .expect("示例预制体应当能读回");
        assert_eq!(prefab_back.name, prefab.name);
        assert_eq!(prefab_back.entities, prefab.entities);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn every_enum_variant_example_parses_to_that_variant() {
        let doc = scene_schema();

        assert_variants(&doc, "LightKind", &["Directional", "Point"]);
        assert_variants(&doc, "BodyKind", &["Static", "Dynamic", "Kinematic"]);
        assert_variants(
            &doc,
            "CollisionShape",
            &["Cuboid", "Ball", "Capsule", "Cylinder", "Auto"],
        );
        assert_variants(&doc, "AnimCondition", &["AtLeast", "Below"]);
        assert_variants(&doc, "AnimLayerMode", &["Additive", "Override"]);
        assert_variants(&doc, "UiKind", &["Panel", "Text", "Image", "Button"]);
        assert_variants(&doc, "UiAlign", &["Left", "Center", "Right"]);
        assert_variants(&doc, "UiVAlign", &["Top", "Middle", "Bottom"]);
        assert_variants(&doc, "EmissionShape", &["Point", "Sphere", "Box"]);
        assert_variants(&doc, "ParticleBlend", &["Alpha", "Additive"]);
        assert_variants(
            &doc,
            "FieldValue",
            &[
                "F32", "F64", "I32", "I64", "U32", "Bool", "String", "Vec2", "Vec3", "Vec4",
            ],
        );

        assert_variant_examples(&doc, "LightKind", |k| match k {
            LightKind::Directional => "Directional",
            LightKind::Point => "Point",
        });
        assert_variant_examples(&doc, "BodyKind", |k| match k {
            BodyKind::Static => "Static",
            BodyKind::Dynamic => "Dynamic",
            BodyKind::Kinematic => "Kinematic",
        });
        assert_variant_examples(&doc, "CollisionShape", |s| match s {
            CollisionShape::Cuboid { .. } => "Cuboid",
            CollisionShape::Ball { .. } => "Ball",
            CollisionShape::Capsule { .. } => "Capsule",
            CollisionShape::Cylinder { .. } => "Cylinder",
            CollisionShape::Auto => "Auto",
        });
        assert_variant_examples(&doc, "AnimCondition", |c| match c {
            AnimCondition::AtLeast { .. } => "AtLeast",
            AnimCondition::Below { .. } => "Below",
        });
        assert_variant_examples(&doc, "AnimLayerMode", |m| match m {
            AnimLayerMode::Additive => "Additive",
            AnimLayerMode::Override => "Override",
        });
        assert_variant_examples(&doc, "UiKind", |k| match k {
            UiKind::Panel => "Panel",
            UiKind::Text => "Text",
            UiKind::Image => "Image",
            UiKind::Button => "Button",
        });
        assert_variant_examples(&doc, "UiAlign", |a| match a {
            UiAlign::Left => "Left",
            UiAlign::Center => "Center",
            UiAlign::Right => "Right",
        });
        assert_variant_examples(&doc, "UiVAlign", |a| match a {
            UiVAlign::Top => "Top",
            UiVAlign::Middle => "Middle",
            UiVAlign::Bottom => "Bottom",
        });
        assert_variant_examples(&doc, "EmissionShape", |s| match s {
            EmissionShape::Point => "Point",
            EmissionShape::Sphere { .. } => "Sphere",
            EmissionShape::Box { .. } => "Box",
        });
        assert_variant_examples(&doc, "ParticleBlend", |b| match b {
            ParticleBlend::Alpha => "Alpha",
            ParticleBlend::Additive => "Additive",
        });
        assert_variant_examples(&doc, "FieldValue", |v| match v {
            FieldValue::F32(_) => "F32",
            FieldValue::F64(_) => "F64",
            FieldValue::I32(_) => "I32",
            FieldValue::I64(_) => "I64",
            FieldValue::U32(_) => "U32",
            FieldValue::Bool(_) => "Bool",
            FieldValue::String(_) => "String",
            FieldValue::Vec2(_) => "Vec2",
            FieldValue::Vec3(_) => "Vec3",
            FieldValue::Vec4(_) => "Vec4",
        });
    }

    /// 穷举解构：任何结构体新增 / 删除字段都会让这条测试编译不过。
    #[allow(unused_variables)]
    #[test]
    fn struct_fields_match_rust_definitions() {
        let doc = scene_schema();

        let Scene {
            name,
            entities,
            lights,
            ambient,
        } = Scene::new("x");
        assert_fields(&doc, "Scene", &["name", "entities", "lights", "ambient"]);

        let Entity {
            name,
            transform,
            layer,
            model,
            material_asset,
            id,
            parent,
            prefab,
            scripts,
            behaviors,
            behavior_params,
            physics,
            animator,
            audio,
            material,
            controller,
            ui,
            particle,
            camera,
            components,
        } = Entity::new("x");
        assert_fields(
            &doc,
            "Entity",
            &[
                "name",
                "transform",
                "layer",
                "model",
                "material_asset",
                "id",
                "parent",
                "prefab",
                "scripts",
                "behaviors",
                "behavior_params",
                "physics",
                "animator",
                "audio",
                "material",
                "controller",
                "ui",
                "particle",
                "camera",
            ],
        );

        let Transform {
            position,
            rotation,
            scale,
        } = Transform::IDENTITY;
        assert_fields(&doc, "Transform", &["position", "rotation", "scale"]);

        let Light {
            name,
            kind,
            direction,
            position,
            color,
            intensity,
            range,
            casts_shadow,
            shadow_resolution,
        } = Light::default();
        assert_fields(
            &doc,
            "Light",
            &[
                "name",
                "kind",
                "direction",
                "position",
                "color",
                "intensity",
                "range",
                "casts_shadow",
                "shadow_resolution",
            ],
        );

        let PrefabInstanceRef {
            source,
            source_id,
            is_root,
        } = PrefabInstanceRef {
            source: String::new(),
            source_id: EntityId::from_raw(1),
            is_root: false,
        };
        assert_fields(
            &doc,
            "PrefabInstanceRef",
            &["source", "source_id", "is_root"],
        );

        let Prefab { name, entities } = Prefab::new("x");
        assert_fields(&doc, "Prefab", &["name", "entities"]);

        let AnimState {
            name,
            clip,
            speed,
            looping,
        } = AnimState::default();
        assert_fields(&doc, "AnimState", &["name", "clip", "speed", "looping"]);

        let AnimTransition { from, to, when } = AnimTransition {
            from: "a".into(),
            to: "b".into(),
            when: AnimCondition::Below {
                param: "p".into(),
                value: 0.0,
            },
        };
        assert_fields(&doc, "AnimTransition", &["from", "to", "when"]);

        let AnimMachine {
            entry,
            parameters,
            states,
            transitions,
        } = AnimMachine::default();
        assert_fields(
            &doc,
            "AnimMachine",
            &["entry", "parameters", "states", "transitions"],
        );

        let AnimAdditive {
            clip,
            weight,
            speed,
            looping,
        } = AnimAdditive::default();
        assert_fields(
            &doc,
            "AnimAdditive",
            &["clip", "weight", "speed", "looping"],
        );

        let AnimLayer {
            name,
            mode,
            clip,
            weight,
            speed,
            looping,
            mask,
        } = AnimLayer::default();
        assert_fields(
            &doc,
            "AnimLayer",
            &["name", "mode", "clip", "weight", "speed", "looping", "mask"],
        );

        let Animator {
            clip,
            speed,
            looping,
            autoplay,
            fade_time,
            machine,
            additive,
            layers,
        } = Animator::default();
        assert_fields(
            &doc,
            "Animator",
            &[
                "clip",
                "speed",
                "looping",
                "autoplay",
                "fade_time",
                "machine",
                "additive",
                "layers",
            ],
        );

        let PhysicsBody {
            kind,
            shape,
            mass,
            friction,
            restitution,
            gravity_scale,
            is_sensor,
        } = PhysicsBody::default();
        assert_fields(
            &doc,
            "PhysicsBody",
            &[
                "kind",
                "shape",
                "mass",
                "friction",
                "restitution",
                "gravity_scale",
                "is_sensor",
            ],
        );

        let AudioSource {
            clip,
            volume,
            pitch,
            looping,
            autoplay,
            spatial,
            min_distance,
            max_distance,
        } = AudioSource::default();
        assert_fields(
            &doc,
            "AudioSource",
            &[
                "clip",
                "volume",
                "pitch",
                "looping",
                "autoplay",
                "spatial",
                "min_distance",
                "max_distance",
            ],
        );

        let MaterialOverride {
            base_color,
            metallic,
            roughness,
        } = MaterialOverride::default();
        assert_fields(
            &doc,
            "MaterialOverride",
            &["base_color", "metallic", "roughness"],
        );

        let MaterialAsset {
            base_color,
            metallic,
            roughness,
            albedo,
        } = MaterialAsset::default();
        assert_fields(
            &doc,
            "MaterialAsset",
            &["base_color", "metallic", "roughness", "albedo"],
        );

        let CharacterController {
            radius,
            height,
            step_offset,
            slope_limit,
            skin_width,
            snap_to_ground,
        } = CharacterController::default();
        assert_fields(
            &doc,
            "CharacterController",
            &[
                "radius",
                "height",
                "step_offset",
                "slope_limit",
                "skin_width",
                "snap_to_ground",
            ],
        );

        let Camera {
            fov,
            near,
            far,
            active,
        } = Camera::default();
        assert_fields(&doc, "Camera", &["fov", "near", "far", "active"]);

        let UiElement {
            kind,
            anchor_min,
            anchor_max,
            offset_min,
            offset_max,
            style,
            text,
            image,
            visible,
            interactable,
            state,
        } = UiElement::default();
        assert_fields(
            &doc,
            "UiElement",
            &[
                "kind",
                "anchor_min",
                "anchor_max",
                "offset_min",
                "offset_max",
                "style",
                "text",
                "image",
                "visible",
                "interactable",
            ],
        );
        // state 是运行时字段：目录里不该有它（上面的期望列表已排除）。

        let UiStyle {
            background,
            border_color,
            border_width,
            corner_radius,
            padding,
        } = UiStyle::default();
        assert_fields(
            &doc,
            "UiStyle",
            &[
                "background",
                "border_color",
                "border_width",
                "corner_radius",
                "padding",
            ],
        );

        let UiTextStyle {
            content,
            size,
            color,
            align,
            valign,
            wrap,
            bold,
        } = UiTextStyle::default();
        assert_fields(
            &doc,
            "UiTextStyle",
            &["content", "size", "color", "align", "valign", "wrap", "bold"],
        );

        let UiImageStyle {
            path,
            tint,
            fill,
            keep_aspect,
        } = UiImageStyle::default();
        assert_fields(
            &doc,
            "UiImageStyle",
            &["path", "tint", "fill", "keep_aspect"],
        );

        let ParticleEmitter {
            emitting,
            rate,
            max_particles,
            duration,
            looping,
            lifetime,
            speed,
            direction,
            spread,
            shape,
            gravity,
            drag,
            start_size,
            end_size,
            start_color,
            end_color,
            blend,
            seed,
            burst,
        } = ParticleEmitter::default();
        assert_fields(
            &doc,
            "ParticleEmitter",
            &[
                "emitting",
                "rate",
                "max_particles",
                "duration",
                "looping",
                "lifetime",
                "speed",
                "direction",
                "spread",
                "shape",
                "gravity",
                "drag",
                "start_size",
                "end_size",
                "start_color",
                "end_color",
                "blend",
                "seed",
            ],
        );
        // burst 是运行时字段：目录里不该有它（上面的期望列表已排除）。
    }

    /// 枚举变体的字段名 / 载荷类型也要穷举：新增字段会让 match 模式编译失败。
    #[test]
    fn enum_variant_fields_match_rust_definitions() {
        let doc = scene_schema();

        fn check(
            doc: &SchemaDoc,
            type_name: &str,
            variant_name: &str,
            fields: &[&str],
            kind: VariantKind,
        ) {
            let variant = schema_of(doc, type_name)
                .variants
                .iter()
                .find(|v| v.name == variant_name)
                .unwrap_or_else(|| panic!("{type_name} 缺变体 {variant_name}"));
            assert_eq!(variant.kind, kind, "{type_name}.{variant_name} 种类不对");
            let got: Vec<&str> = variant.fields.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(got, fields, "{type_name}.{variant_name} 字段漂移");
        }

        fn collision(shape: &CollisionShape) -> (&'static str, &'static [&'static str]) {
            match shape {
                CollisionShape::Cuboid { half_extents } => {
                    let _ = half_extents;
                    ("Cuboid", &["half_extents"])
                }
                CollisionShape::Ball { radius } => {
                    let _ = radius;
                    ("Ball", &["radius"])
                }
                CollisionShape::Capsule {
                    half_height,
                    radius,
                } => {
                    let _ = (half_height, radius);
                    ("Capsule", &["half_height", "radius"])
                }
                CollisionShape::Cylinder {
                    half_height,
                    radius,
                } => {
                    let _ = (half_height, radius);
                    ("Cylinder", &["half_height", "radius"])
                }
                CollisionShape::Auto => ("Auto", &[]),
            }
        }
        for shape in [
            CollisionShape::Cuboid {
                half_extents: [1.0; 3],
            },
            CollisionShape::Ball { radius: 1.0 },
            CollisionShape::Capsule {
                half_height: 1.0,
                radius: 0.5,
            },
            CollisionShape::Cylinder {
                half_height: 1.0,
                radius: 0.5,
            },
            CollisionShape::Auto,
        ] {
            let (name, fields) = collision(&shape);
            let kind = if fields.is_empty() {
                VariantKind::Unit
            } else {
                VariantKind::Struct
            };
            check(&doc, "CollisionShape", name, fields, kind);
        }

        fn condition(condition: &AnimCondition) -> (&'static str, &'static [&'static str]) {
            match condition {
                AnimCondition::AtLeast { param, value } => {
                    let _ = (param, value);
                    ("AtLeast", &["param", "value"])
                }
                AnimCondition::Below { param, value } => {
                    let _ = (param, value);
                    ("Below", &["param", "value"])
                }
            }
        }
        for cond in [
            AnimCondition::AtLeast {
                param: "p".into(),
                value: 0.0,
            },
            AnimCondition::Below {
                param: "p".into(),
                value: 0.0,
            },
        ] {
            let (name, fields) = condition(&cond);
            check(&doc, "AnimCondition", name, fields, VariantKind::Struct);
        }

        fn emission(shape: &EmissionShape) -> (&'static str, &'static [&'static str]) {
            match shape {
                EmissionShape::Point => ("Point", &[]),
                EmissionShape::Sphere { radius } => {
                    let _ = radius;
                    ("Sphere", &["radius"])
                }
                EmissionShape::Box { half_extents } => {
                    let _ = half_extents;
                    ("Box", &["half_extents"])
                }
            }
        }
        for shape in [
            EmissionShape::Point,
            EmissionShape::Sphere { radius: 1.0 },
            EmissionShape::Box {
                half_extents: [1.0; 3],
            },
        ] {
            let (name, fields) = emission(&shape);
            let kind = if fields.is_empty() {
                VariantKind::Unit
            } else {
                VariantKind::Struct
            };
            check(&doc, "EmissionShape", name, fields, kind);
        }

        fn field_value(value: &FieldValue) -> (&'static str, &'static str) {
            match value {
                FieldValue::F32(_) => ("F32", "f32"),
                FieldValue::F64(_) => ("F64", "f64"),
                FieldValue::I32(_) => ("I32", "i32"),
                FieldValue::I64(_) => ("I64", "i64"),
                FieldValue::U32(_) => ("U32", "u32"),
                FieldValue::Bool(_) => ("Bool", "bool"),
                FieldValue::String(_) => ("String", "String"),
                FieldValue::Vec2(_) => ("Vec2", "Vec2"),
                FieldValue::Vec3(_) => ("Vec3", "Vec3"),
                FieldValue::Vec4(_) => ("Vec4", "Vec4"),
            }
        }
        for value in [
            FieldValue::F32(0.0),
            FieldValue::F64(0.0),
            FieldValue::I32(0),
            FieldValue::I64(0),
            FieldValue::U32(0),
            FieldValue::Bool(false),
            FieldValue::String(String::new()),
            FieldValue::Vec2(glam::Vec2::ZERO),
            FieldValue::Vec3(glam::Vec3::ZERO),
            FieldValue::Vec4(glam::Vec4::ZERO),
        ] {
            let (name, payload) = field_value(&value);
            let variant = schema_of(&doc, "FieldValue")
                .variants
                .iter()
                .find(|v| v.name == name)
                .unwrap_or_else(|| panic!("FieldValue 缺变体 {name}"));
            assert_eq!(variant.kind, VariantKind::Newtype);
            assert_eq!(variant.payload.as_deref(), Some(payload));
        }

        // 纯单元变体的枚举：每个变体都必须是 Unit，且没有字段 / 载荷。
        for type_name in [
            "LightKind",
            "BodyKind",
            "AnimLayerMode",
            "UiKind",
            "UiAlign",
            "UiVAlign",
            "ParticleBlend",
        ] {
            for variant in &schema_of(&doc, type_name).variants {
                assert_eq!(variant.kind, VariantKind::Unit, "{type_name}.{}", variant.name);
                assert!(variant.fields.is_empty());
                assert!(variant.payload.is_none());
            }
        }
    }

    /// 目录里写的默认值必须和 serde 真正做的事一致（省略字段的实测行为）。
    #[test]
    fn serde_defaults_match_the_catalog_claims() {
        let scene: Scene = parse("(name:\"Legacy\",entities:[])");
        assert_eq!(scene.lights, vec![Light::default()]);
        assert_eq!(scene.ambient, [0.06, 0.06, 0.08]);

        let entity: Entity = parse("(name:\"Old\")");
        assert_eq!(entity.transform, Transform::IDENTITY);
        assert_eq!(entity.model, "");
        assert!(entity.material_asset.is_none());
        assert!(entity.parent.is_none());
        assert!(entity.scripts.is_empty());
        assert!(entity.behavior_params.is_empty());
        assert!(entity.physics.is_none());

        let light: Light = parse("(name:\"L\")");
        assert_eq!(light.kind, LightKind::Directional);
        assert_eq!(light.direction, [0.5, -1.0, 0.5]);
        assert_eq!(light.intensity, 3.0);
        assert!(light.casts_shadow);

        let body: PhysicsBody = parse("(kind:Static)");
        assert_eq!(body.shape, CollisionShape::Auto);
        assert_eq!(body.friction, 0.5);
        assert_eq!(body.gravity_scale, 1.0);
        assert!(!body.is_sensor);

        let animator: Animator = parse("(clip:\"Idle\")");
        assert_eq!(animator.speed, 1.0);
        assert!(animator.looping);
        assert!(animator.autoplay);
        assert_eq!(animator.fade_time, 0.25);
        assert!(animator.machine.is_none());

        let audio: AudioSource = parse("(clip:\"a.wav\")");
        assert_eq!(audio.volume, 1.0);
        assert!(audio.autoplay);
        assert!(!audio.spatial);
        assert_eq!(audio.max_distance, 50.0);

        let camera: Camera = parse("(fov:90.0)");
        assert_eq!(camera.near, 0.1);
        assert_eq!(camera.far, 1000.0);
        assert!(camera.active);

        let controller: CharacterController = parse("(radius:0.4)");
        assert_eq!(controller.height, 1.8);
        assert_eq!(controller.slope_limit, 45.0);
        assert_eq!(controller.snap_to_ground, 0.3);

        let ui: UiElement = parse("(kind:Button)");
        assert!(ui.interactable, "省略 interactable 必须是 true");
        assert_eq!(ui.style, UiStyle::default(), "省略 style 必须是透明样式");
        assert_eq!(ui.offset_max, [200.0, 100.0]);
        assert!(ui.visible);

        let over: MaterialOverride = parse("()");
        assert_eq!(over, MaterialOverride::default());

        let material: MaterialAsset = parse("(metallic:1.0)");
        assert_eq!(material.base_color, [1.0; 4]);
        assert_eq!(material.roughness, 0.45);
        assert_eq!(material.albedo, "");
    }
}
