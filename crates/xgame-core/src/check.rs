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
//! - 加载成功但引用悬空记在 [`CheckReport`] 里（`has_issues()`）——
//!   缺资产不阻断运行时加载（有兜底），但检查方必须知道。

use std::fmt::Write as _;
use std::path::Path;

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
    /// 引用了但盘上不存在的模型（"名字 → 路径"）。
    pub missing_models: Vec<String>,
    /// 引用了但盘上不存在的脚本（"名字 → 路径"）。
    pub missing_scripts: Vec<String>,
}

impl CheckReport {
    /// 有没有问题（缺引用算问题；只缺资产不阻断加载，但 CI 应当知道）。
    pub fn has_issues(&self) -> bool {
        !self.missing_models.is_empty() || !self.missing_scripts.is_empty()
    }

    /// 人和机器都能读的摘要（标准输出用；纯 ASCII，避开控制台编码坑）。
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "check: entities={} lights={} script_refs={}",
            self.entities, self.lights, self.script_refs
        );
        if self.missing_models.is_empty() && self.missing_scripts.is_empty() {
            let _ = writeln!(out, "check: OK (all referenced assets exist)");
            return out;
        }
        let _ = writeln!(
            out,
            "check: WARN {} missing asset(s)",
            self.missing_models.len() + self.missing_scripts.len()
        );
        for m in &self.missing_models {
            let _ = writeln!(out, "  missing model:  {m}");
        }
        for s in &self.missing_scripts {
            let _ = writeln!(out, "  missing script: {s}");
        }
        out
    }
}

/// 无头检查一个项目：清单能解析、入口场景能加载、引用的模型 / 脚本真实存在。
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

    for entity in &scene.entities {
        // `builtin://cube` 这类是**生成的几何**（渲染器直接造，盘上没有文件），
        // 当成缺失资产报出来就是狼来了 —— 调用方只要真的断了引用才该知道。
        if entity.has_model()
            && !entity.model.starts_with("builtin://")
            && !project.path(&entity.model).is_file()
        {
            report
                .missing_models
                .push(format!("{} -> {}", entity.name, entity.model));
        }
        for script in &entity.scripts {
            report.script_refs += 1;
            if !project.path(script).is_file() {
                report
                    .missing_scripts
                    .push(format!("{} -> {}", entity.name, script));
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

    /// 悬空的模型 / 脚本引用必须被点名 —— 这是检查对 CI 和 AI 智能体的
    /// 核心价值：改完资产引用，一条命令就知道改没改坏。
    #[test]
    fn dangling_references_are_reported() {
        let project = temp_project("dangling");
        let mut scene = project.load_entry_scene().expect("加载默认场景");
        scene.entities.push(
            crate::Entity::new("Ghost")
                .with_model("Resources/NotThere.glb")
                .with_script("Scripts/NotThere.lua"),
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
        assert_eq!(report.script_refs, 1);

        let text = report.render();
        assert!(text.contains("missing model"), "{text}");
        assert!(text.contains("missing script"), "{text}");

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
}
