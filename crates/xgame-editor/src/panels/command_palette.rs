//! 命令面板（Ctrl+Shift+P / F1）——编辑器里所有可执行动作的模糊搜索入口。
//!
//! # 为什么单独做一块
//!
//! 功能越堆越多之后，"工具栏第几个按钮 / 哪个菜单里"成了找功能的主要成本。
//! 命令面板把**动作名**直接暴露给键盘：Ctrl+Shift+P 打开、打几个字、回车，
//! 与 VS Code 同一套肌肉记忆。它不发明新动作 —— 每条命令都是对既有
//! [`EditorState`] 方法 / 请求位 / 面板切换的薄封装，执行路径与点按钮
//! **逐字相同**（不在这里重写一份"保存场景"的逻辑）。
//!
//! # 模糊匹配
//!
//! 子序列匹配 + 计分：连续命中、词首命中加分，间隙扣分。匹配对象是
//! 「分类 · 标签」与关键词（英文别名），所以打 `save` 或「保存」都能命中。
//! 空查询 = 全量按定义顺序列出。
//!
//! # 键盘
//!
//! ↑↓ 换选中、Enter 执行、Esc 关闭。方向键与 Enter 在画查询框**之前**就被
//! `consume_key` 偷走 —— 否则聚焦的单行 TextEdit 会把它们吃掉（光标移动 /
//! 失焦），列表导航就失灵了。
//!
//! # 截图确定性
//!
//! `--ui-command` 截图模式打开面板时**不**给查询框焦点：聚焦的 TextEdit
//! 光标按墙钟闪烁，会让"同一份代码逐字节可复现"的截图验收失效。

use crate::panels::dock::{self, PanelTab};
use crate::panels::theme::{self, colors};
use crate::state::{EditorState, PlayMode};
use xgame_core::UiKind;
use xgame_render::mesh::BuiltinMesh;

/// 面板打开后可执行的动作。
///
/// 全部是对既有入口的转发：状态方法、请求位（由 `editor_ui` 后续步骤消费）、
/// 或布局树操作。**不承载任何业务逻辑**。
#[derive(Clone, Debug)]
pub enum PaletteAction {
    Save,
    SaveAs,
    Undo,
    Redo,
    Play,
    Pause,
    Stop,
    NewScene,
    OpenScene(String),
    SetStartupScene,
    ToggleGrid,
    ToggleOrtho,
    TogglePhysicsDebug,
    OpenSettings,
    OpenPreferences,
    BuildProject,
    BuildPlugins,
    RefreshAssets,
    ReloadScripts,
    CreateEntity,
    CreatePrimitive(BuiltinMesh),
    CreateUi(UiKind),
    Duplicate,
    Copy,
    Paste,
    Delete,
    FocusPanel(PanelTab),
    ResetLayout,
    ReturnToHub,
}

/// 一条命令：显示标签 + 可搜索关键词 + 快捷键提示 + 动作。
pub struct Command {
    /// 分类（列表里以暗色前缀显示，也参与匹配）。
    pub category: &'static str,
    /// 显示标签。
    pub label: String,
    /// 追加的搜索关键词（英文别名等；标签本身总是参与匹配）。
    pub keywords: String,
    /// 右侧暗色快捷键提示（没有就空串）。
    pub shortcut: &'static str,
    /// 执行的动作。
    pub action: PaletteAction,
    /// 只在编辑态可执行（Play / Pause 下置灰）。
    pub edit_only: bool,
}

