//! 项目枢纽——编辑器启动后的第一屏。
//!
//! 参考 Unity Hub / Godot 项目管理器的做法：编辑器本身不知道「要编辑哪个项目」，
//! 得先在这里**创建**或**打开**一个项目目录，之后才会进入编辑器主界面。
//!
//! 这样游戏项目就完全独立于引擎源码：引擎仓库里只有 `crates/`，项目可以放在
//! 磁盘上任何地方（默认建议放在引擎仓库的 `projects/` 下）。
//!
//! # 为什么不用独立的启动器程序
//!
//! 商业引擎里 Unity 有独立的 Hub，Godot 则是编辑器内置项目管理器。这里选了后者：
//! 编辑器二进制同时承担「枢纽」和「编辑器」两个界面，靠 [`HubAction`] 切换，
//! 少一个需要单独分发的可执行文件。

use std::path::{Path, PathBuf};

use egui::RichText;
use xgame_core::{ProjectError, ProjectHandle};

use crate::panels::{icons, theme};
use crate::recents::RecentProjects;

/// 项目枢纽每帧返回的动作。
pub enum HubAction {
    /// 什么也没发生，继续停留在此界面。
    None,
    /// 用户选定了项目，主程序应进入编辑器主界面。
    Open(Box<ProjectHandle>),
}

/// 项目枢纽界面状态。
pub struct ProjectHub {
    /// 最近打开的项目。
    recents: RecentProjects,
    /// 「最近项目」的磁盘校验结果缓存。
    ///
    /// `RecentProjects::existing()` 会对每条记录做一次 `is_file()`。这是磁盘 IO，
    /// 而且一旦某条指向失效的网络路径或已拔出的移动盘，单次 `is_file()` 就可能
    /// 阻塞几百毫秒。之前这行直接写在 `ui()` 里 —— 等于每帧去戳一遍磁盘，
    /// 界面自然一顿一顿的。现在只在列表真的变化（首次加载 / 打开 / 移除）时刷新。
    recent_paths: Vec<PathBuf>,
    /// 新建项目的名称。
    new_name: String,
    /// 新建项目的上级目录。
    new_location: String,
    /// 打开已有项目的路径。
    open_path: String,
    /// 提示 / 错误信息（红色显示）。
    message: Option<String>,
}

impl ProjectHub {
    pub fn new() -> Self {
        let recents = RecentProjects::load();
        let recent_paths = recents.existing().cloned().collect();
        Self {
            recents,
            recent_paths,
            new_name: "MyGame".to_string(),
            new_location: default_projects_dir().display().to_string(),
            open_path: String::new(),
            message: None,
        }
    }

    /// 重新校验「最近项目」并刷新缓存。列表变化后调用。
    fn refresh_recents(&mut self) {
        self.recent_paths = self.recents.existing().cloned().collect();
    }

    /// 尝试直接打开命令行传入的项目路径（跳过 UI 交互）。
    ///
    /// 路径无效时把错误显示在界面上，而不是直接退出——用户还能在界面里另选一个。
    pub fn try_open(&mut self, path: &Path) -> HubAction {
        self.open_project_at(path.to_path_buf())
    }

