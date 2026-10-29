//! 无头项目检查（`check_project`）。
//!
//! # 为什么它是独立的库函数而不是某个二进制里的私货
//!
//! 验证项目"能不能被完整加载"是**编辑器（`--check` CLI）、CI 与 AI 智能体
//! （MCP `project_check` 工具）三方共用**的能力，放哪一家里都要被另外两家抄一遍
//! —— 抄三份的验证逻辑早晚各自漂移。收在 core 里，谁都能调，语义只有一份。
//!
//! # 契约
//!
//! - 清单坏 / **入口场景**加载失败返回 `Err`（启动契约断了，退出码 1）；
//! - 其余场景加载失败记进 [`CheckReport::invalid_scenes`]，检查继续跑完 ——
//!   一张关卡坏掉不该掩盖其他关卡的问题（退出码 2）；
//! - 加载成功但引用悬空 / 资产缺身份证记在 [`CheckReport`] 里（`has_issues()`）——
//!   缺资产不阻断运行时加载（有兜底），但检查方必须知道。
//!
//! # 为什么检查必须只读
//!
//! `--check` 在 CI 与智能体的"改 → 查 → 验"闭环里被反复调用。它一旦会写盘
//! （比如顺手给缺 `.meta` 的资产生成身份证），"查一次"就改变了项目状态，
//! diff 里会冒出意料之外的文件。所以这里只读索引、只报告；生成 `.meta`
//! 是编辑器打开项目与保存场景时的动作（[`ProjectHandle::ensure_asset_metas`]）。

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::asset::is_guid_reference;
use crate::project::ProjectHandle;

/// 检查结果：能加载到什么程度、有哪些可疑引用。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CheckReport {
    /// 成功加载的场景数（入口 + `Scenes/*.ron`）。
    pub scenes: usize,
    /// 全部场景合计的实体数。
    pub entities: usize,
    /// 全部场景合计的光源数。
    pub lights: usize,
    /// 实体上挂的脚本引用总数。
    pub script_refs: usize,
    /// 实体上挂的音频引用总数。
    pub audio_refs: usize,
    /// 实体上引用的材质资产总数。
    pub material_refs: usize,
    /// 已登记的资产数（有 `.meta` 的）。
    pub assets: usize,
    /// 引用了但盘上不存在的模型（"名字 → 路径"）。
    pub missing_models: Vec<String>,
    /// 引用了但盘上不存在的材质（"实体 → 路径"）。
    pub missing_materials: Vec<String>,
    /// 材质里引用了但盘上不存在的贴图（"材质 → 贴图路径"）。
    pub missing_material_textures: Vec<String>,
    /// 解析失败 / 读不出来的材质文件（"路径 (原因)"）。
    pub invalid_materials: Vec<String>,
    /// 引用了但盘上不存在的脚本（"名字 → 路径"）。
    pub missing_scripts: Vec<String>,
    /// 引用了但盘上不存在的音频（"名字 → 路径"）。
    pub missing_audio: Vec<String>,
    /// UI 元素引用了但盘上不存在的图片（"名字 → 路径"）。
    pub missing_ui_images: Vec<String>,
    /// 引用了但盘上不存在的预制体（只报实例根，避免整棵实例刷屏）。
    pub missing_prefabs: Vec<String>,
    /// 场景 / 预制体里写了 `guid://…` 但资产库里找不到的（悬空 GUID）。
    ///
    /// 与 `missing_*` 分开报：那些是"路径找不到文件"，这些是"身份查无此物"。
    /// 后者通常意味着资产被删了、或者 `.meta` 丢了 —— 修法不一样。
    pub dangling_guids: Vec<String>,
    /// 有资产文件但没有 `.meta` 的（项目相对路径）。
    pub missing_metas: Vec<String>,
    /// 非入口场景加载失败的（"路径 (原因)"）。
    ///
    /// 入口场景坏掉是 `Err`（启动契约断了）；其余关卡坏掉只算问题：多场景
    /// 项目里一张关卡写坏，不该让整条检查命令失去"其他关卡怎么样"的视野。
    pub invalid_scenes: Vec<String>,
    /// `.meta` 还在但资产文件没了的（孤儿）。
    pub orphan_metas: Vec<String>,
    /// 解析失败的 `.meta`。
    pub invalid_metas: Vec<String>,
    /// 两个资产共用一个 GUID。
    pub guid_conflicts: Vec<String>,
    /// 实体声明的层索引超出 0..=31 的（"场景: 实体 -> layer N"）。
    ///
    /// 运行时会把越界值取模兜底（不 panic），但那是**保命**不是**正确**：
    /// 越界通常意味着手写场景抄错了层号，只有报出来才修得掉。
    pub invalid_layers: Vec<String>,
    /// 场景文件里有**重复实体 ID** 的（"场景: id N ×K"）。
    ///
    /// 为什么要单独查：`load_scene` 会在加载边界把重复 ID 修掉
    /// （`Scene::ensure_unique_ids`），于是"加载后再看"永远看不见这档事 ——
    /// 报告全绿，而文件里的落盘伤一直跟着项目走。它是"删一个物体连带删别的"
    /// 的成因，必须在**修之前**看原始文本才知道有没有。
    ///
    /// 修法是打开场景保存一次（修好的 ID 就写回去了）；重复 ID 下父子关系
    /// 已不可还原，层级可能要手工校正。
    pub duplicate_ids: Vec<String>,
    /// 预制体文件里有**重复实体 ID** 的（"预制体: id N ×K"）。
    ///
    /// 与 [`Self::duplicate_ids`] 分开报，因为**修法不同**：场景的重复 ID 会在
    /// 加载边界被 `Scene::ensure_unique_ids` 重分配（同文件引用，损失有界）；
    /// 预制体的 ID 被场景里的 `PrefabInstanceRef::source_id` 跨文件引用，
    /// 加载边界**不许**改 —— 改了就让所有既有实例的 `source_id` 集体失配，
    /// `Apply` 时整棵树被当成"新增"。所以这里只报告，修法是打开该预制体重新保存。
    pub duplicate_prefab_ids: Vec<String>,
    /// 场景里 `PrefabInstanceRef::source_id` 在对应预制体里**找不到**的。
    ///
    /// 跨文件引用的完整性检查。成因：预制体被编辑过（删了实体）、或在外部
    /// 手改过 ID。后果是 `Apply` / `Revert` 对不上位 —— `capture_subtree_ids`
    /// 取不到值就退回 `EntityId::new()`，于是资产里凭空多出一份重复实体。
    /// 按 `(场景, 预制体)` 聚合报一条，避免一个 20 节点实例刷 20 行。
    pub dangling_source_ids: Vec<String>,
    /// 场景的**语义**缺陷（"场景: 描述"），来自 [`crate::validate::scene_defects`]。
    ///
    /// 与上一条的区别：那些查的是"引用通不通"，这里查的是"数据本身能不能用"
    /// —— 悬空父节点、层级成环、`NaN` 坐标、未归一化的四元数。它们**都能通过
    /// RON 解析**，所以只有主动体检才看得见；症状是"实体不在层级树里 / 位置是
    /// NaN / 删一个连带删两个"，与加载失败完全不同源。
    ///
    /// 重复 ID 与越界层号不在这里（各有专门字段，那两条在加载边界/文本层就已
    /// 查过，重复报只会让问题数虚高）。
    pub semantic_defects: Vec<String>,
    /// 发布清单（`build.scenes`）的有效顺序，已规范化去重。
    ///
    /// 空清单不在这里体现：`scene_order()` 会退化成只有入口场景，那是"未配置"
    /// 而不是"配了一个场景"。想区分"配没配"看清单原文是否为空。
    pub build_scene_order: Vec<String>,
    /// 清单里点名了、但盘上没有 / 读不出来的关卡（"路径 (原因)"）。
    ///
    /// **算问题**：发布会把这些关卡一起打进去，运行时切过去必然失败。
    pub build_missing_scenes: Vec<String>,
    /// `Scenes/` 下有、清单里没列的关卡（只提示，不算问题）。
    ///
    /// 未配置清单时不报告 —— 否则每个老项目都会刷一屏"你的关卡都没列入"，
    /// 而它们本来就没打算配置发布清单。
    pub build_unlisted_scenes: Vec<String>,
    /// 路径型引用（非 `guid://`）里**越出项目根**或**Windows 不认**的写法
    /// （"场景: 实体 -> 路径 (原因)"）。
    ///
    /// 为什么要单独查：`ProjectHandle::path` 只是 `root.join(相对路径)`，而
    /// `Path::join` 遇到绝对路径会**整个替换**掉根；`../..` 则逐级上跳。于是
    /// 手写 / 下载来的场景可以引用项目外的任意文件，而且读得到就算"存在"，
    /// 今天的检查一句都不报。资产侧（`normalize_asset_relative`）早就有这道门，
    /// 场景侧一直只挡 `..` 与绝对路径的场景**文件本身**，引用的资产没人管。
    ///
    /// **算问题**（进退出码 2）：这种引用在别人机器上必然断，在作者机器上却是
    /// "能跑"，正是最难查的一类。
    pub unsafe_paths: Vec<String>,
}