/// 模糊匹配计分：`query` 的字符按序在 `text` 里出现才命中，返回分数（越大越前）。
///
/// 刻意做得很朴素：子序列 + 连续加分 + 词首加分 + 间隙扣分。不上完整
/// fuzzy-matcher 依赖 —— 命令表就几十条，线性扫一遍的开销测不出来，
/// 而匹配语义自己掌控才不会哪天升级依赖时悄悄变掉。
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    let t: Vec<char> = text.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut score = 0i32;
    let mut cursor = 0usize;
    let mut prev: Option<usize> = None;
    for &qc in &q {
        let found = t[cursor..].iter().position(|&c| c == qc)?;
        let pos = cursor + found;
        score += 10;
        // 连续命中（打 `sav` 命中 `save`）比散落命中好。
        if prev.is_some_and(|p| p + 1 == pos) {
            score += 8;
        }
        // 词首 / 串首命中（打 `sc` 命中 `Save Scene` 的两个词首）更好。
        if pos == 0 || matches!(t[pos - 1], ' ' | '·' | '(' | '/') {
            score += 6;
        }
        // 间隙越长越差（封顶，避免长标签被扣成负分后淹没短标签的散命中）。
        score -= (found as i32).min(6);
        prev = Some(pos);
        cursor = pos + 1;
    }
    Some(score)
}

/// 按查询过滤命令表，返回（分数, 下标）按分数降序、同分按定义顺序。
pub fn filter_commands(commands: &[Command], query: &str) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = commands
        .iter()
        .enumerate()
        .filter_map(|(i, cmd)| {
            let haystack = format!("{} · {} {}", cmd.category, cmd.label, cmd.keywords);
            let score = fuzzy_score(query, &haystack)?;
            Some((score, i))
        })
        .collect();
    if query.is_empty() {
        // 空查询 = 定义顺序（分数全 0，稳定排序下标即定义顺序）。
        scored.sort_by_key(|&(_, i)| i);
    } else {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    scored.into_iter().map(|(_, i)| i).collect()
}

/// 命令面板的界面状态（打开位 / 查询串 / 选中行）。
pub struct CommandPalette {
    open: bool,
    query: String,
    selected: usize,
    /// 打开后第一帧把焦点放进查询框（`request_focus` 只在那一帧发一次）。
    want_focus: bool,
    /// 打开时冻结的命令表：不逐帧扫磁盘（`scene_list` 是目录扫描）。
    commands: Vec<Command>,
}

impl CommandPalette {
    pub fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            want_focus: false,
            commands: Vec::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 收起面板（toggle / Esc 之外的程序化关闭）。
    pub fn close(&mut self) {
        self.open = false;
    }

    /// 打开面板：构建命令表、清空查询、选中第一行。
    ///
    /// `focus_query` 为 `false` 时查询框不拿焦点 —— `--ui-command` 截图
    /// 模式专用（聚焦 TextEdit 的光标闪烁会破坏逐字节可复现）。
    pub fn open(&mut self, state: &EditorState, focus_query: bool) {
        self.commands = build_commands(state);
        self.query.clear();
        self.selected = 0;
        self.want_focus = focus_query;
        self.open = true;
    }

