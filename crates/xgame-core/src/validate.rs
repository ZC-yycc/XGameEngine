//! 场景的**语义**体检 —— 反序列化成功 ≠ 数据能用。
//!
//! # 与 [`crate::check`] 的分工
//!
//! `check` 管**外部**：引用的模型 / 脚本 / 材质在不在盘上、GUID 通不通、
//! `.meta` 齐不齐。它回答"这份工程能不能装起来"。
//!
//! 这里管**内部**：实体之间的关系和数值本身合不合法 —— 重复 ID、父节点指向
//! 不存在的实体、层级成环、坐标里塞了 `NaN`、四元数没归一化、层号越界。
//! 这些数据**全能通过 RON 解析**，然后以"物体不见了 / 位置是 `NaN` / 层级面板
//! 空白 / 删一个连带删两个"的形式出现，与解析错误完全不同源。
//!
//! # 为什么需要它
//!
//! 智能体（MCP `scene_write`）与手写 RON 都是**整段文本**写入，`ron::from_str`
//! 只保证语法。没有这一层，"写成功"只意味着"字节落了盘"，语义坏掉的部分要等
//! 到有人打开编辑器才暴露 —— 那时作者手里没有"哪一行错了"的信息。
//!
//! # 已知边界
//!
//! - **只在单文件内判定**。跨文件唯一性由 `(source, source_id)` 表达
//!   （见决策记录 §五十三），不是本模块的职责。
//! - 预制体文件不做语义体检：MCP 没有写预制体的通道，而预制体一旦被实例化
//!   就落进场景，场景这一侧会照到。
//! - 重复 ID 只能在**加载修数据之前**看见（[`Scene::ensure_unique_ids`] 会顺手
//!   改掉）—— 所以 [`crate::check`] 那份仍从磁盘原文统计，不从内存看。

use crate::scene::Scene;

/// 语义缺陷分类。
///
/// 分类而不是只给一段文字：调用方要按类决定**怎么处置**（拒绝写入 / 只报告），
/// 也要按类做统计与测试断言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefectKind {
    /// 两条实体的 `id` 相同。后果是删除连坐：删一个把另一个也带走。
    DuplicateId,
    /// `parent` 指向不存在的实体 ID。运行时按"根节点"处理 —— 不报错但**不是**
    /// 作者写的那个层级。
    DanglingParent,
    /// `parent` 指向自己。
    SelfParent,
    /// 层级成环（A 的父是 B、B 的父是 A）。运行时用环检测兜底（结果退化、不崩），
    /// 但两个节点都不会出现在层级树的根上，编辑器里看起来"消失了"。
    ParentCycle,
    /// 位置 / 旋转 / 缩放里有 `NaN` 或 `±∞`。会把世界矩阵污染成 `NaN`，
    /// 该实体连同它的子节点一起被剔除。
    NonFiniteTransform,
    /// 光源的 `direction` / `position` / `color` / `intensity` 里有 `NaN` 或 `±∞`。
    ///
    /// 光源不是实体（`Scene::lights` 是独立数组，带自己的世界方向与坐标），所以
    /// 变换那一条扫不到它。症状比坏的变换更响：`NaN` 一进着色就整帧全黑或全白。
    NonFiniteLight,
    /// 旋转四元数长度偏离 1（手写 / 外部工具写入的典型伤）。非单位四元数
    /// 经矩阵展开会**附带缩放与切变**，物体变歪而不是消失。
    RotationNotUnit,
    /// 层索引超出 `0..32`。运行时取模兜底，实体因此滑到别的层上。
    LayerOutOfRange,
}

impl DefectKind {
    /// 机器可读的短标签（纯 ASCII，`--check` 的输出要能进 CI 与 diff）。
    pub fn label(self) -> &'static str {
        match self {
            Self::DuplicateId => "duplicate id",
            Self::DanglingParent => "dangling parent",
            Self::SelfParent => "self parent",
            Self::ParentCycle => "parent cycle",
            Self::NonFiniteTransform => "non-finite transform",
            Self::NonFiniteLight => "non-finite light",
            Self::RotationNotUnit => "rotation not unit",
            Self::LayerOutOfRange => "bad layer",
        }
    }

    /// 是否属于"身份 / 格式"类（`--check` 已经有专门字段报它们，别再报一遍）。
    pub fn is_reported_elsewhere(self) -> bool {
        matches!(self, Self::DuplicateId | Self::LayerOutOfRange)
    }
}

