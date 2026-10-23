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
            + self.build_missing_scenes.len()
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
    for (relative, scene) in &scenes {
        report.scenes += 1;
        report.entities += scene.entities.len();
        report.lights += scene.lights.len();
        check_scene_references(&project, relative, scene, &mut report, &mut materials);
    }

    // -- 材质内容（引用的材质能不能读、里面的贴图在不在）------------------
    for (material, entity) in materials {
        if is_guid_reference(&material) {
            continue; // 上面已按悬空 GUID 报过，不再重复
        }
        if !project.path(&material).is_file() {
            report
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
                    report.dangling_guids.push(format!(
                        "{entity} -> {material} albedo -> {}",
                        asset.albedo
                    ));
                } else if !project.path(&asset.albedo).is_file() {
                    report
                        .missing_material_textures
                        .push(format!("{material} -> {}", asset.albedo));
                }
            }
        }
    }

    Ok(report)
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
) {
    let at = |name: &str| format!("{relative}: {name}");

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
            } else if !project.path(script).is_file() {
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
                } else if !project.path(&audio.clip).is_file() {
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
                } else if !project.path(&image.path).is_file() {
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
                } else if !project.path(&link.source).is_file() {
                    report
                        .missing_prefabs
                        .push(format!("{} -> {}", at(&entity.name), link.source));
                }
            }
        }
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
        std::fs::write(
            project.path("Resources/Materials/Good.mat"),
            "()",
        )
        .unwrap();
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
        project.manifest_mut().build.scenes = vec![
            entry,
            "Scenes/Ghost.ron".into(),
            "../Outside.ron".into(),
        ];
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
