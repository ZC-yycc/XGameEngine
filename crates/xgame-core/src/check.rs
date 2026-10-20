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
//! - 任何**加载失败**（清单坏、入口场景坏）返回 `Err`；
//! - 加载成功但引用悬空 / 资产缺身份证记在 [`CheckReport`] 里（`has_issues()`）——
//!   缺资产不阻断运行时加载（有兜底），但检查方必须知道。
//!
//! # 为什么检查必须只读
//!
//! `--check` 在 CI 与智能体的"改 → 查 → 验"闭环里被反复调用。它一旦会写盘
//! （比如顺手给缺 `.meta` 的资产生成身份证），"查一次"就改变了项目状态，
//! diff 里会冒出意料之外的文件。所以这里只读索引、只报告；生成 `.meta`
//! 是编辑器打开项目与保存场景时的动作（[`ProjectHandle::ensure_asset_metas`]）。

use std::fmt::Write as _;
use std::path::Path;

use crate::asset::is_guid_reference;
use crate::project::ProjectHandle;

/// 检查结果：能加载到什么程度、有哪些可疑引用。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CheckReport {
    /// 入口场景的实体数。
    pub entities: usize,
    /// 光源数。
    pub lights: usize,
    /// 实体上挂的脚本引用总数。
    pub script_refs: usize,
    /// 实体上挂的音频引用总数。
    pub audio_refs: usize,
    /// 已登记的资产数（有 `.meta` 的）。
    pub assets: usize,
    /// 引用了但盘上不存在的模型（"名字 → 路径"）。
    pub missing_models: Vec<String>,
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
    /// `.meta` 还在但资产文件没了的（孤儿）。
    pub orphan_metas: Vec<String>,
    /// 解析失败的 `.meta`。
    pub invalid_metas: Vec<String>,
    /// 两个资产共用一个 GUID。
    pub guid_conflicts: Vec<String>,
}

impl CheckReport {
    /// 有没有问题（缺引用算问题；只缺资产不阻断加载，但 CI 应当知道）。
    pub fn has_issues(&self) -> bool {
        !self.missing_models.is_empty()
            || !self.missing_scripts.is_empty()
            || !self.missing_audio.is_empty()
            || !self.missing_ui_images.is_empty()
            || !self.missing_prefabs.is_empty()
            || !self.dangling_guids.is_empty()
            || !self.missing_metas.is_empty()
            || !self.orphan_metas.is_empty()
            || !self.invalid_metas.is_empty()
            || !self.guid_conflicts.is_empty()
    }

    /// 问题总数。
    pub fn issue_count(&self) -> usize {
        self.missing_models.len()
            + self.missing_scripts.len()
            + self.missing_audio.len()
            + self.missing_ui_images.len()
            + self.missing_prefabs.len()
            + self.dangling_guids.len()
            + self.missing_metas.len()
            + self.orphan_metas.len()
            + self.invalid_metas.len()
            + self.guid_conflicts.len()
    }

    /// 人和机器都能读的摘要（标准输出用；纯 ASCII，避开控制台编码坑）。
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "check: entities={} lights={} script_refs={} audio_refs={} assets={}",
            self.entities, self.lights, self.script_refs, self.audio_refs, self.assets
        );
        if !self.has_issues() {
            let _ = writeln!(out, "check: OK (all referenced assets exist)");
            return out;
        }
        let _ = writeln!(out, "check: WARN {} issue(s)", self.issue_count());
        for m in &self.missing_models {
            let _ = writeln!(out, "  missing model:  {m}");
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
        out
    }
}

/// 无头检查一个项目：清单能解析、入口场景能加载、引用的资产真实存在、
/// 资产身份证（`.meta`）齐全且不冲突。
pub fn check_project(root: &Path) -> Result<CheckReport, String> {
    let project = ProjectHandle::open(root).map_err(|e| e.to_string())?;
    let scene = project
        .load_entry_scene()
        .map_err(|e| format!("加载入口场景失败: {e}"))?;

    let mut report = CheckReport {
        entities: scene.entities.len(),
        lights: scene.lights.len(),
        ..CheckReport::default()
    };

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
    for entity in &scene.entities {
        // `builtin://cube` 这类是**生成的几何**（渲染器直接造，盘上没有文件），
        // 当成缺失资产报出来就是狼来了 —— 调用方只要真的断了引用才该知道。
        if entity.has_model() {
            if is_guid_reference(&entity.model) {
                // 加载期解码失败的悬空 GUID 会原样留在内存里。
                report
                    .dangling_guids
                    .push(format!("{} -> {}", entity.name, entity.model));
            } else if !entity.model.starts_with("builtin://")
                && !project.path(&entity.model).is_file()
            {
                report
                    .missing_models
                    .push(format!("{} -> {}", entity.name, entity.model));
            }
        }
        for script in &entity.scripts {
            report.script_refs += 1;
            if is_guid_reference(script) {
                report
                    .dangling_guids
                    .push(format!("{} -> {}", entity.name, script));
            } else if !project.path(script).is_file() {
                report
                    .missing_scripts
                    .push(format!("{} -> {}", entity.name, script));
            }
        }
        if let Some(audio) = &entity.audio {
            if !audio.clip.is_empty() {
                report.audio_refs += 1;
                if is_guid_reference(&audio.clip) {
                    report
                        .dangling_guids
                        .push(format!("{} -> {}", entity.name, audio.clip));
                } else if !project.path(&audio.clip).is_file() {
                    report
                        .missing_audio
                        .push(format!("{} -> {}", entity.name, audio.clip));
                }
            }
        }
        // UI 图片：与模型同款规则（GUID 悬空 vs 路径不存在分开报）。
        if let Some(image) = entity.ui.as_ref().and_then(|ui| ui.image.as_ref()) {
            if !image.path.is_empty() {
                if is_guid_reference(&image.path) {
                    report
                        .dangling_guids
                        .push(format!("{} -> {}", entity.name, image.path));
                } else if !project.path(&image.path).is_file() {
                    report
                        .missing_ui_images
                        .push(format!("{} -> {}", entity.name, image.path));
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
                        .push(format!("{} -> {}", entity.name, link.source));
                } else if !project.path(&link.source).is_file() {
                    report
                        .missing_prefabs
                        .push(format!("{} -> {}", entity.name, link.source));
                }
            }
        }
    }

    Ok(report)
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
}
