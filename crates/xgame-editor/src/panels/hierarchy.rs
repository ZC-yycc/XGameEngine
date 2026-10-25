//! 类 Unity 层级面板——以**树形**列表显示场景的实体层级。
//!
//! # 层级是怎么长出来的
//!
//! 场景里的父子关系只存了一条 `Entity::parent`（按 ID 引用），子节点并不内嵌
//! 列表。面板每帧按 [`Scene::roots`] 从根往下走，得到一个 `(索引, 深度)` 的
//! 扁平化列表，再按深度缩进画出来——**缩进是渲染出来的，不是数据里的**。
//! 这样"唯一真相"始终是那条 `parent`，不会出现"看到的树"和"数据的树"对不上。
//!
//! # 拖拽重挂
//!
//! 从一个节点拖到另一个节点上 = 把前者挂到后者下（保留世界变换，见
//! [`EditorState::reparent_entity`]）；拖到列表空白处 = 移回场景根层；
//! 拖到面板外 = 取消。成环（把父节点拖进自己的子树）会被 `set_parent` 拒绝并在
//! 控制台给出提示，而不是悄悄产生一棵坏树。

use std::collections::HashSet;

use xgame_core::{EntityId, Scene};

use crate::panels::icons::{self, Icon};
use crate::panels::theme::{self, colors};
use crate::scene_renderer::mix_bits;
use crate::state::{EditorState, PlayMode};

/// 树的每一行：实体索引 + 显示深度。
pub struct Row {
    idx: usize,
    depth: usize,
    id: EntityId,
    name: String,
    has_children: bool,
    has_model: bool,
    /// 预制体实例标记：`Some(true)` = 实例根，`Some(false)` = 实例内部节点，
    /// `None` = 不属于任何预制体实例。
    prefab_root: Option<bool>,
    /// 挂了几份脚本（0 = 没挂）。
    script_count: usize,
    /// 是否参与物理。
    has_physics: bool,
}

// ---------------------------------------------------------------------------
// 行模型缓存
// ---------------------------------------------------------------------------

/// 层级面板的行模型缓存。
///
/// # 为什么缓存
///
/// 旧实现每帧把整棵树重新压平、每行 `name.clone()` + 图标 `String` 拼接 ——
/// 一场景几百个实体就是每帧几百次堆分配，而这棵树**绝大多数帧一个字节都没变**。
/// 商业引擎的面板全是"数据变更才重建"（信号/脏标记）；这里用结构指纹达到同样
/// 效果：零分配 O(n) 扫一遍算出指纹，与缓存 key 相同就整段复用上次的行模型，
/// 不同才重建。指纹只覆盖**行显示真正读到**的字段 —— 变换、环境光之类的变化
/// 不会让层级树白重建。
#[derive(Default)]
pub struct RowCache {
    /// 当前 `rows` 对应的 key：`(结构指纹, 折叠版本, 过滤器)`。
    key: Option<(u64, u64, String)>,
    rows: Vec<Row>,
}

impl RowCache {
    /// 取本帧要显示的行（把 Vec **移出**缓存交给调用方，用完经
    /// [`give_back`](Self::give_back) 还回来 —— 面板要在持有行的同时 `&mut state`
    /// 处理点击/右键菜单，直接借缓存里的行会撞借用检查器）。
    fn take_rows(
        &mut self,
        scene: &Scene,
        collapsed: &HashSet<EntityId>,
        collapsed_version: u64,
        filter: &str,
    ) -> Vec<Row> {
        let key = (
            structure_fingerprint(scene),
            collapsed_version,
            filter.to_owned(),
        );
        if self.key.as_ref() == Some(&key) {
            return std::mem::take(&mut self.rows);
        }

        // 父子关系一次解算好，本帧后面所有"按父找子"的查询都走它，不再逐个扫表。
        let (parent_indices, child_map) = scene.hierarchy();
        let flat = if filter.is_empty() {
            flatten(scene, &parent_indices, &child_map, collapsed)
        } else {
            // 搜索态：摊平成匹配项列表（不按层级），与 Unity 的搜索行为一致。
            scene
                .entities
                .iter()
                .enumerate()
                .filter(|(_, e)| e.name.to_lowercase().contains(filter))
                .map(|(i, _)| (i, 0usize))
                .collect()
        };
        let rows = flat
            .into_iter()
            .map(|(idx, depth)| Row {
                idx,
                depth,
                id: scene.entities[idx].id,
                name: scene.entities[idx].name.clone(),
                has_children: !child_map[idx].is_empty(),
                has_model: !scene.entities[idx].model.is_empty(),
                prefab_root: scene.entities[idx].prefab.as_ref().map(|p| p.is_root),
                script_count: scene.entities[idx].scripts.len(),
                has_physics: scene.entities[idx].physics.is_some(),
            })
            .collect();

        self.key = Some(key);
        rows
    }

