//! 源码查看器：控制台里点一条带位置的报错，就在这里高亮打开那一行。
//!
//! # 为什么是窗口而不是面板
//!
//! 停靠布局是**七个面板**的固定集合（`is_usable_layout` 逐个校验），加第八个
//! 会让所有已有的 `editor.ron` 布局当场失效并回退出厂布局 —— 为了一个"看一眼
//! 就关"的浮层付这个代价不值。窗口不占停靠位，也不参与布局校验。
//!
//! # 能打开什么
//!
//! - **脚本报错**（最有价值）：Lua 的错误信息自带 `Scripts/player.lua:42:`，
//!   路径按**项目根**解析（与运行时读脚本同一套 `ProjectHandle` 口径）。
//! - **引擎日志**（`log::warn!` 等）：位置是 `file!()` 给的仓库相对路径。
//!   拿 `dist` 跑的用户本机没有源码 —— 那就把"找不到这个文件"如实说出来，
//!   不假装能打开。
//!
//! 只读：查看器从不写盘，也不参与热重载。

use std::path::{Path, PathBuf};

use crate::panels::theme;
use crate::state::EditorState;

/// 单个文件最多读这么多字节。
///
/// 防止手滑点开一个几百 MB 的日志 / 数据文件把编辑器卡住十几秒：源码文件
/// 很少超过 512 KiB，超了就把"只显示前 N 行"说清楚。
pub const MAX_BYTES: u64 = 512 * 1024;

/// 打开中的源码窗口状态。
pub struct SourceView {
    /// 原样显示的路径（项目相对或仓库相对），标题栏用。
    pub path: String,
    /// 实际读取的文件（解析后的绝对路径）。
    pub resolved: PathBuf,
    /// 要高亮的行（1 起）。文件行数不够时被夹到最后一行。
    pub line: u32,
    /// 文件内容按行切开（`\t` 已换成 4 空格：egui 的等宽字体对制表符宽度
    /// 没有保证，直接用会让缩进看起来忽宽忽窄）。
    pub lines: Vec<String>,
    /// 读不到时的原因（窗口照样开：至少让人知道点的是哪个文件）。
    pub error: Option<String>,
    /// 第一帧把目标行滚到视野中间，之后交给用户。
    pub scroll_to_line: bool,
}

impl SourceView {
    /// 打开一个文件。`root` 是项目根（脚本的相对路径按它解析）。
    ///
    /// 解析顺序：绝对路径 → 项目根拼接 → 当前工作目录。全都不中就把第一个
    /// 候选路径写进错误信息（否则用户只看到"找不到"，不知道在找什么）。
    pub fn open(path: &str, line: u32, root: Option<&Path>) -> Self {
        let mut candidates: Vec<PathBuf> = Vec::new();
        let raw = Path::new(path);
        if raw.is_absolute() {
            candidates.push(raw.to_path_buf());
        } else {
            if let Some(root) = root {
                candidates.push(root.join(raw));
            }
            candidates.push(raw.to_path_buf());
        }
        let resolved = candidates
            .iter()
            .find(|p| p.is_file())
            .cloned()
            .unwrap_or_else(|| candidates.first().cloned().unwrap_or_else(|| raw.to_path_buf()));

        let mut view = Self {
            path: path.to_owned(),
            resolved,
            line: line.max(1),
            lines: Vec::new(),
            error: None,
            scroll_to_line: true,
        };
        match std::fs::metadata(&view.resolved) {
            Err(err) => {
                view.error = Some(format!("打不开 {}：{err}", view.resolved.display()));
            }
            Ok(meta) if meta.len() > MAX_BYTES => {
                view.error = Some(format!(
                    "文件太大（{} KiB），源码查看器只读 {} KiB 以内的文件",
                    meta.len() / 1024,
                    MAX_BYTES / 1024
                ));
            }
            Ok(_) => match std::fs::read(&view.resolved) {
                Err(err) => {
                    view.error = Some(format!("读不了 {}：{err}", view.resolved.display()));
                }
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    view.lines = text
                        .lines()
                        .map(|l| l.replace('\t', "    "))
                        .collect();
                    if view.lines.is_empty() {
                        view.lines.push(String::new());
                    }
                    // 行号夹进有效范围：报错行号常常是"文件已经被改短了"之后
                    // 的老行号，越界时高亮最后一行比什么都不高亮更有用。
                    let max_line = view.lines.len() as u32;
                    view.line = view.line.clamp(1, max_line);
                }
            },
        }
        view
    }
}

