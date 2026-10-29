//! 「⚙ 项目设置」窗口——编辑 `project.ron` 里玩家可调的字段。
//!
//! # 编辑走草稿
//!
//! 打开窗口时把清单克隆成 `settings_draft`，界面上改的全是草稿；按
//! **Apply** 才校验 + 写回内存清单 + 落盘（[`EditorState::apply_settings_draft`]），
//! 关窗 / Cancel 直接丢弃。与 Undo 同理：半成品设置不该边敲键盘边生效——
//! 比如把重力改成 0 的过程中手一抖输成 0.0001，还没敲完就应用毫无意义。
//!
//! # 这个面板为什么值钱
//!
//! 重力 / 固定步长此前硬编码在 `WorldConfig::default()` 里，想调只能改引擎
//! 源码重编译。现在它们是**数据**：改完 Apply，编辑器 Play 与独立游戏二进制
//! 下次启动就用新值（两端走同一个 `WorldConfig::from_project`）。
//! 数据化的另一层意义是给 **AI 智能体**用：能被程序读写校验的配置，
//! 才能被智能体批量调参与自动验证，而不是"打开 GUI 拖滑块"。

use crate::panels::{icons, theme};
use crate::state::EditorState;

/// 关卡顺序列表里的行内动作（收集完再改 Vec，避免边迭代边改）。
#[derive(Clone, Copy)]
enum RowAction {
    Up,
    Down,
    Remove,
}

/// 设置窗口里的单行文本字段：**撑满网格剩下的宽度**。
///
/// 不这么做的话每个字段只有 egui 的默认宽度（~220px），右边缘离窗口边框
/// 还差一大截：一列输入框右边缘不齐，窗口右边又空出一块。网格单元格的
/// `available_width()` 就是"本行到网格右边界"的剩余量，拿它定宽即可。
fn text_field(ui: &mut egui::Ui, value: &mut String) {
    let width = ui.available_width().max(160.0);
    ui.add_sized(
        egui::vec2(width, theme::FIELD_HEIGHT + 2.0),
        egui::TextEdit::singleline(value),
    );
}

/// 矩阵格子边长（含 2px 间隙）。
///
/// 32 列 × 12px（含 1px 间隙）≈ 384px：配合左侧行标刚好塞进加宽后的设置窗口。
/// 再大就得横向滚动，"一眼看完整张矩阵"是这块 UI 的全部意义。
const LAYER_CELL: f32 = 11.0;

/// 层名字网格的行间距（同时也是 [`layer_names`] 算可视高度的依据）。
const NAMES_ROW_GAP: f32 = 3.0;

/// 矩阵左侧行标列宽（两位序号 + 一点点右留白）。
const LAYER_GUTTER: f32 = 13.0;

/// 矩阵上方表头（列号）高度。
const LAYER_HEAD: f32 = 12.0;

/// 矩阵格子之间的间隙。
const LAYER_GAP: f32 = 1.0;

/// 页脚（`Apply` / `Reset` / `Cancel` 那一行）要从滚动区的可用高度里扣掉的量：
/// 按钮高 `FIELD_HEIGHT + 6` + 上方 `add_space(10)` + 一点余量。
const FOOTER_RESERVE: f32 = 52.0;

/// 标题栏要从窗口高度里扣掉的量（egui 的标题栏 + 一点内边距）。
const TITLE_RESERVE: f32 = 34.0;

/// 设置窗口宽度。矩阵按 32 × 11px + 1px 间隙（≈ 383px）+ 行标 13px 设计，
/// 470 正好塞得下；变宽会让其他区块的输入框跟着拉长，整页比例全变。
const SETTINGS_WIDTH: f32 = 470.0;

