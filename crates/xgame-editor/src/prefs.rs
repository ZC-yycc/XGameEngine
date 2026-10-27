//! 编辑器级偏好与面板布局持久化（`editor.ron`）。
//!
//! # 为什么单独一份文件
//!
//! 最近项目只是"历史记录"，坏了丢了无感；布局坏了会让人一打开编辑器就找不到
//! 面板。两者写同一个文件的后果是：一次布局序列化失败会连最近项目一起丢，
//! 所以分成 `recent_projects.ron` 与 `editor.ron`，各自独立读写。
//!
//! # 磁盘是外部输入
//!
//! 读入永远经过 [`EditorPreferences::sanitized`]（数值钳制）与
//! [`EditorPreferences::layout_for_startup`]（布局完整性校验 + 回退出厂布局）。
//! 老版本写的布局少了后来新增的面板、手改出悬空 tile，都只能在这里挡住 ——
//! 再往下就是 `egui_tiles::Tree::ui`，它假定树是合法的。
//!
//! # 写盘是原子的
//!
//! 同目录独占暂存、同步后替换。拖页签时保存是"顺手"发生的，写到一半被
//! 杀进程不该留下半截 RON（下次启动会静默回退默认布局，用户的调整全白费）。
//! 替换失败必须留下旧文件，不能退回覆写。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::panels::dock::{self, PanelTab};
use crate::state::{EditorCamera, EditorState};

/// 配置文件名。
pub const FILE_NAME: &str = "editor.ron";

fn default_camera_speed() -> f32 {
    EditorCamera::DEFAULT_PAN_SPEED
}

fn default_camera_sensitivity() -> f32 {
    EditorCamera::DEFAULT_SENSITIVITY
}

fn default_show_grid() -> bool {
    true
}

fn default_autosave_interval() -> u64 {
    crate::autosave::DEFAULT_INTERVAL_SECS
}

/// 跨项目保存的编辑器偏好。
///
/// 字段全部 `#[serde(default)]`：以后加字段时，老文件照常能读。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorPreferences {
    /// 自由相机平移速度（世界单位/秒）。
    #[serde(default = "default_camera_speed")]
    pub camera_speed: f32,
    /// 右键拖拽转视角的灵敏度。
    #[serde(default = "default_camera_sensitivity")]
    pub camera_sensitivity: f32,
    /// 视口网格默认是否显示。
    #[serde(default = "default_show_grid")]
    pub show_grid: bool,
    /// 自动保存间隔（秒）；`0` = 关闭。
    ///
    /// 归**编辑器**偏好而不是项目设置：它是"这个人怎么用编辑器"，
    /// 换项目不该重置（同相机速度）。
    #[serde(default = "default_autosave_interval")]
    pub autosave_interval_secs: u64,
    /// 停靠布局；`None` = 从没保存过（使用出厂布局）。
    #[serde(default)]
    pub layout: Option<egui_tiles::Tree<PanelTab>>,
}

impl Default for EditorPreferences {
    fn default() -> Self {
        Self {
            camera_speed: default_camera_speed(),
            camera_sensitivity: default_camera_sensitivity(),
            show_grid: default_show_grid(),
            autosave_interval_secs: default_autosave_interval(),
            layout: None,
        }
    }
}

impl EditorPreferences {
    /// 夹取所有从磁盘读出来的数值。
    ///
    /// `f32` 的 NaN 会一路穿过比较与格式化，最后表现成"相机不动 / 速度显示 NaN"；
    /// 在这里换默认值，是唯一能保证坏数据不出现在界面上的位置。
    pub fn sanitized(mut self) -> Self {
        self.camera_speed = clamp_finite(
            self.camera_speed,
            EditorCamera::MIN_PAN_SPEED,
            EditorCamera::MAX_PAN_SPEED,
            EditorCamera::DEFAULT_PAN_SPEED,
        );
        self.camera_sensitivity = clamp_finite(
            self.camera_sensitivity,
            EditorCamera::MIN_SENSITIVITY,
            EditorCamera::MAX_SENSITIVITY,
            EditorCamera::DEFAULT_SENSITIVITY,
        );
        // 间隔可能是手改出来的离谱值（1 秒 = 逐帧写盘；十天 = 等于没有）。
        // `0` 是合法的"关闭"，不能被夹成 10。
        self.autosave_interval_secs =
            crate::autosave::sanitize_interval(self.autosave_interval_secs);
        self
    }

    /// 启动 / 打开项目时使用的布局。
    ///
    /// 没存过 → 出厂布局；存了但校验不过 → 记一条警告并回退出厂布局。
    /// 这里**不修改** `self.layout`：本次运行内保留坏文件便于排查；关闭项目 /
    /// 退出时 [`sync_layout`](Self::sync_layout) 会用实际使用的布局覆盖它（自愈）。
    pub fn layout_for_startup(&self) -> egui_tiles::Tree<PanelTab> {
        match &self.layout {
            None => dock::create_tile_tree(),
            Some(tree) if dock::is_usable_layout(tree) => dock::normalize_layout(tree.clone()),
            Some(_) => {
                log::warn!(
                    "保存的面板布局不完整或已损坏（缺少面板 / 悬空 tile），本次使用默认布局"
                );
                dock::create_tile_tree()
            }
        }
    }