/// 一条语义缺陷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defect {
    pub kind: DefectKind,
    /// 涉及的主体（实体名；成环时是代表节点）。重复 ID 这类没有单一主体时为 `None`。
    pub entity: Option<String>,
    /// 细节，**纯 ASCII 的数字与分隔符**（实体名本身可以含中文，那是数据）。
    pub detail: String,
}

impl Defect {
    /// 一行人类可读描述，形如 `parent cycle at 'Ghost' (Ghost -> Helper -> Ghost)`。
    ///
    /// 拼进 `--check` 的输出与 MCP 的报错里，所以不能有换行（会被当成两条问题）。
    pub fn describe(&self) -> String {
        match &self.entity {
            Some(name) => format!("{} at '{}' ({})", self.kind.label(), name, self.detail),
            None => format!("{} ({})", self.kind.label(), self.detail),
        }
    }
}

/// 四元数"算不算归一化"的容差。
///
/// 取 1e-3 而不是 1e-6：落盘再读回是 f32 原值（没有误差），但从矩阵分解、
/// 多个四元数连乘来的值允许有累积漂移；真正的伤（手写成 `(0,0,0,2)`）偏离
/// 远大于这个量级。卡太紧会把正常工程判红。
const UNIT_TOLERANCE: f32 = 1e-3;