/// 画源码窗口（没有打开时什么都不做）。
pub fn show(state: &mut EditorState, ctx: &egui::Context) {
    // 先 take 出来再放回去：`egui::Window::open(&mut open)` 与闭包里读 view
    // 字段会同时借 `state`，`take` 让两段互斥的借用变成"先拿走、画完再放回"。
    let Some(view) = state.source_view.take() else {
        return;
    };
    let mut open = true;
    let mut scroll_pending = view.scroll_to_line;

    egui::Window::new(format!("源码 · {}", view.path))
        .id(egui::Id::new("source_viewer"))
        .open(&mut open)
        .default_size([760.0, 420.0])
        .resizable(true)
        .show(ctx, |ui| {
            if let Some(err) = &view.error {
                ui.label(
                    egui::RichText::new(err)
                        .size(11.5)
                        .color(theme::colors::WARN),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("（引擎日志里的位置是源码仓库路径：打包后的 exe 不带源码）")
                        .size(10.5)
                        .color(theme::colors::TEXT_DIM),
                );
                return;
            }
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (index, text) in view.lines.iter().enumerate() {
                        let number = index as u32 + 1;
                        let is_target = number == view.line;
                        let bg = if is_target {
                            theme::colors::ACCENT_BLUE.gamma_multiply(0.28)
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        let resp = egui::Frame::none().fill(bg).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                ui.label(
                                    egui::RichText::new(format!("{number:>5}"))
                                        .monospace()
                                        .size(11.0)
                                        .color(if is_target {
                                            theme::colors::TEXT_BRIGHT
                                        } else {
                                            theme::colors::TEXT_DIM
                                        }),
                                );
                                ui.label(
                                    egui::RichText::new(text)
                                        .monospace()
                                        .size(11.0)
                                        .color(if is_target {
                                            theme::colors::TEXT_BRIGHT
                                        } else {
                                            theme::colors::TEXT_PRIMARY
                                        }),
                                );
                            });
                        });
                        if scroll_pending && is_target {
                            ui.scroll_to_rect(resp.response.rect, Some(egui::Align::Center));
                            scroll_pending = false;
                        }
                    }
                });
        });

    if open {
        state.source_view = Some(SourceView {
            scroll_to_line: scroll_pending,
            ..view
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("xgame-srcview-{tag}-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 相对路径按**项目根**解析，行号与内容都拿到。
    #[test]
    fn open_resolves_relative_to_the_project_root() {
        let dir = temp_dir("read");
        std::fs::write(dir.join("player.lua"), "line1\nline2\nline3\n").unwrap();
        let view = SourceView::open("player.lua", 2, Some(&dir));
        assert!(view.error.is_none(), "不该报错: {:?}", view.error);
        assert_eq!(view.lines.len(), 3);
        assert_eq!(view.line, 2);
        assert_eq!(view.lines[1], "line2");
        assert!(view.scroll_to_line, "首帧应滚到目标行");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 行号越界（文件被改短后的老行号）夹到最后一行，不 panic、不空白。
    #[test]
    fn open_clamps_a_stale_line_number() {
        let dir = temp_dir("clamp");
        std::fs::write(dir.join("a.lua"), "only\n").unwrap();
        let view = SourceView::open("a.lua", 999, Some(&dir));
        assert_eq!(view.line, 1, "越界行号应夹到最后一行");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 文件不存在：窗口照样开，错误信息里带**解析后的路径**（否则用户不知道在找什么）。
    #[test]
    fn open_missing_file_reports_the_resolved_path() {
        let dir = temp_dir("missing");
        let view = SourceView::open("nope.lua", 3, Some(&dir));
        let err = view.error.expect("应当报错");
        assert!(err.contains("nope.lua"), "错误信息要带路径: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 超大文件拒绝读取（而不是把编辑器拖死）。
    #[test]
    fn open_refuses_oversized_files() {
        let dir = temp_dir("big");
        let path = dir.join("big.txt");
        let bytes = vec![b'x'; (MAX_BYTES + 1024) as usize];
        std::fs::write(&path, bytes).unwrap();
        let view = SourceView::open("big.txt", 1, Some(&dir));
        let err = view.error.expect("应当拒绝");
        assert!(err.contains("太大"), "应说明是大小问题: {err}");
        assert!(view.lines.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 制表符换成 4 空格（egui 等宽字体不保证制表宽度）。
    #[test]
    fn open_expands_tabs() {
        let dir = temp_dir("tabs");
        std::fs::write(dir.join("t.lua"), "\tx = 1\n").unwrap();
        let view = SourceView::open("t.lua", 1, Some(&dir));
        assert_eq!(view.lines[0], "    x = 1");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
