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
    /// glTF 模型路径（相对于工作区）。空字符串表示没有模型。
    ///
    /// 缺省 = 空串：纯 UI / 纯逻辑实体（触发器、生成点、HUD）本来就没有网格，
    /// 强制写一行 `model: ""` 只是噪音。
    #[serde(default)]
    pub model: String,
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
            model: String::new(),
            id: EntityId::new(),
            parent: None,
            prefab: None,
            scripts: Vec::new(),
            behaviors: Vec::new(),
            behavior_params: BTreeMap::new(),
            physics: None,
            animator: None,
            audio: None,
            material: None,
            controller: None,
            ui: None,
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

    /// 设置变换。
    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
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

    /// 实体是否有模型关联。
    pub fn has_model(&self) -> bool {
        !self.model.is_empty()
    }
}

impl PartialEq for Entity {
    /// 逐字段比较，**唯独不比 `components`**：
    ///
    /// - `components` 是 `#[serde(skip)]` 的运行时数据，不落盘 —— 场景相等
    ///   问的本来就是"落盘内容一样吗"（脏检查的语义），运行时组件与此无关；
    /// - 它装的是 `Box<dyn Any>`，类型擦除后根本没法比，硬比只能比个寂寞。
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.transform == other.transform
            && self.model == other.model
            && self.id == other.id
            && self.parent == other.parent
            && self.prefab == other.prefab
            && self.scripts == other.scripts
            && self.behaviors == other.behaviors
            // 组件参数也是落盘内容（P1.5）——脏检查必须比它，否则调完参按
            // 保存没反应 / 关窗不提示未保存。
            && self.behavior_params == other.behavior_params
            && self.physics == other.physics
            && self.animator == other.animator
            && self.audio == other.audio
            // 材质覆盖同样是落盘内容：调完颜色按保存得有反应。
            && self.material == other.material
            // 角色参数同样是落盘内容：Inspector 里调完半径按保存得有反应。
            && self.controller == other.controller
            // UI 元素同理，但**只比落盘字段**：`UiState`（悬停 / 按下 / 点击）
            // 每帧都在变，算进相等就永远是"未保存"。见 `UiElement::data_eq`。
            && match (&self.ui, &other.ui) {
                (None, None) => true,
                (Some(a), Some(b)) => a.data_eq(b),
                _ => false,
            }
    }
}

