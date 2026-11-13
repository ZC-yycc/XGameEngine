//! 预制体（Prefab）——可复用的实体子树模板。
//!
//! 对应 Unity 的 Prefab、Godot 的 PackedScene：把**一棵带层级的实体子树**
//! 存成一份独立资产，之后可以在任意场景里反复实例化。
//!
//! # 为什么是「子树 + 实例引用」，而不是「普通实体的拷贝」
//!
//! 如果实例化只是把实体克隆一份（引擎里复制粘贴就是这么干的），那实例和
//! 模板之间就再没有关系了：改了模板，场景里几十个副本不会知道。预制体要解决的
//! 正是这件事，所以每个实例实体都记着一行 [`PrefabInstanceRef`]——它来自哪个
//! 资产文件、对应资产里的哪个实体。有了这条线索，[`Apply`](EditorState::apply_prefab)
//! 能把实例的改动写回资产、[`Revert`](EditorState::revert_prefab) 能把实例拉回资产的样子。
//!
//! # 资产格式
//!
//! 预制体就是「一个更小的场景」，同样只用 `Entity::parent`（按 ID 单向引用）
//! 表达层级，子节点由 [`Prefab::children`] 现算——与 [`Scene`] 完全同构。
//! 序列化用 RON，文件后缀 `.prefab.ron`（见 [`PREFAB_EXTENSION`]）。

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::id::EntityId;
use crate::scene::{Entity, Scene};

/// 预制体资产的文件后缀（`.prefab.ron`）。
///
/// 用双后缀而不是裸 `.ron`，是为了和场景文件区分开——[`ProjectHandle::list_prefabs`]
/// 靠它把预制体从同目录下的其它文件里挑出来。
///
/// [`ProjectHandle::list_prefabs`]: crate::project::ProjectHandle::list_prefabs
pub const PREFAB_EXTENSION: &str = "prefab.ron";

// ---------------------------------------------------------------------------
// 实例引用
// ---------------------------------------------------------------------------

/// 实体与它所属预制体资产之间的关联。
///
/// 只有「预制体实例」里的实体才带这个字段；手工创建的普通实体是 `None`。
/// 它以 `#[serde(default)]` 落在 `Entity` 上，因此旧场景文件（没有这个字段）
/// 仍然能正确反序列化，全部落成 `None`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefabInstanceRef {
    /// 预制体资产的**项目相对路径**（如 `Prefabs/Enemy.prefab.ron`）。
    ///
    /// 存相对路径而不是绝对路径：整个项目目录可以随便搬、进版本库、被别人 clone，
    /// 引用都不会失效——与场景里 `Entity::model` 的约定一致。
    pub source: String,
    /// 对应**资产文件内部**那个实体的稳定 ID。
    ///
    /// 实例化时用它在「场景实体」和「资产实体」之间建立一一对应，是 Apply /
    /// Revert 能精确落位的前提。
    pub source_id: EntityId,
    /// 本实体是不是这个实例的**根**。
    ///
    /// 只有根会被整体操作（Apply 写回、Revert 重建、删除连坐整棵实例），
    /// 所以得能和内部节点区分开。
    pub is_root: bool,
}

// ---------------------------------------------------------------------------
// 预制体
// ---------------------------------------------------------------------------

/// 一份预制体的运行时值。
///
/// 结构与 [`Scene`] 同构（实体列表 + 按 ID 的父引用），只是没有光源 / 环境光——
/// 那些属于「场景」，不属于「可复用的对象」。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prefab {
    /// 预制体名（显示用，通常取自资产文件名）。
    pub name: String,
    /// 预制体内的实体。层级由每个实体的 `parent`（按 ID）表达。
    pub entities: Vec<Entity>,
}