/// 体检一个场景，按实体顺序返回缺陷（输出**确定性**：同样的场景永远同样的顺序）。
///
/// 复制一份场景不会引入任何随机性：环检测按索引序遍历，代表节点取环上最小的索引。
pub fn scene_defects(scene: &Scene) -> Vec<Defect> {
    let mut defects: Vec<Defect> = Vec::new();
    let n = scene.entities.len();

    // -- 重复 ID -----------------------------------------------------------
    // 只在"加载后没修过"的对象上看得见。`scene_write` 拿到的是刚解析的原始
    // 数据，所以这里有东西；`--check` 那份从磁盘原文统计（两条路互补）。
    for (id, count) in scene.duplicate_ids() {
        defects.push(Defect {
            kind: DefectKind::DuplicateId,
            entity: None,
            detail: format!("id {} x{count}", id.raw()),
        });
    }

    // -- 父节点指向 ---------------------------------------------------------
    // 用 `parent_indices` 的语义（ID 相同取**第一个**出现的实体）判定"能不能
    // 解析到"，否则同 ID 场景下这里的结论会和渲染时的层级不一致。
    let index_of: std::collections::HashMap<crate::id::EntityId, usize> = {
        let mut map = std::collections::HashMap::with_capacity(n);
        for (i, entity) in scene.entities.iter().enumerate() {
            map.entry(entity.id).or_insert(i);
        }
        map
    };
    for entity in &scene.entities {
        let Some(parent_id) = entity.parent else {
            continue;
        };
        if parent_id == entity.id {
            defects.push(Defect {
                kind: DefectKind::SelfParent,
                entity: Some(entity.name.clone()),
                detail: format!("id {}", parent_id.raw()),
            });
        } else if !index_of.contains_key(&parent_id) {
            defects.push(Defect {
                kind: DefectKind::DanglingParent,
                entity: Some(entity.name.clone()),
                detail: format!("parent id {} not found", parent_id.raw()),
            });
        }
    }

    // -- 层级成环 -----------------------------------------------------------
    // 三色标记，整体 O(n)：每个节点只入链一次。**一个环只报一条** —— 把环上
    // 每个节点都报一遍，20 个节点的环会刷 20 行一模一样的话，看不出是一处坏。
    let parents = scene.parent_indices();
    let mut state = vec![0u8; n]; // 0 = 未访问，1 = 在当前链上，2 = 已归位
    let mut path: Vec<usize> = Vec::new();
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        path.clear();
        let mut current = start;
        loop {
            if state[current] == 2 {
                break;
            }
            if state[current] == 1 {
                // `current` 在链上出现过 → `path` 从它的位置起就是一个环。
                if let Some(pos) = path.iter().position(|&node| node == current) {
                    let cycle = &path[pos..];
                    // 代表节点取**环上最小索引**：同一次检查、多跑几次输出一致。
                    let head_pos = cycle
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, &node)| node)
                        .map(|(k, _)| k)
                        .unwrap_or(0);
                    let ordered: Vec<usize> = cycle[head_pos..]
                        .iter()
                        .chain(cycle[..head_pos].iter())
                        .copied()
                        .collect();
                    let head = ordered[0];
                    // 首尾各写一次，读起来才像"一条链"而不是一串名字。
                    let mut chain: Vec<String> = ordered
                        .iter()
                        .map(|&node| scene.entities[node].name.clone())
                        .collect();
                    chain.push(scene.entities[head].name.clone());
                    defects.push(Defect {
                        kind: DefectKind::ParentCycle,
                        entity: Some(scene.entities[head].name.clone()),
                        detail: format!("chain {}, {} nodes", chain.join(" -> "), ordered.len()),
                    });
                }
                break;
            }
            state[current] = 1;
            path.push(current);
            match parents[current] {
                Some(parent) => current = parent,
                None => break,
            }
        }
        for &node in &path {
            state[node] = 2;
        }
    }

    // -- 变换数值 -----------------------------------------------------------
    for entity in &scene.entities {
        let t = &entity.transform;
        // 逐分量判定（不依赖 `Quat::is_finite` 的具体版本可用性）。
        let non_finite = !(t.position.is_finite()
            && t.scale.is_finite()
            && t.rotation.to_array().iter().all(|c| c.is_finite()));
        if non_finite {
            defects.push(Defect {
                kind: DefectKind::NonFiniteTransform,
                entity: Some(entity.name.clone()),
                detail: first_non_finite(t),
            });
            // 已经不是有限数了，再报"没归一化"只会是噪音。
            continue;
        }
        let length_squared = t.rotation.length_squared();
        if (length_squared - 1.0).abs() > UNIT_TOLERANCE {
            defects.push(Defect {
                kind: DefectKind::RotationNotUnit,
                entity: Some(entity.name.clone()),
                detail: format!("|q|^2 = {length_squared:.6}"),
            });
        }
    }

    // -- 层索引 -------------------------------------------------------------
    for entity in &scene.entities {
        if entity.layer as usize >= crate::layers::LAYER_COUNT {
            defects.push(Defect {
                kind: DefectKind::LayerOutOfRange,
                entity: Some(entity.name.clone()),
                detail: format!(
                    "layer {} (valid 0..{})",
                    entity.layer,
                    crate::layers::LAYER_COUNT - 1
                ),
            });
        }
    }

    // -- 光源数值 -----------------------------------------------------------
    // 光源不是实体（`Scene::lights` 是独立数组，自带世界方向与坐标），上面那只
    // 扫实体的循环照不到它。`NaN` 进着色就是整帧全黑 / 全白，比坏的变换还显眼。
    for light in &scene.lights {
        if let Some((field, value)) = first_non_finite_light(light) {
            defects.push(Defect {
                kind: DefectKind::NonFiniteLight,
                entity: Some(light.name.clone()),
                detail: format!("{field} = {value}"),
            });
        }
    }

    defects
}

/// 光源里第一个非有限的分量（名字 + 值），没有则 `None`。
fn first_non_finite_light(light: &crate::light::Light) -> Option<(&'static str, f32)> {
    let fields = [
        ("direction[0]", light.direction[0]),
        ("direction[1]", light.direction[1]),
        ("direction[2]", light.direction[2]),
        ("position[0]", light.position[0]),
        ("position[1]", light.position[1]),
        ("position[2]", light.position[2]),
        ("color[0]", light.color[0]),
        ("color[1]", light.color[1]),
        ("color[2]", light.color[2]),
        ("intensity", light.intensity),
    ];
    fields.into_iter().find(|(_, value)| !value.is_finite())
}