impl CheckReport {
    /// 有没有问题（缺引用算问题；只缺资产不阻断加载，但 CI 应当知道）。
    pub fn has_issues(&self) -> bool {
        !self.missing_models.is_empty()
            || !self.missing_materials.is_empty()
            || !self.missing_material_textures.is_empty()
            || !self.invalid_materials.is_empty()
            || !self.missing_scripts.is_empty()
            || !self.missing_audio.is_empty()
            || !self.missing_ui_images.is_empty()
            || !self.missing_prefabs.is_empty()
            || !self.dangling_guids.is_empty()
            || !self.invalid_scenes.is_empty()
            || !self.missing_metas.is_empty()
            || !self.orphan_metas.is_empty()
            || !self.invalid_metas.is_empty()
            || !self.guid_conflicts.is_empty()
            || !self.invalid_layers.is_empty()
            || !self.duplicate_ids.is_empty()
            || !self.duplicate_prefab_ids.is_empty()
            || !self.dangling_source_ids.is_empty()
            || !self.semantic_defects.is_empty()
            || !self.unsafe_paths.is_empty()
            || !self.build_missing_scenes.is_empty()
    }

    /// 问题总数。
    pub fn issue_count(&self) -> usize {
        self.missing_models.len()
            + self.missing_materials.len()
            + self.missing_material_textures.len()
            + self.invalid_materials.len()
            + self.missing_scripts.len()
            + self.missing_audio.len()
            + self.missing_ui_images.len()
            + self.missing_prefabs.len()
            + self.dangling_guids.len()
            + self.invalid_scenes.len()
            + self.missing_metas.len()
            + self.orphan_metas.len()
            + self.invalid_metas.len()
            + self.guid_conflicts.len()
            + self.invalid_layers.len()
            + self.duplicate_ids.len()
            + self.duplicate_prefab_ids.len()
            + self.dangling_source_ids.len()
            + self.semantic_defects.len()
            + self.unsafe_paths.len()
            + self.build_missing_scenes.len()
    }

    /// **引用 / 装载**类问题（"引用的东西不在盘上"或"场景装不起来"）。
    ///
    /// 与 [`Self::issue_count`] 的差别是**归谁管**：身份与格式类问题
    /// （`missing_metas` / `orphan_metas` / `guid_conflicts` / `duplicate_ids` /
    /// `invalid_layers`）在同一个工程的打包前后**一模一样**，让构建替它们背锅
    /// 没有意义 —— 那些归 `--check` 管。
    ///
    /// 存在的理由很具体：打包只复制清单声明的内容目录，清单外的引用会**静默**
    /// 落不进包里。比对源码工程与发布包的这一份列表，就能判断"构建有没有把东西
    /// 弄丢"（见 `xgame_editor::builder` 的 `audit_package`）。
    pub fn reference_issues(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for group in [
            &self.missing_models,
            &self.missing_materials,
            &self.missing_material_textures,
            &self.invalid_materials,
            &self.missing_scripts,
            &self.missing_audio,
            &self.missing_ui_images,
            &self.missing_prefabs,
            &self.dangling_guids,
            &self.invalid_scenes,
            &self.unsafe_paths,
            &self.build_missing_scenes,
        ] {
            out.extend(group.iter().cloned());
        }
        out
    }

    /// 人和机器都能读的摘要（标准输出用；纯 ASCII，避开控制台编码坑）。
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "check: scenes={} entities={} lights={} script_refs={} audio_refs={} material_refs={} assets={}",
            self.scenes,
            self.entities,
            self.lights,
            self.script_refs,
            self.audio_refs,
            self.material_refs,
            self.assets
        );
        if !self.has_issues() {
            let _ = writeln!(out, "check: OK (all referenced assets exist)");
        } else {
            let _ = writeln!(out, "check: WARN {} issue(s)", self.issue_count());
        }
        // 发布清单：配置过才打这一行。未配置（老项目 / 单场景）时 `build_scene_order`
        // 为空 —— 那时候打 `order=[Scenes/scene.ron]` 会让人以为项目"配了清单"。
        if !self.build_scene_order.is_empty() {
            let _ = writeln!(
                out,
                "check: build scenes={} order=[{}]",
                self.build_scene_order.len(),
                self.build_scene_order.join(", ")
            );
        }
        for m in &self.missing_models {
            let _ = writeln!(out, "  missing model:  {m}");
        }
        for m in &self.missing_materials {
            let _ = writeln!(out, "  missing material: {m}");
        }
        for m in &self.missing_material_textures {
            let _ = writeln!(out, "  material texture: {m}");
        }
        for m in &self.invalid_materials {
            let _ = writeln!(out, "  invalid material: {m}");
        }
        for s in &self.missing_scripts {
            let _ = writeln!(out, "  missing script: {s}");
        }
        for a in &self.missing_audio {
            let _ = writeln!(out, "  missing audio:  {a}");
        }
        for i in &self.missing_ui_images {
            let _ = writeln!(out, "  missing ui image: {i}");
        }
        for p in &self.missing_prefabs {
            let _ = writeln!(out, "  missing prefab: {p}");
        }
        for g in &self.dangling_guids {
            let _ = writeln!(out, "  dangling guid:  {g}");
        }
        for s in &self.invalid_scenes {
            let _ = writeln!(out, "  invalid scene:  {s}");
        }
        for m in &self.missing_metas {
            let _ = writeln!(out, "  missing .meta:  {m}");
        }
        for m in &self.orphan_metas {
            let _ = writeln!(out, "  orphan .meta:   {m}");
        }
        for m in &self.invalid_metas {
            let _ = writeln!(out, "  invalid .meta:  {m}");
        }
        for c in &self.guid_conflicts {
            let _ = writeln!(out, "  guid conflict:  {c}");
        }
        for l in &self.invalid_layers {
            let _ = writeln!(out, "  bad layer:      {l}");
        }
        for d in &self.duplicate_ids {
            let _ = writeln!(out, "  duplicate id:   {d}");
        }
        for d in &self.duplicate_prefab_ids {
            let _ = writeln!(out, "  duplicate prefab id: {d}");
        }
        for d in &self.dangling_source_ids {
            let _ = writeln!(out, "  dangling source: {d}");
        }
        for d in &self.semantic_defects {
            let _ = writeln!(out, "  semantic:       {d}");
        }
        for p in &self.unsafe_paths {
            let _ = writeln!(out, "  unsafe path:    {p}");
        }
        for s in &self.build_missing_scenes {
            let _ = writeln!(out, "  build scene missing: {s}");
        }
        // 提示（不影响退出码）：写了场景但没列入发布清单。
        for s in &self.build_unlisted_scenes {
            let _ = writeln!(out, "  not in build list: {s}");
        }
        out
    }
}