    /// 画面板；命令在闭包内当场执行（执行路径与工具栏 / 菜单完全一致）。
    pub fn show(
        &mut self,
        state: &mut EditorState,
        tree: &mut egui_tiles::Tree<PanelTab>,
        ctx: &egui::Context,
    ) {
        if !self.open {
            return;
        }

        // 导航键在画查询框之前偷走：聚焦的单行 TextEdit 会吃掉方向键（移光标）
        // 与 Enter（失焦），列表导航就全废了。Esc 也在这里收掉（模态语义）。
        let (up, down, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if esc {
            self.open = false;
            return;
        }

        let mut open = self.open;
        let mut run: Option<PaletteAction> = None;

        egui::Window::new("命令面板")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .show(ctx, |ui| {
                // ── 查询框 ──
                let before = self.query.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("输入命令……（↑↓ 选择 · Enter 执行 · Esc 关闭）")
                        .desired_width(f32::INFINITY),
                );
                if self.want_focus {
                    resp.request_focus();
                    self.want_focus = false;
                }
                if self.query != before {
                    self.selected = 0;
                }

                ui.add_space(4.0);

                // ── 过滤 + 导航 ──
                let hits = filter_commands(&self.commands, &self.query);
                if hits.is_empty() {
                    ui.label(theme::dim_note("无匹配命令"));
                } else {
                    if up {
                        self.selected = self.selected.saturating_sub(1);
                    }
                    if down {
                        self.selected = (self.selected + 1).min(hits.len() - 1);
                    }
                    if enter {
                        if let Some(&idx) = hits.get(self.selected.min(hits.len() - 1)) {
                            if command_enabled(state, &self.commands[idx]) {
                                run = Some(self.commands[idx].action.clone());
                            }
                        }
                    }

                    // 列表高度按行数收（上限后滚动），保证 1280 窗口下不顶到状态栏。
                    let rows = hits.len().min(12) as f32;
                    egui::ScrollArea::vertical()
                        .max_height(rows * theme::ROW_HEIGHT + 4.0)
                        .show(ui, |ui| {
                            for (row, &idx) in hits.iter().enumerate() {
                                let cmd = &self.commands[idx];
                                let selected = row == self.selected;
                                let enabled = command_enabled(state, cmd);
                                let resp = draw_row(ui, cmd, selected, enabled);
                                if selected && (up || down) {
                                    resp.scroll_to_me(Some(egui::Align::Center));
                                }
                                if enabled && resp.clicked() {
                                    run = Some(cmd.action.clone());
                                }
                            }
                        });
                }

                ui.add_space(2.0);
                ui.label(theme::dim_note("↑↓ 选择 · Enter 执行 · Esc 关闭"));
            });

        self.open = open;
        if let Some(action) = run {
            self.open = false;
            execute(action, state, tree);
        }
    }
}

/// 画一行命令（自绘矩形 + 文字，不走 Button / SelectableLabel —— 行高与
/// 配色必须钉死在 `theme` 常量上，控件自带 min_size 会溢出）。
fn draw_row(
    ui: &mut egui::Ui,
    cmd: &Command,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(width, theme::ROW_HEIGHT),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 0.0, colors::SELECTION_BG);
    } else if resp.hovered() {
        painter.rect_filled(rect, 0.0, colors::ROW_HOVER_BG);
    }

    let dim = if enabled {
        colors::TEXT_DIM
    } else {
        colors::TEXT_DIM.gamma_multiply(0.6)
    };
    let label_color = if !enabled {
        colors::TEXT_SECONDARY.gamma_multiply(0.6)
    } else if selected {
        colors::TEXT_BRIGHT
    } else {
        colors::TEXT_PRIMARY
    };
    let font = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::TextStyle::Small.resolve(ui.style());
    let left = rect.left_center();
    painter.text(
        left + egui::vec2(theme::PANEL_MARGIN, 0.0),
        egui::Align2::LEFT_CENTER,
        cmd.category,
        small.clone(),
        dim,
    );
    painter.text(
        left + egui::vec2(theme::LABEL_WIDTH, 0.0),
        egui::Align2::LEFT_CENTER,
        &cmd.label,
        font,
        label_color,
    );
    if !cmd.shortcut.is_empty() {
        painter.text(
            rect.right_center() - egui::vec2(theme::PANEL_MARGIN, 0.0),
            egui::Align2::RIGHT_CENTER,
            cmd.shortcut,
            small,
            dim,
        );
    }
    resp
}

/// 该命令此刻能不能执行（灰显依据；与执行时的硬保护独立双保险）。
fn command_enabled(state: &EditorState, cmd: &Command) -> bool {
    if cmd.edit_only && !state.is_editing() {
        return false;
    }
    match cmd.action {
        PaletteAction::Play => state.play_mode != PlayMode::Playing,
        PaletteAction::Pause => state.play_mode == PlayMode::Playing,
        PaletteAction::Stop => state.play_mode != PlayMode::Edit,
        _ => true,
    }
}