impl Prefab {
    /// 创建一个空预制体。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            entities: Vec::new(),
        }
    }

    /// 从场景里的一棵子树提取预制体（根为 `root`）。
    ///
    /// 提取出来的预制体是**自包含**的：只有子树外的父引用会被清掉（实际只有根
    /// 会有），内部父子关系原样保留。实体的 ID 也原样带进来——这样「用这批实体
    /// 生成预制体」之后，场景里原样的这些实体天然就对应上了资产里的实体
    /// （`source_id` 一一对应），不需要再做一次麻烦的匹配。
    pub fn from_subtree(scene: &Scene, root: usize) -> Self {
        let name = scene
            .entities
            .get(root)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| "Prefab".to_string());
        let mut prefab = Self::new(name);
        prefab.capture_subtree_ids(scene, root, None);
        prefab
    }

    /// 用场景里的一棵子树**覆盖**本预制体的实体列表（不改变 `name`）。
    ///
    /// `Apply` 走这条：把实例当前的样子（含用户改过的变换）重新采集回资产结构。
    pub fn capture_subtree(&mut self, scene: &Scene, root: usize) {
        let source = scene
            .entities
            .get(root)
            .and_then(|e| e.prefab.as_ref())
            .map(|p| p.source.as_str());
        self.capture_subtree_ids(scene, root, source);
    }

    fn capture_subtree_ids(&mut self, scene: &Scene, root: usize, source: Option<&str>) {
        let indices = scene.subtree(root);
        let ids: Vec<EntityId> = indices
            .iter()
            .map(|&i| {
                let entity = &scene.entities[i];
                match source {
                    Some(source) => entity
                        .prefab
                        .as_ref()
                        .filter(|p| p.source == source)
                        .map(|p| p.source_id)
                        .unwrap_or_default(),
                    None => entity.id,
                }
            })
            .collect();
        let mut id_map = HashMap::new();
        for (&i, &id) in indices.iter().zip(&ids) {
            id_map.entry(scene.entities[i].id).or_insert(id);
        }
        self.entities = indices
            .iter()
            .zip(&ids)
            .map(|(&i, &id)| {
                let mut entity = scene.entities[i].clone();
                entity.id = id;
                entity.parent = entity.parent.and_then(|p| id_map.get(&p).copied());
                // 资产本身不携带「我是某个预制体的实例」这条信息——否则
                // 预制体里嵌套预制体，写文件时会把实例元数据也腌进去，回头
                // 一实例化就自带了错误的来源路径。
                entity.prefab = None;
                entity
            })
            .collect();
    }

    /// 实体数量。
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// 按 ID 取实体在 `entities` 中的索引。
    pub fn index_of(&self, id: EntityId) -> Option<usize> {
        self.entities.iter().position(|e| e.id == id)
    }

    /// **只报告**重复的实体 ID（`(id, 出现次数)`，按首次出现排序）。
    ///
    /// # 为什么预制体的重复 ID 只能报告、不能在加载边界修
    ///
    /// 场景的重复 ID 是**同文件内引用**（`parent` 指向本文件里的 ID），
    /// 所以 `Scene::ensure_unique_ids` 可以"重分配 + 让人手工校正层级"——
    /// 损失有界，且能阻止坏数据继续传播。
    ///
    /// 预制体的 ID 是**跨文件引用**：场景里每个实例的
    /// [`PrefabInstanceRef::source_id`] 都指向该预制体文件里的 ID。在加载边界
    /// 重分配它们，会让所有既有场景实例的 `source_id` 集体失配 —— `Apply` 时
    /// `capture_subtree_ids` 取不到对应值、退回 `EntityId::new()`，于是整棵树
    /// 被当成"新增"，资产里凭空多出一份重复实体。**跨文件引用不许静默改**，
    /// 所以这里只提供"看"，修法是把该预制体在编辑器里打开、重新保存一次。
    ///
    /// 重复 ID 的实际危害：`index_of` 用 `position` 取**第一个**匹配，于是
    /// `parent_index` / `children` 对第二条及以后的同 ID 实体给出错误结果
    /// （父子层级错位），而 `instantiate` 虽然按位置分配新 ID（不会产出重复
    /// ID），父引用也只能落到第一个同 ID 实体上。
    pub fn duplicate_ids(&self) -> Vec<(EntityId, usize)> {
        crate::id::duplicate_ids(self.entities.iter().map(|e| e.id))
    }

    /// 父节点索引（与 [`Scene::parent_index`] 同语义）。
    pub fn parent_index(&self, index: usize) -> Option<usize> {
        let parent_id = self.entities.get(index)?.parent?;
        self.index_of(parent_id).filter(|&p| p != index)
    }

    /// 直接子节点索引列表。
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

    /// 根节点索引列表（正常预制体只有一个根）。
    pub fn roots(&self) -> Vec<usize> {
        (0..self.entities.len())
            .filter(|&i| self.parent_index(i).is_none())
            .collect()
    }

    /// 实例化：[`Prefab`] → 一批**可直接加入场景**的新实体。
    ///
    /// 做三件事：
    ///
    /// 1. 给每个实体分配**新的场景 ID**（沿用资产里的 ID 会让"按 ID 查实体"
    ///    分不清两个实例，撤销 / 拾取全乱套）；
    /// 2. 按 旧→新 的映射重定向内部父引用；
    /// 3. 给每个实体贴上 [`PrefabInstanceRef`]，把来源与 `source_id` 记牢，
    ///    这样后续 Apply / Revert 才找得回来。
    ///
    /// `source` 是资产的项目相对路径，写进 [`PrefabInstanceRef::source`]。
    pub fn instantiate(&self, source: &str) -> Vec<Entity> {
        self.instantiate_with(source, |_| EntityId::new())
    }

    /// 同 [`Self::instantiate`]，但由调用方决定每个实体拿到什么 ID。
    ///
    /// 唯一的另一个调用方是 `Scene::revert_prefab`：它要**保留**实例里已有的
    /// ID（按 `source_id` 对位），否则一次 Revert 就会把整棵子树的 ID 全换掉 ——
    /// 场景文件 diff 变成纯噪音，外部对实体 ID 的引用也一起断掉。
    ///
    /// `id_for` 的入参是资产里的实体 ID（也就是实例的 `source_id`），必须**对同一
    /// 个入参给出同一个结果**：父子重定向是按资产 ID 建索引的，同一个资产 ID 落到
    /// 两个不同结果时父引用只能取第一次出现的那个（与撞 ID 的既有取舍一致）。
    pub fn instantiate_with(
        &self,
        source: &str,
        mut id_for: impl FnMut(EntityId) -> EntityId,
    ) -> Vec<Entity> {
        // 先给**每一条**实体各分配一个新 ID —— 按位置分，不按旧 ID 分。
        //
        // 旧实现是 `id_map.insert(entity.id, EntityId::new())`：映射的键是**旧 ID**，
        // 文件里若有两条件物撞了同一个 ID，后一次 insert 会覆盖前一次，于是两条
        // 实体最后拿到**同一个**新 ID。实例化出来的子树自带重复 ID，之后"删一个
        // 连带删另一个"，而且会被下一次保存写回文件，越传越广。
        let new_ids: Vec<EntityId> = self.entities.iter().map(|e| id_for(e.id)).collect();
        // 父引用只能按"旧 ID → 第一次出现的那个新 ID"重定向（撞 ID 时无法区分父子，
        // 保守取第一个 —— 与 `Scene::ensure_unique_ids` 的取舍一致）。
        let mut id_map: HashMap<EntityId, EntityId> = HashMap::with_capacity(self.entities.len());
        for (entity, new_id) in self.entities.iter().zip(&new_ids) {
            id_map.entry(entity.id).or_insert(*new_id);
        }

        self.entities
            .iter()
            .zip(&new_ids)
            .map(|(original, &new_id)| {
                let mut entity = original.clone();
                entity.id = new_id;
                entity.parent = original.parent.and_then(|p| id_map.get(&p).copied());
                let is_root = entity.parent.is_none();
                entity.prefab = Some(PrefabInstanceRef {
                    source: source.to_string(),
                    source_id: original.id,
                    is_root,
                });
                entity
            })
            .collect()
    }

    /// 序列化为 RON。
    pub fn to_ron(&self) -> Result<String, String> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())
    }

    /// 从 RON 反序列化。
    pub fn from_ron(text: &str) -> Result<Self, String> {
        ron::from_str(text).map_err(|e| format!("{e:#}"))
    }
}