/// 包含实体列表的场景。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
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
        let n = self.entities.len();

        // 快路径：整场没有任何父子关系时（平铺场景很常见）不必白建一张 ID 映射表。
        // `world_matrices` 每帧都调这里，省掉这次哈希表分配是有意义的。
        if self.entities.iter().all(|e| e.parent.is_none()) {
            return vec![None; n];
        }

        let index_of: std::collections::HashMap<EntityId, usize> = self
            .entities
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id, i))
            .collect();

        // 父 ID 不存在 / 父就是自己 → 视作根，与 `parent_index` 的语义保持一致。
        self.entities
            .iter()
            .enumerate()
            .map(|(i, e)| {
                e.parent
                    .and_then(|pid| index_of.get(&pid).copied())
                    .filter(|&p| p != i)
            })
            .collect()
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
        let mut chain: Vec<usize> = Vec::new();
        let mut current = index;
        loop {
            if chain.contains(&current) {
                break; // 环：就此打住，把当前节点当根
            }
            chain.push(current);
            match self.parent_index(current) {
                Some(p) => current = p,
                None => break,
            }
        }
        chain
            .iter()
            .rev()
            .fold(Mat4::IDENTITY, |m, &i| m * self.entities[i].transform.local_matrix())
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
    /// `World::pull_dynamic_bodies` 每帧两样都要：父索引用来取父世界矩阵，世界矩阵
    /// 用来把物理算出的世界变换换回局部。分开调 `parent_indices()` +
    /// `world_matrices()` 会把 `hierarchy()` 跑**两遍**（后者内部也要父索引），
    /// 而这是每帧每固定步都要走的路径。合起来只解算一次。
    pub fn world_matrices_with_parents(&self) -> (Vec<Option<usize>>, Vec<Mat4>) {
        let n = self.entities.len();
        let parent_indices = self.parent_indices();

        // 先用 `Mat4::IDENTITY` 占位 + 一张"算过了没"的位图，而不是 `Vec<Option<Mat4>>`：
        // `Mat4` 没有可用 niche，`Option<Mat4>` 会从 64 字节涨到 80，2000 个实体就是
        // 多出 32KB 的内存搬运 —— 而这个函数在**每一次**解算的世界矩阵都在热路径上
        // （游戏每帧、运行时每个固定步）。
        let mut out: Vec<Mat4> = vec![Mat4::IDENTITY; n];
        let mut done: Vec<bool> = vec![false; n];
        // 环保护用的"在当前链上"标记。之所以不用 `chain.contains()`：
        // 那是 O(链长) 的逐次扫描，遇到"子节点排在父节点前面"的深链
        // （实体 0 是叶子、n-1 是根）整条链会退化成 O(n²)。布尔表是 O(1)。
        let mut on_chain: Vec<bool> = vec![false; n];

        // 链缓冲**提到循环外**复用。
        //
        // 这是本函数最容易踩的性能坑：写成循环体内的 `Vec::new()` 之后，**平铺场景**
        // （所有实体都是根节点，非常常见）每算一个实体就要为一条长度为 1 的链分配一次，
        // 整次调用就是 n 次小分配。热路径上的 n 次分配比那点矩阵乘法贵得多。
        let mut chain: Vec<usize> = Vec::new();

        for i in 0..n {
            if done[i] {
                continue;
            }
            // 从 i 向上收集一条"还没算过"的链
            chain.clear();
            let mut current = i;
            loop {
                if done[current] || on_chain[current] {
                    break;
                }
                chain.push(current);
                on_chain[current] = true;
                match parent_indices[current] {
                    Some(p) => current = p,
                    None => break,
                }
            }
            // 链顶节点的父节点若已算过，就以它为基底；否则从单位阵起算
            let mut base = chain
                .last()
                .and_then(|&last| parent_indices[last])
                .filter(|&p| done[p])
                .map(|p| out[p])
                .unwrap_or(Mat4::IDENTITY);

            for &idx in chain.iter().rev() {
                base *= self.entities[idx].transform.local_matrix();
                out[idx] = base;
                done[idx] = true;
            }
        }

        (parent_indices, out)
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
        let parent = self.entities[index]
            .parent
            .and_then(|id| self.index_of(id));
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
    pub fn remove_subtree(&mut self, index: usize) -> Vec<Entity> {
        let doomed = self.subtree(index);
        if doomed.is_empty() {
            return Vec::new();
        }
        let ids: Vec<EntityId> = doomed.iter().map(|&i| self.entities[i].id).collect();
        let mut removed = Vec::new();
        let mut kept = Vec::with_capacity(self.entities.len());
        for entity in std::mem::take(&mut self.entities) {
            if ids.contains(&entity.id) {
                removed.push(entity);
            } else {
                kept.push(entity);
            }
        }
        self.entities = kept;
        removed
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
    pub fn instantiate_prefab(&mut self, prefab: &crate::prefab::Prefab, source: &str) -> Option<usize> {
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

    // -- 层级 -------------------------------------------------------------

    /// 造一个带父子关系的场景：root -> child -> grandchild，各带平移。
    fn hierarchy_scene() -> (Scene, [usize; 3]) {
        let mut scene = Scene::new("Hierarchy");
        let root = scene.add_entity(
            Entity::new("Root").with_transform(Transform::from_translation(Vec3::new(1.0, 0.0, 0.0))),
        );
        let child = scene.add_entity(
            Entity::new("Child").with_transform(Transform::from_translation(Vec3::new(0.0, 2.0, 0.0))),
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
            Entity::new("Root").with_transform(Transform::from_translation(Vec3::new(1.0, 0.0, 0.0))),
        );
        let child_id = scene.add_entity(
            Entity::new("Child").with_transform(Transform::from_translation(Vec3::new(0.0, 2.0, 0.0))),
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
            assert_eq!(*mat, scene.world_matrix(i), "第 {i} 个节点批量/单个结果不一致");
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
                assert_eq!(parent_indices[i], scene.parent_index(i), "父索引不一致: {i}");
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
        assert!((before.position - after.position).length() < 1e-5, "世界位置应当保持不变");
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

    #[test]
    fn remove_entity_promotes_children() {
        let (mut scene, [r, c, g]) = hierarchy_scene();
        scene.remove_entity(scene.entities[c].id);
        // grandchild 被提升到 root 下，而不是随 child 一起消失
        assert_eq!(scene.entities.len(), 2);
        assert_eq!(scene.children(r), vec![scene.index_of(scene.entities[1].id).unwrap()]);
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
        entity.behavior_params.insert("Spinner".to_owned(), fields.clone());
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
        let m = restored.entities[0]
            .material
            .expect("往返后材质覆盖仍在");
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

    // -- 角色控制器 --------------------------------------------------------

    /// 角色参数必须能落盘往返 —— 手感是「作品的一部分」（能上多高的台阶、
    /// 能爬多陡的坡），打包出去的玩家拿的必须是作者调好的值。
    #[test]
    fn controller_survives_ron_roundtrip() {
        let mut scene = Scene::new("A");
        scene.add_entity(Entity::new("Player").with_controller(crate::CharacterController {
            radius: 0.4,
            height: 2.0,
            step_offset: 0.5,
            slope_limit: 60.0,
            skin_width: 0.01,
            snap_to_ground: 0.4,
        }));
        let text = ron::ser::to_string_pretty(&scene, ron::ser::PrettyConfig::default()).unwrap();
        assert!(text.contains("controller"), "角色组件必须落盘");
        let restored: Scene = ron::from_str(&text).unwrap();
        let c = restored.entities[0]
            .controller
            .expect("往返后角色组件仍在");
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
        assert!(!text.contains("controller"), "普通实体不该写出 controller 字段");

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

        assert_eq!(scene.prefab_instance_root(ci), Some(ri), "内部节点应上溯到实例根");
        assert_eq!(scene.prefab_instance_root(ri), Some(ri), "根解析到自身");

        let nid = scene.add_entity(Entity::new("Plain"));
        assert_eq!(
            scene.prefab_instance_root(scene.index_of(nid).unwrap()),
            None,
            "普通实体不属于任何实例"
        );
    }
}