/// 层名字编辑区：32 行，`序号 + 名字`。
///
/// 名字留空 = 退回 `Layer N` 显示（`Layers::set_name` 会删掉条目），
/// 所以"清空"是恢复默认，而不是造出一个空名字的层。
fn layer_names(ui: &mut egui::Ui, layers: &mut xgame_core::Layers) -> bool {
    let mut changed = false;
    egui::ScrollArea::vertical()
        .id_salt("layer_names")
        // 32 行全铺开会把下面的矩阵顶出窗口；这里只给 5 行的高度。
        // 高度要按"实际行高"算：网格行高 = 控件高 `FIELD_HEIGHT` + 行间距 3，
        // 直接用 `ROW_HEIGHT * 5`（=110）会小于真实 5 行（≈115），
        // 第 5 行被裁掉一半 —— 看起来像渲染错位。
        .max_height((theme::FIELD_HEIGHT + NAMES_ROW_GAP) * 5.0 + 2.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("layer_names_grid")
                .num_columns(2)
                .spacing([8.0, NAMES_ROW_GAP])
                .show(ui, |ui| {
                    for layer in 0..xgame_core::LAYER_COUNT as u8 {
                        ui.label(
                            egui::RichText::new(format!("{layer:>2}"))
                                .monospace()
                                .size(10.0)
                                .color(theme::colors::TEXT_SECONDARY),
                        );
                        let mut name = layers.name(layer).to_string();
                        let resp = ui.add_sized(
                            egui::vec2(150.0, theme::FIELD_HEIGHT),
                            egui::TextEdit::singleline(&mut name)
                                .hint_text(format!("Layer {layer}")),
                        );
                        if resp.changed() {
                            layers.set_name(layer, name);
                            changed = true;
                        }
                        ui.end_row();
                    }
                });
        });
    changed
}

