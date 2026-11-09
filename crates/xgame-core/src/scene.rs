//! 场景与实体类型。
//!
//! 场景是游戏世界的顶层容器，包含实体列表。实体现在拥有稳定 ID
//! 和可选的运行时组件集合，向后兼容旧版 RON 场景文件。

use std::collections::BTreeMap;

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::component::ComponentMap;
use crate::id::EntityId;
use crate::light::Light;
use crate::transform::Transform;

/// `Entity.layer` 的 `skip_serializing_if`：0 层（默认）不写进场景文件。
fn is_default_layer(layer: &u8) -> bool {
    *layer == crate::layers::DEFAULT_LAYER
}

/// 场景图中的单个游戏对象。
///
/// 每个实体有一个稳定标识符（`id`）、名称、变换和关联模型路径。
/// `components` 字段存储运行时组件，不会被序列化到场景文件中。
///
/// 旧版 .ron 文件（不含 `id` / `parent` 字段）仍可正确反序列化，因为
/// 两者都使用 `#[serde(default)]`：`id` 会分配一个新 ID，`parent` 补成
/// `None`（即所有实体都落在场景根层）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// 实体的显示名称。
    pub name: String,
    /// **局部空间**变换（相对父节点；没有父节点时即世界变换）。
    ///
    /// 缺省 = 单位变换。旧场景文件全部写满三个分量，行为不变；新文件里
    /// **纯 UI 实体可以整段省掉** —— UI 的位置由锚点决定，`transform` 对它
    /// 没有任何意义，强制写一段单位变换只是噪音。
    #[serde(default)]
    pub transform: Transform,
    /// 层索引（0–31）。决定它与哪些层发生碰撞 / 触发（矩阵在 `project.ron`
    /// 的 `layers` 节，见 [`crate::layers`]）。
    ///
    /// 缺省 = 0（`Default` 层），且默认矩阵全通 —— 所以旧场景文件没有这个
    /// 字段时行为与引入层之前**逐位一致**；`skip_serializing_if` 让 0 层的
    /// 实体不写这一行（绝大多数实体都在 0 层）。
    ///
    /// 越界值（> 31）不会被读盘时静默修正：`--check` 会把它报出来，
    /// 运行时按 [`crate::layers::sanitize_layer`] 取模 —— 报错交给工具，
    /// 运行保证不 panic 也不越位。
    #[serde(default, skip_serializing_if = "is_default_layer")]
    pub layer: u8,
    /// glTF 模型路径（相对于工作区）。空字符串表示没有模型。
    ///
    /// 缺省 = 空串：纯 UI / 纯逻辑实体（触发器、生成点、HUD）本来就没有网格，
    /// 强制写一行 `model: ""` 只是噪音。
    #[serde(default)]
    pub model: String,
    /// LOD 组（距离分级 + 绘制距离）。**空 = 没有 LOD**（绝大多数实体）。
    ///
    /// `levels` 非空时 [`Entity::model`] 被**完全忽略**（渲染与拾取都用第 0 级）——
    /// 不写"基础模型 + 追加级"的双轨语义，避免"改了 Model 却看不见变化"。
    /// 判定规则、距离口径与偏置语义见 [`crate::lod`] 的模块文档（那里是唯一
    /// 一份实现，渲染器只是调用方）。
    ///
    /// 兼容策略与其他可选组件同款：旧场景文件没有这个字段 → 空组，
    /// `skip_serializing_if` 让没有 LOD 的实体不写这一行。
    #[serde(default, skip_serializing_if = "crate::lod::LodGroup::is_empty")]
    pub lod: crate::lod::LodGroup,
    /// 共享材质资产路径（项目相对路径，例如 `Resources/Materials/Wood.mat`）。
    ///
    /// `None` = 用模型自带材质。引用了材质资产后，资产里的金属度 / 粗糙度
    /// **替换**模型参数、`base_color` 乘在模型基础色上、`albedo` 替换模型贴图
    /// （完整优先级见 [`crate::material::MaterialAsset`]）。
    ///
    /// 与 [`Entity::material`]（逐实体临时覆盖）**正交**：资产负责"这是什么
    /// 材质"，覆盖负责"这个实例想怎么微调"，两层可以同时存在。
    /// 兼容策略与其他可选组件同款：旧场景文件没有这个字段 → `None`，
    /// 没引用材质的实体不写这一行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material_asset: Option<String>,
    /// 稳定唯一标识符。旧版文件反序列化时自动分配新 ID。
    #[serde(default)]
    pub id: EntityId,
    /// 父实体的 ID。`None` = 场景根节点。
    ///
    /// 层级刻意**按 ID 单向引用**，而不是在每个实体里内嵌一份子节点列表：
    /// 内嵌列表一旦与实体列表不同步（删除、粘贴、反序列化都可能踩），就会留下
    /// "幽灵子节点"。这里子节点永远由 [`Scene::children`] 现算，唯一真相
    /// 就是这一条 `parent`——牺牲一点查询开销，换掉一整类不一致 bug。
    #[serde(default)]
    pub parent: Option<EntityId>,
    /// 预制体实例来源——只有「预制体实例」里的实体才带（见 [`crate::prefab`]）。
    ///
    /// `skip_serializing_if` 让普通实体的场景文件里完全不出现这个字段，
    /// 场景文件不会被一行行 `prefab: None` 灌满；同时 `#[serde(default)]`
    /// 保证旧文件（没这个字段）照常加载成 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefab: Option<crate::prefab::PrefabInstanceRef>,
    /// 挂在这个实体上的脚本（项目相对路径，例如 `Scripts/Player.lua`）。
    ///
    /// 存的是**路径**而不是脚本内容：脚本是资产，要能一改全生效、
    /// 要在多个实体间共享、要能进版本库。把内容内联进场景文件，
    /// 会让"改一次逻辑"变成"改几十个场景"。
    ///
    /// 顺序即执行顺序（同一实体内多个脚本按列表顺序跑），这是刻意保留的
    /// 一个可控点：玩家脚本依赖另一个脚本先算出的状态时，能靠顺序表达。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scripts: Vec<String>,
    /// 挂在这个实体上的 Rust 行为组件名（注册表里的名字，见
    /// `xgame-runtime::BehaviorRegistry`）。
    ///
    /// 双轨语言架构的 Rust 轨道（UE 模式）：名字在运行时查注册表实例化成
    /// `Lifecycle` 组件。与 `scripts` 完全同款的兼容模式 —— 旧场景文件没有
    /// 这个字段 → 空列表（纯数据、零行为），`skip_serializing_if` 让没挂
    /// 行为的实体不写这一行。
    ///
    /// 顺序即执行顺序（同实体内多个行为按列表顺序跑），理由同 `scripts`。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub behaviors: Vec<String>,
    /// 这些组件的**字段存盘值**（P1.5「组件字段进场景文件」）：
    /// 注册名 →（字段寻址名 → 值）。
    ///
    /// 为什么要挂在实体上而不是别的地方：参数是**这份场景的设定**，属于资产；
    /// 此前它只活在编辑器内存里（P1 的 `edit_components`），存盘不带走 ——
    /// 打包出去的玩家拿不到编辑器调好的手感。装配期（`World` 实例化组件、
    /// 编辑器构造 Edit 副本）按名套用，键用注册名而非列表下标：增删 / 重排
    /// `behaviors` 不会让参数错位到别的组件上（下标错位 = 把 A 的参数写进
    /// B 的字段，两边都是 `f32`，编译器管不着）。
    ///
    /// 兼容模式与 `behaviors` 同款：旧场景文件没有这个字段 → 空表，
    /// `skip_serializing_if` 让没调过参的实体不写这一行。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub behavior_params: BTreeMap<String, crate::inspect::FieldValues>,
    /// 物理属性。`None` = 不参与物理（纯视觉实体）。
    ///
    /// 旧场景文件没有这个字段，反序列化成 `None` —— 语义上正好等于
    /// "老场景里的物体都是静态装饰"，与当时的行为一致，不会突然被重力拽走。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physics: Option<crate::physics::PhysicsBody>,
    /// 动画播放器。`None` = 不播动画（绑定姿态 / 静态网格）。
    ///
    /// 与 `physics` 同样的兼容策略：旧场景文件没有这个字段 → `None`，
    /// 老场景照旧渲染；`skip_serializing_if` 让没配动画的实体在场景文件里
    /// 不出现这一行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animator: Option<crate::animation::Animator>,
    /// 形变目标权重（morph target / blend shape）。**空** = 用模型自带的默认权重。
    ///
    /// 这一层是"静态表情 / 手动摆脸"：脚本 `set_morph`、Inspector 滑条写这里。
    /// 它是**逐下标**的，下标含义来自模型 —— 换模型后长度对不上时，短的用模型
    /// 默认补齐、长的丢弃（见 `xgame_render::morph::MorphTargets::resolve_into`），
    /// 绝不把权重错位到别的表情上。
    ///
    /// 与动画的优先级：剪辑里**有**权重轨道时以动画为准（Unity 同款语义，
    /// 被动画驱动的属性不听手动覆盖），没有轨道才用这里的值。
    ///
    /// 兼容策略与其他可选组件同款：旧场景文件没有这个字段 → 空，
    /// `skip_serializing_if` 让不带表情的实体不写这一行。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub morph_weights: Vec<f32>,
    /// 音频源。`None` = 静默实体。
    ///
    /// 兼容策略与 `animator` 同款（旧文件 → `None`；没配音频不写这一行）。
    /// 引擎侧封装在 `xgame-audio`（kira），这里只是纯数据声明 ——
    /// 场景文件从不引用 kira 类型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<crate::audio::AudioSource>,
    /// 逐实体材质覆盖（PBR 参数乘子 / 替换）。`None` = 纯用模型自带材质。
    ///
    /// 兼容策略与 `audio` 同款（旧文件 → `None`；没配覆盖不写这一行）。
    /// 语义与理由见 [`crate::material::MaterialOverride`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<crate::material::MaterialOverride>,
    /// 角色控制器参数。`None` = 这个实体不做角色移动（普通动态 /
    /// 运动学物体，或纯视觉）。
    ///
    /// 与 `physics` **正交**：`physics` 描述碰撞体本身，`controller` 描述
    /// 怎么移动它（台阶 / 坡度 / 贴地吸附）。挂了 `controller` 的实体会被
    /// 运行时接管成位置驱动的运动学胶囊，`PhysicsBody` 里声明的运动学
    /// 类型此时不再作数（理由见 [`crate::character::CharacterController`]）。
    ///
    /// 兼容策略与 `material` 同款：旧场景文件没有这个字段 → `None`，
    /// 老场景照常加载；没配角色的实体不写这一行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller: Option<crate::character::CharacterController>,
    /// 屏幕空间 UI 元素。`None` = 这个实体不参与 UI。
    ///
    /// 与 `physics`（碰撞体）/ `controller`（角色）**三者正交**：一个实体
    /// 可以是纯 UI（挂 UI + 不挂模型 / 物理），也可以既是 3D 物体又是 UI
    /// （少见，但语义上并不冲突）。
    ///
    /// 位置由**锚点**决定，`transform` 不参与 UI 布局 —— 理由见
    /// [`crate::ui`] 的模块文档。
    ///
    /// 兼容策略与其他可选组件同款：旧场景文件没有这个字段 → `None`，
    /// 老场景照常加载；没配 UI 的实体不写这一行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<crate::ui::UiElement>,
    /// 粒子发射器。`None` = 这个实体不发射粒子（零成本）。
    ///
    /// **纯视觉、纯声明**：模拟在 `xgame-particle`（按实体 id 分桶存单个粒子），
    /// 绘制在 `xgame-render`。这里的字段决定"长什么样"，不决定"现在有几颗"。
    ///
    /// 与 `model` 正交：可以挂在有模型的物体上当火焰附着点，也可以挂空实体
    /// （只有发射器、没有网格 —— 篝火、烟雾口都是这样）。
    ///
    /// 兼容策略与其他可选组件同款：旧场景文件没有这个字段 → `None` 且当帧
    /// 零开销；没配粒子的实体不写这一行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub particle: Option<crate::particle::ParticleEmitter>,
    /// 相机（透视投影参数 + 主相机标记）。`None` = 这个实体不是相机。
    ///
    /// 做成**实体组件**而不是 `Scene` 级数据（像 `Light` 那样），是因为相机
    /// 需要被 Transform 层级带着走：挂在角色下当肩视角、被脚本每帧推着跟随，
    /// 这些都是"相机是一个物体"的直接后果。理由详见 [`crate::camera`]。
    ///
    /// 选谁当主相机不在这里决定 —— 唯一规则是
    /// [`Scene::active_camera_index`]，宿主（独立游戏 / 编辑器 Play /
    /// 无头截图）都读那一个，别各自写一套。
    ///
    /// 兼容策略与其他可选组件同款：旧场景文件没有这个字段 → `None`，
    /// 老场景照常加载（宿主退回各自的旧机位）；没配相机的实体不写这一行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<crate::camera::Camera>,
    /// 运行时组件集合（反序列化时默认为空）。
    #[serde(skip)]
    pub components: ComponentMap,
}