    /// 把偏好应用到刚创建的编辑器状态。
    pub fn apply_to_state(&self, state: &mut EditorState) {
        state.editor_camera.pan_speed = self.camera_speed;
        state.editor_camera.sensitivity = self.camera_sensitivity;
        state.show_grid = self.show_grid;
        state.set_autosave_interval(self.autosave_interval_secs);
    }

    /// 把当前布局同步进偏好；返回是否真的变了。
    ///
    /// 比较整棵树只在"松手 / 关闭项目 / 退出"这些低频时刻发生；返回 `false`
    /// 时调用方**不要写盘**（同一棵树反复同步不该反复写磁盘）。
    pub fn sync_layout(&mut self, tree: &egui_tiles::Tree<PanelTab>) -> bool {
        if self.layout.as_ref() == Some(tree) {
            return false;
        }
        self.layout = Some(tree.clone());
        true
    }

    /// 从默认配置路径读取；缺失 / 损坏都退默认值（不阻塞启动）。
    pub fn load() -> Self {
        let Some(path) = store_path() else {
            return Self::default();
        };
        Self::load_from(&path)
    }

    /// 纯路径版本（便于单测）。文件不存在不算错误。
    pub fn load_from(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match ron::from_str::<Self>(&text) {
                Ok(prefs) => prefs.sanitized(),
                Err(e) => {
                    log::warn!("编辑器偏好 {} 解析失败，使用默认值: {e}", path.display());
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                log::warn!("编辑器偏好 {} 读取失败，使用默认值: {e}", path.display());
                Self::default()
            }
        }
    }

    /// 写回默认配置路径；失败只记警告。
    ///
    /// 返回 `bool` 只是给测试与调用方判断"这次到底写没写"，UI 不依赖它。
    pub fn save(&self) -> bool {
        let Some(path) = store_path() else {
            return false;
        };
        match self.save_to(&path) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("编辑器偏好 {} 写入失败: {e}", path.display());
                false
            }
        }
    }

    /// 写盘（同目录独占暂存、同步后替换；失败不覆盖原文件）。
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        write_staged(path, &text)
    }
}

/// 两份编辑器配置共用同一条替换路径，避免最近项目列表退回直接覆写。
///
/// 实现只在 `xgame_core::fsx` 一处（同目录独占暂存 + 同步 + 替换 + 超龄暂存回收），
/// 这里保留薄包装是因为调用点在编辑器这一层、错误类型也用 `io::Error`。
pub(crate) fn write_staged(path: &Path, text: &str) -> std::io::Result<()> {
    xgame_core::fsx::write_atomic(path, text)
}