/// 32×32 碰撞矩阵。
///
/// 语义是"勾上 = 两层碰撞"，**默认全勾**（与 `Layers` 的默认一致）。
/// 点一格会同时改两侧（`set_collides` 写两份掩码），所以矩阵永远对称 ——
/// 手改 RON 写出不对称的一对时，这里显示的是**实际生效**的值
///（`collides` 与 rapier 一样要求两侧都同意）。
///
/// 布局是**手工画在一张画布上**，不用 `Grid`（实测数据都写在下面）：
/// - `Grid` 的行高由格子里的子控件决定。塞 `ui.label` 时行距 25px、
///   塞定尺 `allocate_exact_size` 时 21px —— 都不是 `LAYER_CELL + 1`。
///   32 行就是 670~800px，矩阵顶出窗口，页脚的 Apply 直接被挤出屏幕
///   （只看代码绝对看不出来，必须量截图）。
/// - 1024 个格子做成 1024 个 widget 也是白拿 1024 次交互区计算。
///
/// 现在只有 1 个交互矩形 + 1 次命中测试，几何完全可控：整体
/// `LAYER_GUTTER + 32 × 12` 宽、`LAYER_HEAD + 32 × 12` 高，一格不多。
fn layer_matrix(ui: &mut egui::Ui, layers: &mut xgame_core::Layers) -> bool {
    let mut changed = false;
    let count = xgame_core::LAYER_COUNT;
    let pitch = LAYER_CELL + LAYER_GAP;
    // 最后一列/行后面不留间隙，否则右下角多出一块空白。
    let size = egui::vec2(
        LAYER_GUTTER + pitch * count as f32 - LAYER_GAP,
        LAYER_HEAD + pitch * count as f32 - LAYER_GAP,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter_at(rect);
    // 原点 = 第 0 行第 0 列格子的左上角（左上角留给表头与行标）。
    let origin = rect.min + egui::vec2(LAYER_GUTTER, LAYER_HEAD);
    let cell_rect = |row: usize, col: usize| {
        egui::Rect::from_min_size(
            origin + egui::vec2(col as f32 * pitch, row as f32 * pitch),
            egui::vec2(LAYER_CELL, LAYER_CELL),
        )
    };

    // 表头：每 4 列标一个号（11px 宽塞不下两位数字），完整的一对在悬浮提示里。
    for col in 0..count {
        if col % 4 != 0 {
            continue;
        }
        let center = origin + egui::vec2(col as f32 * pitch + LAYER_CELL * 0.5, -LAYER_HEAD * 0.5);
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            format!("{col}"),
            egui::FontId::monospace(8.0),
            theme::colors::TEXT_SECONDARY,
        );
    }

    // 行标：11px 的行高塞不下"层名 + 序号"，名字只在悬浮提示里给。
    for row in 0..count {
        let center = origin + egui::vec2(-3.0, row as f32 * pitch + LAYER_CELL * 0.5);
        painter.text(
            center,
            egui::Align2::RIGHT_CENTER,
            format!("{row:>2}"),
            egui::FontId::monospace(8.0),
            theme::colors::TEXT_SECONDARY,
        );
    }

    // 指针落在哪个格子：间隙也算最近的格子（1px 的缝不该"点空"）。
    let hovered = response.hover_pos().and_then(|pos| {
        let rel = pos - origin;
        if rel.x < 0.0 || rel.y < 0.0 {
            return None;
        }
        let col = (rel.x / pitch) as usize;
        let row = (rel.y / pitch) as usize;
        (row < count && col < count).then_some((row, col))
    });

    for row in 0..count {
        for col in 0..count {
            let r = cell_rect(row, col);
            let on = layers.collides(row as u8, col as u8);
            if on {
                painter.rect_filled(r, 1.5, theme::colors::ACCENT_BLUE);
            } else {
                painter.rect_filled(r, 1.5, theme::colors::INPUT_BG);
                painter.rect_stroke(
                    r,
                    1.5,
                    egui::Stroke::new(1.0_f32, theme::colors::INPUT_BORDER),
                );
            }
        }
    }

    // 悬浮高亮：白框标出当前格，再给一条带层名的提示 ——
    // 矩阵这么密，没有提示根本不知道点的是哪一对。
    let response = if let Some((row, col)) = hovered {
        let r = cell_rect(row, col);
        painter.rect_stroke(
            r.expand(0.5),
            1.5,
            egui::Stroke::new(1.0_f32, theme::colors::TEXT_BRIGHT),
        );
        let on = layers.collides(row as u8, col as u8);
        response
            .on_hover_text(format!(
                "{} × {}：{}",
                layers.label(row as u8),
                layers.label(col as u8),
                if on {
                    "碰撞（点一下关掉）"
                } else {
                    "不碰撞（点一下打开）"
                }
            ))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };

    if response.clicked() {
        if let Some((row, col)) = hovered {
            layers.set_collides(row as u8, col as u8, !layers.collides(row as u8, col as u8));
            changed = true;
        }
    }
    changed
}