/// 执行一条命令。全部是既有入口的转发，**不重写业务逻辑**。
fn execute(action: PaletteAction, state: &mut EditorState, tree: &mut egui_tiles::Tree<PanelTab>) {
    match action {
        PaletteAction::Save => {
            let _ = state.save_scene();
        }
        PaletteAction::SaveAs => {
            state.save_as_open = true;
            state.save_as_name = state.scene.name.clone();
        }
        PaletteAction::Undo => state.undo(),
        PaletteAction::Redo => state.redo(),
        // 进 Play 与 `--ui-play` 同一条路径：`enter_play_mode` 不推 play_mode
        // （那由工具栏的跃迁监听做），这里手动推 —— 面板在跃迁监听**之后**画，
        // 不推就得到「运行时在跑、界面写着 Edit」的僵尸态。
        PaletteAction::Play => match state.play_mode {
            PlayMode::Edit => {
                state.enter_play_mode();
                if state.game_runtime.is_some() {
                    state.play_mode = PlayMode::Playing;
                }
            }
            PlayMode::Paused => state.play_mode = PlayMode::Playing,
            PlayMode::Playing => {}
        },
        PaletteAction::Pause => {
            if state.play_mode == PlayMode::Playing {
                state.play_mode = PlayMode::Paused;
                state.log_info("⏸ Pause — 物理与脚本暂停，场景保持当前状态");
            }
        }
        PaletteAction::Stop => {
            if state.play_mode != PlayMode::Edit {
                state.play_mode = PlayMode::Edit;
                state.exit_play_mode();
            }
        }
        PaletteAction::NewScene => state.request_new_scene(),
        PaletteAction::OpenScene(relative) => state.request_scene_open(relative),
        PaletteAction::SetStartupScene => {
            let current = state.current_scene.clone();
            let _ = state.set_startup_scene(&current);
        }
        PaletteAction::ToggleGrid => state.show_grid = !state.show_grid,
        PaletteAction::ToggleOrtho => {
            let on = state.editor_camera.toggle_ortho();
            state.log_info(if on { "投影：正交" } else { "投影：透视" });
        }
        PaletteAction::TogglePhysicsDebug => {
            state.show_physics_debug = !state.show_physics_debug
        }
        // 窗口类走请求位：由 `editor_ui` 既有步骤消费（与工具栏点齿轮逐字一致）。
        PaletteAction::OpenSettings => state.settings_open = true,
        PaletteAction::OpenPreferences => state.open_preferences = true,
        PaletteAction::BuildProject => state.build_project(),
        PaletteAction::BuildPlugins => state.build_plugins(),
        PaletteAction::RefreshAssets => state.refresh_assets(),
        PaletteAction::ReloadScripts => state.reload_scripts(),
        PaletteAction::CreateEntity => {
            state.create_entity();
        }
        PaletteAction::CreatePrimitive(kind) => {
            state.create_primitive(kind);
        }
        PaletteAction::CreateUi(kind) => {
            state.create_ui(kind);
        }
        PaletteAction::Duplicate => state.duplicate_entity(),
        PaletteAction::Copy => state.copy_selected(),
        PaletteAction::Paste => state.paste_entity(),
        PaletteAction::Delete => state.delete_selected(),
        PaletteAction::FocusPanel(tab) => dock::activate(tree, tab),
        PaletteAction::ResetLayout => {
            *tree = dock::create_tile_tree();
            state.log_info("已恢复默认面板布局");
        }
        PaletteAction::ReturnToHub => state.return_to_hub = true,
    }
}