/// 有限值钳制：非有限输入直接换默认值（不是钳到端点 —— NaN 钳完还是 NaN）。
fn clamp_finite(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

/// 默认配置文件路径。
fn store_path() -> Option<PathBuf> {
    crate::config::file(FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        // 不引 tempfile：文件名带进程 id + 纳秒，够避免并行测试互撞。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "xgame-prefs-{tag}-{}-{nanos}.ron",
            std::process::id()
        ))
    }

    #[test]
    fn defaults_match_the_editor_camera() {
        let prefs = EditorPreferences::default();
        assert_eq!(prefs.camera_speed, EditorCamera::DEFAULT_PAN_SPEED);
        assert_eq!(prefs.camera_sensitivity, EditorCamera::DEFAULT_SENSITIVITY);
        assert!(prefs.show_grid);
        assert!(prefs.layout.is_none());
    }

    #[test]
    fn sanitize_clamps_out_of_range_and_non_finite() {
        let prefs = EditorPreferences {
            camera_speed: 999.0,
            camera_sensitivity: -1.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(prefs.camera_speed, EditorCamera::MAX_PAN_SPEED);
        assert_eq!(prefs.camera_sensitivity, EditorCamera::MIN_SENSITIVITY);

        let nan = EditorPreferences {
            camera_speed: f32::NAN,
            camera_sensitivity: f32::INFINITY,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(nan.camera_speed, EditorCamera::DEFAULT_PAN_SPEED);
        assert_eq!(nan.camera_sensitivity, EditorCamera::DEFAULT_SENSITIVITY);
    }

    /// 布局是数据格式：序列化再读回来必须仍是同一棵树，且七个面板都在。
    #[test]
    fn layout_round_trips_through_ron() {
        let prefs = EditorPreferences {
            layout: Some(dock::create_tile_tree()),
            ..Default::default()
        };
        let text = ron::ser::to_string_pretty(&prefs, ron::ser::PrettyConfig::default()).unwrap();
        let back: EditorPreferences = ron::from_str(&text).unwrap();

        let restored = back.layout_for_startup();
        assert!(dock::is_usable_layout(&restored));
        assert_eq!(restored, dock::create_tile_tree());
    }

    /// 少了 Profiler 的老布局必须被识别并回退出厂布局（不是少画一个面板）。
    #[test]
    fn layout_missing_a_pane_falls_back_to_default() {
        // 只保留其它六个：模拟老版本文件（当时还没有 Profiler 变体）。
        // 用“重新构造”而不是 `remove`，避免依赖 egui_tiles 的删除语义。
        let mut tiles = egui_tiles::Tiles::default();
        let ids = PanelTab::ALL
            .iter()
            .filter(|p| **p != PanelTab::Profiler)
            .map(|p| tiles.insert_pane(p.clone()))
            .collect::<Vec<_>>();
        let root = tiles.insert_tab_tile(ids);
        let tree = egui_tiles::Tree::new("editor_tree", root, tiles);

        let prefs = EditorPreferences {
            layout: Some(tree),
            ..Default::default()
        };
        let restored = prefs.layout_for_startup();
        assert_eq!(restored, dock::create_tile_tree());
        assert!(dock::is_usable_layout(&restored));
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "not a ron file {{{").unwrap();
        let prefs = EditorPreferences::load_from(&path);
        assert_eq!(prefs, EditorPreferences::default());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_file_is_defaults() {
        let path = temp_path("missing");
        let prefs = EditorPreferences::load_from(&path);
        assert_eq!(prefs, EditorPreferences::default());
    }

    #[test]
    fn save_then_load_keeps_values_and_layout() {
        let path = temp_path("roundtrip");
        let prefs = EditorPreferences {
            camera_speed: 12.5,
            camera_sensitivity: 0.004,
            show_grid: false,
            autosave_interval_secs: 90,
            layout: Some(dock::create_tile_tree()),
        };
        prefs.save_to(&path).unwrap();

        let back = EditorPreferences::load_from(&path);
        assert_eq!(back.camera_speed, 12.5);
        assert_eq!(back.camera_sensitivity, 0.004);
        assert!(!back.show_grid);
        assert_eq!(back.autosave_interval_secs, 90);
        assert_eq!(back.layout, prefs.layout);
        assert!(dock::is_usable_layout(&back.layout_for_startup()));

        // 临时文件不该留在配置目录里。
        assert!(!path.with_extension("ron.tmp").exists());
        let _ = std::fs::remove_file(path);
    }

    /// 坏数值从磁盘读进来也要被夹住，不能等到相机真的跑起来才发现。
    #[test]
    fn load_sanitizes_bad_numbers() {
        let path = temp_path("badnums");
        std::fs::write(
            &path,
            "(camera_speed: NaN, camera_sensitivity: 99.0, show_grid: false, layout: None)",
        )
        .unwrap();
        let prefs = EditorPreferences::load_from(&path);
        assert_eq!(prefs.camera_speed, EditorCamera::DEFAULT_PAN_SPEED);
        assert_eq!(prefs.camera_sensitivity, EditorCamera::MAX_SENSITIVITY);
        let _ = std::fs::remove_file(path);
    }

    /// 布局同步：第一次（没存过）算变化，第二次（同一棵树）必须是无操作 ——
    /// 否则每次松手 / 关窗都会白写一次磁盘。
    #[test]
    fn sync_layout_detects_change_then_becomes_noop() {
        let mut prefs = EditorPreferences::default();
        let tree = dock::create_tile_tree();

        assert!(prefs.sync_layout(&tree));
        assert_eq!(prefs.layout.as_ref(), Some(&tree));
        assert!(!prefs.sync_layout(&tree));
    }

    #[cfg(windows)]
    #[test]
    fn locked_preferences_preserve_previous_contents_and_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let path = temp_path("locked-save");
        let old = EditorPreferences::default();
        old.save_to(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        let new = EditorPreferences {
            camera_speed: 12.5,
            ..old
        };
        assert!(new.save_to(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(lock);
        new.save_to(&path).unwrap();
        assert_eq!(EditorPreferences::load_from(&path), new);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stale_preference_staging_does_not_block_save() {
        let path = temp_path("stale-save");
        let legacy = path.with_extension("ron.tmp");
        std::fs::write(&legacy, "旧版残留").unwrap();
        let prefs = EditorPreferences::default();
        prefs.save_to(&path).unwrap();
        assert_eq!(EditorPreferences::load_from(&path), prefs);
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "旧版残留");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(legacy).unwrap();
    }

    /// 枚举变体名就是磁盘格式：`PanelTab::SceneView` 必须写成 `SceneView`。
    #[test]
    fn panel_tab_names_are_stable_in_ron() {
        let text = ron::to_string(&PanelTab::SceneView).unwrap();
        assert_eq!(text, "SceneView");
        let back: PanelTab = ron::from_str("SceneView").unwrap();
        assert_eq!(back, PanelTab::SceneView);
    }
}