pub fn show(state: &mut EditorState, ctx: &egui::Context) {
    if !state.settings_open {
        return;
    }

    // 第一次显示（或上次被 Cancel/关闭丢弃）时克隆清单开草稿。
    if state.settings_draft.is_none() {
        state.settings_draft = Some(state.project.manifest().clone());
    }

    // 发布清单的"可添加项"= 盘上所有场景。只在窗口开着时扫一次目录
    // （文件数是个位数），换来"刚在 Scenes 菜单里新建的场景立刻可选"。
    let all_scenes = state.project.list_scenes();

    let mut open = state.settings_open;
    let mut apply = false;
    let mut reset = false;
    let mut cancel = false;

    // 内容比一屏高：**必须给滚动区一个上限**，否则窗口会按内容高度长到屏幕外。
    // 上限在闭包里算 —— 它要用 `ui.available_height()`（见那里的注释）。

    // 窗口高度按屏幕定死（内容恒定地比一屏高，自适应只会让高度随内容抖动）。
    // 上限 920：再高也只是让滚动区更长，反而在 1080 的屏上顶到边缘。
    let window_h = (ctx.screen_rect().height() - 60.0).clamp(360.0, 920.0);

    egui::Window::new("Project Settings")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(SETTINGS_WIDTH, window_h))
        .default_width(SETTINGS_WIDTH)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            let Some(draft) = state.settings_draft.as_mut() else {
                return;
            };

            // **页脚必须先从可用高度里扣掉**：窗口自己能长多高不是
            // `screen_rect()` 说了算的（实测 1400 高的屏上，窗口内部只给到
            // 1005 —— 上下还留着锚点边距）。按屏幕算 `body_max` 会让滚动区
            // 吃掉全部可用高度，页脚的 Apply / Reset / Cancel 被挤出窗口外，
            // 用户连"确认"都点不到。所以基准一律用 `ui.available_height()`。
            // 滚动区高度 = 窗口总高 - 标题栏 - 页脚。三者都是**常量**，
            // 不依赖窗口自身的高度（依赖它就会形成反馈环：窗口小时算出的上限
            // 更小 → 窗口更小，实测能让整个面板缩成 400px 高）。
            let body_max = (window_h - TITLE_RESERVE - FOOTER_RESERVE).max(200.0);

            egui::ScrollArea::vertical()
                .max_height(body_max)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    // ── 项目 ──
                    theme::section_bar(ui, "项目");
                    egui::Grid::new("set_proj")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .min_col_width(88.0)
                        .show(ui, |ui| {
                            ui.label("项目名");
                            text_field(ui, &mut draft.name);
                            ui.end_row();

                            ui.label("入口场景")
                                .on_hover_text("打开项目时加载的场景（项目相对路径）");
                            text_field(ui, &mut draft.entry_scene);
                            ui.end_row();
                        });

                    ui.add_space(10.0);

                    // ── 构建 ──
                    //
                    // 顺序即关卡索引：脚本 `scene_index()` / `load_scene_at(i)` 读的就是
                    // 这份清单。用列表而不是一段逗号分隔的文本框，是因为"顺序"本身
                    // 是这份数据的语义 —— 让用户手打顺序等于把 `index + 1` 的 bug
                    // 挪到配置里。
                    theme::section_bar(ui, "构建（版本 / 场景顺序）");
                    egui::Grid::new("set_build")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .min_col_width(88.0)
                        .show(ui, |ui| {
                            ui.label("版本号").on_hover_text(
                                "发布版本，只用于展示（独立游戏启动日志 / 构建报告）",
                            );
                            text_field(ui, &mut draft.build.version);
                            ui.end_row();
                        });

                    ui.add_space(4.0);
                    ui.label("关卡顺序").on_hover_text(
                        "顺序即关卡索引：脚本 scene_index() 取当前关、load_scene_at(i) 切关",
                    );
                    if draft.build.scenes.is_empty() {
                        ui.label(
                            egui::RichText::new(
                                "未配置：运行时不提供关卡顺序，脚本只能按路径 load_scene()。",
                            )
                            .color(theme::colors::TEXT_DIM),
                        );
                    }

                    // 先收集动作再改列表：`iter()` 期间不能 `&mut` 同一个 Vec。
                    let mut action: Option<(usize, RowAction)> = None;
                    for (index, path) in draft.build.scenes.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let up = ui
                                .add_enabled(index > 0, egui::Button::new("上移"))
                                .on_hover_text("往前一关（索引 -1）");
                            if up.clicked() {
                                action = Some((index, RowAction::Up));
                            }
                            let down = ui
                                .add_enabled(
                                    index + 1 < draft.build.scenes.len(),
                                    egui::Button::new("下移"),
                                )
                                .on_hover_text("往后一关（索引 +1）");
                            if down.clicked() {
                                action = Some((index, RowAction::Down));
                            }
                            ui.label(format!("{index}. {path}"));
                            if icons::close_button(ui, "从发布清单里移除（不删文件）")
                            {
                                action = Some((index, RowAction::Remove));
                            }
                        });
                    }
                    match action {
                        Some((index, RowAction::Up)) => draft.build.scenes.swap(index, index - 1),
                        Some((index, RowAction::Down)) => draft.build.scenes.swap(index, index + 1),
                        Some((index, RowAction::Remove)) => {
                            draft.build.scenes.remove(index);
                        }
                        None => {}
                    }

                    // 添加：只列磁盘上还没进清单的场景（重复项在 Apply 时还会再挡一次）。
                    let available: Vec<String> = all_scenes
                        .iter()
                        .filter(|scene| !draft.build.scenes.iter().any(|listed| listed == *scene))
                        .cloned()
                        .collect();
                    if available.is_empty() {
                        ui.label(
                            egui::RichText::new("（所有场景都已列入）")
                                .color(theme::colors::TEXT_DIM),
                        );
                    } else {
                        egui::ComboBox::from_id_salt("set_build_add")
                            .selected_text("添加场景…")
                            .show_ui(ui, |ui| {
                                for scene in &available {
                                    if ui.selectable_label(false, scene).clicked() {
                                        draft.build.scenes.push(scene.clone());
                                    }
                                }
                            });
                    }

                    ui.add_space(10.0);

                    // ── 窗口 ──
                    theme::section_bar(ui, "窗口");
                    egui::Grid::new("set_win")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .min_col_width(88.0)
                        .show(ui, |ui| {
                            ui.label("标题");
                            text_field(ui, &mut draft.window.title);
                            ui.end_row();

                            ui.label("尺寸");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::DragValue::new(&mut draft.window.width)
                                        .range(320..=16_384)
                                        .suffix(" px"),
                                );
                                ui.label("×");
                                ui.add(
                                    egui::DragValue::new(&mut draft.window.height)
                                        .range(240..=16_384)
                                        .suffix(" px"),
                                );
                            });
                            ui.end_row();

                            ui.label("可调整大小");
                            ui.checkbox(&mut draft.window.resizable, "");
                            ui.end_row();

                            ui.label("垂直同步")
                                .on_hover_text("关闭后帧率不封顶、输入延迟更低（编辑器默认关）");
                            ui.checkbox(&mut draft.window.vsync, "");
                            ui.end_row();
                        });

                    ui.add_space(10.0);

                    // ── 运行时 ──
                    theme::section_bar(ui, "运行时（物理 / 时间）");
                    egui::Grid::new("set_rt")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .min_col_width(88.0)
                        .show(ui, |ui| {
                            ui.label("重力")
                                .on_hover_text("世界重力加速度（m/s²），Y 轴向下为负");
                            ui.horizontal(|ui| {
                                ui.label("X");
                                ui.add(
                                    egui::DragValue::new(&mut draft.runtime.gravity[0]).speed(0.1),
                                );
                                ui.label("Y");
                                ui.add(
                                    egui::DragValue::new(&mut draft.runtime.gravity[1]).speed(0.1),
                                );
                                ui.label("Z");
                                ui.add(
                                    egui::DragValue::new(&mut draft.runtime.gravity[2]).speed(0.1),
                                );
                            });
                            ui.end_row();

                            ui.label("固定步长").on_hover_text(
                                "物理 / 脚本的固定步长。越小越精确越吃 CPU；\
                                 保存时会夹进 [1/240s, 1/10s]",
                            );
                            // 界面用毫秒编辑（60Hz ≈ 16.7ms，比 0.016666 秒好读），
                            // 落盘仍是秒 —— 运行时只认秒。
                            let mut ms = draft.runtime.fixed_dt * 1000.0;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut ms)
                                        .range(4.0..=100.0)
                                        .suffix(" ms"),
                                )
                                .changed()
                            {
                                draft.runtime.fixed_dt = ms / 1000.0;
                            }
                            ui.end_row();
                        });

                    ui.add_space(10.0);

                    // ── 层与碰撞矩阵 ──
                    //
                    // 层是**项目级约定**：同一批层号在所有关卡里含义相同，所以矩阵
                    // 放项目设置而不是场景文件。默认全通 —— 不动它就跟引入层之前
                    // 完全一样；只有在真需要"这几类东西互不搭理"时才进来关格子。
                    theme::section_bar(ui, "层与碰撞矩阵");
                    theme::note(
                        ui,
                        "每个实体有一个层（Inspector 顶部的 Layer），这里决定谁和谁碰。\
                     默认全通（不配 = 老行为）；矩阵对称，点一格同时改两侧；\
                     落盘只写改动（project.ron 的 layers 节）。",
                    );
                    ui.add_space(4.0);
                    layer_names(ui, &mut draft.layers);
                    ui.add_space(6.0);
                    theme::note(
                        ui,
                        "碰撞矩阵：行 = 层，列 = 与哪些层碰撞（蓝色 = 碰撞）。默认全勾；\
                     点一下切换，两侧同时生效。列号每 4 列标一个，悬浮可看完整一对。",
                    );
                    layer_matrix(ui, &mut draft.layers);

                    ui.add_space(10.0);

                    // ── 画面 ──
                    theme::section_bar(ui, "画面（曝光 / 色调映射 / 泛光 / FXAA / TAA）");
                    egui::Grid::new("set_render")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .min_col_width(88.0)
                        .show(ui, |ui| {
                            ui.label("色调映射").on_hover_text(
                                "ACES：高光滚降、过曝仍保留层次（默认）；\
                             Off：只曝光 + 钳制（旧管线语义，高光裁成纯白）",
                            );
                            ui.horizontal(|ui| {
                                ui.selectable_value(
                                    &mut draft.render.tone_map,
                                    xgame_core::ToneMapMode::Aces,
                                    "ACES",
                                );
                                ui.selectable_value(
                                    &mut draft.render.tone_map,
                                    xgame_core::ToneMapMode::Off,
                                    "Off",
                                );
                            });
                            ui.end_row();

                            ui.label("曝光")
                                .on_hover_text("线性光乘子：1.0 不增不减；保存时夹进 [0.05, 20]");
                            ui.add(
                                egui::DragValue::new(&mut draft.render.exposure)
                                    .range(
                                        xgame_core::RenderConfig::MIN_EXPOSURE
                                            ..=xgame_core::RenderConfig::MAX_EXPOSURE,
                                    )
                                    .speed(0.02)
                                    .fixed_decimals(2),
                            );
                            ui.end_row();

                            ui.label("FXAA").on_hover_text(
                                "后处理抗锯齿：平滑 MSAA 覆盖不到的几何 / 着色器走样。\
                             默认关（MSAA 已经处理几何边缘，开着会改变旧画面）；\
                             低采样档位或细长几何多的场景收益最明显。\
                             需要场景先渲进 HDR 中间标，属于后处理链的最后一趟。\
                             注意：TAA 开着时 FXAA 自动让位（叠起来只是糊两遍）",
                            );
                            ui.checkbox(&mut draft.render.fxaa.enabled, "");
                            ui.end_row();

                            ui.label("TAA").on_hover_text(
                                "时域抗锯齿：每帧把相机投影抖动一个亚像素，再把重投影后的\
                             历史帧混进来 —— 静态画面收敛到「每像素采了 8 个子像素」。\
                             默认关；对细金属丝 / 远处栏杆 / 高频纹理这类 MSAA 管不到的\
                             走样有效，代价是运动物体会轻微拖影（v1 没有逐物体运动向量），\
                             且与 FXAA 互斥（开着 TAA 时 FXAA 自动关）",
                            );
                            ui.checkbox(&mut draft.render.taa.enabled, "");
                            ui.end_row();

                            ui.label("TAA 历史权重").on_hover_text(
                                "历史帧占比：越大越稳、残影越久；0.9 是常用折中\
                             （静止约 10 帧收敛）。保存时夹进 [0, 0.98]",
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.render.taa.history_weight)
                                    .range(0.0..=xgame_core::TaaConfig::MAX_HISTORY_WEIGHT)
                                    .speed(0.01)
                                    .fixed_decimals(2),
                            )
                            .on_hover_text("TAA 关着时这一项不生效");
                            ui.end_row();

                            ui.label("SSAO").on_hover_text(
                                "屏幕空间环境光遮蔽：接触阴影 / 缝隙压暗（半分辨率深度）。\
                             默认关（画质增强默认不动画面）；开着时 AO 直接乘进场景色\
                             （含直接光）。噪声是确定性的，同机截图逐字节可复现",
                            );
                            ui.checkbox(&mut draft.render.ssao.enabled, "");
                            ui.end_row();

                            ui.label("SSAO 强度").on_hover_text(
                                "遮蔽强度：0 = 无遮蔽效果；2.0 是「看得出但不脏」的常用上限。\
                             保存时夹进 [0, 4]",
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.render.ssao.strength)
                                    .range(0.0..=xgame_core::SsaoConfig::MAX_STRENGTH)
                                    .speed(0.02)
                                    .fixed_decimals(2),
                            );
                            ui.end_row();

                            ui.label("SSAO 半径").on_hover_text(
                                "采样半径（世界单位）：太大 = 远处物体也遮蔽近处；\
                             太小 = 看不出。保存时夹进 [0, 10]",
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.render.ssao.radius)
                                    .range(0.0..=xgame_core::SsaoConfig::MAX_RADIUS)
                                    .speed(0.01)
                                    .fixed_decimals(2),
                            );
                            ui.end_row();

                            ui.label("SSAO 偏置").on_hover_text(
                                "深度比较偏置（世界单位）：挡掉「表面遮蔽自己」的 acne。\
                             保存时夹进 [0, 1]",
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.render.ssao.bias)
                                    .range(0.0..=xgame_core::SsaoConfig::MAX_BIAS)
                                    .speed(0.005)
                                    .fixed_decimals(3),
                            );
                            ui.end_row();

                            ui.label("泛光强度").on_hover_text(
                                "Bloom 光晕的叠加比例：0 = 关闭（渲染器整段跳过，零成本）；\
                             常用 0.4~0.8。保存时夹进 [0, 10]",
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.render.bloom.intensity)
                                    .range(0.0..=xgame_core::BloomConfig::MAX_INTENSITY)
                                    .speed(0.02)
                                    .fixed_decimals(2),
                            );
                            ui.end_row();

                            ui.label("泛光阈值").on_hover_text(
                                "多亮算亮（曝光后的线性亮度，1.0 ≈ 显示白点）。\
                             保存时夹进 [0, 100]",
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.render.bloom.threshold)
                                    .range(0.0..=xgame_core::BloomConfig::MAX_THRESHOLD)
                                    .speed(0.05)
                                    .fixed_decimals(2),
                            );
                            ui.end_row();

                            ui.label("泛光软膝").on_hover_text(
                                "阈值附近多宽的一段『淡入』（单位同阈值）：0 = 硬阈值，\
                             亮斑边缘会有硬边。保存时夹进 [0, 4]",
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.render.bloom.knee)
                                    .range(0.0..=xgame_core::BloomConfig::MAX_KNEE)
                                    .speed(0.02)
                                    .fixed_decimals(2),
                            );
                            ui.end_row();
                        });

                    ui.add_space(10.0);
                    theme::section_bar(ui, "天空（程序化天空盒 + 环境光照 IBL）");
                    egui::Grid::new("set_sky")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .min_col_width(88.0)
                        .show(ui, |ui| {
                            ui.label("启用").on_hover_text(
                                "开着时：背景是程序化天空，环境光 = ambient + IBL（两者相加）。\
                             关着时整段跳过，画面与天空上线前逐位一致。\
                             太阳的位置/颜色取场景里第一盏 active 的方向光",
                            );
                            ui.checkbox(&mut draft.render.sky.enabled, "");
                            ui.end_row();

                            ui.label("强度")
                                .on_hover_text("天空辐射的整体乘子；保存时夹进 [0, 10]");
                            ui.add(
                                egui::DragValue::new(&mut draft.render.sky.intensity)
                                    .range(0.0..=xgame_core::SkyConfig::MAX_INTENSITY)
                                    .speed(0.02)
                                    .fixed_decimals(2),
                            );
                            ui.end_row();

                            ui.label("天顶色");
                            ui.horizontal(|ui| {
                                ui.color_edit_button_rgb(&mut draft.render.sky.zenith);
                            });
                            ui.end_row();

                            ui.label("地平线色");
                            ui.horizontal(|ui| {
                                ui.color_edit_button_rgb(&mut draft.render.sky.horizon);
                            });
                            ui.end_row();

                            ui.label("地面色");
                            ui.horizontal(|ui| {
                                ui.color_edit_button_rgb(&mut draft.render.sky.ground);
                            });
                            ui.end_row();

                            ui.label("太阳盘大小")
                                .on_hover_text("半径（度）。默认 1.5°；保存时夹进 [0.1, 20]");
                            ui.add(
                                egui::DragValue::new(&mut draft.render.sky.sun_size)
                                    .range(
                                        xgame_core::SkyConfig::MIN_SUN_SIZE
                                            ..=xgame_core::SkyConfig::MAX_SUN_SIZE,
                                    )
                                    .speed(0.05)
                                    .fixed_decimals(2),
                            );
                            ui.end_row();

                            ui.label("太阳强度").on_hover_text(
                                "太阳盘与光晕的乘子；没有方向光时整段不出现。保存时夹进 [0, 10]",
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.render.sky.sun_intensity)
                                    .range(0.0..=xgame_core::SkyConfig::MAX_SUN_INTENSITY)
                                    .speed(0.02)
                                    .fixed_decimals(2),
                            );
                            ui.end_row();
                        });

                    ui.add_space(10.0);

                    ui.add_space(10.0);

                    // ── 内容目录 ──
                    theme::section_bar(ui, "内容目录（项目相对路径）");
                    egui::Grid::new("set_dirs")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .min_col_width(88.0)
                        .show(ui, |ui| {
                            let c = &mut draft.content;
                            ui.label("场景");
                            text_field(ui, &mut c.scenes_dir);
                            ui.end_row();
                            ui.label("模型");
                            text_field(ui, &mut c.models_dir);
                            ui.end_row();
                            ui.label("贴图");
                            text_field(ui, &mut c.textures_dir);
                            ui.end_row();
                            ui.label("音频");
                            text_field(ui, &mut c.audio_dir);
                            ui.end_row();
                            ui.label("着色器");
                            text_field(ui, &mut c.shaders_dir);
                            ui.end_row();
                            ui.label("预制体");
                            text_field(ui, &mut c.prefabs_dir);
                            ui.end_row();
                            ui.label("脚本");
                            text_field(ui, &mut c.scripts_dir);
                            ui.end_row();
                        });
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("Apply").color(egui::Color32::WHITE))
                            .fill(theme::colors::ACCENT_BLUE)
                            .min_size(egui::vec2(80.0, theme::FIELD_HEIGHT + 6.0)),
                    )
                    .on_hover_text("校验并写入 project.ron")
                    .clicked()
                {
                    apply = true;
                }
                if ui
                    .add(
                        egui::Button::new("Reset")
                            .min_size(egui::vec2(80.0, theme::FIELD_HEIGHT + 6.0)),
                    )
                    .on_hover_text("丢弃本次修改，恢复清单里的原值")
                    .clicked()
                {
                    reset = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new("Cancel")
                                .min_size(egui::vec2(80.0, theme::FIELD_HEIGHT + 6.0)),
                        )
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            });
        });

    state.settings_open = open && !cancel;

    if apply {
        // 校验失败时草稿被 `apply_settings_draft` 留在原处 —— 窗口继续
        // 显示，用户直接改错的那个字段；成功时草稿已被 take，下一帧
        // 重新克隆清单，显示的正好是刚写回的值。
        let _ = state.apply_settings_draft();
    }
    if reset {
        state.settings_draft = Some(state.project.manifest().clone());
    }
    if !open {
        // 关窗（含右上角 X）：丢草稿。
        state.settings_draft = None;
    }
}