impl Entity {
    /// 创建一个新实体，自动分配 ID。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            transform: Transform::IDENTITY,
            layer: crate::layers::DEFAULT_LAYER,
            model: String::new(),
            lod: crate::lod::LodGroup::default(),
            material_asset: None,
            id: EntityId::new(),
            parent: None,
            prefab: None,
            scripts: Vec::new(),
            behaviors: Vec::new(),
            behavior_params: BTreeMap::new(),
            physics: None,
            animator: None,
            morph_weights: Vec::new(),
            audio: None,
            material: None,
            controller: None,
            ui: None,
            particle: None,
            camera: None,
            components: ComponentMap::new(),
        }
    }

    /// 从组件创建实体（用于逐步迁移到纯组件方式）。
    pub fn from_name(name: impl Into<String>) -> Self {
        Self::new(name)
    }

    /// 给实体附加模型路径。
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// 给实体附上一个 LOD 组（各级模型 + 距离阈值，见 [`crate::lod`]）。
    ///
    /// 与 [`Entity::with_model`] 二选一：`lod` 非空时 `model` 被忽略。
    pub fn with_lod(mut self, lod: crate::lod::LodGroup) -> Self {
        self.lod = lod;
        self
    }

    /// 给实体指定共享材质资产（`Resources/Materials/Wood.mat`）。
    pub fn with_material_asset(mut self, material: impl Into<String>) -> Self {
        self.material_asset = Some(material.into());
        self
    }

    /// 设置变换。
    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    /// 设置层索引（越界按 [`crate::layers::sanitize_layer`] 取模）。
    pub fn with_layer(mut self, layer: u8) -> Self {
        self.layer = crate::layers::sanitize_layer(layer);
        self
    }

    /// 挂一个脚本（追加到执行顺序末尾）。
    pub fn with_script(mut self, script: impl Into<String>) -> Self {
        self.scripts.push(script.into());
        self
    }

    /// 设置物理属性。
    pub fn with_physics(mut self, physics: crate::physics::PhysicsBody) -> Self {
        self.physics = Some(physics);
        self
    }

    /// 挂一个动画播放器。
    pub fn with_animator(mut self, animator: crate::animation::Animator) -> Self {
        self.animator = Some(animator);
        self
    }

    /// 挂一个音频源。
    pub fn with_audio(mut self, audio: crate::audio::AudioSource) -> Self {
        self.audio = Some(audio);
        self
    }

    /// 设置逐实体材质覆盖。
    pub fn with_material(mut self, material: crate::material::MaterialOverride) -> Self {
        self.material = Some(material);
        self
    }

    /// 挂一个角色控制器（与 `physics` 正交，见 [`Entity::controller`]）。
    pub fn with_controller(mut self, controller: crate::character::CharacterController) -> Self {
        self.controller = Some(controller);
        self
    }

    /// 挂一个粒子发射器（纯视觉，见 [`Entity::particle`]）。
    pub fn with_particle(mut self, particle: crate::particle::ParticleEmitter) -> Self {
        self.particle = Some(particle);
        self
    }

    /// 挂一台相机（见 [`Entity::camera`]）。
    pub fn with_camera(mut self, camera: crate::camera::Camera) -> Self {
        self.camera = Some(camera);
        self
    }

    /// 这个实体是否参与物理。
    pub fn has_physics(&self) -> bool {
        self.physics.is_some()
    }

    /// 是否有脚本。
    pub fn has_scripts(&self) -> bool {
        !self.scripts.is_empty()
    }

    /// 向实体添加一个运行时组件。
    pub fn add_component<T: crate::component::Component>(&mut self, component: T) -> Option<T> {
        self.components.insert(component)
    }

    /// 获取运行时组件的引用。
    pub fn component<T: crate::component::Component>(&self) -> Option<&T> {
        self.components.get()
    }

    /// 获取运行时组件的可变引用。
    pub fn component_mut<T: crate::component::Component>(&mut self) -> Option<&mut T> {
        self.components.get_mut()
    }

    /// 实体是否有模型关联（含 LOD 各级）。
    ///
    /// LOD 实体可以把 `model` 留空、只写 `lod.levels` —— 那种实体**有几何**，
    /// `--check` 必须照旧检查各级的模型文件。
    pub fn has_model(&self) -> bool {
        !self.model.is_empty() || !self.lod.is_empty()
    }

    /// 距离 `distance` 下这个实体该用哪一级、哪个模型（`bias` 见 [`crate::lod`]）。
    ///
    /// `None` = 这个距离上不该画（没有模型，或超出绘制距离）。**判定在查模型
    /// 缓存之前完成** —— 远处实体即使网格没加载也能被判"不画"。
    pub fn lod_at(&self, distance: f32, bias: f32) -> Option<(usize, &str)> {
        if self.lod.is_empty() {
            // 没有 LOD：`model` 就是唯一一级，且不做距离剔除（老场景逐位不变）。
            return (!self.model.is_empty()).then_some((0, self.model.as_str()));
        }
        self.lod.resolve(distance, bias)
    }

    /// 第 `level` 级用的模型（作者下标；空模型 / 越界 → `None`）。
    ///
    /// 渲染器把级号记进批次，画的时候按它回查模型 —— 两种形态（有 / 没有 LOD）
    /// 必须在**同一个函数**里归一，否则"关掉 LOD 的实体"会查不到模型。
    pub fn model_at(&self, level: usize) -> Option<&str> {
        let path = if self.lod.is_empty() {
            if level == 0 {
                &self.model
            } else {
                return None;
            }
        } else {
            &self.lod.levels.get(level)?.model
        };
        (!path.is_empty()).then_some(path.as_str())
    }

    /// 这个实体**可能用到**的全部模型路径（`model` + 各级 LOD）。
    ///
    /// 宿主预加载用：所有级都得进缓存，否则"走近了突然空白一帧"。
    /// `model` 即使在有 LOD 时被忽略也照样带上 —— 多加载一份的代价远小于
    /// "作者删掉 LOD 后模型不在缓存里"的诡异首帧。
    pub fn model_paths(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.model.as_str())
            .chain(self.lod.levels.iter().map(|level| level.model.as_str()))
            .filter(|path| !path.is_empty())
    }

    /// 当前实体可由 runtime 消费的根运动配置。
    ///
    /// 动态体的位置归物理求解器，静态体按定义不动；两者若再接受动画位移就会出现
    /// 第二个位置所有者。因此它们返回 `None`，渲染侧也不会从骨架里剥掉根位移。
    /// 角色控制器优先于误配的 `physics`，普通实体与运动学体则由 runtime 唯一落地。
    pub fn effective_root_motion(&self) -> Option<&crate::animation::RootMotion> {
        let root = self.animator.as_ref()?.root_motion.as_ref()?;
        if self.controller.is_some()
            || self.physics.is_none()
            || self
                .physics
                .is_some_and(|body| body.kind == crate::physics::BodyKind::Kinematic)
        {
            Some(root)
        } else {
            None
        }
    }
}

impl PartialEq for Entity {
    /// 逐字段比较，**唯独不比 `components`**：
    ///
    /// - `components` 是 `#[serde(skip)]` 的运行时数据，不落盘 —— 场景相等
    ///   问的本来就是"落盘内容一样吗"（脏检查的语义），运行时组件与此无关；
    /// - 它装的是 `Box<dyn Any>`，类型擦除后根本没法比，硬比只能比个寂寞。
    fn eq(&self, other: &Self) -> bool {
        // 穷尽解构是编译期检查：以后给 Entity 加字段却忘了决定它是否参与
        // 脏检查，这里会编译失败，而不是悄悄把新字段排除在保存提示之外。
        let Self {
            name,
            transform,
            layer,
            model,
            lod,
            material_asset,
            id,
            parent,
            prefab,
            scripts,
            behaviors,
            behavior_params,
            physics,
            animator,
            morph_weights,
            audio,
            material,
            controller,
            ui,
            particle,
            camera,
            components: _,
        } = self;
        let Self {
            name: other_name,
            transform: other_transform,
            layer: other_layer,
            model: other_model,
            lod: other_lod,
            material_asset: other_material_asset,
            id: other_id,
            parent: other_parent,
            prefab: other_prefab,
            scripts: other_scripts,
            behaviors: other_behaviors,
            behavior_params: other_behavior_params,
            physics: other_physics,
            animator: other_animator,
            morph_weights: other_morph_weights,
            audio: other_audio,
            material: other_material,
            controller: other_controller,
            ui: other_ui,
            particle: other_particle,
            camera: other_camera,
            components: _,
        } = other;

        name == other_name
            && transform == other_transform
            && layer == other_layer
            && model == other_model
            && lod == other_lod
            && material_asset == other_material_asset
            && id == other_id
            && parent == other_parent
            && prefab == other_prefab
            && scripts == other_scripts
            && behaviors == other_behaviors
            && behavior_params == other_behavior_params
            && physics == other_physics
            && animator == other_animator
            && morph_weights == other_morph_weights
            && audio == other_audio
            && material == other_material
            && controller == other_controller
            // UI / 粒子只比落盘字段，忽略各自的交互态和 burst 运行态。
            && match (ui, other_ui) {
                (None, None) => true,
                (Some(a), Some(b)) => a.data_eq(b),
                _ => false,
            }
            && match (particle, other_particle) {
                (None, None) => true,
                (Some(a), Some(b)) => a.data_eq(b),
                _ => false,
            }
            && camera == other_camera
    }
}