/// 定位第一个非有限分量的名字（报错要能指到字段，不能只说"变换坏了"）。
fn first_non_finite(t: &crate::transform::Transform) -> String {
    let fields = [
        ("position.x", t.position.x),
        ("position.y", t.position.y),
        ("position.z", t.position.z),
        ("rotation.x", t.rotation.x),
        ("rotation.y", t.rotation.y),
        ("rotation.z", t.rotation.z),
        ("rotation.w", t.rotation.w),
        ("scale.x", t.scale.x),
        ("scale.y", t.scale.y),
        ("scale.z", t.scale.z),
    ];
    for (name, value) in fields {
        if !value.is_finite() {
            return format!("{name} = {value}");
        }
    }
    // 走到这里说明"整体 is_finite 为假"与逐分量判定不一致 —— 不该发生，
    // 但也不能 panic（体检本身不能成为崩溃点）。
    "non-finite component".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Entity, Scene};
    use glam::{Quat, Vec3};

    /// 加一个实体，返回它在 `entities` 里的索引。
    fn add(scene: &mut Scene, name: &str) -> usize {
        let id = scene.add_entity(Entity::new(name));
        scene.find_entity(id).expect("刚加的实体")
    }

    fn kinds(scene: &Scene) -> Vec<DefectKind> {
        scene_defects(scene).into_iter().map(|d| d.kind).collect()
    }

    /// 正常层级场景必须是零缺陷 —— 否则这道闸门会把所有工程拦在门外。
    #[test]
    fn a_healthy_hierarchy_has_no_defects() {
        let mut scene = Scene::new("t");
        let root = add(&mut scene, "root");
        let child = add(&mut scene, "child");
        scene.set_parent(child, Some(root));
        scene.entities[child].transform.position = Vec3::new(1.0, 2.0, 3.0);
        scene.entities[child].transform.rotation =
            crate::euler_deg_to_quat(Vec3::new(30.0, 45.0, 60.0));
        assert_eq!(scene_defects(&scene), Vec::new());
    }

    /// 重复 ID：删一个会连带删另一个，必须在写盘前拦住。
    #[test]
    fn duplicate_ids_are_reported() {
        let mut scene = Scene::new("t");
        let a = add(&mut scene, "A");
        let b = add(&mut scene, "B");
        scene.entities[b].id = scene.entities[a].id;
        let defects = scene_defects(&scene);
        assert_eq!(defects.len(), 1, "{defects:?}");
        assert_eq!(defects[0].kind, DefectKind::DuplicateId);
        assert!(defects[0].detail.ends_with("x2"), "{:?}", defects[0]);
    }

    /// 父节点指向不存在的实体 / 指向自己：两种都在运行时"静默当根"，
    /// 但作者写的层级并不是这样。
    #[test]
    fn dangling_and_self_parents_are_reported() {
        let mut scene = Scene::new("t");
        let a = add(&mut scene, "A");
        let b = add(&mut scene, "B");
        scene.entities[a].parent = Some(crate::EntityId::from_raw(9999));
        scene.entities[b].parent = Some(scene.entities[b].id);

        let defects = scene_defects(&scene);
        let kinds: Vec<DefectKind> = defects.iter().map(|d| d.kind).collect();
        assert_eq!(
            kinds,
            vec![DefectKind::DanglingParent, DefectKind::SelfParent],
            "{defects:?}"
        );
        assert!(defects[0].describe().contains("9999"), "{defects:?}");
    }

    /// 层级成环：**一个环只报一条**（20 节点的环刷 20 行等于没报），
    /// 且描述里能读出环路。
    #[test]
    fn a_parent_cycle_is_reported_once() {
        let mut scene = Scene::new("t");
        let a = add(&mut scene, "A");
        let b = add(&mut scene, "B");
        let c = add(&mut scene, "C");
        scene.entities[a].parent = Some(scene.entities[c].id);
        scene.entities[b].parent = Some(scene.entities[a].id);
        scene.entities[c].parent = Some(scene.entities[b].id);

        let defects = scene_defects(&scene);
        assert_eq!(defects.len(), 1, "{defects:?}");
        assert_eq!(defects[0].kind, DefectKind::ParentCycle);
        let text = defects[0].describe();
        assert!(text.contains("3 nodes"), "{text}");
        // 环路按**父指针**方向读（A 的父是 C、C 的父是 B、B 的父是 A），
        // 首尾同名 —— 不是按实体索引顺序，那样读不出"谁挂在谁下面"。
        assert!(text.contains("A -> C -> B -> A"), "{text}");
    }

    /// 环检测不能把"长链"误判成环（环检测写错最典型的症状）。
    #[test]
    fn a_long_chain_is_not_a_cycle() {
        let mut scene = Scene::new("t");
        let mut prev = add(&mut scene, "n0");
        for i in 1..64 {
            let node = add(&mut scene, &format!("n{i}"));
            scene.set_parent(node, Some(prev));
            prev = node;
        }
        assert_eq!(scene_defects(&scene), Vec::new());
    }

    /// `NaN` 坐标与未归一化的四元数：都能通过 RON 解析，症状是物体消失 / 变歪。
    #[test]
    fn bad_transform_numbers_are_reported() {
        let mut scene = Scene::new("t");
        let a = add(&mut scene, "NaN");
        let b = add(&mut scene, "Skewed");
        scene.entities[a].transform.position.x = f32::NAN;
        scene.entities[b].transform.rotation = Quat::from_xyzw(0.0, 0.0, 0.0, 2.0);

        let defects = scene_defects(&scene);
        assert_eq!(
            defects.iter().map(|d| d.kind).collect::<Vec<_>>(),
            vec![DefectKind::NonFiniteTransform, DefectKind::RotationNotUnit],
            "{defects:?}"
        );
        // 报错要指到字段，不能只说"变换坏了"。
        assert!(defects[0].describe().contains("position.x"), "{defects:?}");
        assert!(defects[1].describe().contains("|q|^2 = 4.0"), "{defects:?}");
    }

    /// 已经是 NaN 的实体不再额外报"没归一化"（噪音会让真正的问题更难读）。
    #[test]
    fn non_finite_does_not_also_complain_about_normalization() {
        let mut scene = Scene::new("t");
        let a = add(&mut scene, "Broken");
        scene.entities[a].transform.rotation = Quat::from_xyzw(f32::NAN, 0.0, 0.0, 0.0);
        assert_eq!(kinds(&scene), vec![DefectKind::NonFiniteTransform]);
    }

    /// 光源数值：`NaN` 进着色是整帧全黑 / 全白，比坏的变换还显眼。
    ///
    /// 光源不在 `entities` 里（`Scene::lights` 是独立数组），扫实体那只循环照不到，
    /// 所以单独一条。报错要指到分量（`direction[1]`），不能只说"光源坏了"。
    #[test]
    fn non_finite_lights_are_reported() {
        let mut scene = Scene::new("t");
        add(&mut scene, "Lamp");
        let mut light = crate::light::Light {
            name: "Sun".to_string(),
            ..Default::default()
        };
        light.direction[1] = f32::NAN;
        scene.lights.push(light);

        let defects = scene_defects(&scene);
        assert_eq!(defects.len(), 1, "{defects:?}");
        assert_eq!(defects[0].kind, DefectKind::NonFiniteLight);
        assert!(
            defects[0].describe().contains("direction[1]"),
            "{defects:?}"
        );
    }

    /// 层号越界（`--check` 有专门字段报它，所以标成"别处已报"）。
    #[test]
    fn layer_out_of_range_is_reported_and_flagged_as_elsewhere() {
        let mut scene = Scene::new("t");
        let a = add(&mut scene, "High");
        scene.entities[a].layer = crate::layers::LAYER_COUNT as u8;
        let defects = scene_defects(&scene);
        assert_eq!(defects.len(), 1, "{defects:?}");
        assert_eq!(defects[0].kind, DefectKind::LayerOutOfRange);
        assert!(defects[0].kind.is_reported_elsewhere());
        // 另外两个身份类也必须标对，否则 `--check` 会重复报。
        assert!(DefectKind::DuplicateId.is_reported_elsewhere());
        assert!(!DefectKind::ParentCycle.is_reported_elsewhere());
    }

    /// 输出顺序必须确定（同一次检查多跑几次要能 diff）。
    #[test]
    fn defect_order_is_deterministic() {
        let mut scene = Scene::new("t");
        for i in 0..8 {
            let a = add(&mut scene, &format!("e{i}"));
            scene.entities[a].transform.scale.y = f32::INFINITY;
        }
        let first = scene_defects(&scene);
        let second = scene_defects(&scene);
        assert_eq!(first, second);
        let names: Vec<&str> = first
            .iter()
            .map(|d| d.entity.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(names, vec!["e0", "e1", "e2", "e3", "e4", "e5", "e6", "e7"]);
    }
}