    /// 把用完的行还回缓存（下一帧指纹没变就直接复用）。
    fn give_back(&mut self, rows: Vec<Row>) {
        self.rows = rows;
    }
}

/// 行显示真正读到的字段的结构指纹（零分配线性扫描）。
///
/// 漏收一个字段 = 界面上"改了不重建"的脏行；多收无害（白重建一次）。
fn structure_fingerprint(scene: &Scene) -> u64 {
    let mut hash = 0u64;
    for e in &scene.entities {
        hash = mix_bits(hash, e.id.raw());
        hash = mix_bits(hash, e.parent.map_or(0, EntityId::raw));
        for byte in e.name.as_bytes() {
            hash = mix_bits(hash, *byte as u64);
        }
        hash = mix_bits(hash, !e.model.is_empty() as u64);
        hash = mix_bits(hash, e.prefab.as_ref().map_or(2, |p| p.is_root as u64));
        hash = mix_bits(hash, e.scripts.len() as u64);
        hash = mix_bits(hash, e.physics.is_some() as u64);
    }
    hash
}

/// 把场景层级压平成带深度的行列表（跳过被折叠节点的子树）。
///
/// `parent_indices` / `child_map` 由调用方从**一次** `Scene::hierarchy()` 里取出传进来。
/// 原先在递归里逐节点调 `scene.children()`，每个节点都要扫一遍整张实体表，整棵树
/// 就成了 O(n²) —— 这个函数每帧都跑，实体一多就是明显的界面卡顿。
fn flatten(
    scene: &Scene,
    parent_indices: &[Option<usize>],
    child_map: &[Vec<usize>],
    collapsed: &HashSet<EntityId>,
) -> Vec<(usize, usize)> {
    fn walk(
        scene: &Scene,
        child_map: &[Vec<usize>],
        collapsed: &HashSet<EntityId>,
        idx: usize,
        depth: usize,
        out: &mut Vec<(usize, usize)>,
    ) {
        if out.len() > 10_000 {
            return; // 防御：坏数据也不至于把界面拖死
        }
        out.push((idx, depth));
        if collapsed.contains(&scene.entities[idx].id) {
            return;
        }
        for &child in &child_map[idx] {
            walk(scene, child_map, collapsed, child, depth + 1, out);
        }
    }

    let mut out = Vec::new();
    for (idx, parent) in parent_indices.iter().enumerate() {
        if parent.is_none() {
            walk(scene, child_map, collapsed, idx, 0, &mut out);
        }
    }
    out
}