    pub fn ui(&mut self, ctx: &egui::Context) -> HubAction {
        // -- 先把可变状态取出来，避免闭包里到处借用 self --
        let mut new_name = std::mem::take(&mut self.new_name);
        let mut new_location = std::mem::take(&mut self.new_location);
        let mut open_path = std::mem::take(&mut self.open_path);

        let mut browse_new_location = false;
        let mut browse_open_path = false;
        let mut do_create = false;
        let mut do_open = false;
        let mut open_recent: Option<PathBuf> = None;
        let mut forget_recent: Option<PathBuf> = None;

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::colors::WINDOW_BG))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(48.0);

                    ui.label(theme::bold("XGameEngine").size(26.0));
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("选择、创建或打开一个游戏项目")
                            .size(12.0)
                            .color(theme::colors::TEXT_SECONDARY),
                    );
                    ui.add_space(20.0);

                    // 高度必须给成**真实可用高度**：`vec2(640, 0)` 会把子 ui 的
                    // `max_rect` 高度设成 0，里面的 `ScrollArea` 于是按"可用高度 0"
                    // 夹自己的视口 —— 表现是最近项目列表只画出前三行。
                    ui.allocate_ui_with_layout(
                        egui::vec2(640.0, ui.available_height()),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            // 同 field_row 的坑：`allocate_ui_with_layout` 只按子 ui
                            // 的 min_rect 占位，不撑宽的话这一列会缩到最宽那张卡片
                            // 的宽度，卡片之间的左右边距全看内容。
                            ui.set_min_width(640.0);
                            // ── 新建项目 ──
                            theme::card_frame().show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                theme::card_title(ui, "新建项目");

                                theme::field_row(ui, "名称", |ui| {
                                    ui.add_sized(
                                        egui::vec2(ui.available_width(), theme::FIELD_HEIGHT + 2.0),
                                        egui::TextEdit::singleline(&mut new_name)
                                            .hint_text("MyGame"),
                                    );
                                });
                                theme::field_row(ui, "位置", |ui| {
                                    let browse_w = 62.0;
                                    let field_w =
                                        (ui.available_width() - browse_w - 5.0).max(120.0);
                                    ui.add_sized(
                                        egui::vec2(field_w, theme::FIELD_HEIGHT + 2.0),
                                        egui::TextEdit::singleline(&mut new_location),
                                    );
                                    if ui
                                        .add_sized(
                                            egui::vec2(browse_w, theme::FIELD_HEIGHT + 2.0),
                                            egui::Button::new("浏览…"),
                                        )
                                        .clicked()
                                    {
                                        browse_new_location = true;
                                    }
                                });
                                ui.label(theme::dim_note("项目将创建在「位置/名称」目录下"));

                                ui.add_space(8.0);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("创建项目").color(egui::Color32::WHITE),
                                        )
                                        .fill(theme::colors::ACCENT_BLUE)
                                        .min_size(egui::vec2(96.0, theme::FIELD_HEIGHT + 6.0)),
                                    )
                                    .clicked()
                                {
                                    do_create = true;
                                }
                            });

                            ui.add_space(12.0);

                            // ── 打开已有项目 ──
                            theme::card_frame().show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                theme::card_title(ui, "打开项目");

                                theme::field_row(ui, "路径", |ui| {
                                    let browse_w = 62.0;
                                    let field_w =
                                        (ui.available_width() - browse_w - 5.0).max(120.0);
                                    ui.add_sized(
                                        egui::vec2(field_w, theme::FIELD_HEIGHT + 2.0),
                                        egui::TextEdit::singleline(&mut open_path)
                                            .hint_text("项目根目录（含 project.ron）"),
                                    );
                                    if ui
                                        .add_sized(
                                            egui::vec2(browse_w, theme::FIELD_HEIGHT + 2.0),
                                            egui::Button::new("浏览…"),
                                        )
                                        .clicked()
                                    {
                                        browse_open_path = true;
                                    }
                                });

                                ui.add_space(8.0);
                                if ui
                                    .add(
                                        egui::Button::new("打开项目").min_size(egui::vec2(
                                            96.0,
                                            theme::FIELD_HEIGHT + 6.0,
                                        )),
                                    )
                                    .clicked()
                                {
                                    do_open = true;
                                }
                            });

                            ui.add_space(12.0);

                            // ── 最近项目 ──
                            theme::card_frame().show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("最近项目")
                                            .family(theme::bold_family())
                                            .size(13.0)
                                            .color(theme::colors::TEXT_BRIGHT),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(theme::dim_note(format!(
                                                "{} 个",
                                                self.recent_paths.len()
                                            )));
                                        },
                                    );
                                });
                                ui.add_space(2.0);
                                {
                                    let width = ui.available_width();
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(width, 1.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().hline(
                                        rect.x_range(),
                                        rect.center().y,
                                        egui::Stroke::new(
                                            1.0_f32,
                                            theme::colors::SEPARATOR_SOFT,
                                        ),
                                    );
                                }
                                ui.add_space(4.0);

                                if self.recent_paths.is_empty() {
                                    ui.label(theme::dim_note("还没有打开过项目"));
                                }

                                egui::ScrollArea::vertical()
                                    .max_height(200.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        for path in &self.recent_paths {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 8.0;
                                                // 名字列定宽：路径因此**对齐成一列**，
                                                // 不会因为项目名长短而在每行左右跳。
                                                let name = display_name(path);
                                                let clicked = ui
                                                    .allocate_ui_with_layout(
                                                        egui::vec2(112.0, 20.0),
                                                        egui::Layout::left_to_right(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            // 同上：不撑宽度的话名字列只有
                                                            // 文字那么宽，"BehaviorsDemo" 会把
                                                            // 路径顶到右边，整列对不齐。
                                                            ui.set_min_width(112.0);
                                                            let response = ui.add(
                                                                egui::Label::new(
                                                                    RichText::new(name)
                                                                        .size(12.0),
                                                                )
                                                                .truncate()
                                                                .sense(egui::Sense::click()),
                                                            );
                                                            if response.hovered() {
                                                                ui.ctx().set_cursor_icon(
                                                                    egui::CursorIcon::PointingHand,
                                                                );
                                                            }
                                                            response.clicked()
                                                        },
                                                    )
                                                    .inner;
                                                if clicked {
                                                    open_recent = Some(path.clone());
                                                }
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(path.display().to_string())
                                                            .size(10.5)
                                                            .color(theme::colors::TEXT_DIM),
                                                    )
                                                    .truncate(),
                                                );
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        if icons::close_button(
                                                            ui,
                                                            "从最近列表中移除",
                                                        ) {
                                                            forget_recent = Some(path.clone());
                                                        }
                                                    },
                                                );
                                            });
                                        }
                                    });
                            });

                            // ── 提示 / 错误 ──
                            if let Some(message) = &self.message {
                                ui.add_space(10.0);
                                // 错误不要只甩一行红字：给一条**带底的横幅**
                                // （警告图标 + 11% 红底 + 1px 红边），否则在
                                // 长路径里那行字很容易被当成说明文字读过去。
                                let width = ui.available_width();
                                // 先排版再占位：错误信息里带路径，窄窗口下会换行，
                                // 固定 26px 高会把第二行裁掉。
                                let galley = ui.painter().layout(
                                    message.clone(),
                                    egui::FontId::proportional(11.5),
                                    theme::colors::ERROR,
                                    (width - 34.0).max(40.0),
                                );
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(width, (galley.size().y + 12.0).max(26.0)),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect(
                                    rect,
                                    3.0,
                                    theme::colors::ERROR.gamma_multiply(0.12),
                                    egui::Stroke::new(
                                        1.0_f32,
                                        theme::colors::ERROR.gamma_multiply(0.45),
                                    ),
                                );
                                let icon_rect = egui::Rect::from_min_size(
                                    egui::pos2(rect.left() + 8.0, rect.center().y - 6.0),
                                    egui::vec2(12.0, 12.0),
                                );
                                icons::paint(
                                    ui.painter(),
                                    icon_rect,
                                    icons::Icon::Warning,
                                    theme::colors::ERROR,
                                );
                                ui.painter().galley(
                                    egui::pos2(
                                        rect.left() + 26.0,
                                        rect.center().y - galley.size().y * 0.5,
                                    ),
                                    galley,
                                    theme::colors::ERROR,
                                );
                            }
                        },
                    );
                });
            });

        // -- 回写可变状态 --
        self.new_name = new_name;
        self.new_location = new_location;
        self.open_path = open_path;

        // -- 原生目录选择器（阻塞式，放在 egui 帧结束之后调用）--
        if browse_new_location {
            if let Some(dir) = pick_folder("选择新项目的上级目录", &self.new_location) {
                self.new_location = dir.display().to_string();
            }
        }
        if browse_open_path {
            if let Some(dir) = pick_folder("选择项目根目录", &self.open_path) {
                self.open_path = dir.display().to_string();
            }
        }

        // -- 处理动作 --
        if let Some(path) = forget_recent {
            self.recents.remove(&path);
            self.refresh_recents();
        }

        if do_create {
            let location = self.new_location.clone();
            let name = self.new_name.clone();
            return self.create_project_at(PathBuf::from(location), &name);
        }

        if do_open {
            let path = PathBuf::from(self.open_path.trim());
            if path.as_os_str().is_empty() {
                self.message = Some("请先填写项目路径".into());
            } else {
                return self.open_project_at(path);
            }
        }

        if let Some(path) = open_recent {
            return self.open_project_at(path);
        }

        HubAction::None
    }

    // -- 动作实现 --

    fn open_project_at(&mut self, path: PathBuf) -> HubAction {
        match ProjectHandle::open(&path) {
            Ok(project) => {
                self.recents.add(project.root());
                self.refresh_recents();
                self.message = None;
                HubAction::Open(Box::new(project))
            }
            Err(e) => {
                self.message = Some(humanize(&e));
                HubAction::None
            }
        }
    }

    fn create_project_at(&mut self, parent: PathBuf, name: &str) -> HubAction {
        if name.trim().is_empty() {
            self.message = Some("项目名不能为空".into());
            return HubAction::None;
        }
        match ProjectHandle::create(&parent, name) {
            Ok(project) => {
                self.recents.add(project.root());
                self.refresh_recents();
                self.message = None;
                HubAction::Open(Box::new(project))
            }
            Err(e) => {
                self.message = Some(humanize(&e));
                HubAction::None
            }
        }
    }
}