/// 无头检查一个项目：清单能解析、入口场景能加载、**所有场景**引用的资产
/// 真实存在、资产身份证（`.meta`）齐全且不冲突。
pub fn check_project(root: &Path) -> Result<CheckReport, String> {
    let project = ProjectHandle::open(root).map_err(|e| e.to_string())?;

    // 入口场景先单独加载：它是启动契约的一部分，坏掉必须让 `--check` 硬失败
    // （退出码 1），而不是混进"有问题的场景"里只算警告。
    let entry_scene = project
        .load_entry_scene()
        .map_err(|e| format!("加载入口场景失败: {e}"))?;
    let entry = project.manifest().entry_scene.clone();
    let entry_key = entry.replace('\\', "/");

    let mut report = CheckReport::default();
    // 清单**自己**写的内容目录也要体检：越界的写法不会在读取时报错，只会让
    // 扫描 / 打包悄悄跑到项目外，症状是"检查全绿、成品缺整棵子树"。
    report
        .unsafe_paths
        .extend(project.manifest().content_path_defects());
    let mut scenes = vec![(entry.clone(), entry_scene)];
    // 已经装载过的场景键（正斜杠）。清单里的关卡未必在 `Scenes/` 下（比如
    // `Levels/L1.ron`），所以两个来源都要装载，靠这份集合去重 —— 否则同一张
    // 关卡会被算两遍实体、引用问题也会报两次。
    let mut loaded: Vec<String> = vec![entry_key.clone()];

    // -- 发布清单（`build.scenes`）----------------------------------------
    // 清单 = "要发布的关卡"，顺序 = 关卡索引。三件事：
    // 1) 逐项规范化（越界 / 非 `.ron` 算问题，与编辑器共用同一份规则）；
    // 2) 把清单里的关卡装载进来（可能在 `Scenes/` 之外，否则永远不被检查）；
    // 3) 报告"盘上有、清单没列"的场景 —— 只提示，不改退出码。
    let configured: Vec<String> = project
        .manifest()
        .build
        .scenes
        .iter()
        .map(|raw| raw.trim().replace('\\', "/"))
        .filter(|path| !path.is_empty())
        .collect();
    if !configured.is_empty() {
        let mut order: Vec<String> = Vec::new();
        for raw in &configured {
            match crate::project::normalize_scene_relative(raw) {
                Ok(relative) => {
                    if !order.contains(&relative) {
                        order.push(relative);
                    }
                }
                Err(e) => report.build_missing_scenes.push(format!("{raw} ({e})")),
            }
        }
        for relative in &order {
            if loaded.iter().any(|key| key == relative) {
                continue;
            }
            match project.load_scene(relative) {
                Ok(scene) => {
                    loaded.push(relative.clone());
                    scenes.push((relative.clone(), scene));
                }
                Err(e) => report
                    .build_missing_scenes
                    .push(format!("{relative} ({e})")),
            }
        }
        for relative in project.list_scenes() {
            if !order.contains(&relative) {
                report.build_unlisted_scenes.push(relative);
            }
        }
        report.build_scene_order = order;
    }

    for relative in project.list_scenes() {
        let key = relative.replace('\\', "/");
        if loaded.iter().any(|seen| seen == &key) {
            continue;
        }
        match project.load_scene(&relative) {
            Ok(scene) => {
                loaded.push(key);
                scenes.push((relative, scene));
            }
            // 单张关卡坏掉不阻断检查：报告里点名，其他场景照常查完。
            Err(e) => report.invalid_scenes.push(format!("{relative} ({e})")),
        }
    }

    // -- 重复实体 ID（只在原始文本上看得见）--------------------------------
    //
    // 必须放在**所有场景都装完之后**、并且独立于 `scenes` 里的内存副本：
    // `load_scene` 已经把重复 ID 修好了（`ensure_unique_ids`），从内存里查
    // 永远是干净的。这里按 `loaded` 的顺序回到磁盘原文再看一遍，输出才是
    // 确定性的（同一次检查、多跑几次结果一致，方便 diff）。
    for relative in &loaded {
        if let Ok(duplicates) = project.scene_duplicate_ids(relative) {
            for (id, count) in duplicates {
                report
                    .duplicate_ids
                    .push(format!("{relative}: id {} ×{count}", id.raw()));
            }
        }
        // 读不出来 / 解析失败上面已经按 `invalid_scenes` 报过，不重复报。
    }

    // -- 预制体文件的重复实体 ID（同样只在原始文本上看得见）----------------
    //
    // 场景那份可以在加载边界修，预制体这份**不能**（ID 被场景的 `source_id`
    // 跨文件引用，重分配会让所有既有实例失配），所以这里只报告。汇总在
    // `project.list_prefabs()` 上而不是场景引用上：没被任何场景用到的预制体
    // 也可能是坏的，打开它就立刻掉进同一个坑。
    for relative in project.list_prefabs() {
        if let Ok(duplicates) = project.prefab_duplicate_ids(&relative) {
            for (id, count) in duplicates {
                report
                    .duplicate_prefab_ids
                    .push(format!("{relative}: id {} ×{count}", id.raw()));
            }
        }
        // 解析失败的预制体此处静默跳过：内容级诊断不在本轮范围，硬报会
        // 把"坏 RON"和"重复 ID"混成一类问题。
    }

    // -- 资产库诊断（只读，不生成任何文件）--------------------------------
    {
        let assets = project.assets();
        report.assets = assets.len();
        let diagnostics = assets.diagnostics();
        for path in &diagnostics.missing_meta {
            report.missing_metas.push(path.clone());
        }
        for path in &diagnostics.orphan_meta {
            report.orphan_metas.push(path.clone());
        }
        for bad in &diagnostics.invalid_meta {
            report
                .invalid_metas
                .push(format!("{} ({})", bad.path, bad.reason));
        }
        for conflict in &diagnostics.conflicts {
            report
                .guid_conflicts
                .push(format!("{} -> {:?}", conflict.guid, conflict.paths));
        }
    }

    // -- 场景引用 ---------------------------------------------------------
    // 材质按路径**跨场景**去重：同一个材质挂在一百个实体上时，坏文件只该报一次。
    let mut materials: BTreeMap<String, String> = BTreeMap::new();
    // 预制体 ID 集合缓存：同一份预制体被十个实例引用时只解析一次。
    // `None` = 读不出来（已在 `missing_prefabs` / `dangling_guids` 报过），
    // 缓存住这个"坏"避免每个实例都重试一遍磁盘。
    let mut prefab_ids: BTreeMap<String, Option<crate::id::EntitySet>> = BTreeMap::new();
    for (relative, scene) in &scenes {
        report.scenes += 1;
        report.entities += scene.entities.len();
        report.lights += scene.lights.len();
        check_scene_references(
            &project,
            relative,
            scene,
            &mut report,
            &mut materials,
            &mut prefab_ids,
        );
        // 语义体检：重复 ID 与越界层号在这里跳过 —— 它们分别由
        // `duplicate_ids`（磁盘原文）与 `invalid_layers`（实体字段）报过，
        // 同一件事报两遍会让 `issue_count` 虚高。
        for defect in crate::validate::scene_defects(scene) {
            if defect.kind.is_reported_elsewhere() {
                continue;
            }
            report
                .semantic_defects
                .push(format!("{relative}: {}", defect.describe()));
        }
    }

    // -- 材质内容（引用的材质能不能读、里面的贴图在不在）------------------
    for (material, entity) in materials {
        if is_guid_reference(&material) {
            continue; // 上面已按悬空 GUID 报过，不再重复
        }
        // 越界的材质路径已经点名，**不必再探盘、更不能去读它**（读的可能是
        // 项目外的任意文件）。
        if note_unsafe_path(&mut report, &entity, "material", &material) {
            continue;
        }
        if !project.path(&material).is_file() {            report
                .missing_materials
                .push(format!("{entity} -> {material}"));
            continue;
        }
        match project.load_material(&material) {
            Err(e) => report.invalid_materials.push(format!("{material} ({e})")),
            Ok(asset) => {
                if asset.albedo.is_empty() {
                    continue;
                }
                if is_guid_reference(&asset.albedo) {
                    report
                        .dangling_guids
                        .push(format!("{entity} -> {material} albedo -> {}", asset.albedo));
                } else if !note_unsafe_path(&mut report, &material, "albedo", &asset.albedo)
                    && !project.path(&asset.albedo).is_file()
                {
                    report
                        .missing_material_textures
                        .push(format!("{material} -> {}", asset.albedo));
                }
            }
        }
    }

    Ok(report)
}