/// 包含实体列表的场景。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// 文件级版本；旧文件省略时是 v1。v2 尚无解码器时必须报错，避免忽略组件数据。
    #[serde(
        default = "crate::file_format::v1",
        skip_serializing_if = "crate::file_format::is_v1",
        deserialize_with = "crate::file_format::deserialize_v1"
    )]
    pub format_version: u32,
    /// 场景名称。
    pub name: String,
    /// 场景中的实体列表。
    pub entities: Vec<Entity>,
    /// 场景光源。旧场景文件没有这个字段，反序列化时补一盏默认方向光
    /// （而不是空列表）——否则老工程一打开就是全黑，很容易被误判成
    /// "渲染坏了"。
    #[serde(default = "default_lights")]
    pub lights: Vec<Light>,
    /// 全局环境光（线性 RGB）。所有表面都能收到的最低亮度，
    /// 用来避免背光面纯黑。
    #[serde(default = "default_ambient")]
    pub ambient: [f32; 3],
}

/// 旧场景（没有 `lights` 字段）的兜底：给一盏默认方向光。
fn default_lights() -> Vec<Light> {
    vec![Light::default()]
}

/// 旧场景（没有 `ambient` 字段）的兜底环境光。
fn default_ambient() -> [f32; 3] {
    [0.06, 0.06, 0.08]
}