/// 在 `Ui` 内渲染层级面板内容。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    let is_editing = state.is_editing();

    // ── 运行态提示条 ──
    //
    // 放在搜索框**之前**：这是面板级的模式提示，UE 的 World Outliner 也把它压在
    // 最上面。跟在搜索框后面会被读成"过滤结果的一部分"，而它其实和搜索无关。
    //
    // 底色 + 图标 + 分色文字（运行 = 绿、暂停 = 黄）：原先是一整行 `WARN` 黄字，
    // 和工具栏绿色的 "Playing" 对不上，用户会以为是"出错了"。
    if !is_editing {
        let (icon, label, tint) = match state.play_mode {
            PlayMode::Playing => (Icon::Play, "Playing", colors::PLAY_GREEN),
            PlayMode::Paused => (Icon::Pause, "Paused", colors::WARN),
            PlayMode::Edit => (Icon::Dot, "Edit", colors::TEXT_SECONDARY),
        };
        egui::Frame::default()
            .fill(tint.gamma_multiply(0.16))
            .stroke(egui::Stroke::new(1.0_f32, tint.gamma_multiply(0.45)))
            .rounding(egui::Rounding::same(2.0))
            .inner_margin(egui::Margin::symmetric(5.0, 2.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    icons::small_icon(ui, icon, 9.0, tint);
                    ui.label(egui::RichText::new(label).size(11.0).color(tint));
                    ui.label(
                        egui::RichText::new("— Scene locked")
                            .size(11.0)
                            .color(colors::TEXT_DIM),
                    );
                    // 撑满整行：`Frame` 只围住内容的 `min_rect`，不加这一句横幅
                    // 就缩成一个药丸（UE 的模式提示条是通栏的）。
                    ui.add_space(ui.available_width().max(0.0));
                });
            });
        ui.add_space(3.0);
    }

    // ── 搜索过滤器 ──
    // 只留一个整宽搜索框；"+" 与底部「Create Empty」重复，去掉可以让搜索框
    // 独占一行（此前加号把输入宽挤掉 24px，占位文字都快放不下）。
    ui.add_sized(
        egui::vec2(ui.available_width(), theme::FIELD_HEIGHT),
        egui::TextEdit::singleline(&mut state.hierarchy_filter).hint_text("Search…"),
    );

    ui.add_space(2.0);

    // ── 收集本帧要显示的行 ──
    // 走行模型缓存：结构指纹没变就复用上一帧的（零 String 分配），见 `RowCache`。
    // 行从缓存里**移出来**用，帧尾再还回去 —— 这样循环里要 `&mut state`（右键菜单
    // 改场景）不会和"借缓存里的行"撞借用检查器。
    let filter = state.hierarchy_filter.trim().to_lowercase();
    let rows: Vec<Row> = state.hierarchy_cache.take_rows(
        &state.scene,
        &state.collapsed_entities,
        state.collapsed_version,
        &filter,
    );

    let dragging = state.hierarchy_drag.is_some();
    let pointer = ui.ctx().pointer_interact_pos();

    let mut row_rects: Vec<egui::Rect> = Vec::with_capacity(rows.len());
    let mut clicked_select: Option<usize> = None;
    let mut toggle_fold: Option<EntityId> = None;
    let mut drag_started: Option<EntityId> = None;

    let scroll = egui::ScrollArea::vertical().show(ui, |ui| {
        if rows.is_empty() {
            ui.weak(if filter.is_empty() {
                "Scene is empty — use Create to add objects."
            } else {
                "No objects match the search."
            });
            return;
        }

        let row_height = theme::ROW_HEIGHT;
        let full_width = ui.available_width();

        for row in &rows {
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(full_width, row_height),
                egui::Sense::click_and_drag(),
            );
            row_rects.push(rect);

            let selected = state.selected_entity == Some(row.idx);
            let is_drop_target = dragging
                && state.hierarchy_drag != Some(row.id)
                && pointer.is_some_and(|p| rect.contains(p));

            // 背景：选中是实心蓝，悬停是浅一档的灰，拖拽目标是强调蓝。
            let painter = ui.painter();
            if is_drop_target {
                painter.rect_filled(rect, 2.0, colors::ACCENT_BLUE.gamma_multiply(0.75));
            } else if selected {
                painter.rect_filled(rect, 0.0, colors::SELECTION_BG);
            } else if resp.hovered() {
                painter.rect_filled(rect, 0.0, colors::ROW_HOVER_BG);
            }

            let mut x = rect.left() + 4.0 + row.depth as f32 * 12.0;
            let center_y = rect.center().y;

            // 折叠箭头：固定 14px 列宽，画矢量三角（字形的 ▼ 基线不稳）。
            if row.has_children {
                let arrow_hit = egui::Rect::from_min_size(
                    egui::pos2(x, rect.top()),
                    egui::vec2(14.0, row_height),
                );
                let arrow_resp = ui.interact(
                    arrow_hit,
                    ui.id().with(("fold", row.idx)),
                    egui::Sense::click(),
                );
                icons::paint(
                    painter,
                    egui::Rect::from_center_size(arrow_hit.center(), egui::Vec2::splat(9.0)),
                    if state.collapsed_entities.contains(&row.id) {
                        Icon::ChevronRight
                    } else {
                        Icon::ChevronDown
                    },
                    if arrow_resp.hovered() {
                        colors::TEXT_BRIGHT
                    } else {
                        colors::TEXT_SECONDARY
                    },
                );
                if arrow_resp.clicked() {
                    toggle_fold = Some(row.id);
                }
            }
            x += 16.0;

            // 图标：预制体 = 菱形（紫）、有模型 = 小方块、空实体 = 圆点。
            // 单图标 + 右侧徽标，避免"一行两个图标"的拥挤感。
            let (icon, icon_color) = match row.prefab_root {
                Some(true) => (Icon::Diamond, colors::BADGE_PREFAB),
                Some(false) => (Icon::Diamond, colors::BADGE_PREFAB.gamma_multiply(0.6)),
                None if row.has_model => (Icon::Cube, colors::TEXT_SECONDARY),
                None => (Icon::Dot, colors::TEXT_DIM),
            };
            icons::paint(
                painter,
                egui::Rect::from_center_size(
                    egui::pos2(x + 6.0, center_y),
                    egui::Vec2::splat(12.0),
                ),
                icon,
                icon_color,
            );
            x += 18.0;

            let text_color = if selected {
                colors::TEXT_BRIGHT
            } else {
                colors::TEXT_PRIMARY
            };
            painter.text(
                egui::pos2(x, center_y),
                egui::Align2::LEFT_CENTER,
                &row.name,
                egui::FontId::proportional(12.5),
                text_color,
            );

            // 右侧徽标（从右往左排）：物理 P / 脚本 S（多份带数量）。
            let mut badge_x = rect.right() - 6.0;
            if row.has_physics {
                badge_x -= badge(
                    painter,
                    badge_x,
                    center_y,
                    "P",
                    colors::BADGE_PHYSICS,
                    selected,
                );
            }
            if row.script_count > 0 {
                let label = if row.script_count > 1 {
                    format!("S{}", row.script_count)
                } else {
                    "S".to_owned()
                };
                badge(
                    painter,
                    badge_x,
                    center_y,
                    &label,
                    colors::BADGE_SCRIPT,
                    selected,
                );
            }

            // 交互
            if resp.clicked() && toggle_fold.is_none() {
                clicked_select = Some(row.idx);
            }
            if resp.drag_started() && is_editing {
                drag_started = Some(row.id);
            }
            if resp.double_clicked() && row.has_children {
                toggle_fold = Some(row.id);
            }

            // 右键上下文菜单
            let row_idx = row.idx;
            let row_id = row.id;
            let row_prefab_root = row.prefab_root;
            resp.context_menu(|ui| {
                let ce = ui
                    .add_enabled(is_editing, egui::Button::new("Create Empty"))
                    .on_disabled_hover_text("Cannot modify scene while playing");
                if ce.clicked() {
                    state.create_entity();
                    ui.close_menu();
                }
                ui.add_enabled_ui(is_editing, |ui| {
                    ui.menu_button("Create Primitive", |ui| {
                        for kind in xgame_render::mesh::BuiltinMesh::ALL {
                            if ui.button(kind.label()).clicked() {
                                state.create_primitive(kind);
                                ui.close_menu();
                            }
                        }
                    });
                    ui.menu_button("Create UI", |ui| {
                        for kind in xgame_core::UiKind::ALL {
                            if ui.button(kind.label()).clicked() {
                                state.create_ui(kind);
                                ui.close_menu();
                            }
                        }
                    });
                });

                ui.separator();
                let dup = ui
                    .add_enabled(is_editing, egui::Button::new("Duplicate"))
                    .on_disabled_hover_text("Cannot modify scene while playing");
                if dup.clicked() {
                    state.selected_entity = Some(row_idx);
                    state.duplicate_entity();
                    ui.close_menu();
                }
                if ui.button("Copy").clicked() {
                    state.selected_entity = Some(row_idx);
                    state.copy_selected();
                    ui.close_menu();
                }
                let paste = ui
                    .add_enabled(is_editing, egui::Button::new("Paste as Child"))
                    .on_disabled_hover_text("Cannot modify scene while playing");
                if paste.clicked() {
                    // 记下"父"是谁，粘贴完把新副本挂上去
                    let parent_before = state.scene.entities.len();
                    state.paste_entity();
                    if state.scene.entities.len() > parent_before {
                        if let Some(new_idx) = state.selected_entity {
                            state.reparent_entity(new_idx, Some(row_idx));
                        }
                    }
                    ui.close_menu();
                }

                ui.separator();
                // ── 预制体 ──
                let mk = ui
                    .add_enabled(is_editing, egui::Button::new("Create Prefab from This…"))
                    .on_disabled_hover_text("Cannot modify scene while playing");
                if mk.clicked() {
                    state.selected_entity = Some(row_idx);
                    state.begin_create_prefab();
                    ui.close_menu();
                }
                if row_prefab_root == Some(true) {
                    if ui.button("Apply to Prefab").clicked() {
                        state.apply_prefab(row_idx);
                        ui.close_menu();
                    }
                    if ui.button("Revert from Prefab").clicked() {
                        state.revert_prefab(row_idx);
                        ui.close_menu();
                    }
                }

                ui.separator();
                let has_parent = state.scene.entities[row_idx].parent.is_some();
                let unparent = ui
                    .add_enabled(is_editing && has_parent, egui::Button::new("Unparent"))
                    .on_disabled_hover_text(if has_parent {
                        "Cannot modify scene while playing"
                    } else {
                        "Already at scene root"
                    });
                if unparent.clicked() {
                    state.reparent_entity(row_idx, None);
                    ui.close_menu();
                }

                ui.separator();
                let del = ui
                    .add_enabled(is_editing, egui::Button::new("Delete"))
                    .on_disabled_hover_text("Cannot modify scene while playing");
                if del.clicked() {
                    state.selected_entity = Some(row_idx);
                    state.delete_selected();
                    ui.close_menu();
                }
                let _ = row_id;
            });
        }
    });

    // ── 应用本帧收集到的行内改动 ──
    if let Some(id) = toggle_fold {
        if !state.collapsed_entities.remove(&id) {
            state.collapsed_entities.insert(id);
        }
        // 折叠集合变了 → 版本号 +1，下一帧行缓存会按新 key 重建。
        state.collapsed_version += 1;
    }
    if let Some(idx) = clicked_select {
        state.selected_entity = Some(idx);
    }
    if let Some(id) = drag_started {
        state.hierarchy_drag = Some(id);
    }

    // ── 拖拽落点判定 ──
    // 只在"指针抬起"的那一帧处理，且只在拖拽确实发生过（hierarchy_drag 被置位）时。
    if dragging && ui.input(|i| i.pointer.any_released()) {
        if let Some(dragged) = state.hierarchy_drag.take() {
            if let Some(child_idx) = state.scene.index_of(dragged) {
                let target = pointer.and_then(|p| row_rects.iter().position(|r| r.contains(p)));
                match target {
                    Some(t) => {
                        let parent_idx = rows[t].idx;
                        state.reparent_entity(child_idx, Some(parent_idx));
                    }
                    None => {
                        // 落在列表空白处 = 移回根层；落在面板外 = 取消（不产生改动）
                        let inside = pointer.is_some_and(|p| scroll.inner_rect.contains(p));
                        if inside {
                            state.reparent_entity(child_idx, None);
                        }
                    }
                }
            }
        }
    }

    // ── 底部「＋ Add」──
    //
    // UE 的 Content Browser / Outliner 都是一个整宽的 Add 按钮 + 下拉类型列表；
    // 比并排两个小按钮更整齐（并排时两个按钮宽度由文字决定，右边永远空一块）。
    ui.add_space(2.0);
    ui.separator();
    {
        let add_id = ui.id().with("hierarchy_add_menu");
        let response = ui
            .add_enabled_ui(is_editing, |ui| {
                icons::add_row(ui, "Add", "新建实体 / 内置图元")
            })
            .inner
            .on_disabled_hover_text("播放模式下不能修改场景");

        if response.clicked() {
            ui.memory_mut(|memory| memory.toggle_popup(add_id));
        }

        egui::popup_below_widget(
            ui,
            add_id,
            &response,
            egui::PopupCloseBehavior::CloseOnClick,
            |ui| {
                ui.set_min_width(180.0);
                if ui.button("Empty Entity").clicked() {
                    state.create_entity();
                }
                ui.separator();
                for kind in xgame_render::mesh::BuiltinMesh::ALL {
                    if ui.button(kind.label()).clicked() {
                        state.create_primitive(kind);
                    }
                }
                ui.separator();
                // UI 与 3D 图元分开放：它们的摆放方式完全不同（锚点 vs 变换），
                // 混在一个列表里会误导"选了就能拖进场景"。
                for kind in xgame_core::UiKind::ALL {
                    if ui.button(format!("UI {}", kind.label())).clicked() {
                        state.create_ui(kind);
                    }
                }
            },
        );
    }

    // ── 键盘快捷键 ──
    if is_editing {
        if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            state.delete_selected();
        }
        if ui.input(|i| i.key_pressed(egui::Key::D) && i.modifiers.ctrl) {
            state.duplicate_entity();
        }
        if ui.input(|i| i.key_pressed(egui::Key::C) && i.modifiers.ctrl) {
            state.copy_selected();
        }
        if ui.input(|i| i.key_pressed(egui::Key::V) && i.modifiers.ctrl) {
            state.paste_entity();
        }
    }

    // 行模型还回缓存：指纹没变的下一帧直接复用，零 String 分配。
    state.hierarchy_cache.give_back(rows);
}