// ---------------------------------------------------------------------------
// 路径工具
// ---------------------------------------------------------------------------

/// 从预制体资产的项目相对路径里取出显示名。
///
/// `Prefabs/Enemy.prefab.ron` → `Enemy`。取不出名字时退回整条路径，
/// 保证 UI 上永远有东西可显示。
pub fn prefab_display_name(relative: &str) -> String {
    let file = Path::new(relative)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| relative.to_string());
    let stem = file
        .strip_suffix(PREFAB_EXTENSION)
        .map(|s| s.trim_end_matches('.'))
        .unwrap_or(&file);
    if stem.is_empty() {
        relative.to_string()
    } else {
        stem.to_string()
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::Transform;
    use glam::Vec3;

    /// 造一个 3 个实体、两层深的小场景：Root -> Child -> Grandchild。
    fn sample_scene() -> (Scene, usize) {
        let mut scene = Scene::new("Sample");
        let root = scene.add_entity(Entity::new("Root").with_model("builtin://cube"));
        let child = scene.add_entity(
            Entity::new("Child")
                .with_model("builtin://sphere")
                .with_transform(Transform::from_translation(Vec3::new(0.0, 1.0, 0.0))),
        );
        let grandchild = scene.add_entity(
            Entity::new("Grandchild")
                .with_model("builtin://cylinder")
                .with_transform(Transform::from_translation(Vec3::new(0.0, 0.5, 0.0))),
        );
        scene.set_parent(
            scene.index_of(child).unwrap(),
            Some(scene.index_of(root).unwrap()),
        );
        scene.set_parent(
            scene.index_of(grandchild).unwrap(),
            Some(scene.index_of(child).unwrap()),
        );
        let root = scene.index_of(root).unwrap();
        (scene, root)
    }

    #[test]
    fn applying_instance_preserves_ids_and_remaps_new_children() {
        let (scene, root) = sample_scene();
        let mut prefab = Prefab::from_subtree(&scene, root);
        let ids: Vec<_> = prefab.entities.iter().map(|e| e.id).collect();
        let source = "Prefabs/Root.prefab.ron";
        let mut instance = Scene::new("Instance");
        instance.entities = prefab.instantiate(source);
        let mut added = Entity::new("Added");
        added.parent = Some(instance.entities[1].id);
        instance.entities.push(added);
        prefab.capture_subtree(&instance, 0);
        for (name, id) in ["Root", "Child", "Grandchild"].into_iter().zip(&ids) {
            assert_eq!(
                prefab.entities.iter().find(|e| e.name == name).unwrap().id,
                *id
            );
        }
        assert_eq!(
            prefab
                .entities
                .iter()
                .find(|e| e.name == "Added")
                .unwrap()
                .parent,
            Some(ids[1])
        );
        assert!(prefab.entities.iter().all(|e| e.prefab.is_none()));
    }

    #[test]
    fn creating_another_prefab_from_instance_keeps_scene_ids() {
        let (scene, root) = sample_scene();
        let original = Prefab::from_subtree(&scene, root);
        let mut instance = Scene::new("Instance");
        instance.entities = original.instantiate("Prefabs/Old.prefab.ron");
        let created = Prefab::from_subtree(&instance, 0);
        for entity in &instance.entities {
            assert!(created.index_of(entity.id).is_some());
        }
    }

    #[test]
    fn from_subtree_captures_hierarchy() {
        let (scene, root) = sample_scene();
        let prefab = Prefab::from_subtree(&scene, root);
        assert_eq!(prefab.len(), 3);
        assert_eq!(prefab.roots().len(), 1, "提取出来应当只有一个根");
        // 父引用指向子树内部，结构完整
        let r = prefab.roots()[0];
        let c = prefab.children(r);
        assert_eq!(c.len(), 1);
        assert_eq!(prefab.children(c[0]).len(), 1);
        // 名字取自根实体
        assert_eq!(prefab.name, "Root");
    }

    #[test]
    fn from_subtree_drops_outside_parent() {
        // 中间节点为根提取时，它到原父节点的引用必须被清掉
        let (scene, root) = sample_scene();
        let child = scene.children(root)[0];
        let prefab = Prefab::from_subtree(&scene, child);
        assert_eq!(prefab.len(), 2, "child + grandchild");
        assert!(prefab.roots() == vec![0] || prefab.roots().len() == 1);
        assert!(prefab.entities[prefab.roots()[0]].parent.is_none());
    }

    #[test]
    fn capture_subtree_strips_instance_metadata() {
        let (scene, root) = sample_scene();
        let mut prefab = Prefab::from_subtree(&scene, root);
        // 模拟：场景里的实体本身是某个实例（带了 prefab 引用）
        let mut scene2 = scene.clone();
        for e in &mut scene2.entities {
            e.prefab = Some(PrefabInstanceRef {
                source: "Prefabs/Old.prefab.ron".into(),
                source_id: e.id,
                is_root: false,
            });
        }
        prefab.capture_subtree(&scene2, root);
        // 资产文件里不该残留实例来源
        assert!(prefab.entities.iter().all(|e| e.prefab.is_none()));
    }

    #[test]
    fn instantiate_remaps_ids_and_marks_source() {
        let (scene, root) = sample_scene();
        let prefab = Prefab::from_subtree(&scene, root);
        let old_ids: Vec<EntityId> = prefab.entities.iter().map(|e| e.id).collect();

        let instances = prefab.instantiate("Prefabs/Root.prefab.ron");
        assert_eq!(instances.len(), 3);

        // 全是新 ID，且不与资产里的旧 ID 撞车
        for e in &instances {
            assert!(!old_ids.contains(&e.id), "实例实体不该沿用资产里的 ID");
        }
        // 内部父引用仍指向同一批新实体
        let roots: Vec<&Entity> = instances
            .iter()
            .filter(|e| e.prefab.as_ref().is_some_and(|p| p.is_root))
            .collect();
        assert_eq!(roots.len(), 1, "实例里应当恰好有一个根");

        // 每个实体都记了来源与资产内的对应 ID
        for e in &instances {
            let link = e.prefab.as_ref().expect("实例实体必须带来源");
            assert_eq!(link.source, "Prefabs/Root.prefab.ron");
            assert!(
                old_ids.contains(&link.source_id),
                "source_id 应当指向资产内的实体"
            );
        }
        // 父子结构完整（重映射后）
        let root_id = roots[0].id;
        let children: Vec<&Entity> = instances
            .iter()
            .filter(|c| c.parent == Some(root_id))
            .collect();
        assert_eq!(children.len(), 1);
    }

    #[test]
    fn two_instances_are_independent() {
        let (scene, root) = sample_scene();
        let prefab = Prefab::from_subtree(&scene, root);
        let a = prefab.instantiate("Prefabs/Root.prefab.ron");
        let b = prefab.instantiate("Prefabs/Root.prefab.ron");
        // 两批实例的 ID 集合必须完全不相交
        for ea in &a {
            assert!(b.iter().all(|eb| eb.id != ea.id), "两次实例化的 ID 撞了");
        }
    }

    #[test]
    fn roundtrips_through_ron() {
        let (scene, root) = sample_scene();
        let prefab = Prefab::from_subtree(&scene, root);
        let text = prefab.to_ron().unwrap();
        let restored = Prefab::from_ron(&text).unwrap();
        assert_eq!(restored.name, "Root");
        assert_eq!(restored.len(), 3);
        assert_eq!(restored.roots().len(), 1);
        let r = restored.roots()[0];
        assert_eq!(restored.children(r).len(), 1);
        // ID 往返后保持稳定（Apply / Revert 的映射基础）
        let ids_before: Vec<EntityId> = prefab.entities.iter().map(|e| e.id).collect();
        let ids_after: Vec<EntityId> = restored.entities.iter().map(|e| e.id).collect();
        assert_eq!(ids_before, ids_after);
    }

    #[test]
    fn display_name_from_path() {
        assert_eq!(prefab_display_name("Prefabs/Enemy.prefab.ron"), "Enemy");
        assert_eq!(prefab_display_name("A/B/C/Player.prefab.ron"), "Player");
        assert_eq!(prefab_display_name("Weird.txt"), "Weird.txt");
        assert_eq!(prefab_display_name("Prefabs/中文名.prefab.ron"), "中文名");
    }

    #[test]
    fn scene_instantiate_prefab_appends_and_returns_root() {
        let (scene, root) = sample_scene();
        let prefab = Prefab::from_subtree(&scene, root);

        let mut target = Scene::new("Target");
        target.add_entity(Entity::new("Existing"));
        let new_root = target
            .instantiate_prefab(&prefab, "Prefabs/Root.prefab.ron")
            .expect("非空预制体应当能实例化");

        assert_eq!(target.entities.len(), 4, "原有 1 + 新增 3");
        assert!(target.entities[new_root]
            .prefab
            .as_ref()
            .is_some_and(|p| p.is_root));
        // 新增的实例是一棵独立的树
        assert_eq!(target.subtree(new_root).len(), 3);
    }

    #[test]
    fn empty_prefab_instantiates_to_nothing() {
        let prefab = Prefab::new("Empty");
        let mut scene = Scene::new("S");
        assert!(scene.instantiate_prefab(&prefab, "x.prefab.ron").is_none());
        assert!(prefab.instantiate("x.prefab.ron").is_empty());
    }

    /// 资产文件里撞了 ID 时，实例化仍须给出互不相同的新 ID。
    ///
    /// 旧实现按**旧 ID** 建「旧 → 新」映射，两条同 ID 的实体会互相覆盖，最后
    /// 共用同一个新 ID —— 实例化出来的子树自带重复 ID，之后删一条就带走另一条。
    #[test]
    fn instantiate_keeps_new_ids_distinct_when_the_source_has_duplicates() {
        let mut prefab = Prefab::new("Dup");
        let shared = EntityId::from_raw(4242);
        let mut a = Entity::new("A");
        a.id = shared;
        let mut b = Entity::new("B");
        b.id = shared;
        prefab.entities.push(a);
        prefab.entities.push(b);

        let out = prefab.instantiate("Prefabs/Dup.prefab.ron");
        assert_eq!(out.len(), 2);
        assert_ne!(
            out[0].id, out[1].id,
            "两条源实体撞 ID 时，实例也必须各拿一个新 ID"
        );
        // `source_id` 仍指回资产里的原 ID，Apply / Revert 的对位不受影响。
        assert!(
            out.iter()
                .all(|e| e.prefab.as_ref().is_some_and(|p| p.source_id == shared)),
            "source_id 应当保持资产里的原值"
        );
    }
}