impl Scene {
    /// 创建一个空白场景。
    ///
    /// 自带一盏默认方向光和一点环境光——全黑的空场景对新手太劝退，
    /// 而且会让人以为相机或渲染管线出了问题。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format_version: crate::file_format::V1,
            name: name.into(),
            entities: Vec::new(),
            lights: default_lights(),
            ambient: default_ambient(),
        }
    }

    /// 按 EntityId 查找实体索引（线性搜索）。
    pub fn find_entity(&self, id: EntityId) -> Option<usize> {
        self.entities.iter().position(|e| e.id == id)
    }

    /// 解析**主相机**：返回它在 `entities` 中的索引。
    ///
    /// 唯一的相机选择规则，三宿主（独立游戏 / 编辑器 Play / 无头截图）与
    /// 脚本 API 全读这一处 —— 各写一套的话，"编辑器里看到的是 A 机位、
    /// 打包出来是 B 机位"这类 bug 会一直复发（项目设置早已吃过这个亏，
    /// 所以走 `from_project` 单入口）。
    ///
    /// 规则（按优先级）：
    /// 1. 第一个 `active == true` 的相机；
    /// 2. 没有 active 的相机时，退回第一个相机（把唯一的相机误点成 inactive
    ///    不该让画面变成"没有相机"）；
    /// 3. 一台相机都没有 → `None`（宿主退回旧机位，老场景零影响）。
    ///
    /// 用**场景顺序**而不是"谁先被创建"：场景文件的顺序就是作者眼里的层级顺序，
    /// 重排场景会改变结果，这是可预期、可 diff 的。
    pub fn active_camera_index(&self) -> Option<usize> {
        let mut first_any = None;
        for (index, entity) in self.entities.iter().enumerate() {
            let Some(camera) = entity.camera.as_ref() else {
                continue;
            };
            if camera.active {
                return Some(index);
            }
            first_any.get_or_insert(index);
        }
        first_any
    }

    /// 主相机实体（见 [`active_camera_index`](Self::active_camera_index)）。
    pub fn active_camera(&self) -> Option<&Entity> {
        self.active_camera_index().map(|i| &self.entities[i])
    }

    /// 按名称查找实体。
    pub fn find_by_name(&self, name: &str) -> Option<&Entity> {
        self.entities.iter().find(|e| e.name == name)
    }

    /// 添加实体并返回其 ID。
    pub fn add_entity(&mut self, entity: Entity) -> EntityId {
        let id = entity.id;
        self.entities.push(entity);
        id
    }

    /// 按 ID 移除**单个**实体。
    ///
    /// 它的子节点会被提升到被删实体的父节点下（"删除中间节点、子树不散架"），
    /// 而不是连坐删除。要连子树一起删，用 [`remove_subtree`](Self::remove_subtree)。
    pub fn remove_entity(&mut self, id: EntityId) -> Option<Entity> {
        let idx = self.find_entity(id)?;
        let promote_to = self.entities[idx].parent;
        for entity in &mut self.entities {
            if entity.parent == Some(id) {
                entity.parent = promote_to;
            }
        }
        Some(self.entities.remove(idx))
    }

    // -----------------------------------------------------------------------
    // 层级（父子关系）
    // -----------------------------------------------------------------------

    /// 按 ID 取实体在 `entities` 中的索引。
    ///
    /// 与 [`find_entity`](Self::find_entity) 等价；提供这个名字是为了在层级相关
    /// 代码里读起来更贴合语义（"把 id 换成索引"）。
    pub fn index_of(&self, id: EntityId) -> Option<usize> {
        self.entities.iter().position(|e| e.id == id)
    }

    /// 按索引查父节点在 `entities` 中的索引。
    ///
    /// 父 ID 指向一个不存在的实体（比如手工编辑过的场景文件）时返回 `None`，
    /// 视作根节点——坏数据不应该让整棵树算不出来。
    pub fn parent_index(&self, index: usize) -> Option<usize> {
        let parent_id = self.entities.get(index)?.parent?;
        self.index_of(parent_id).filter(|&p| p != index)
    }

    /// 某个实体的直接子节点索引列表（按实体顺序）。
    ///
    /// 每次调用都会扫一遍整张实体表（O(n)）。**在循环里遍历层级时不要调它**
    /// ——那会把一次遍历退化成 O(n²)。那种场景请先用 [`child_map`](Self::child_map)
    /// 一次性建好子节点索引，再按索引取。
    pub fn children(&self, index: usize) -> Vec<usize> {
        let Some(id) = self.entities.get(index).map(|e| e.id) else {
            return Vec::new();
        };
        self.entities
            .iter()
            .enumerate()
            .filter(|(i, e)| *i != index && e.parent == Some(id))
            .map(|(i, _)| i)
            .collect()
    }

    /// 一次性解算**所有**实体的父子关系，把「反复扫表」的层级算法收成 O(n)。
    ///
    /// 返回 `parent_indices`（父索引，`None` = 根）与 `child_map`（每个索引的子索引
    /// 列表，顺序与 [`children`](Self::children) 一致）。
    ///
    /// 场景的父子关系只存了 `Entity::parent` 这一条 ID 引用（见 `Entity::parent`
    /// 的说明），所以任何"按父找子"的算法天然是 O(n) 一次的；面板要逐帧遍历整棵树
    /// 时，逐节点调 `children()` 就是 O(n²)。这里用一张 ID→索引表把两边都压到 O(n)。
    pub fn hierarchy(&self) -> (Vec<Option<usize>>, Vec<Vec<usize>>) {
        let n = self.entities.len();
        let parent_indices = self.parent_indices();

        let mut child_map: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, parent) in parent_indices.iter().enumerate() {
            if let Some(p) = *parent {
                child_map[p].push(i);
            }
        }

        (parent_indices, child_map)
    }

    /// 一次性解算所有实体的父索引（与 `entities` 同序同长，`None` = 根）。
    ///
    /// 只需要父方向时用这个，比 [`hierarchy`](Self::hierarchy) 少建一整张子表 ——
    /// 平铺场景里子表是 n 个空 `Vec`，纯浪费。
    pub fn parent_indices(&self) -> Vec<Option<usize>> {
        let mut parents = Vec::new();
        let mut index_of = std::collections::HashMap::new();
        crate::scene_transforms::parent_indices_into(&self.entities, &mut parents, &mut index_of);
        parents
    }

    /// 一次性建好「每个索引 → 子索引列表」（顺序与 [`children`](Self::children) 一致）。
    ///
    /// 需要反复查子节点时（层级面板展开、子树操作）用它，别在循环里调 `children`。
    pub fn child_map(&self) -> Vec<Vec<usize>> {
        self.hierarchy().1
    }

    /// 根节点（没有父节点，或父节点不存在）的索引列表。O(n)。
    pub fn roots(&self) -> Vec<usize> {
        let (parent_indices, _) = self.hierarchy();
        (0..self.entities.len())
            .filter(|&i| parent_indices[i].is_none())
            .collect()
    }

    /// `ancestor` 是否是 `node` 的祖先（严格向上，不含自身）。
    pub fn is_ancestor(&self, ancestor: usize, node: usize) -> bool {
        let mut current = node;
        // 自环或手工构造的环会让朴素向上走陷入死循环，用步数上限兜底。
        for _ in 0..self.entities.len().max(1) + 1 {
            match self.parent_index(current) {
                Some(p) => {
                    if p == ancestor {
                        return true;
                    }
                    current = p;
                }
                None => return false,
            }
        }
        true // 步数内没走到根 = 存在环，保守认定为祖先，避免把它挂到自己子树下
    }

    /// 计算实体的**世界矩阵**（从根一路乘到该节点）。
    ///
    /// 中间带环检测：数据坏了也只是结果退化，绝不无限递归。
    pub fn world_matrix(&self, index: usize) -> Mat4 {
        if index >= self.entities.len() {
            return Mat4::IDENTITY;
        }
        // 沿父链向上时把矩阵左乘进结果，就能直接得到“根…父×自身”，无需先把
        // 整条链塞进临时 Vec 再反转。根运动与角色控制器每帧都会走这里，哪怕一只
        // 根实体每帧一次小分配也违反 World::update 的零分配合同。
        //
        // 坏数据里的环仍与旧实现同语义：Floyd 先算出“进入环前长度 + 环长”，
        // 随后每个唯一节点恰好乘一次；不能只用步数上限多乘一个重复节点。
        let next = |current: usize| self.parent_index(current);
        let mut slow = Some(index);
        let mut fast = Some(index);
        let meeting = loop {
            slow = slow.and_then(next);
            fast = fast.and_then(next).and_then(next);
            match (slow, fast) {
                (Some(a), Some(b)) if a == b => break Some(a),
                (None, _) | (_, None) => break None,
                _ => {}
            }
        };

        let unique_count = meeting.map(|meeting| {
            let mut prefix = 0usize;
            let mut a = index;
            let mut b = meeting;
            while a != b {
                a = next(a).expect("Floyd 环入口的两条链都必须留在环上");
                b = next(b).expect("Floyd 环入口的两条链都必须留在环上");
                prefix += 1;
            }
            let mut cycle = 1usize;
            let mut cursor = next(a).expect("检测到的环至少有一条父边");
            while cursor != a {
                cursor = next(cursor).expect("环内父边不能中断");
                cycle += 1;
            }
            prefix + cycle
        });

        let mut out = Mat4::IDENTITY;
        let mut current = Some(index);
        let mut visited = 0usize;
        while let Some(i) = current {
            if unique_count.is_some_and(|limit| visited >= limit) {
                break;
            }
            out = self.entities[i].transform.local_matrix() * out;
            visited += 1;
            current = next(i);
        }
        out
    }

    /// 一次性算出所有实体的世界矩阵（与 `entities` 同序同长）。
    ///
    /// 逐帧渲染、物理同步都要用，逐个调用 [`world_matrix`](Self::world_matrix)
    /// 会在深树上退化成 O(n·深度)。这里自底向上带记忆化，整体 O(n)。
    ///
    /// 注意 `parent_indices` 必须**一次性**算好：直接调 [`parent_index`](Self::parent_index)
    /// 每条边都要线性扫一遍实体表找父 ID，整个函数会退化成 O(n²)。
    pub fn world_matrices(&self) -> Vec<Mat4> {
        self.world_matrices_with_parents().1
    }

    /// 一次层级解算**同时**给出父索引与世界矩阵。
    ///
    /// 分开调 `parent_indices()` 与 `world_matrices()` 会重复建父索引。
    /// 需要逐帧复用缓冲或按父子顺序回写时，使用 [`crate::SceneTransforms`]。
    pub fn world_matrices_with_parents(&self) -> (Vec<Option<usize>>, Vec<Mat4>) {
        let mut transforms = crate::SceneTransforms::default();
        transforms.update(self);
        transforms.into_parts()
    }

    /// 实体的世界变换（位置 / 旋转 / 缩放）。
    pub fn world_transform(&self, index: usize) -> Transform {
        Transform::from_matrix(self.world_matrix(index))
    }

    /// 直接设置实体的**世界**位置（内部换算成局部）。
    ///
    /// 物理（角色控制器 / 动态刚体）始终在世界空间工作，而实体存的是局部
    /// 变换。根节点占绝大多数，走零矩阵运算的快路径；有父节点时才解一次
    /// 父世界矩阵的逆 —— 角色通常不挂父节点，这条慢路径只在真需要时付钱。
    ///
    /// 父世界矩阵不可逆（父节点缩放含 0）时退化成"把世界坐标当局部写"：
    /// 宁可位置偏一点，也不要把 NaN 写进变换。
    pub fn set_world_position(&mut self, index: usize, position: Vec3) {
        if index >= self.entities.len() {
            return;
        }
        let parent = self.entities[index].parent.and_then(|id| self.index_of(id));
        let local = match parent {
            None => position,
            Some(p) => {
                let parent_world = self.world_matrix(p);
                if parent_world.determinant().abs() > 1e-12 {
                    parent_world.inverse().transform_point3(position)
                } else {
                    position
                }
            }
        };
        self.entities[index].transform.position = local;
    }

    /// 重新挂载父节点，**保留世界变换**（Unity 的 "keep world transform" 语义）。
    ///
    /// 返回 `false` 表示这次挂载被拒绝：索引越界、挂到自己身上、或挂到自己的
    /// 子孙节点下（会成环）。拒绝时不做任何改动。
    pub fn set_parent(&mut self, child: usize, parent: Option<usize>) -> bool {
        if child >= self.entities.len() {
            return false;
        }
        if let Some(p) = parent {
            if p >= self.entities.len() || p == child {
                return false;
            }
            if self.is_ancestor(child, p) {
                return false; // 不能把父节点挂进自己的子树
            }
        }

        let world = self.world_matrix(child);
        let parent_world = parent
            .map(|p| self.world_matrix(p))
            .unwrap_or(Mat4::IDENTITY);
        // 局部 = 父世界⁻¹ · 世界。父世界不可逆时（含 0 缩放）退化成不改局部，
        // 总比把实体扔到 NaN 坐标里强。
        let det = parent_world.determinant();
        let local = if det.abs() > 1e-12 {
            parent_world.inverse() * world
        } else {
            self.entities[child].transform.local_matrix()
        };

        self.entities[child].parent = parent.map(|p| self.entities[p].id);
        self.entities[child].transform = Transform::from_matrix(local);
        true
    }

    /// 某个实体及其全部子孙的索引（前序，含自身）。
    pub fn subtree(&self, index: usize) -> Vec<usize> {
        let mut out = Vec::new();
        if index >= self.entities.len() {
            return out;
        }
        // 子节点索引一次建好：在循环里逐个调 `children()` 会把整次遍历拖成 O(n²)。
        let child_map = self.child_map();
        let mut visited = vec![false; self.entities.len()];
        let mut stack = vec![index];
        while let Some(i) = stack.pop() {
            if visited[i] {
                continue; // 环保护
            }
            visited[i] = true;
            out.push(i);
            for &child in &child_map[i] {
                stack.push(child);
            }
        }
        out
    }

    /// 删除某个实体**连同其整棵子树**，返回被删掉的实体。
    ///
    /// 这是编辑器 `Delete` 的语义（与 Unity 一致：删父节点，子节点一起走）。
    ///
    /// # 为什么按**索引掩码**筛，而不是先收集 ID 再按 ID 比对
    ///
    /// 旧实现是 `ids.contains(&entity.id)`。它隐含了一个"ID 全场景唯一"的前提，
    /// 而这个前提会被读盘破坏（文件里存的是上一次运行的 ID，进程重启后计数器
    /// 又从 1 开始）—— 一旦两条件实物撞了 ID，删其中一条会把另一条也带走，
    /// 现象就是"删一个物体，别的物体跟着消失"。
    ///
    /// `subtree` 给出的是**索引**，索引天然唯一，用掩码筛就没有这个前提。
    /// 顺带把 `O(n²)`（外层 n 次 × `contains` 线性扫）降成 `O(n)`。
    pub fn remove_subtree(&mut self, index: usize) -> Vec<Entity> {
        let doomed = self.subtree(index);
        if doomed.is_empty() {
            return Vec::new();
        }
        let mut mask = vec![false; self.entities.len()];
        for &i in &doomed {
            mask[i] = true;
        }

        let mut removed = Vec::with_capacity(doomed.len());
        let mut kept = Vec::with_capacity(self.entities.len() - doomed.len());
        for (i, entity) in std::mem::take(&mut self.entities).into_iter().enumerate() {
            if mask[i] {
                removed.push(entity);
            } else {
                kept.push(entity);
            }
        }
        self.entities = kept;
        removed
    }

    /// 修复**重复 ID**，返回被重新分配的条数。
    ///
    /// 读盘边界调一次（见 `ProjectHandle::parse_scene_text`）。存在意义：
    /// 旧版本的计数器不对齐会把这个 bug **写进文件**（两条件物同一个 `id:`），
    /// 光修分配器救不回已有的坏文件 —— 这里把后出现的重复项改成一个新 ID，
    /// 让它们重新可区分。
    ///
    /// # 已知局限
    ///
    /// 文件里既然出现重复 ID，"谁是谁的父节点"这件事**已经不可还原**了：
    /// 指向该 ID 的 `parent` 会落到第一个同 ID 的实体上。层级可能因此差一层，
    /// 但"删除连坐"这种破坏性后果不会再发生 —— 修的是可恢复性，不是原样复原。
    pub fn ensure_unique_ids(&mut self) -> usize {
        let mut seen: std::collections::HashSet<EntityId> =
            std::collections::HashSet::with_capacity(self.entities.len());
        let mut repaired = 0;
        for entity in &mut self.entities {
            if seen.insert(entity.id) {
                continue;
            }
            entity.id = EntityId::new();
            seen.insert(entity.id);
            repaired += 1;
        }
        repaired
    }

    /// **只报告**重复的实体 ID（`(id, 出现次数)`，按首次出现排序）。
    ///
    /// 与 [`Self::ensure_unique_ids`] 是一对：那个在加载边界**修数据**，这个
    /// 只**看数据**。存在的理由很具体 —— 加载期既然会顺手修好，任何"加载后
    /// 再检查"的路径（`--check` / CI / MCP）就永远看不到落盘伤，而落盘伤正是
    /// "删一个物体连带删别的"的成因。想知道文件里到底有没有病，只能在修之前看。
    ///
    /// 顺序按**首次出现**排，不用 `HashMap` 的迭代顺序：`--check` 的输出要能
    /// 直接 diff，随机顺序会让每次运行都不一样。
    pub fn duplicate_ids(&self) -> Vec<(EntityId, usize)> {
        crate::id::duplicate_ids(self.entities.iter().map(|e| e.id))
    }

    /// 场景是否为空（无实体）。
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// 实体数量。
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// 返回所有具有模型的实体（用于物理初始化等）。
    pub fn entities_with_model(&self) -> impl Iterator<Item = &Entity> {
        self.entities.iter().filter(|e| e.has_model())
    }

    // -----------------------------------------------------------------------
    // 预制体实例
    // -----------------------------------------------------------------------

    /// 实例化预制体并把它追加到场景，返回**实例根**在 `entities` 中的索引。
    ///
    /// 预制体为空时返回 `None`（不产生任何改动）。追加的实体是一棵自包含的树，
    /// 落在场景根层——调用方若想让它挂在某个父节点下，再调 [`set_parent`](Self::set_parent)
    /// （会保留世界变换）。
    pub fn instantiate_prefab(
        &mut self,
        prefab: &crate::prefab::Prefab,
        source: &str,
    ) -> Option<usize> {
        if prefab.is_empty() {
            return None;
        }
        let base = self.entities.len();
        let new_entities = prefab.instantiate(source);
        // 实例根在新增块里的偏移（正常只有一个）
        let root_offset = new_entities
            .iter()
            .position(|e| e.prefab.as_ref().is_some_and(|p| p.is_root))?;
        self.entities.extend(new_entities);
        Some(base + root_offset)
    }

    /// 用资产覆盖回实例（Unity 的 Revert），**保留实例内实体的 ID**。
    ///
    /// 与 [`Self::instantiate_prefab`] 的差别只在"ID 从哪来"：
    ///
    /// | | 新增实例 | Revert |
    /// |---|---|---|
    /// | 实例里已有（按 `source_id` 对位） | — | **沿用旧 ID** |
    /// | 资产里新增 | 新 ID | 新 ID |
    /// | 实例里多出来（资产里没有） | — | 删除 |
    ///
    /// 为什么必须保留旧 ID：整棵重建会把子树里每个实体的 ID 全换一遍，于是
    /// ① 场景文件的 diff 变成纯噪音（用户只是点了"还原"，却看到整块 ID 变了）；
    /// ② 任何按 ID 记住这些实体的地方（跨帧缓存、外部工具、用户脚本里存的 id）
    /// 全部失效。Unity 的 Revert 也是保留 ID 的。
    ///
    /// `root` 必须落在实例根上；返回**新实例根**的索引。索引会变（子树被整块
    /// 重建、可能不在末尾），调用方必须用返回值而不是旧索引。
    ///
    /// 根的**世界变换**刻意不由这里还原 —— 那是宿主（编辑器 / 工具）的事：
    /// 用户把实例拖到别处之后再点"还原"，要的是内容回到出厂，位置留在原地。
    pub fn revert_prefab(
        &mut self,
        root: usize,
        prefab: &crate::prefab::Prefab,
        source: &str,
    ) -> Option<usize> {
        if prefab.is_empty() || root >= self.entities.len() {
            return None;
        }
        // 旧实体的 `来源资产 ID → 当前实体 ID`：Revert 后要按它把 ID 认回去。
        // 只认**同一个来源**的实例实体，避免把别的预制体的 ID 错认过来。
        let keep: std::collections::HashMap<EntityId, EntityId> = self
            .subtree(root)
            .into_iter()
            .filter_map(|index| {
                let entity = self.entities.get(index)?;
                let link = entity.prefab.as_ref().filter(|p| p.source == source)?;
                Some((link.source_id, entity.id))
            })
            .collect();

        // 整块替换：删掉旧子树，再按资产的实体表重建。索引在这里全部重排。
        self.remove_subtree(root);

        let base = self.entities.len();
        let rebuilt = prefab.instantiate_with(source, |source_id| {
            keep.get(&source_id).copied().unwrap_or_else(EntityId::new)
        });
        let root_offset = rebuilt
            .iter()
            .position(|e| e.prefab.as_ref().is_some_and(|p| p.is_root))?;
        self.entities.extend(rebuilt);

        let new_root = base + root_offset;
        // 保险：万一旧数据里两条实体共用了同一个 ID（手改过的场景），沿用会造出
        // 重复 ID。这里补一次去重 —— 只可能动到"多出来的那条"，正常的实例不受影响。
        self.ensure_unique_ids();
        // `ensure_unique_ids` 可能重排 ID 但不会改变索引顺序，所以 `new_root` 仍有效。
        Some(new_root)
    }

    /// 找实体所属的**预制体实例根**索引。
    ///
    /// 沿父链上溯，遇到第一个 `is_root` 的实体即为答案；自己就是实例内实体、
    /// 或完全不属于任何实例时返回 `None`。
    pub fn prefab_instance_root(&self, index: usize) -> Option<usize> {
        let mut current = index;
        // 步数上限兜底：数据坏了（成环）也不至于把 UI 卡死。
        for _ in 0..self.entities.len().max(1) + 1 {
            match self.entities.get(current)?.prefab.as_ref() {
                Some(link) if link.is_root => return Some(current),
                // 在实例内部但不是根：继续往上找根
                Some(_) => current = self.parent_index(current)?,
                // 不是实例实体：到此为止
                None => return None,
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn entity_new_has_id() {
        let e = Entity::new("Test");
        assert_eq!(e.name, "Test");
        // ID 自动分配且非零
        assert_ne!(e.id.raw(), 0);
    }

    #[test]
    fn persisted_layer_participates_in_equality() {
        let saved = Entity::new("Actor");
        let mut edited = saved.clone();
        edited.layer = 3;
        assert_ne!(saved, edited, "仅修改层也必须让场景变脏");
    }

    #[test]
    fn persisted_morph_weights_participate_in_equality() {
        let saved = Entity::new("Face");
        let mut edited = saved.clone();
        edited.morph_weights = vec![0.5];
        assert_ne!(saved, edited, "仅修改形变权重也必须让场景变脏");
    }

    /// 形变目标权重随实体走一遍 RON 往返，一个分量都不能丢。
    ///
    /// 权重是**浮点数组**：一旦序列化格式漂了（比如被写成字符串或整型），
    /// 表情会在存盘再打开之后变成全 0 —— 而且没有任何报错。
    #[test]
    fn morph_weights_survive_a_ron_roundtrip() {
        let mut entity = Entity::new("Face");
        entity.morph_weights = vec![0.25, 1.0, 0.0, 0.5];
        let text = ron::to_string(&entity).expect("实体可序列化");
        let back: Entity = ron::from_str(&text).expect("实体可反序列化");
        assert_eq!(back.morph_weights, entity.morph_weights);
    }

    /// 没有形变权重的实体**不写**这一行：场景文件不该被空数组灌满。
    #[test]
    fn entity_without_morph_weights_omits_the_field() {
        let text = ron::to_string(&Entity::new("Plain")).expect("可序列化");
        assert!(!text.contains("morph_weights"), "空表不该落盘：{text}");
    }

    /// 旧场景文件没有这个字段 → 空表（行为与引入 morph 之前逐位一致）。
    #[test]
    fn old_scene_without_morph_field_still_loads() {
        let entity: Entity = ron::from_str(r#"(name: "Legacy")"#).expect("旧实体可反序列化");
        assert!(entity.morph_weights.is_empty());
    }

    #[test]
    fn scene_add_find() {
        let mut scene = Scene::new("Test");
        let e = Entity::new("Box").with_model("test.glb");
        let id = scene.add_entity(e);
        assert_eq!(scene.find_entity(id), Some(0));
        assert!(scene.find_by_name("Box").is_some());
    }

    #[test]
    fn scene_remove() {
        let mut scene = Scene::new("Test");
        let e = Entity::new("ToRemove");
        let id = scene.add_entity(e);
        assert!(scene.remove_entity(id).is_some());
        assert!(scene.is_empty());
    }

    #[test]
    fn entity_id_uniqueness() {
        let a = EntityId::new();
        let b = EntityId::new();
        assert_ne!(a, b);
    }

    // -- 粒子发射器（组件落盘与相等语义）----------------------------------

    #[test]
    fn particle_component_roundtrips_through_ron() {
        let mut emitter = crate::particle::ParticleEmitter::sparks();
        emitter.seed = 7;
        let entity = Entity::new("Spark").with_particle(emitter.clone());
        let text = ron::to_string(&entity).unwrap();
        assert!(text.contains("particle"), "挂了发射器就得写出来: {text}");
        let back: Entity = ron::from_str(&text).unwrap();
        assert_eq!(back.particle.unwrap(), emitter);
    }

    #[test]
    fn entity_without_particle_writes_no_line() {
        let text = ron::to_string(&Entity::new("Plain")).unwrap();
        assert!(
            !text.contains("particle"),
            "没配粒子的实体不该被一行 `particle: None` 灌满: {text}"
        );
    }

    #[test]
    fn particle_equality_ignores_burst_but_not_config() {
        let a = Entity::new("E").with_particle(crate::particle::ParticleEmitter::default());
        let mut b = a.clone();
        // 脚本写 burst = 运行态，不该让编辑器显示"未保存"。
        if let Some(p) = b.particle.as_mut() {
            p.burst = 12;
        }
        assert_eq!(a, b, "burst 不进相等判定");
        if let Some(p) = b.particle.as_mut() {
            p.rate = 1.0;
        }
        assert_ne!(a, b, "配置改了必须算改动");
    }

    // -- 相机（组件落盘 / 主相机解析）--------------------------------------

    #[test]
    fn camera_component_roundtrips_through_ron() {
        let camera = crate::camera::Camera {
            fov: 75.0,
            near: 0.05,
            far: 200.0,
            active: false,
        };
        let entity = Entity::new("Main Camera")
            .with_transform(Transform::from_translation(Vec3::new(0.0, 2.0, -4.0)))
            .with_camera(camera.clone());
        let text = ron::to_string(&entity).unwrap();
        assert!(text.contains("camera"), "挂了相机就得写出来: {text}");
        let back: Entity = ron::from_str(&text).unwrap();
        assert_eq!(back.camera.unwrap(), camera);
        // 位置也得跟着落盘 —— 机位就是"相机在哪"。
        assert_eq!(back.transform.position, Vec3::new(0.0, 2.0, -4.0));
    }

    #[test]
    fn entity_without_camera_writes_no_line() {
        let text = ron::to_string(&Entity::new("Plain")).unwrap();
        assert!(
            !text.contains("camera"),
            "没配相机的实体不该被 `camera: None` 灌满: {text}"
        );
    }

    #[test]
    fn old_scene_without_camera_field_loads_as_none() {
        // 这条盯的是**向后兼容**：相机字段出现之前的所有场景文件，
        // 都必须照常加载成"没有相机"，而不是解析失败。
        let mut scene = Scene::new("Old");
        scene.add_entity(Entity::new("Box").with_model("builtin://cube"));
        let text = ron::to_string(&scene).unwrap();
        let back: Scene = ron::from_str(&text).unwrap();
        assert!(back.entities.iter().all(|e| e.camera.is_none()));
    }

    #[test]
    fn camera_equality_tracks_projection_params() {
        // 改了 FOV 必须算"未保存"（脏检查靠 PartialEq）。
        let a = Entity::new("Cam").with_camera(crate::camera::Camera::default());
        let mut b = a.clone();
        if let Some(c) = b.camera.as_mut() {
            c.fov = 90.0;
        }
        assert_ne!(a, b);
        // active 也是落盘内容：多机位切换要能存盘。
        let mut c = a.clone();
        if let Some(cam) = c.camera.as_mut() {
            cam.active = false;
        }
        assert_ne!(a, c);
    }

    #[test]
    fn active_camera_prefers_active_then_falls_back() {
        let mut scene = Scene::new("Cams");
        scene.add_entity(Entity::new("NoCam"));
        // 第一个相机是 inactive -> 不该被选中（除非没有 active 的）。
        let idle = crate::camera::Camera {
            active: false,
            ..Default::default()
        };
        scene.add_entity(Entity::new("Idle Cam").with_camera(idle.clone()));
        let main = scene.add_entity(Entity::new("Main Cam").with_camera(Default::default()));

        assert_eq!(
            scene.active_camera_index(),
            Some(scene.index_of(main).unwrap())
        );
        assert_eq!(
            scene.active_camera().map(|e| e.name.as_str()),
            Some("Main Cam")
        );

        // 唯一 / 全部 inactive：退回第一个相机，而不是"没有相机"。
        let mut only_idle = Scene::new("OnlyIdle");
        only_idle.add_entity(Entity::new("Idle Cam").with_camera(idle));
        assert_eq!(only_idle.active_camera_index(), Some(0));

        // 一台都没有 -> None（宿主退回旧机位）。
        let mut none = Scene::new("None");
        none.add_entity(Entity::new("Box"));
        assert_eq!(none.active_camera_index(), None);
    }

    // -- 层级 -------------------------------------------------------------

    /// 造一个带父子关系的场景：root -> child -> grandchild，各带平移。
    fn hierarchy_scene() -> (Scene, [usize; 3]) {
        let mut scene = Scene::new("Hierarchy");
        let root = scene.add_entity(
            Entity::new("Root")
                .with_transform(Transform::from_translation(Vec3::new(1.0, 0.0, 0.0))),
        );
        let child = scene.add_entity(
            Entity::new("Child")
                .with_transform(Transform::from_translation(Vec3::new(0.0, 2.0, 0.0))),
        );
        let grandchild = scene.add_entity(
            Entity::new("Grandchild")
                .with_transform(Transform::from_translation(Vec3::new(0.0, 0.0, 3.0))),
        );
        assert!(scene.set_parent(
            scene.index_of(child).unwrap(),
            Some(scene.index_of(root).unwrap())
        ));
        assert!(scene.set_parent(
            scene.index_of(grandchild).unwrap(),
            Some(scene.index_of(child).unwrap())
        ));
        let r = scene.index_of(root).unwrap();
        let c = scene.index_of(child).unwrap();
        let g = scene.index_of(grandchild).unwrap();
        (scene, [r, c, g])
    }

    #[test]
    fn world_matrix_composes_parent_chain() {
        // 直接写 parent 字段（等价于场景文件里写死父子关系），不触发"保留世界变换"
        // 的局部重算——这里要验证的正是矩阵级联本身。
        let mut scene = Scene::new("Hierarchy");
        let root_id = scene.add_entity(
            Entity::new("Root")
                .with_transform(Transform::from_translation(Vec3::new(1.0, 0.0, 0.0))),
        );
        let child_id = scene.add_entity(
            Entity::new("Child")
                .with_transform(Transform::from_translation(Vec3::new(0.0, 2.0, 0.0))),
        );
        let grandchild_id = scene.add_entity(
            Entity::new("Grandchild")
                .with_transform(Transform::from_translation(Vec3::new(0.0, 0.0, 3.0))),
        );
        let ci = scene.index_of(child_id).unwrap();
        let gi = scene.index_of(grandchild_id).unwrap();
        scene.entities[ci].parent = Some(root_id);
        scene.entities[gi].parent = Some(child_id);

        // 世界位置 = 1 + 0 + 0, 0 + 2 + 0, 0 + 0 + 3
        let world = scene.world_transform(gi);
        assert!((world.position - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-5);
        // 父节点自身的世界变换不受影响
        assert!((scene.world_transform(ci).position - Vec3::new(1.0, 2.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn world_matrices_batch_matches_single() {
        let (scene, [r, c, g]) = hierarchy_scene();
        let batch = scene.world_matrices();
        for idx in [r, c, g] {
            assert_eq!(batch[idx], scene.world_matrix(idx));
        }
    }

    /// 深链，且**子节点排在父节点前面**（实体 0 是叶子、最后一个是根）。
    ///
    /// 这是"链缓冲复用 + 环保护"最容易写错的形状：向上走时每一步都要跨过
    /// 已经处理过的下标。两种顺序（父前 / 子前）都必须给出一致结果。
    #[test]
    fn world_matrices_handles_reversed_deep_chain() {
        const DEPTH: usize = 64;
        let mut scene = Scene::new("deep");
        for i in 0..DEPTH {
            scene.add_entity(
                Entity::new(format!("n{i}"))
                    .with_transform(Transform::from_translation(Vec3::new(1.0, 0.0, 0.0))),
            );
        }
        // n0 -> n1 -> ... -> n(DEPTH-1)，即子节点下标比父节点小。
        for i in 0..DEPTH - 1 {
            let parent = scene.entities[i + 1].id;
            scene.entities[i].parent = Some(parent);
        }

        let m = scene.world_matrices();
        // 每级平移 (1,0,0)，所以第 i 个节点的世界 x = 它到根的节点数。
        assert!(
            (m[0].w_axis.x - DEPTH as f32).abs() < 1e-4,
            "叶子应当把整条链的平移累加起来，x = {}",
            m[0].w_axis.x
        );
        assert!(
            (m[DEPTH - 1].w_axis.x - 1.0).abs() < 1e-4,
            "根节点只有自己那一级，x = {}",
            m[DEPTH - 1].w_axis.x
        );
        // 批量结果必须与逐个解算完全一致 —— 一致性比数值本身更重要。
        for (i, mat) in m.iter().enumerate() {
            assert_eq!(
                *mat,
                scene.world_matrix(i),
                "第 {i} 个节点批量/单个结果不一致"
            );
        }
    }

    /// 数据坏了（父引用成环）也不该死循环或产出 NaN —— 退化成可用的结果即可。
    /// `world_matrices` 在热路径上，任何一次"卡住"都等于整个编辑器冻死。
    #[test]
    fn world_matrices_survives_a_parent_cycle() {
        let mut scene = Scene::new("cycle");
        let a = scene.add_entity(Entity::new("a"));
        let b = scene.add_entity(Entity::new("b"));
        let ai = scene.index_of(a).unwrap();
        let bi = scene.index_of(b).unwrap();
        scene.entities[ai].parent = Some(b);
        scene.entities[bi].parent = Some(a);

        let m = scene.world_matrices();
        assert_eq!(m.len(), 2);
        assert!(m.iter().all(|mm| mm.is_finite()), "成环不该产出 NaN");
    }

    #[test]
    fn children_and_roots() {
        let (scene, [r, c, _]) = hierarchy_scene();
        assert_eq!(scene.roots(), vec![r]);
        assert_eq!(scene.children(r), vec![c]);
        assert!(scene.children(c).len() == 1);
    }

    /// `hierarchy()` 是 `children()` / `roots()` / `parent_index()` 的批量版本，
    /// 三条逐节点 API 是它的"逐条版"。二者必须完全一致 —— 不一致就等于层级解算
    /// 有两套真相，面板显示的和实际渲染的会分家。
    #[test]
    fn hierarchy_batch_matches_per_node_apis() {
        let (mut scene, [r, c, g]) = hierarchy_scene();

        let check = |scene: &Scene| {
            let (parent_indices, child_map) = scene.hierarchy();
            for i in 0..scene.entities.len() {
                assert_eq!(
                    parent_indices[i],
                    scene.parent_index(i),
                    "父索引不一致: {i}"
                );
                assert_eq!(child_map[i], scene.children(i), "子列表不一致: {i}");
            }
            let roots: Vec<usize> = (0..scene.entities.len())
                .filter(|&i| parent_indices[i].is_none())
                .collect();
            assert_eq!(roots, scene.roots());
        };

        check(&scene);
        scene.set_parent(g, Some(r));
        check(&scene);
        scene.set_parent(c, None);
        check(&scene);
    }

    /// 父 ID 指向不存在的实体（手工编辑过的场景文件）应当被当作根，
    /// 而不是让整棵树的解算崩掉或挂到别人身上。
    #[test]
    fn hierarchy_treats_dangling_parent_as_root() {
        let mut scene = Scene::new("dangling");
        scene.add_entity(Entity::new("orphan"));
        scene.entities[0].parent = Some(EntityId::new()); // 指向一个不存在的 ID

        let (parent_indices, child_map) = scene.hierarchy();
        assert_eq!(parent_indices[0], None);
        assert!(child_map[0].is_empty());
        assert_eq!(scene.roots(), vec![0]);
        assert_eq!(scene.world_matrices().len(), 1);
    }

    #[test]
    fn reparent_preserves_world_transform() {
        let (mut scene, [r, c, g]) = hierarchy_scene();
        let before = scene.world_transform(g);
        // 把 grandchild 从 child 挪到 root 下
        assert!(scene.set_parent(g, Some(r)));
        let after = scene.world_transform(g);
        assert!(
            (before.position - after.position).length() < 1e-5,
            "世界位置应当保持不变"
        );
        assert_eq!(scene.children(r).len(), 2);

        // 挪回根层（解除父子）
        assert!(scene.set_parent(g, None));
        let unparented = scene.world_transform(g);
        assert!((before.position - unparented.position).length() < 1e-5);
        assert_eq!(scene.roots().len(), 2);

        // child 现在没有子节点了
        assert!(scene.children(c).is_empty());
    }

    #[test]
    fn reparent_rejects_cycles_and_self() {
        let (mut scene, [r, c, g]) = hierarchy_scene();
        // 不能把 root 挂到自己的子孙下
        assert!(!scene.set_parent(r, Some(c)));
        assert!(!scene.set_parent(r, Some(g)));
        // 不能挂到自己身上
        assert!(!scene.set_parent(c, Some(c)));
        // 结构没被破坏
        assert_eq!(scene.roots(), vec![r]);
        assert_eq!(scene.children(r), vec![c]);
    }

    #[test]
    fn remove_subtree_deletes_descendants() {
        let (mut scene, [r, c, g]) = hierarchy_scene();
        let removed = scene.remove_subtree(r);
        assert_eq!(removed.len(), 3);
        assert!(scene.entities.is_empty());
        // 确认 id 对得上（顺序即原 entities 顺序）
        let ids: Vec<EntityId> = removed.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 3);
        let _ = (c, g);
    }

    /// 撞 ID 时，删一条不能把另一条一起带走。
    ///
    /// 这是"删一个物体、别的物体跟着消失"的最小复现。旧实现先收集要删的 ID 再
    /// 用 `ids.contains(&entity.id)` 筛选，隐含"ID 全场景唯一"的前提；读盘回来的
    /// 实体与新分配的实体一旦共用 ID，两条都会被收走。
    #[test]
    fn remove_subtree_spares_other_entities_that_share_an_id() {
        let mut scene = Scene::new("Collide");
        let shared = EntityId::from_raw(777);

        // 模拟"从文件读出来"的实体：ID 是上一次运行分配的。
        let mut loaded = Entity::new("Loaded");
        loaded.id = shared;
        scene.add_entity(loaded);
        let loaded_child_id = scene.add_entity(Entity::new("LoadedChild"));
        assert!(scene.set_parent(
            scene.index_of(loaded_child_id).unwrap(),
            Some(scene.index_of(shared).unwrap())
        ));

        // 模拟"计数器没对齐时新建的实体"：拿到同一个 ID。
        let mut fresh = Entity::new("Fresh");
        fresh.id = shared;
        let fresh_idx = scene.entities.len();
        scene.add_entity(fresh);

        let removed = scene.remove_subtree(fresh_idx);
        assert_eq!(removed.len(), 1, "只该删掉 Fresh：{removed:?}");
        assert_eq!(removed[0].name, "Fresh");
        assert_eq!(scene.entities.len(), 2, "Loaded 与它的子节点必须还在");
        assert!(scene.index_of(shared).is_some(), "Loaded 仍在场景里");
        assert!(
            scene.index_of(loaded_child_id).is_some(),
            "Loaded 的子节点也必须还在"
        );
    }

    #[test]
    fn ensure_unique_ids_reassigns_duplicates() {
        let mut scene = Scene::new("Dup");
        let shared = EntityId::from_raw(31);
        let mut a = Entity::new("A");
        a.id = shared;
        let mut b = Entity::new("B");
        b.id = shared;
        let mut c = Entity::new("C");
        c.id = EntityId::from_raw(32);
        scene.add_entity(a);
        scene.add_entity(b);
        scene.add_entity(c);

        assert_eq!(scene.ensure_unique_ids(), 1, "只有第二个 A/B 需要改");
        // 第一个出现的保留原 ID（父引用都指向它）
        assert_eq!(scene.entities[0].id, shared);
        assert_ne!(scene.entities[1].id, shared);
        assert_eq!(scene.entities[2].id, EntityId::from_raw(32));
        // 幂等：再调一次没有可修的
        assert_eq!(scene.ensure_unique_ids(), 0);
    }

    /// ID 必须原样往返：这条同时守着自定义 `Deserialize` 的**格式兼容性**
    /// （RON 里仍是 `id: (12)`），以及"保存一次不该把 ID 洗一遍"。
    #[test]
    fn scene_ids_round_trip_unchanged() {
        let mut scene = Scene::new("RT");
        scene.add_entity(Entity::new("A"));
        scene.add_entity(Entity::new("B"));
        let before: Vec<EntityId> = scene.entities.iter().map(|e| e.id).collect();

        let text = ron::to_string(&scene).expect("场景应当能序列化");
        let mut back: Scene = ron::from_str(&text).expect("场景应当能反序列化");

        let after: Vec<EntityId> = back.entities.iter().map(|e| e.id).collect();
        assert_eq!(before, after, "ID 必须原样往返");
        assert_eq!(back.ensure_unique_ids(), 0, "往返不该产生重复 ID");
    }

    /// 读盘必须把全局计数器推过已加载的 ID。
    ///
    /// 不推的后果是下一条 `EntityId::new()` 返回一个**已被占用**的 ID ——
    /// 这正是"删一个连坐删别的"的源头。
    #[test]
    fn deserializing_an_id_reserves_the_id_counter() {
        #[derive(serde::Deserialize)]
        struct Holder {
            id: EntityId,
        }

        let loaded: Holder = ron::from_str("(id: (4000000))").expect("newtype 的 RON 形态");
        assert_eq!(loaded.id.raw(), 4_000_000);
        assert!(
            EntityId::new().raw() > 4_000_000,
            "读盘后新建的 ID 必须排在已加载 ID 之后，否则会撞"
        );
    }

    #[test]
    fn remove_entity_promotes_children() {
        let (mut scene, [r, c, g]) = hierarchy_scene();
        scene.remove_entity(scene.entities[c].id);
        // grandchild 被提升到 root 下，而不是随 child 一起消失
        assert_eq!(scene.entities.len(), 2);
        assert_eq!(
            scene.children(r),
            vec![scene.index_of(scene.entities[1].id).unwrap()]
        );
        let _ = g;
    }

    #[test]
    fn old_scene_without_parent_field_loads() {
        // 旧格式：没有 id / parent 字段，必须能解析且全部落在根层
        let ron = r#"Scene(
            name: "Old",
            entities: [
                (name: "A", transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)), model: ""),
                (name: "B", transform: (position: (1.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)), model: ""),
            ],
        )"#;
        let scene: Scene = ron::from_str(ron).expect("旧场景应当能反序列化");
        assert_eq!(scene.entities.len(), 2);
        assert!(scene.entities.iter().all(|e| e.parent.is_none()));
        assert_eq!(scene.roots().len(), 2);
        // 旧文件没有 animator 字段 → 全部是 None（= 不动画，与当时行为一致）
        assert!(scene.entities.iter().all(|e| e.animator.is_none()));
    }

    /// 旧场景文件（没有 behaviors 字段）必须照常加载：空行为列表 = 纯数据实体。
    ///
    /// 双轨计划 P0 的字段兼容回归网 —— serde 属性写错（漏 `default`）
    /// 会让所有既有场景文件当场加载失败。
    #[test]
    fn old_scene_without_behaviors_field_loads() {
        let ron = r#"Scene(
            name: "Old",
            entities: [
                (name: "A", transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)), model: ""),
            ],
        )"#;
        let scene: Scene = ron::from_str(ron).expect("旧场景应当能反序列化");
        assert!(scene.entities[0].behaviors.is_empty());

        // 挂了行为的实体：字段落盘、顺序保持（顺序即执行顺序）。
        let mut scene = Scene::new("New");
        let mut entity = Entity::new("A");
        entity.behaviors = vec!["Spinner".to_owned(), "Drift".to_owned()];
        scene.add_entity(entity);
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(text.contains("behaviors"), "挂了行为的实体必须落盘");
        let restored: Scene = ron::from_str(&text).unwrap();
        assert_eq!(restored.entities[0].behaviors, vec!["Spinner", "Drift"]);

        // 没挂行为的实体不写 behaviors 行（`skip_serializing_if` 的承诺）。
        let mut plain = Scene::new("Plain");
        plain.add_entity(Entity::new("A"));
        let text = ron::ser::to_string_pretty(&plain, ron::ser::PrettyConfig::default()).unwrap();
        assert!(!text.contains("behaviors"), "空列表不该污染场景文件");
    }

    /// P1.5：组件字段值必须落盘往返，且**旧场景文件**（没有 behavior_params
    /// 字段）照常加载。
    ///
    /// 双重回归：serde 属性漏 `default` ⇒ 旧文件加载失败（当场红）；
    /// 键用下标而不是注册名 ⇒ 这里的改名场景会读出错位的参数（更阴）。
    #[test]
    fn behavior_params_survive_ron_roundtrip_and_old_scenes_load() {
        // ① 旧文件没有 behavior_params 字段 → 空表
        let ron = r#"Scene(
            name: "Old",
            entities: [
                (name: "A", transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)), model: "", behaviors: ["Spinner"]),
            ],
        )"#;
        let scene: Scene = ron::from_str(ron).expect("旧场景应当能反序列化");
        assert!(scene.entities[0].behavior_params.is_empty());

        // ② 挂了参数的实体：逐值往返
        use crate::inspect::FieldValue;
        let mut scene = Scene::new("New");
        let mut entity = Entity::new("A");
        entity.behaviors = vec!["Spinner".to_owned()];
        let mut fields = crate::inspect::FieldValues::new();
        fields.insert("speed".to_owned(), FieldValue::F32(75.0));
        fields.insert("enabled".to_owned(), FieldValue::Bool(true));
        fields.insert("label".to_owned(), FieldValue::String("玩家".into()));
        entity
            .behavior_params
            .insert("Spinner".to_owned(), fields.clone());
        scene.add_entity(entity);

        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(text.contains("behavior_params"), "调过参的实体必须落盘");
        assert!(text.contains("speed"), "字段寻址名落盘");
        let restored: Scene = ron::from_str(&text).unwrap();
        assert_eq!(
            restored.entities[0].behavior_params.get("Spinner"),
            Some(&fields),
            "往返后逐值相等"
        );

        // ③ 没调过参的实体不写这一行（skip_serializing_if 的承诺）
        let mut plain = Scene::new("Plain");
        plain.add_entity(Entity::new("A"));
        let text = ron::ser::to_string_pretty(&plain, ron::ser::PrettyConfig::default()).unwrap();
        assert!(!text.contains("behavior_params"), "空表不该污染场景文件");

        // ④ 参数变化必须被 PartialEq 看见（编辑器脏检查靠它）
        let mut a = Entity::new("A");
        let mut b = a.clone();
        assert_eq!(a, b);
        let mut fields = crate::inspect::FieldValues::new();
        fields.insert("speed".to_owned(), FieldValue::F32(1.0));
        b.behavior_params.insert("Spinner".to_owned(), fields);
        assert_ne!(a, b, "只改参数也算改动，否则关窗不提示未保存");
        a.behavior_params = b.behavior_params.clone();
        assert_eq!(a, b);
    }

    // -- 动画组件 -----------------------------------------------------------

    /// 动画配置必须能落盘往返——它和 transform 一样是"作品的一部分"。
    #[test]
    fn animator_survives_ron_roundtrip() {
        let mut scene = Scene::new("A");
        scene.add_entity(
            Entity::new("Fox")
                .with_model("Resources/Fox.gltf")
                .with_animator(crate::animation::Animator::new("Walk")),
        );
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(text.contains("animator"), "动画组件必须落盘");
        let restored: Scene = ron::from_str(&text).unwrap();
        let a = restored.entities[0]
            .animator
            .as_ref()
            .expect("往返后动画组件仍在");
        assert_eq!(a.clip, "Walk");
        assert!(a.looping && a.autoplay);
        assert!((a.speed - 1.0).abs() < f32::EPSILON);
    }

    /// 没配动画的实体不该写出 `animator: None`——与 `prefab` 同一条理由：
    /// 场景文件不该被无意义的行灌满。
    #[test]
    fn plain_entity_omits_animator_field() {
        let mut scene = Scene::new("A");
        scene.add_entity(Entity::new("Plain"));
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(!text.contains("animator"), "普通实体不该写出 animator 字段");
    }

    /// 脏检查语义 = 落盘内容是否相同：改动画配置必须让实体"不相等"，
    /// 否则在 Inspector 里换剪辑不会被记成改动（不脏、不提示保存）。
    #[test]
    fn animator_participates_in_entity_equality() {
        let a = Entity::new("Fox").with_animator(crate::animation::Animator::new("Walk"));
        let mut b = a.clone();
        assert_eq!(a, b, "同一配置应当相等");
        b.animator.as_mut().unwrap().clip = "Run".into();
        assert_ne!(a, b, "换剪辑必须改变相等性（脏检查依赖它）");
    }

    // -- 音频组件 -----------------------------------------------------------

    /// 音频源必须能落盘往返——它是"作品的一部分"（何时响、响多大声）。
    #[test]
    fn audio_source_survives_ron_roundtrip() {
        let mut scene = Scene::new("A");
        scene.add_entity(
            Entity::new("Beep").with_audio(
                crate::audio::AudioSource::new("Resources/Audio/Beep.wav")
                    .with_looping(true)
                    .with_spatial(true),
            ),
        );
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(text.contains("audio"), "音频组件必须落盘");
        let restored: Scene = ron::from_str(&text).unwrap();
        let a = restored.entities[0]
            .audio
            .as_ref()
            .expect("往返后音频组件仍在");
        assert_eq!(a.clip, "Resources/Audio/Beep.wav");
        assert!(a.looping && a.spatial);
        assert!(a.autoplay, "autoplay 默认 true");
    }

    /// 没配音频的实体不该写出 `audio: None`（与 `animator` 同一条理由）。
    #[test]
    fn plain_entity_omits_audio_field() {
        let mut scene = Scene::new("A");
        scene.add_entity(Entity::new("Plain"));
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(!text.contains("audio"), "普通实体不该写出 audio 字段");
    }

    /// 脏检查语义：改音频配置必须让实体"不相等"，否则 Inspector 里换
    /// 剪辑不会被记成改动。
    #[test]
    fn audio_participates_in_entity_equality() {
        let a = Entity::new("Beep").with_audio(crate::audio::AudioSource::new("a.wav"));
        let mut b = a.clone();
        assert_eq!(a, b, "同一配置应当相等");
        b.audio.as_mut().unwrap().clip = "b.wav".into();
        assert_ne!(a, b, "换剪辑必须改变相等性（脏检查依赖它）");
    }

    // -- 材质覆盖 -----------------------------------------------------------

    /// 材质覆盖必须能落盘往返 —— 它是"作品的一部分"（什么颜色、多亮、多金属）。
    #[test]
    fn material_override_survives_ron_roundtrip() {
        let mut scene = Scene::new("A");
        scene.add_entity(Entity::new("Crate").with_material(crate::MaterialOverride {
            base_color: Some([0.8, 0.25, 0.2, 1.0]),
            metallic: Some(0.0),
            roughness: Some(0.65),
        }));
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(text.contains("material"), "材质覆盖必须落盘");
        let restored: Scene = ron::from_str(&text).unwrap();
        let m = restored.entities[0].material.expect("往返后材质覆盖仍在");
        assert_eq!(m.base_color, Some([0.8, 0.25, 0.2, 1.0]));
        assert_eq!(m.metallic, Some(0.0));
        assert_eq!(m.roughness, Some(0.65));
    }

    /// 没配材质的实体不该写出 `material: None`（与 `audio` 同一条理由）；
    /// 旧场景文件（没有这个字段）解析成 `None` = 行为逐位不变。
    #[test]
    fn plain_entity_omits_material_field() {
        let mut scene = Scene::new("A");
        scene.add_entity(Entity::new("Plain"));
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(!text.contains("material"), "普通实体不该写出 material 字段");

        // 旧文件路径：手工构造一条不含 material 的实体 RON，必须解析成 None。
        let old = r#"Scene(
            name: "Old",
            entities: [
                (name: "A", transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)), model: ""),
            ],
        )"#;
        let parsed: Scene = ron::from_str(old).expect("旧文件必须能解析");
        assert!(parsed.entities[0].material.is_none());
    }

    /// 脏检查语义：改材质必须让实体"不相等"（Inspector 拖颜色滑块依赖它）。
    #[test]
    fn material_participates_in_entity_equality() {
        let a = Entity::new("Crate").with_material(crate::MaterialOverride {
            base_color: Some([1.0, 1.0, 1.0, 1.0]),
            ..Default::default()
        });
        let mut b = a.clone();
        assert_eq!(a, b, "同一配置应当相等");
        b.material.as_mut().unwrap().roughness = Some(0.9);
        assert_ne!(a, b, "改材质必须改变相等性（脏检查依赖它）");
    }

    // -- 材质资产引用 -------------------------------------------------------

    /// 材质资产引用必须落盘往返；没引用时整行不写。
    #[test]
    fn material_asset_reference_survives_roundtrip() {
        let mut scene = Scene::new("A");
        scene.add_entity(Entity::new("Crate").with_material_asset("Resources/Materials/Wood.mat"));
        scene.add_entity(Entity::new("Plain"));
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert_eq!(
            text.matches("material_asset").count(),
            1,
            "只有引用了材质的实体才写这一行: {text}"
        );

        let restored: Scene = ron::from_str(&text).unwrap();
        assert_eq!(
            restored.entities[0].material_asset.as_deref(),
            Some("Resources/Materials/Wood.mat")
        );
        assert!(restored.entities[1].material_asset.is_none());

        // 旧文件（没有这个字段）→ None，渲染行为不变。
        let old = r#"Scene(
            name: "Old",
            entities: [
                (name: "A", transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)), model: ""),
            ],
        )"#;
        let parsed: Scene = ron::from_str(old).expect("旧文件必须能解析");
        assert!(parsed.entities[0].material_asset.is_none());
    }

    /// 换材质必须让实体"不相等"（编辑器脏检查依赖它）。
    #[test]
    fn material_asset_participates_in_entity_equality() {
        let a = Entity::new("Crate").with_material_asset("Resources/Materials/A.mat");
        let mut b = a.clone();
        assert_eq!(a, b, "同一引用应当相等");
        b.material_asset = Some("Resources/Materials/B.mat".into());
        assert_ne!(a, b, "换材质必须改变相等性");
        b.material_asset = None;
        assert_ne!(a, b, "清空材质也必须改变相等性");
    }

    // -- 角色控制器 --------------------------------------------------------

    /// 角色参数必须能落盘往返 —— 手感是「作品的一部分」（能上多高的台阶、
    /// 能爬多陡的坡），打包出去的玩家拿的必须是作者调好的值。
    #[test]
    fn controller_survives_ron_roundtrip() {
        let mut scene = Scene::new("A");
        scene.add_entity(
            Entity::new("Player").with_controller(crate::CharacterController {
                radius: 0.4,
                height: 2.0,
                step_offset: 0.5,
                slope_limit: 60.0,
                skin_width: 0.01,
                snap_to_ground: 0.4,
            }),
        );
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(text.contains("controller"), "角色组件必须落盘");
        let restored: Scene = ron::from_str(&text).unwrap();
        let c = restored.entities[0].controller.expect("往返后角色组件仍在");
        assert_eq!(c.radius, 0.4);
        assert_eq!(c.height, 2.0);
        assert_eq!(c.step_offset, 0.5);
        assert_eq!(c.slope_limit, 60.0);
    }

    /// 没配角色的实体不该写出 `controller: None`；旧场景文件（没有这个
    /// 字段）也必须照常加载成 `None`（= 非角色实体，行为逐位不变）。
    #[test]
    fn plain_entity_omits_controller_field() {
        let mut scene = Scene::new("A");
        scene.add_entity(Entity::new("Plain"));
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(
            !text.contains("controller"),
            "普通实体不该写出 controller 字段"
        );

        // 旧文件路径：手工构造一条不含 controller 的实体 RON。
        let old = r#"Scene(
            name: "Old",
            entities: [
                (name: "A", transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)), model: ""),
            ],
        )"#;
        let parsed: Scene = ron::from_str(old).expect("旧文件必须能解析");
        assert!(parsed.entities[0].controller.is_none());
    }

    /// 脏检查语义：改角色参数必须让实体"不相等"（Inspector 调台阶高度
    /// 依赖它，否则关窗不提示未保存）。
    #[test]
    fn controller_participates_in_entity_equality() {
        let a = Entity::new("Player").with_controller(crate::CharacterController::default());
        let mut b = a.clone();
        assert_eq!(a, b, "同一配置应当相等");
        b.controller.as_mut().unwrap().step_offset = 0.5;
        assert_ne!(a, b, "改台阶高度必须改变相等性（脏检查依赖它）");
    }

    #[test]
    fn hierarchy_roundtrips_through_ron() {
        let (scene, _) = hierarchy_scene();
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        let restored: Scene = ron::from_str(&text).unwrap();
        // 父引用按 ID 持久化，还原后结构不变
        assert_eq!(restored.roots().len(), 1);
        let root = restored.roots()[0];
        let child = restored.children(root)[0];
        assert_eq!(restored.children(child).len(), 1);
    }

    // -- 预制体实例 --------------------------------------------------------

    #[test]
    fn prefab_ref_survives_ron_roundtrip() {
        let mut scene = Scene::new("P");
        let mut entity = Entity::new("Inst");
        entity.prefab = Some(crate::prefab::PrefabInstanceRef {
            source: "Prefabs/A.prefab.ron".into(),
            source_id: entity.id,
            is_root: true,
        });
        scene.add_entity(entity);

        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(text.contains("prefab"), "预制体来源必须落盘");
        let restored: Scene = ron::from_str(&text).unwrap();
        let link = restored.entities[0]
            .prefab
            .as_ref()
            .expect("往返后来源仍在");
        assert_eq!(link.source, "Prefabs/A.prefab.ron");
        assert!(link.is_root);
    }

    #[test]
    fn plain_entity_omits_prefab_field() {
        // 普通实体不该把 `prefab: None` 写进场景文件——否则文件会被这种
        // 无意义的行灌满，手工编辑时可读性极差。
        let mut scene = Scene::new("P");
        scene.add_entity(Entity::new("Plain"));
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(!text.contains("prefab"), "普通实体不该写出 prefab 字段");
    }

    #[test]
    fn prefab_instance_root_walks_up_to_root() {
        let mut scene = Scene::new("P");
        let mut root = Entity::new("Root");
        root.prefab = Some(crate::prefab::PrefabInstanceRef {
            source: "x.prefab.ron".into(),
            source_id: root.id,
            is_root: true,
        });
        let rid = scene.add_entity(root);

        let mut child = Entity::new("Child");
        child.prefab = Some(crate::prefab::PrefabInstanceRef {
            source: "x.prefab.ron".into(),
            source_id: child.id,
            is_root: false,
        });
        let cid = scene.add_entity(child);
        let ci = scene.index_of(cid).unwrap();
        scene.entities[ci].parent = Some(rid);
        let ri = scene.index_of(rid).unwrap();

        assert_eq!(
            scene.prefab_instance_root(ci),
            Some(ri),
            "内部节点应上溯到实例根"
        );
        assert_eq!(scene.prefab_instance_root(ri), Some(ri), "根解析到自身");

        let nid = scene.add_entity(Entity::new("Plain"));
        assert_eq!(
            scene.prefab_instance_root(scene.index_of(nid).unwrap()),
            None,
            "普通实体不属于任何实例"
        );
    }

    // -- Revert -----------------------------------------------------------------

    /// 造一份「Root + 两个子节点」的资产。
    fn three_node_prefab() -> crate::prefab::Prefab {
        let mut prefab = crate::prefab::Prefab::new("Enemy");
        let root = Entity::new("Root");
        let mut a = Entity::new("A");
        a.parent = Some(root.id);
        let mut b = Entity::new("B");
        b.parent = Some(root.id);
        prefab.entities = vec![root, a, b];
        prefab
    }

    #[test]
    fn revert_prefab_keeps_ids_and_drops_removed_nodes() {
        let source = "Prefabs/Enemy.prefab.ron";
        let v1 = three_node_prefab();
        let mut scene = Scene::new("S");
        let root_idx = scene.instantiate_prefab(&v1, source).unwrap();
        // 实例里"资产 ID → 场景 ID"的对照：这是 Revert 保 ID 的唯一依据。
        let instance_id = |s: &Scene, asset_id: EntityId| -> EntityId {
            s.entities
                .iter()
                .find(|e| e.prefab.as_ref().is_some_and(|p| p.source_id == asset_id))
                .unwrap_or_else(|| panic!("实例里找不到 source_id {asset_id:?}"))
                .id
        };
        let root_id = instance_id(&scene, v1.entities[0].id);
        let a_id = instance_id(&scene, v1.entities[1].id);
        let b_id = instance_id(&scene, v1.entities[2].id);
        assert_eq!(
            scene.entities[root_idx].id, root_id,
            "实例根就在返回的索引上"
        );

        // 资产 v2：Root + A（改名）+ C —— B 被删掉、C 是新增。
        // 从 v1 克隆，资产 ID 天然稳定（这就是"资产内 ID 稳定"的语义）。
        let mut v2 = v1.clone();
        v2.entities[1].name = "A renamed".into();
        let mut c = Entity::new("C");
        c.parent = Some(v1.entities[0].id);
        v2.entities[2] = c;

        let new_root = scene
            .revert_prefab(root_idx, &v2, source)
            .expect("非空资产应能还原");
        // 按**名字**取 ID：`subtree` 是 DFS（兄弟逆序），拿位置对号会脆。
        let node = |s: &Scene, name: &str| -> usize {
            s.subtree(new_root)
                .into_iter()
                .find(|&i| s.entities[i].name == name)
                .unwrap_or_else(|| panic!("还原后找不到实体 {name}"))
        };
        assert_eq!(scene.subtree(new_root).len(), 3, "资产里有三个实体");
        assert_eq!(node(&scene, "Root"), new_root, "实例根就在返回的索引上");
        assert_eq!(
            scene.entities[node(&scene, "Root")].id,
            root_id,
            "实例根必须沿用原 ID"
        );
        assert_eq!(
            scene.entities[node(&scene, "A renamed")].id,
            a_id,
            "存活节点必须沿用原 ID"
        );
        let c_id = scene.entities[node(&scene, "C")].id;
        assert_ne!(c_id, b_id, "资产里新增的节点不能捡被删节点的 ID");
        assert_ne!(c_id, root_id);
        assert_ne!(c_id, a_id);
        assert!(scene.index_of(b_id).is_none(), "资产里已删的节点必须消失");
        assert_eq!(scene.parent_index(node(&scene, "C")), Some(new_root));
        assert_eq!(scene.duplicate_ids(), Vec::new(), "还原后不能有重复 ID");
    }

    /// 空资产：**什么都不动**。旧路径是"先把实例删掉，再发现没东西可放"，
    /// 用户点一下还原反而丢了整棵子树。
    #[test]
    fn revert_prefab_with_an_empty_asset_keeps_the_instance() {
        let source = "Prefabs/Keep.prefab.ron";
        let mut v1 = crate::prefab::Prefab::new("Keep");
        v1.entities = vec![Entity::new("Root")];
        let mut scene = Scene::new("S");
        let root_idx = scene.instantiate_prefab(&v1, source).unwrap();
        let root_id = scene.entities[root_idx].id;

        let empty = crate::prefab::Prefab::new("Empty");
        assert!(scene.revert_prefab(root_idx, &empty, source).is_none());
        assert_eq!(scene.entity_count(), 1, "空资产不该删掉实例");
        assert_eq!(scene.entities[0].id, root_id);
    }

    /// 手改过的场景里实例内部可能撞 ID；沿用旧 ID 不能把重复带进结果。
    #[test]
    fn revert_prefab_repairs_duplicate_ids_in_a_hand_edited_instance() {
        let source = "Prefabs/Dup.prefab.ron";
        let v1 = three_node_prefab();
        let mut scene = Scene::new("S");
        let root_idx = scene.instantiate_prefab(&v1, source).unwrap();
        let root_id = scene.entities[root_idx].id;
        // 手改：子节点被写成和根同一个 ID（只有编辑器外部改文件才会这样）。
        scene.entities[root_idx + 1].id = root_id;
        assert_eq!(scene.duplicate_ids().len(), 1, "前置条件：确实撞了");

        let new_root = scene.revert_prefab(root_idx, &v1, source).unwrap();
        assert_eq!(scene.entities[new_root].id, root_id, "根保留原 ID");
        assert_eq!(scene.duplicate_ids(), Vec::new(), "还原后必须自愈");
    }
}