/// 画一个右对齐的小徽标（S = 有脚本 / P = 参与物理），返回占用宽度（含间距）。
///
/// 用「描边 + 淡底 + 字母」而不是字形图标：字形图标的字宽不稳定，
/// 一多就会挤在一起；字母徽标在 13px 高度里永远排得下。
/// 行尾徽标（P = 物理 / S = 脚本）。
///
/// `selected` 只影响底色：选中行是蓝底，徽标那层 12% 的同色底会直接糊进
/// 蓝色里（"S" 几乎看不见）。选中时改用半透明黑底 + 提亮一档的字色，
/// 徽标的色相仍然保留 —— 用户靠颜色认"这是脚本还是物理"。
fn badge(
    painter: &egui::Painter,
    right: f32,
    center_y: f32,
    text: &str,
    color: egui::Color32,
    selected: bool,
) -> f32 {
    let text_color = if selected {
        color.gamma_multiply(1.25)
    } else {
        color
    };
    let galley =
        painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(9.0), text_color);
    let width = (galley.size().x + 7.0).max(13.0);
    let rect = egui::Rect::from_min_size(
        egui::pos2(right - width, center_y - 6.5),
        egui::vec2(width, 13.0),
    );
    painter.rect_filled(
        rect,
        2.0,
        if selected {
            egui::Color32::from_black_alpha(110)
        } else {
            color.gamma_multiply(0.12)
        },
    );
    painter.galley(
        egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        text_color,
    );
    width + 3.0
}