impl Default for ProjectHub {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/// 打开原生「选择文件夹」对话框。用户取消时返回 `None`。
fn pick_folder(title: &str, start: &str) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(title);
    let start = PathBuf::from(start.trim());
    if start.is_dir() {
        dialog = dialog.set_directory(&start);
    }
    dialog.pick_folder()
}

/// 最近项目条目显示的名字：优先用目录名。
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// 默认的项目根目录。
///
/// 启发式：如果当前工作目录看起来像引擎源码根目录（存在 `crates/` 或 `Cargo.toml`），
/// 就用 `<cwd>/projects`；否则退回用户文档目录下的 `XGameEngine Projects`。
/// 这样 `cargo run` 的开发流程开箱即用，而独立分发的编辑器也不会往奇怪的地方写东西。
fn default_projects_dir() -> PathBuf {
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join("crates").is_dir() || cwd.join("Cargo.toml").is_file() {
            return cwd.join("projects");
        }
    }

    let base = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from));

    match base {
        Some(home) => home.join("Documents").join("XGameEngine Projects"),
        None => PathBuf::from("projects"),
    }
}

/// 把 `ProjectError` 转成给用户看的中文提示。
fn humanize(error: &ProjectError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无头跑 `frames` 帧，返回 (UI 构建耗时, tessellate 耗时, 每帧布局轮数)，单位毫秒。
    ///
    /// egui 的 `Context::run` 在内容触发「需要重排」时会把闭包再跑一遍，
    /// 所以数闭包调用次数就等于**布局 pass 数** —— 这个数字是判断界面
    /// 「为什么卡」最直接的指标：耗时不高的界面如果 pass 数 > 1，
    /// 说明布局每帧都没收敛。
    ///
    /// **tessellate 必须单独测**：`ctx.run` 只返回未镶嵌的 shapes，
    /// 把它们变成三角形是调用方（eframe）在之后做的，那才是渲染前 CPU 的大头。
    fn measure(hub: &mut ProjectHub, frames: usize) -> (f64, f64, f64) {
        let ctx = egui::Context::default();
        // 挂上中文字体：缺字会走字体回退，耗时和真实界面不一样。
        crate::panels::theme::install_fonts(&ctx);

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440.0, 900.0),
            )),
            ..Default::default()
        };

        // 预热：前几帧要建字体图集 / 分配 id，不参与计时。
        for _ in 0..3 {
            let _ = ctx.run(input.clone(), |ctx| {
                let _ = hub.ui(ctx);
            });
        }

        let mut passes = 0usize;
        let mut ui_ms = 0.0f64;
        let mut tess_ms = 0.0f64;
        for _ in 0..frames {
            let t0 = std::time::Instant::now();
            let output = ctx.run(input.clone(), |ctx| {
                passes += 1;
                let _ = hub.ui(ctx);
            });
            let t1 = std::time::Instant::now();

            let font_tex_size = ctx.fonts(|f| f.font_image_size());
            let mut tess = egui::epaint::Tessellator::new(
                1.0,
                egui::epaint::TessellationOptions::default(),
                font_tex_size,
                vec![],
            );
            let _ = tess.tessellate_shapes(output.shapes);
            let t2 = std::time::Instant::now();

            ui_ms += (t1 - t0).as_secs_f64() * 1000.0;
            tess_ms += (t2 - t1).as_secs_f64() * 1000.0;
        }

        (
            ui_ms / frames as f64,
            tess_ms / frames as f64,
            passes as f64 / frames as f64,
        )
    }

    /// 项目枢纽在**没有交互**时不能要求持续重绘。
    ///
    /// `wants_repaint() == true` 会让 eframe 改用 `ControlFlow::Poll` 一直出帧：
    /// 静态界面却在空转，CPU 白烧；一旦每帧还有点别的开销，立刻变成肉眼可见的卡顿。
    /// 这条断言专门盯住「项目选择页面极其卡顿」这个症状。
    /// 项目枢纽在**界面稳定之后**不该再请求重绘 —— 否则编辑器会一直空转出帧。
    ///
    /// 这里必须给 egui 一个**显式的时钟**。枢纽里有一个 `ScrollArea`，它的滚动条
    /// 做淡入淡出动画，动画期间请求重绘是正常的、短暂的。而旧写法用墙钟只跑 4 帧：
    /// 帧跑得快就"动画还没结束"、跑得慢就"已经结束"，于是这条测试时灵时不灵
    /// （实测：单独跑必挂，和几十个测试并行跑反而常过）。推进时间到动画结束再断言，
    /// 测的才是真正想要的性质 —— "稳定之后不空转"。
    #[test]
    fn hub_is_quiet_when_idle() {
        let mut hub = ProjectHub::new();
        let ctx = egui::Context::default();
        crate::panels::theme::install_fonts(&ctx);

        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440.0, 900.0),
            )),
            ..Default::default()
        };

        // 先推进足够长的时间，让滚动条淡入之类的动画彻底结束。
        for i in 0..20 {
            input.time = Some(i as f64 * 0.1);
            let _ = ctx.run(input.clone(), |ctx| {
                let _ = hub.ui(ctx);
            });
        }
        // 再看"已经稳定"的这一帧有没有请求重绘。
        input.time = Some(2.0);
        let _ = ctx.run(input.clone(), |ctx| {
            let _ = hub.ui(ctx);
        });

        assert!(
            !ctx.has_requested_repaint(),
            "项目枢纽稳定后仍在请求重绘 —— 编辑器会一直空转出帧"
        );
    }

    /// 项目枢纽是「启动后第一屏」，必须又快又稳。
    ///
    /// 这里不设硬性阈值（CI 机器差异大），但断言两件事：
    /// 布局必须一轮收敛（多轮说明界面里有「宽度依赖内容、内容又依赖宽度」的循环），
    /// 且 UI + tessellate 的合计必须远低于 16.7 ms（60 fps 的预算）。
    #[test]
    fn hub_layout_converges_in_one_pass() {
        let mut hub = ProjectHub::new();
        let (ui_ms, tess_ms, passes) = measure(&mut hub, 20);
        println!(
            "[hub bench] ui={ui_ms:.3} ms  tess={tess_ms:.3} ms  合计 {:.3} ms/frame, {passes:.2} passes",
            ui_ms + tess_ms
        );
        assert!(
            passes <= 1.01,
            "项目枢纽每帧布局了 {passes:.2} 轮 —— 应当一轮收敛"
        );
        assert!(
            ui_ms + tess_ms < 16.0,
            "项目枢纽每帧耗时 {:.2} ms，超出 60 fps 预算",
            ui_ms + tess_ms
        );
    }
}