/// 记一条"路径型引用越界 / Windows 不认"的问题，返回它是不是**不合法**。
///
/// 只在**非 `guid://`** 的引用上调用：GUID 引用没有路径语义，越界是另一回事
/// （由 `dangling_guids` 管）。`builtin://` 也不是文件，调用方自己排除。
///
/// 返回值是给调用方用的：路径不合法时**不必**再去探"文件在不在" —— 那个路径
/// 在本机碰巧有文件，与"这个引用合不合法"是两件事，报两遍只会让问题数虚高。
/// 标签用 [`PathDefect::ascii`] 而不是中文：报告要能被 CI 文本匹配，
/// 也要能在 GBK 控制台上看清。
fn note_unsafe_path(report: &mut CheckReport, at: &str, kind: &str, raw: &str) -> bool {
    match crate::project::normalize_project_relative(raw) {
        Ok(_) => false,
        Err(defect) => {
            report
                .unsafe_paths
                .push(format!("{at} -> {kind} {raw} ({})", defect.ascii()));
            true
        }
    }
}

/// 检查单张场景里的实体引用。
///
/// `relative` 会拼在每条问题前面（`Scenes/Level2.ron: Ghost -> ...`）：多场景
/// 项目里只说实体名根本定位不到是哪张关卡。材质收集到 `materials` 里跨场景
/// 去重，值记"首次引用它的场景 + 实体"，让缺失材质也有落点。
fn check_scene_references(
    project: &ProjectHandle,
    relative: &str,
    scene: &crate::Scene,
    report: &mut CheckReport,
    materials: &mut BTreeMap<String, String>,
    prefab_ids: &mut BTreeMap<String, Option<crate::id::EntitySet>>,
) {
    let at = |name: &str| format!("{relative}: {name}");
    // 悬空 `source_id` 按预制体聚合成一条，避免一个 20 节点实例刷 20 行。
    // 值 = (缺失数量, 首个缺失实体的名字, 首个缺失的 source_id)。
    let mut dangling_sources: BTreeMap<String, (usize, String, u64)> = BTreeMap::new();

    for entity in &scene.entities {
        // 层索引越界：读盘刻意**不修正**（修正会把配置错误藏起来），运行时按
        // `sanitize_layer` 取模兜底 —— 所以这里必须报，否则"我明明设了第 40 层"
        // 永远查不出来，只会表现为一群实体莫名挤在同一层上。
        if entity.layer as usize >= crate::layers::LAYER_COUNT {
            report
                .invalid_layers
                .push(format!("{} -> layer {}", at(&entity.name), entity.layer));
        }
        // `builtin://cube` 这类是**生成的几何**（渲染器直接造，盘上没有文件），
        // 当成缺失资产报出来就是狼来了 —— 调用方只要真的断了引用才该知道。
        if entity.has_model() {
            if is_guid_reference(&entity.model) {
                // 加载期解码失败的悬空 GUID 会原样留在内存里。
                report
                    .dangling_guids
                    .push(format!("{} -> {}", at(&entity.name), entity.model));
            } else if !entity.model.starts_with("builtin://")
                && !note_unsafe_path(report, &at(&entity.name), "model", &entity.model)
                && !project.path(&entity.model).is_file()
            {
                report
                    .missing_models
                    .push(format!("{} -> {}", at(&entity.name), entity.model));
            }
        }
        if let Some(material) = &entity.material_asset {
            if !material.is_empty() {
                report.material_refs += 1;
                materials
                    .entry(material.clone())
                    .or_insert_with(|| at(&entity.name));
                if is_guid_reference(material) {
                    // 加载期解码失败的悬空 GUID 会原样留在内存里。
                    report
                        .dangling_guids
                        .push(format!("{} -> {}", at(&entity.name), material));
                }
            }
        }
        for script in &entity.scripts {
            report.script_refs += 1;
            if is_guid_reference(script) {
                report
                    .dangling_guids
                    .push(format!("{} -> {}", at(&entity.name), script));
            } else if !note_unsafe_path(report, &at(&entity.name), "script", script)
                && !project.path(script).is_file()
            {
                report
                    .missing_scripts
                    .push(format!("{} -> {}", at(&entity.name), script));
            }
        }
        if let Some(audio) = &entity.audio {
            if !audio.clip.is_empty() {
                report.audio_refs += 1;
                if is_guid_reference(&audio.clip) {
                    report
                        .dangling_guids
                        .push(format!("{} -> {}", at(&entity.name), audio.clip));
                } else if !note_unsafe_path(report, &at(&entity.name), "audio", &audio.clip)
                    && !project.path(&audio.clip).is_file()
                {
                    report
                        .missing_audio
                        .push(format!("{} -> {}", at(&entity.name), audio.clip));
                }
            }
        }
        // UI 图片：与模型同款规则（GUID 悬空 vs 路径不存在分开报）。
        if let Some(image) = entity.ui.as_ref().and_then(|ui| ui.image.as_ref()) {
            if !image.path.is_empty() {
                if is_guid_reference(&image.path) {
                    report
                        .dangling_guids
                        .push(format!("{} -> {}", at(&entity.name), image.path));
                } else if !note_unsafe_path(report, &at(&entity.name), "ui image", &image.path)
                    && !project.path(&image.path).is_file()
                {
                    report
                        .missing_ui_images
                        .push(format!("{} -> {}", at(&entity.name), image.path));
                }
            }
        }
        // 预制体引用：整棵实例的每个实体都带同一份 `source`，只报根一次，
        // 否则一个 20 节点的实例会刷 20 行一模一样的问题。
        if let Some(link) = &entity.prefab {
            if link.is_root {
                if is_guid_reference(&link.source) {
                    report
                        .dangling_guids
                        .push(format!("{} -> {}", at(&entity.name), link.source));
                } else if !note_unsafe_path(report, &at(&entity.name), "prefab", &link.source)
                    && !project.path(&link.source).is_file()
                {
                    report
                        .missing_prefabs
                        .push(format!("{} -> {}", at(&entity.name), link.source));
                }
            }
            // 跨文件引用的完整性：`source_id` 必须能在**对应的**预制体里找到。
            // 与上面的"文件在不在"是两件事 —— 文件在、但里面那个实体没了
            // （预制体被编辑过），`Apply` / `Revert` 就对不上位，而且不报错。
            if !is_guid_reference(&link.source) {
                let known = prefab_ids
                    .entry(link.source.clone())
                    .or_insert_with(|| {
                        project
                            .load_prefab(&link.source)
                            .ok()
                            .map(|p| p.entities.iter().map(|e| e.id).collect())
                    })
                    .as_ref();
                if let Some(known) = known {
                    if !known.contains(&link.source_id) {
                        let slot = dangling_sources.entry(link.source.clone()).or_insert((
                            0,
                            entity.name.clone(),
                            link.source_id.raw(),
                        ));
                        slot.0 += 1;
                    }
                }
            }
        }
    }

    for (source, (count, first_name, first_id)) in dangling_sources {
        // 文案保持纯 ASCII（见 `render` 的说明）：控制台代码页不是 UTF-8 时，
        // 中文会变成乱码，而 CI 读的就是这行文本。实体名本身可能含中文，
        // 那是**数据**，不是这里的文案。
        report.dangling_source_ids.push(format!(
            "{relative}: {source} missing {count} source_id(s) (first: {first_name} -> id {first_id})"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时项目（用完记得删）。
    fn temp_project(tag: &str) -> ProjectHandle {
        let base = std::env::temp_dir().join(format!(
            "xgame_check_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        ProjectHandle::create(&base, "CheckTest").expect("创建临时项目")
    }

    /// 干净项目：加载成功、零悬空引用。
    #[test]
    fn clean_project_passes() {
        let project = temp_project("clean");
        let report = check_project(project.root()).expect("干净项目应能检查");
        assert!(!report.has_issues(), "{}", report.render());
        assert_eq!(report.scenes, 1, "默认项目只有入口场景");
        assert!(report.entities >= 1, "默认场景至少有一个实体");
        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 重复 ID 必须在**原始文件**上被看见。
    ///
    /// 加载边界会顺手把重复 ID 修掉（`Scene::ensure_unique_ids`），所以"加载后
    /// 再检查"永远干净 —— 报告必须回到文件本身去看，否则 CI 全绿而落盘伤一直
    /// 跟着项目走（它是"删一个物体连带删别的"的成因）。
    ///
    /// 旧实现根本没有这个字段（编译都过不去，那是最强形式的"红"）；顺带钉住
    /// **只读**契约：查一次不能把文件改好。
    #[test]
    fn duplicate_entity_ids_are_reported_even_though_loading_repairs_them() {
        let project = temp_project("dup_ids");
        let entry = project.manifest().entry_scene.clone();
        let mut scene = project.load_entry_scene().expect("加载默认场景");
        let shared = scene.add_entity(crate::Entity::new("Twin"));
        let mut second = crate::Entity::new("Twin Twin");
        second.id = shared;
        scene.entities.push(second);
        project.write_scene(&entry, &scene).expect("写回场景");

        let report = check_project(project.root()).expect("检查应能跑完");
        assert!(
            !report.duplicate_ids.is_empty(),
            "重复 ID 必须被报出来：{}",
            report.render()
        );
        assert!(
            report.duplicate_ids[0].contains("×2"),
            "应当报出出现次数，实际 {}",
            report.duplicate_ids[0]
        );
        // 文本输出也要有：CI 读的是 `--check` 的标准输出，不是结构体。
        assert!(
            report.render().contains("duplicate id"),
            "报告文本必须包含这一行：{}",
            report.render()
        );

        let after = project.scene_duplicate_ids(&entry).expect("再读一次");
        assert!(!after.is_empty(), "检查必须只读，不能把重复 ID 修好写回去");

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 语义体检接进 `--check`：层级成环与非有限变换都要报，且算问题（进退出码）。
    ///
    /// 与上面那条重复 ID 的区别：这一层查的是**能通过 RON 解析**的数据伤。
    /// 症状是"实体不在层级树里 / 位置是 NaN / 删一个连带删两个"，没有一个会
    /// 报错，所以只有主动体检才看得见。
    #[test]
    fn semantic_defects_are_reported() {
        let project = temp_project("semantic");
        let entry = project.manifest().entry_scene.clone();
        let mut scene = project.load_entry_scene().expect("加载默认场景");
        // ① 两节点成环（都不再是根 → 层级面板里两个都"消失"）。
        let a = scene.add_entity(crate::Entity::new("LoopA"));
        let b = scene.add_entity(crate::Entity::new("LoopB"));
        let ai = scene.find_entity(a).expect("刚加");
        let bi = scene.find_entity(b).expect("刚加");
        scene.entities[ai].parent = Some(b);
        scene.entities[bi].parent = Some(a);
        // ② 非有限坐标（世界矩阵被污染成 NaN，整个子树消失）。
        let nan = scene.add_entity(crate::Entity::new("Nan"));
        let ni = scene.find_entity(nan).expect("刚加");
        scene.entities[ni].transform.position.y = f32::NEG_INFINITY;
        project.write_scene(&entry, &scene).expect("写回场景");

        let report = check_project(project.root()).expect("检查应能跑完");
        let joined = report.semantic_defects.join("\n");
        assert!(joined.contains("parent cycle"), "缺环报告：{joined}");
        assert!(
            joined.contains("non-finite transform"),
            "缺 NaN 报告：{joined}"
        );
        assert!(
            report.has_issues(),
            "语义缺陷必须算问题（退出码由它决定）：{}",
            report.render()
        );
        assert!(
            report.render().contains("semantic:"),
            "报告文本必须包含这一行：{}",
            report.render()
        );
        // 这两条属于"数据本身"，不是"引用的东西不在盘上"：不该让打包去背锅。
        assert!(
            report.reference_issues().is_empty(),
            "语义缺陷不该进 reference_issues：{:?}",
            report.reference_issues()
        );

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 预制体里的重复 ID 必须被报告，且**绝不能**在加载边界被修。
    ///
    /// 场景那份可以在加载边界重分配（同文件引用，损失有界）；预制体的 ID 被
    /// 场景里的 `PrefabInstanceRef::source_id` 跨文件引用 —— 一旦重分配，
    /// 所有既有实例的 `source_id` 集体失配，`Apply` 会把整棵树当成"新增"。
    /// 所以这条测试同时钉两件事：**报得出来** + **没被改**。
    #[test]
    fn duplicate_prefab_ids_are_reported_and_never_repaired() {
        use crate::prefab::Prefab;

        let project = temp_project("dup_prefab_ids");
        let mut prefab = Prefab::new("Twin");
        let first = crate::Entity::new("A");
        let shared = first.id;
        prefab.entities.push(first);
        let mut second = crate::Entity::new("B");
        second.id = shared;
        prefab.entities.push(second);
        let relative = "Prefabs/Twin.prefab.ron";
        project.write_prefab(relative, &prefab).expect("写预制体");

        let report = check_project(project.root()).expect("检查应能跑完");
        assert!(
            report.has_issues(),
            "预制体重复 ID 必须算问题（`--check` 退出码由它决定）：{}",
            report.render()
        );
        assert!(
            !report.duplicate_prefab_ids.is_empty(),
            "预制体的重复 ID 必须被报出来：{}",
            report.render()
        );
        assert!(
            report.duplicate_prefab_ids[0].contains("×2"),
            "应当报出出现次数，实际 {}",
            report.duplicate_prefab_ids[0]
        );
        assert!(
            report.render().contains("duplicate prefab id"),
            "报告文本必须包含这一行：{}",
            report.render()
        );

        // 只读 + 不修：再读一次，两个实体仍然是同一个 ID。
        let after = project.load_prefab(relative).expect("再读一次");
        assert_eq!(
            after.entities[0].id, after.entities[1].id,
            "预制体的重复 ID 不许在加载边界被静默重分配（跨文件引用）"
        );
        assert!(
            !project
                .prefab_duplicate_ids(relative)
                .expect("只读统计")
                .is_empty(),
            "检查必须只读，不能把文件里的重复 ID 改好"
        );

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 场景里 `source_id` 指向预制体里不存在的实体，必须被报告。
    ///
    /// 这是"文件在、但里面那个实体没了"（预制体被编辑过 / ID 被手改）。
    /// 不报的后果是 `Apply` / `Revert` 静默对不上位：`capture_subtree_ids`
    /// 取不到值就退回 `EntityId::new()`，资产里凭空多出一份重复实体。
    #[test]
    fn dangling_source_id_is_reported() {
        use crate::prefab::{Prefab, PrefabInstanceRef};

        let project = temp_project("dangling_source");
        let mut prefab = Prefab::new("Solo");
        let alive = crate::Entity::new("Alive");
        let alive_id = alive.id;
        prefab.entities.push(alive);
        let prefab_relative = "Prefabs/Solo.prefab.ron";
        project
            .write_prefab(prefab_relative, &prefab)
            .expect("写预制体");

        let mut scene = project.load_entry_scene().expect("加载默认场景");
        // 对得上的一份 + 对不上的一份。
        let mut ok = crate::Entity::new("GoodInstance");
        ok.prefab = Some(PrefabInstanceRef {
            source: prefab_relative.to_string(),
            source_id: alive_id,
            is_root: true,
        });
        scene.entities.push(ok);
        let mut bad = crate::Entity::new("StaleInstance");
        bad.prefab = Some(PrefabInstanceRef {
            source: prefab_relative.to_string(),
            source_id: crate::EntityId::from_raw(9_999),
            is_root: true,
        });
        scene.entities.push(bad);
        let entry = project.manifest().entry_scene.clone();
        project.write_scene(&entry, &scene).expect("写场景");

        let report = check_project(project.root()).expect("检查应能跑完");
        assert!(
            report.has_issues(),
            "悬空 source_id 必须算问题（`--check` 退出码由它决定）：{}",
            report.render()
        );
        assert_eq!(
            report.dangling_source_ids.len(),
            1,
            "同一预制体只聚合成一条：{}",
            report.render()
        );
        let line = &report.dangling_source_ids[0];
        assert!(
            line.contains("missing 1 source_id") && line.contains("StaleInstance"),
            "应点名缺失数量与首个实体，实际 {line}"
        );
        assert!(
            !line.contains("GoodInstance"),
            "对得上的实例不该出现在报告里：{line}"
        );
        assert!(
            report.render().contains("dangling source"),
            "报告文本必须包含这一行：{}",
            report.render()
        );

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 悬空的模型 / 脚本 / 音频引用必须被点名 —— 这是检查对 CI 和 AI 智能体
    /// 的核心价值：改完资产引用，一条命令就知道改没改坏。
    #[test]
    fn dangling_references_are_reported() {
        let project = temp_project("dangling");
        let mut scene = project.load_entry_scene().expect("加载默认场景");
        scene.entities.push(
            crate::Entity::new("Ghost")
                .with_model("Resources/NotThere.glb")
                .with_script("Scripts/NotThere.lua")
                .with_audio(crate::AudioSource::new("Resources/NotThere.wav")),
        );
        // 内置几何不是文件引用，绝不能被报成"缺失"。
        scene
            .entities
            .push(crate::Entity::new("Cube").with_model("builtin://cube"));
        let entry = project.manifest().entry_scene.clone();
        project.write_scene(&entry, &scene).expect("写场景");

        let report = check_project(project.root()).expect("检查应成功");
        assert!(report.has_issues());
        assert_eq!(report.missing_models.len(), 1, "{}", report.render());
        assert_eq!(report.missing_scripts.len(), 1, "{}", report.render());
        assert_eq!(report.missing_audio.len(), 1, "{}", report.render());
        assert_eq!(report.script_refs, 1);
        assert_eq!(report.audio_refs, 1);

        let text = report.render();
        assert!(text.contains("missing model"), "{text}");
        assert!(text.contains("missing script"), "{text}");
        assert!(text.contains("missing audio"), "{text}");

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 路径型引用必须落在项目内：`../../x.glb` 这类写法今天能通过检查
    /// （`Path::join` 遇到绝对路径会**整个替换**掉项目根），于是"从外面下载来的
    /// 场景能读走项目外的任意文件"。它不是加载失败，是**加载成功但来源不对**。
    #[test]
    fn references_escaping_the_project_are_reported() {
        let project = temp_project("escape");
        let mut scene = project.load_entry_scene().expect("加载默认场景");
        scene.entities.push(
            crate::Entity::new("Escape").with_model("../../secret.glb"),
        );
        scene.entities.push(
            crate::Entity::new("Absolute").with_model("/etc/passwd"),
        );
        // 负对照：正常的项目内相对路径一句都不该报。
        scene.entities.push(
            crate::Entity::new("Normal").with_model("builtin://cube"),
        );
        let entry = project.manifest().entry_scene.clone();
        project.write_scene(&entry, &scene).expect("写场景");

        let report = check_project(project.root()).expect("检查应成功");
        assert_eq!(report.unsafe_paths.len(), 2, "{}", report.render());
        assert!(
            report.unsafe_paths.iter().any(|p| p.contains("secret.glb")),
            "{:?}",
            report.unsafe_paths
        );
        assert!(
            report.has_issues(),
            "越界引用必须进退出码 2，而不是只当提示"
        );
        assert!(report.reference_issues().iter().any(|p| p.contains("secret.glb")));
        assert!(report.render().contains("unsafe path"), "{}", report.render());

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 清单里的内容目录也是路径：`../..` 会让扫描与打包一起跑出项目外。
    ///
    /// 今天这种写法**一声不响** —— `list_scenes` 扫不到任何东西、构建复制不到
    /// 目录，症状是"检查全绿、成品缺整棵子树"。`--check` 必须点名。
    #[test]
    fn manifest_content_dirs_are_validated() {
        let mut project = temp_project("manifest_paths");
        project.manifest_mut().content.scenes_dir = "../outside-scenes".into();
        project.save().expect("写清单");

        let report = check_project(project.root()).expect("检查应成功");
        assert!(report.has_issues(), "{}", report.render());
        assert!(
            report
                .unsafe_paths
                .iter()
                .any(|p| p.contains("content.scenes_dir")),
            "内容目录越界必须点名: {:?}",
            report.unsafe_paths
        );

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 非入口场景也要检查：只查入口场景时，第二关的断引用会一路漏到运行时。
    ///
    /// 旧实现只 `load_entry_scene()`，这条断言必红（`missing_models` 为空）。
    #[test]
    fn non_entry_scenes_are_checked() {
        let project = temp_project("all_scenes");
        let mut level2 = crate::project::default_scene();
        level2.name = "Level2".to_owned();
        level2.entities.push(
            crate::Entity::new("Ghost")
                .with_model("Resources/NotThere.glb")
                .with_script("Scripts/NotThere.lua"),
        );
        project.write_scene("Scenes/Level2.ron", &level2).unwrap();

        let report = check_project(project.root()).expect("坏的是非入口场景，检查不该失败");
        assert_eq!(report.scenes, 2, "{}", report.render());
        assert!(report.has_issues());
        assert_eq!(report.missing_models.len(), 1, "{}", report.render());
        assert!(
            report.missing_models[0].starts_with("Scenes/Level2.ron:"),
            "问题必须带场景路径，否则定位不到是哪一关: {}",
            report.missing_models[0]
        );
        assert_eq!(report.missing_scripts.len(), 1, "{}", report.render());
        assert!(report.render().contains("Scenes/Level2.ron"));

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 非入口场景文件本身写坏：记 `invalid_scenes` + 退出码 2，不返回 `Err`；
    /// 入口场景写坏才是 `Err`（退出码 1，启动契约断了）。
    #[test]
    fn broken_non_entry_scene_is_an_issue_not_fatal() {
        let project = temp_project("broken_scene");
        std::fs::write(project.path("Scenes/Level2.ron"), "this is not ron ((").unwrap();

        let report = check_project(project.root()).expect("单张关卡坏掉不该让整条命令失败");
        assert_eq!(report.invalid_scenes.len(), 1, "{}", report.render());
        assert!(
            report.invalid_scenes[0].starts_with("Scenes/Level2.ron"),
            "{}",
            report.invalid_scenes[0]
        );
        assert!(report.has_issues());
        assert!(report.render().contains("invalid scene"));

        let entry = project.manifest().entry_scene.clone();
        std::fs::write(project.path(&entry), "this is not ron ((").unwrap();
        assert!(
            check_project(project.root()).is_err(),
            "入口场景坏掉必须硬失败（退出码 1）"
        );

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 材质引用：缺文件 / 坏文件 / 缺贴图都要点名，且同一材质只报一次。
    #[test]
    fn material_references_are_checked() {
        let project = temp_project("materials");
        std::fs::create_dir_all(project.path("Resources/Textures")).unwrap();
        std::fs::create_dir_all(project.path("Resources/Materials")).unwrap();
        std::fs::write(project.path("Resources/Textures/OK.png"), b"png").unwrap();
        std::fs::write(project.path("Resources/Materials/Good.mat"), "()").unwrap();
        std::fs::write(
            project.path("Resources/Materials/NoTex.mat"),
            "(albedo: \"Resources/Textures/Gone.png\")",
        )
        .unwrap();
        std::fs::write(project.path("Resources/Materials/Bad.mat"), "not ron ((").unwrap();

        let mut scene = project.load_entry_scene().unwrap();
        scene
            .entities
            .push(crate::Entity::new("Good").with_material_asset("Resources/Materials/Good.mat"));
        // 同一份坏材质挂两次：只该报一行。
        scene.entities.push(
            crate::Entity::new("NoTexA").with_material_asset("Resources/Materials/NoTex.mat"),
        );
        scene.entities.push(
            crate::Entity::new("NoTexB").with_material_asset("Resources/Materials/NoTex.mat"),
        );
        scene
            .entities
            .push(crate::Entity::new("Bad").with_material_asset("Resources/Materials/Bad.mat"));
        scene.entities.push(
            crate::Entity::new("Missing").with_material_asset("Resources/Materials/Gone.mat"),
        );
        let entry = project.manifest().entry_scene.clone();
        project.write_scene(&entry, &scene).unwrap();

        let report = check_project(project.root()).unwrap();
        assert_eq!(report.material_refs, 5, "{}", report.render());
        assert_eq!(report.missing_materials.len(), 1, "{}", report.render());
        assert!(report.missing_materials[0].contains("Gone.mat"));
        assert_eq!(report.invalid_materials.len(), 1, "{}", report.render());
        assert!(report.invalid_materials[0].contains("Bad.mat"));
        assert_eq!(
            report.missing_material_textures.len(),
            1,
            "同一材质挂两次只能报一次: {}",
            report.render()
        );
        assert!(report.missing_material_textures[0].contains("NoTex.mat"));
        assert!(report.has_issues());
        assert!(report.render().contains("missing material"));
        assert!(report.render().contains("invalid material"));

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 坏清单（不是合法 RON）→ `Err`（CLI 退出码 1 的依据）。
    #[test]
    fn broken_manifest_is_an_error() {
        let project = temp_project("broken");
        std::fs::write(project.path("project.ron"), "this is not ron ((").expect("写坏清单");
        assert!(check_project(project.root()).is_err());
        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 资产缺 `.meta` 要报，但检查本身**不许写盘**（只读契约）。
    #[test]
    fn missing_meta_is_reported_without_writing() {
        let project = temp_project("meta");
        std::fs::create_dir_all(project.path("Resources")).unwrap();
        std::fs::write(project.path("Resources/Box.glb"), b"glb").unwrap();

        let report = check_project(project.root()).expect("检查应成功");
        assert_eq!(report.assets, 0);
        assert_eq!(report.missing_metas, vec!["Resources/Box.glb".to_owned()]);
        assert!(report.has_issues());
        assert!(
            !project.path("Resources/Box.glb.meta").exists(),
            "--check 必须只读：它不该顺手生成 .meta"
        );

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 悬空 GUID（资产被删，.meta 也没了）→ 单独报 dangling，而不是混进
    /// "missing model"（后者是路径找不到文件，修法不同）。
    #[test]
    fn dangling_guid_is_reported_separately() {
        let project = temp_project("dangling_guid");
        std::fs::create_dir_all(project.path("Resources")).unwrap();
        std::fs::write(project.path("Resources/Box.glb"), b"glb").unwrap();

        let mut scene = project.load_entry_scene().unwrap();
        scene
            .entities
            .push(crate::Entity::new("Box").with_model("Resources/Box.glb"));
        let entry = project.manifest().entry_scene.clone();
        project.write_scene(&entry, &scene).unwrap();

        // 删资产 + .meta：场景里留着 guid://，库里查不到。
        std::fs::remove_file(project.path("Resources/Box.glb")).unwrap();
        std::fs::remove_file(project.path("Resources/Box.glb.meta")).unwrap();
        project.refresh_assets();

        let report = check_project(project.root()).unwrap();
        assert_eq!(report.dangling_guids.len(), 1, "{}", report.render());
        assert!(report.dangling_guids[0].contains("guid://"));
        assert!(report.missing_models.is_empty(), "不该混进路径缺失");
        assert!(report.render().contains("dangling guid"));

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// GUID 冲突必须被检查出来 —— 冲突时引用解析结果是"看运气"的。
    #[test]
    fn guid_conflict_is_reported() {
        let project = temp_project("conflict");
        std::fs::create_dir_all(project.path("Resources")).unwrap();
        std::fs::write(project.path("Resources/A.glb"), b"a").unwrap();
        std::fs::write(project.path("Resources/B.glb"), b"b").unwrap();
        let shared = crate::AssetGuid::generate().to_reference();
        let guid = shared.trim_start_matches(crate::GUID_SCHEME);
        for name in ["A", "B"] {
            std::fs::write(
                project.path(format!("Resources/{name}.glb.meta")),
                format!("(guid: \"{guid}\")"),
            )
            .unwrap();
        }
        project.refresh_assets();

        let report = check_project(project.root()).unwrap();
        assert_eq!(report.guid_conflicts.len(), 1, "{}", report.render());
        assert!(report.render().contains("guid conflict"));

        let _ = std::fs::remove_dir_all(project.root());
    }

    // -- 发布清单（build.scenes）------------------------------------------

    /// 复制入口场景到另一个路径（内容一样，场景名不同）。
    fn add_level(project: &ProjectHandle, relative: &str) {
        let mut scene = project.load_entry_scene().expect("入口场景");
        scene.name = relative.to_string();
        project.write_scene(relative, &scene).expect("写场景");
    }

    /// 清单里的关卡要**被装载**：哪怕它不在 `Scenes/` 下（`Levels/` 这种布局），
    /// 否则它的悬空引用永远查不出来，发布会带着坏引用出门。
    /// 顺序必须原样保留，重复项只留一次（否则 `scene_index()` 有两个答案）。
    #[test]
    fn build_list_is_loaded_and_ordered() {
        let mut project = temp_project("build_ok");
        add_level(&project, "Scenes/Level2.ron");
        add_level(&project, "Levels/L3.ron");
        let entry = project.manifest().entry_scene.clone();
        project.manifest_mut().build.scenes = vec![
            entry.clone(),
            "Scenes\\Level2.ron".into(), // 反斜杠 + 重复（规范化后与下一项相同）
            "Scenes/Level2.ron".into(),
            "Levels/L3.ron".into(),
        ];
        project.save().unwrap();

        let report = check_project(project.root()).unwrap();
        assert!(!report.has_issues(), "{}", report.render());
        assert_eq!(
            report.build_scene_order,
            vec![
                entry.clone(),
                "Scenes/Level2.ron".to_string(),
                "Levels/L3.ron".to_string(),
            ],
            "顺序保留、重复去掉、反斜杠规范化"
        );
        assert_eq!(report.scenes, 3, "入口 + Level2 + Levels/L3 都要装载并检查");
        assert!(report.build_unlisted_scenes.is_empty(), "全部列入了");
        let text = report.render();
        assert!(text.contains("check: build scenes=3"), "{text}");

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 清单指向不存在的关卡 = **问题**（退出码 2）：发布出去切过去必然失败。
    /// 路径越界同样算问题，而且原因要写清（"必须是项目相对路径"）。
    #[test]
    fn build_list_missing_or_illegal_scene_is_an_issue() {
        let mut project = temp_project("build_bad");
        let entry = project.manifest().entry_scene.clone();
        project.manifest_mut().build.scenes =
            vec![entry, "Scenes/Ghost.ron".into(), "../Outside.ron".into()];
        project.save().unwrap();

        let report = check_project(project.root()).unwrap();
        assert!(report.has_issues(), "{}", report.render());
        assert_eq!(report.build_missing_scenes.len(), 2, "{}", report.render());
        // 两类错误分开落在同一份清单里：规范化失败的（越界）先入，装载失败的（缺文件）后入。
        // 断言只认内容不认次序 —— 换句更难听的话：别让测试钉住"谁先被检查"。
        assert!(
            report
                .build_missing_scenes
                .iter()
                .any(|m| m.contains("Scenes/Ghost.ron")),
            "缺文件要报路径: {:?}",
            report.build_missing_scenes
        );
        assert!(
            report
                .build_missing_scenes
                .iter()
                .any(|m| m.contains("项目相对路径") || m.contains("`..`")),
            "越界要报原因: {:?}",
            report.build_missing_scenes
        );
        assert!(report.render().contains("build scene missing"));
        // 顺序 = **语法合法**的清单项：越界项（`../`）进不来，缺文件的进得来。
        // 缺文件不该被悄悄"从顺序里剔掉" —— 那会让后面的关卡索引整体前移，
        // 与清单对不上（Author 看到的顺序和运行时拿到的顺序必须是同一份）。
        assert_eq!(
            report.build_scene_order,
            vec![
                project.manifest().entry_scene.clone(),
                "Scenes/Ghost.ron".to_string(),
            ]
        );

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// "盘上有、清单没列"只是**提示**：Unity 的 Build Settings 也不会因为你多写了
    /// 一张场景就判构建失败。所以它不能把 `has_issues()` 弄成 true —— 否则
    /// 每个配了清单的项目都会在 CI 里红。
    #[test]
    fn unlisted_scene_is_a_note_not_an_issue() {
        let mut project = temp_project("build_unlisted");
        add_level(&project, "Scenes/Level2.ron");
        let entry = project.manifest().entry_scene.clone();
        project.manifest_mut().build.scenes = vec![entry];
        project.save().unwrap();

        let report = check_project(project.root()).unwrap();
        assert!(!report.has_issues(), "{}", report.render());
        assert_eq!(
            report.build_unlisted_scenes,
            vec!["Scenes/Level2.ron".to_string()]
        );
        assert!(report.render().contains("not in build list"));

        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 未配置清单：不提示、不打 build 行 —— 老项目不该被新功能刷屏。
    #[test]
    fn empty_build_list_stays_quiet() {
        let project = temp_project("build_empty");
        add_level(&project, "Scenes/Level2.ron");
        let report = check_project(project.root()).unwrap();
        assert!(!report.has_issues(), "{}", report.render());
        assert!(report.build_scene_order.is_empty());
        assert!(report.build_unlisted_scenes.is_empty());
        assert!(!report.render().contains("check: build"));
        assert_eq!(report.scenes, 2, "未配置清单也照常检查所有场景");

        let _ = std::fs::remove_dir_all(project.root());
    }
}
