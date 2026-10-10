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
        let key = (structure_fingerprint(scene), collapsed_version, filter.to_owned());
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

    // ── 搜索过滤器 ──
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.hierarchy_filter)
                .hint_text("Search...")
                .desired_width(f32::INFINITY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let resp = ui
                .add_enabled(is_editing, egui::Button::new("+"))
                .on_disabled_hover_text("Cannot modify scene while playing");
            if resp.clicked() {
                state.create_entity();
            }
        });
    });

    if !is_editing {
        let play_label = match state.play_mode {
            PlayMode::Playing => "▶ Playing",
            PlayMode::Paused => "⏸ Paused",
            PlayMode::Edit => "",
        };
        ui.label(
            egui::RichText::new(format!("{play_label} — Scene locked"))
                .color(egui::Color32::YELLOW)
                .size(11.0),
        );
    }

    ui.separator();

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

        let row_height = 20.0;
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

            // 背景
            let painter = ui.painter();
            if is_drop_target {
                // 拖拽悬停目标：高亮描边，提示"松手就挂到这里"
                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(60, 90, 130));
            } else if selected {
                painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
            } else if resp.hovered() {
                painter.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.bg_fill);
            }

            let mut x = rect.left() + 4.0 + row.depth as f32 * 14.0;
            let center_y = rect.center().y;

            // 折叠箭头（只有含子节点时才画，并且只有它可点）
            if row.has_children {
                let arrow = if state.collapsed_entities.contains(&row.id) {
                    "▶"
                } else {
                    "▼"
                };
                let arrow_rect = egui::Rect::from_min_size(
                    egui::pos2(x, rect.top()),
                    egui::vec2(14.0, row_height),
                );
                let arrow_resp =
                    ui.interact(arrow_rect, ui.id().with(("fold", row.idx)), egui::Sense::click());
                painter.text(
                    arrow_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    arrow,
                    egui::FontId::proportional(10.0),
                    egui::Color32::from_gray(180),
                );
                if arrow_resp.clicked() {
                    toggle_fold = Some(row.id);
                }
            }
            x += 16.0;

            // 图标 + 名称
            // 预制体实例用菱形：根是实心的 🔷，内部节点是空心的 🔹 —— 一眼就能
            // 看出"这一坨是一份资产实例"，而不是一堆普通对象。
            //
            // 后面追加两个徽标：📜 = 挂了脚本（多于一份时带数量），⚙ = 参与物理。
            // 这两样决定了实体"运行时会不会动"，比有没有模型更值得一眼看到。
            let mut icon = String::from(match row.prefab_root {
                Some(true) => "🔷",
                Some(false) => "🔹",
                None if row.has_model => "📦",
                None => "•",
            });
            if row.script_count > 0 {
                if row.script_count > 1 {
                    icon.push_str(&format!("📜{}", row.script_count));
                } else {
                    icon.push('📜');
                }
            }
            if row.has_physics {
                icon.push('⚙');
            }
            let text_color = if selected {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().text_color()
            };
            painter.text(
                egui::pos2(x, center_y),
                egui::Align2::LEFT_CENTER,
                format!("{icon} {}", row.name),
                egui::FontId::proportional(13.0),
                text_color,
            );

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

    // ── 底部创建按钮 ──
    ui.separator();
    ui.horizontal(|ui| {
        let create_resp = ui
            .add_enabled(is_editing, egui::Button::new("Create Empty"))
            .on_disabled_hover_text("Cannot modify scene while playing");
        if create_resp.clicked() {
            state.create_entity();
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
        });
    });

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