/// 构建命令表（打开面板时一次性建好；场景相关命令按下当时的磁盘清单）。
fn build_commands(state: &EditorState) -> Vec<Command> {
    fn cmd(
        category: &'static str,
        label: &str,
        keywords: &str,
        shortcut: &'static str,
        action: PaletteAction,
        edit_only: bool,
    ) -> Command {
        Command {
            category,
            label: label.to_owned(),
            keywords: keywords.to_owned(),
            shortcut,
            action,
            edit_only,
        }
    }

    let mut out = vec![
        cmd("文件", "保存场景", "save", "Ctrl+S", PaletteAction::Save, true),
        cmd(
            "文件",
            "场景另存为",
            "save as",
            "Ctrl+Shift+S",
            PaletteAction::SaveAs,
            true,
        ),
        cmd("文件", "新建场景", "new scene", "", PaletteAction::NewScene, true),
        cmd("文件", "回到项目枢纽", "hub close project", "", PaletteAction::ReturnToHub, false),
        cmd("编辑", "撤销", "undo", "Ctrl+Z", PaletteAction::Undo, true),
        cmd("编辑", "重做", "redo", "Ctrl+Shift+Z", PaletteAction::Redo, true),
        cmd("编辑", "复制实体", "duplicate", "Ctrl+D", PaletteAction::Duplicate, true),
        cmd("编辑", "复制", "copy", "Ctrl+C", PaletteAction::Copy, true),
        cmd("编辑", "粘贴", "paste", "Ctrl+V", PaletteAction::Paste, true),
        cmd("编辑", "删除选中", "delete", "Del", PaletteAction::Delete, true),
        cmd("播放", "播放", "play run", "", PaletteAction::Play, false),
        cmd("播放", "暂停", "pause", "", PaletteAction::Pause, false),
        cmd("播放", "停止", "stop", "", PaletteAction::Stop, false),
        cmd(
            "场景",
            "设为启动场景",
            "startup entry",
            "",
            PaletteAction::SetStartupScene,
            true,
        ),
        cmd(
            "创建",
            "创建空实体",
            "empty entity",
            "",
            PaletteAction::CreateEntity,
            true,
        ),
        cmd(
            "创建",
            "创建立方体",
            "cube",
            "",
            PaletteAction::CreatePrimitive(BuiltinMesh::Cube),
            true,
        ),
        cmd(
            "创建",
            "创建球体",
            "sphere",
            "",
            PaletteAction::CreatePrimitive(BuiltinMesh::Sphere),
            true,
        ),
        cmd(
            "创建",
            "创建圆柱",
            "cylinder",
            "",
            PaletteAction::CreatePrimitive(BuiltinMesh::Cylinder),
            true,
        ),
        cmd(
            "创建",
            "创建平面",
            "plane",
            "",
            PaletteAction::CreatePrimitive(BuiltinMesh::Plane),
            true,
        ),
        cmd(
            "创建",
            "创建 UI 面板",
            "ui panel",
            "",
            PaletteAction::CreateUi(UiKind::Panel),
            true,
        ),
        cmd(
            "创建",
            "创建 UI 文本",
            "ui text",
            "",
            PaletteAction::CreateUi(UiKind::Text),
            true,
        ),
        cmd(
            "创建",
            "创建 UI 按钮",
            "ui button",
            "",
            PaletteAction::CreateUi(UiKind::Button),
            true,
        ),
        cmd("视图", "切换网格", "grid", "", PaletteAction::ToggleGrid, false),
        // `edit_only = true`：正交只影响编辑态视口（Play 态用游戏相机），
        // Play 里点它什么都不会变 —— 那就干脆不让点。
        cmd(
            "视图",
            "切换正交投影",
            "ortho projection",
            "5",
            PaletteAction::ToggleOrtho,
            true,
        ),
        cmd(
            "视图",
            "切换物理调试绘制",
            "physics debug",
            "",
            PaletteAction::TogglePhysicsDebug,
            false,
        ),
        cmd("视图", "转到：层级", "hierarchy", "", PaletteAction::FocusPanel(PanelTab::Hierarchy), false),
        cmd("视图", "转到：资源", "assets", "", PaletteAction::FocusPanel(PanelTab::Assets), false),
        cmd("视图", "转到：预制体", "prefabs", "", PaletteAction::FocusPanel(PanelTab::Prefabs), false),
        cmd("视图", "转到：视口", "viewport scene", "", PaletteAction::FocusPanel(PanelTab::SceneView), false),
        cmd("视图", "转到：检视器", "inspector", "", PaletteAction::FocusPanel(PanelTab::Inspector), false),
        cmd("视图", "转到：控制台", "console log", "", PaletteAction::FocusPanel(PanelTab::Console), false),
        cmd("视图", "转到：性能", "profiler", "", PaletteAction::FocusPanel(PanelTab::Profiler), false),
        cmd("视图", "恢复默认布局", "reset layout", "", PaletteAction::ResetLayout, true),
        cmd("构建", "构建项目", "build", "", PaletteAction::BuildProject, true),
        cmd("构建", "构建插件", "build plugins", "", PaletteAction::BuildPlugins, true),
        cmd("工具", "刷新资产", "refresh assets", "", PaletteAction::RefreshAssets, false),
        cmd("工具", "重载脚本", "reload scripts", "", PaletteAction::ReloadScripts, false),
        cmd("窗口", "打开项目设置", "project settings", "", PaletteAction::OpenSettings, false),
        cmd("窗口", "打开偏好设置", "preferences", "", PaletteAction::OpenPreferences, false),
    ];
    // 打开场景是磁盘清单（`scene_list` 做一次目录扫描）：只在打开面板时取，
    // 不逐帧重建。当前场景也在列表里 —— 切回去是幂等的。
    for relative in state.scene_list() {
        let name = relative.rsplit('/').next().unwrap_or(&relative).to_owned();
        out.push(cmd(
            "场景",
            &format!("打开场景：{name}"),
            &format!("open scene {relative}"),
            "",
            PaletteAction::OpenScene(relative),
            true,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(category: &'static str, label: &str, keywords: &str) -> Command {
        Command {
            category,
            label: label.to_owned(),
            keywords: keywords.to_owned(),
            shortcut: "",
            action: PaletteAction::ToggleGrid,
            edit_only: false,
        }
    }

    /// 连续命中排在散命中之前；词首命中再加一档。
    #[test]
    fn fuzzy_prefers_consecutive_and_word_head() {
        // 连续命中（`save` 几乎打满）比同字符散开强。
        let exact = fuzzy_score("save", "save scene").unwrap();
        let scattered = fuzzy_score("save", "szzazzvzze").unwrap();
        assert!(exact > scattered, "连续命中 {exact} 应高于散命中 {scattered}");

        // 词首命中（`a` 在 apple 首）比词中命中（`a` 在 banana 中）强。
        let head = fuzzy_score("a", "apple").unwrap();
        let mid = fuzzy_score("a", "banana").unwrap();
        assert!(head > mid, "词首命中 {head} 应高于词中命中 {mid}");
    }

    /// 大小写不敏感；查不到返回 None。
    #[test]
    fn fuzzy_case_insensitive_and_none_on_miss() {
        assert_eq!(fuzzy_score("SAVE", "save"), fuzzy_score("save", "save"));
        assert!(fuzzy_score("xyz", "save").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0), "空查询全命中");
    }

    /// 空查询保持定义顺序；有查询按分数降序、同分按下标。
    #[test]
    fn filter_keeps_definition_order_for_empty_query() {
        let cmds = vec![
            command("文件", "保存场景", "save"),
            command("播放", "播放", "play"),
            command("视图", "切换网格", "grid"),
        ];
        let hits = filter_commands(&cmds, "");
        assert_eq!(hits, vec![0, 1, 2]);

        let hits = filter_commands(&cmds, "play");
        assert_eq!(hits, vec![1], "只留播放");
    }

    /// 中文标签按子序列命中（打「保存」要能命中「保存场景」）。
    #[test]
    fn chinese_label_matches_by_subsequence() {
        let cmds = vec![
            command("文件", "保存场景", "save"),
            command("播放", "暂停", "pause"),
        ];
        let hits = filter_commands(&cmds, "保存");
        assert_eq!(hits, vec![0]);
        let hits = filter_commands(&cmds, "暂停");
        assert_eq!(hits, vec![1]);
    }
}
