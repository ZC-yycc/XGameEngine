//! 资源浏览器（Assets）——浏览项目里的全部资产，并把它们放进场景。
//!
//! 对应 Unity 的 Project 窗口 / Godot 的 FileSystem 面板。此前项目里只有一个
//! Prefabs 面板：模型、纹理、音频**看不见也拖不进场景**，每加一个资产都得手改
//! RON —— 这正是 P0 资产债里"资源浏览器"那一半（另一半是 GUID，见
//! `xgame_core::asset`）。
//!
//! # 数据从哪来
//!
//! 列表来自 [`EditorState::asset_browser`]，由 [`EditorState::refresh_assets`]
//! 在打开项目、点 ⟳、导入资产后刷新 —— 与 Prefabs / Scripts 面板同一套缓存纪律：
//! **不逐帧扫盘**。这里显示的是"资产库里登记过的"资产，也就是有 `.meta` 的资产；
//! 缺 `.meta` 的会在刷新时自动补上（打开编辑器时已做一次）。
//!
//! # 交互
//!
//! - 单击 = 选中（底部显示路径 / GUID / 导入设置）
//! - 双击 = 放进场景（模型建实体、预制体实例化），落在相机正前方
//! - 按住拖到视口 = 放在鼠标射线打到的地面位置（落点逻辑在 `scene_view`）
//!
//! 拖拽刻意不用 egui 的 `DragAndDrop` payload：视口是一块自绘的离屏纹理，
//! 命中判定与落点本来就要自己算，一条 [`EditorState::asset_drag`] 比跨控件传
//! payload 更直观、也更容易写回归测试。
//!
//! # 为什么目录行也自绘
//!
//! `egui::CollapsingHeader` 的行高、缩进、字色都走 egui 默认 style，混在
//! 一排 22px 自绘行里会显得"一行高一行矮"（这个项目在 Hierarchy 上踩过）。
//! 目录行因此与资产行共用同一套 `ROW_HEIGHT` / 图标 / 颜色常量，
//! 展开状态存在 egui 的临时数据里（`insert_temp`），不进 `EditorState`。

use std::collections::BTreeMap;

use xgame_core::AssetKind;

use crate::panels::{icons, theme};
use crate::state::EditorState;

/// 面板里一行的动作（先收集、后应用，避免渲染时同时可变借用 state）。
enum Action {
    Select(String),
    BeginDrag(String),
    Place(String),
}

/// 顶层入口（`#[derive(Panel)]` 按蛇形名分发到这里）。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    header(state, ui);
    ui.add_space(2.0);
    ui.add_sized(
        egui::vec2(ui.available_width(), theme::FIELD_HEIGHT + 2.0),
        egui::TextEdit::singleline(&mut state.asset_filter).hint_text("Search assets…"),
    );

    ui.add_space(4.0);
    ui.separator();

    let entries = filtered_entries(state);
    let tree = build_tree(&entries);

    // 详情区要占掉一块固定高度：`ScrollArea` 默认会吃掉全部可用高度，
    // 结果底部的"路径 / GUID / 导入设置"永远被挤到视野之外（第一版就是这样，
    // 截图才发现）。高度随"有没有选中"变化 —— 没选中时只需要一行提示，
    // 把空间让给列表（否则 8 个资产只能看见 7 个）。
    let reserve = if state.asset_selected.is_some() {
        (ui.available_height() * 0.46).clamp(120.0, 150.0)
    } else {
        34.0
    };
    let list_height = (ui.available_height() - reserve).max(60.0);

    let mut actions: Vec<Action> = Vec::new();
    egui::ScrollArea::vertical()
        .max_height(list_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if entries.is_empty() {
                ui.add_space(4.0);
                ui.label(theme::dim_note(if state.asset_browser.is_empty() {
                    "还没有资产 —— 把模型 / 纹理 / 音频放进 Resources/ 后点右上角 ⟳"
                } else {
                    "没有匹配的资产"
                }));
                return;
            }
            let filter = state.asset_filter.trim().to_lowercase();
            render_node(&tree, "", 0, &filter, ui, state, &mut actions);
        });

    // 拖拽的收尾统一放在 `main.rs` 的帧末：面板可能被拖成非活动页签而不绘制，
    // 只在这里判"松手"会漏。放置动作在 `scene_view` 里做（它知道落点）。

    ui.separator();
    details(state, ui, &mut actions);

    for action in actions {
        match action {
            Action::Select(path) => state.asset_selected = Some(path),
            Action::BeginDrag(path) => {
                state.asset_selected = Some(path.clone());
                state.asset_drag = Some(path);
            }
            Action::Place(path) => {
                let position = state.placement_point_in_front();
                state.spawn_asset_at(&path, position);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 顶部
// ---------------------------------------------------------------------------

fn header(state: &mut EditorState, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Assets")
                .family(theme::bold_family())
                .color(theme::colors::TEXT_PRIMARY),
        );
        ui.label(theme::dim_note(format!("{} 项", state.asset_browser.len())));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icons::icon_button(
                ui,
                icons::Icon::Refresh,
                18.0,
                "重新扫描项目资产\n（给新资产补 .meta、拾取外部改动）",
                false,
            )
            .clicked()
            {
                state.refresh_assets();
            }
        });
    });
}

// ---------------------------------------------------------------------------
// 列表
// ---------------------------------------------------------------------------

/// 过滤后的资产（保持原顺序：按路径字典序）。
fn filtered_entries(state: &EditorState) -> Vec<(String, AssetKind)> {
    let filter = state.asset_filter.trim().to_lowercase();
    state
        .asset_browser
        .iter()
        .filter(|(path, _)| filter.is_empty() || path.to_lowercase().contains(&filter))
        .cloned()
        .collect()
}

/// 目录树（目录按名字排序，文件保持输入顺序 —— 输入本身是路径字典序，
/// 于是两次扫描得到的树逐节点一致）。
#[derive(Default)]
struct DirNode {
    dirs: BTreeMap<String, DirNode>,
    files: Vec<(String, AssetKind)>,
}

fn build_tree(entries: &[(String, AssetKind)]) -> DirNode {
    let mut root = DirNode::default();
    for (path, kind) in entries {
        let mut cursor = &mut root;
        let mut parts: Vec<&str> = path.split('/').collect();
        let file = parts.pop().unwrap_or(path.as_str());
        for part in parts {
            cursor = cursor.dirs.entry(part.to_string()).or_default();
        }
        cursor.files.push((file.to_string(), *kind));
    }
    root
}

/// 递归渲染一层目录。
///
/// `prefix` 是当前目录的项目相对前缀（带结尾斜杠），用来把短文件名还原成
/// 完整路径 —— 不能靠 `ends_with` 反查，那会让两个同名文件互相串。
fn render_node(
    node: &DirNode,
    prefix: &str,
    depth: usize,
    filter: &str,
    ui: &mut egui::Ui,
    state: &EditorState,
    actions: &mut Vec<Action>,
) {
    for (name, child) in &node.dirs {
        let child_prefix = join_dir(prefix, name);
        let open = dir_row(ui, name, depth, &child_prefix);
        if open {
            render_node(child, &child_prefix, depth + 1, filter, ui, state, actions);
        }
    }
    for (file, kind) in &node.files {
        let full = join_path(prefix, file);
        asset_row(ui, &full, file, *kind, depth, filter, state, actions);
    }
}

/// 目录前缀 + 文件名 → 完整项目相对路径。
///
/// 单独抽出来是为了能被测试钉住：靠 `ends_with` 反查会让两个目录下的同名文件
/// 互相串（`Resources/A/Box.glb` 与 `Resources/B/Box.glb`），那时点谁都打开同一个。
fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}{name}")
    }
}

/// 目录前缀 + 子目录名 → 子目录前缀（**带结尾斜杠**，供下一层拼接）。
fn join_dir(prefix: &str, name: &str) -> String {
    format!("{prefix}{name}/")
}

/// 目录行：箭头 + 名字，整行可点、悬停浮底。返回是否展开。
fn dir_row(ui: &mut egui::Ui, name: &str, depth: usize, key: &str) -> bool {
    use egui::{Align2, FontId, Pos2, Vec2};

    let id = egui::Id::new(("asset_dir", key));
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true);

    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, theme::ROW_HEIGHT),
        egui::Sense::click(),
    );
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 0.0, theme::colors::ROW_HOVER_BG);
    }
    let indent = depth as f32 * 10.0;
    let arrow = egui::Rect::from_center_size(
        Pos2::new(rect.left() + 10.0 + indent, rect.center().y),
        Vec2::splat(10.0),
    );
    icons::paint(
        painter,
        arrow,
        if open {
            icons::Icon::ChevronDown
        } else {
            icons::Icon::ChevronRight
        },
        theme::colors::TEXT_SECONDARY,
    );
    // 文件夹图标也用矢量：与小尺寸下"箭头 + 文字"相比更能一眼分出层级。
    let folder = egui::Rect::from_center_size(
        Pos2::new(arrow.right() + 8.0, rect.center().y),
        Vec2::splat(11.0),
    );
    icons::paint(painter, folder, icons::Icon::Folder, theme::colors::BADGE_PHYSICS);
    painter.text(
        Pos2::new(folder.right() + 5.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        FontId::new(12.0, theme::bold_family()),
        theme::colors::TEXT_PRIMARY,
    );

    if response.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    open
}

/// 一行资产：图标 + 文件名 + 悬停浮底 + 选中高亮。
#[allow(clippy::too_many_arguments)]
fn asset_row(
    ui: &mut egui::Ui,
    full_path: &str,
    file_name: &str,
    kind: AssetKind,
    depth: usize,
    filter: &str,
    state: &EditorState,
    actions: &mut Vec<Action>,
) {
    use egui::{Align2, FontId, Pos2, Vec2};

    let selected = state.asset_selected.as_deref() == Some(full_path);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, theme::ROW_HEIGHT),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 0.0, theme::colors::SELECTION_BG);
    } else if response.hovered() {
        painter.rect_filled(rect, 0.0, theme::colors::ROW_HOVER_BG);
    }

    let indent = depth as f32 * 10.0;
    let icon_rect = egui::Rect::from_center_size(
        Pos2::new(rect.left() + 16.0 + indent, rect.center().y),
        Vec2::splat(11.0),
    );
    icons::paint(painter, icon_rect, kind_icon(kind), kind_color(kind));

    painter.text(
        Pos2::new(icon_rect.right() + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        file_name,
        FontId::proportional(12.0),
        if selected {
            theme::colors::TEXT_BRIGHT
        } else {
            theme::colors::TEXT_PRIMARY
        },
    );

    if response.clicked() {
        actions.push(Action::Select(full_path.to_string()));
    }
    if response.drag_started() {
        actions.push(Action::BeginDrag(full_path.to_string()));
    }
    if response.double_clicked() {
        actions.push(Action::Place(full_path.to_string()));
    }

    let hover = format!(
        "{full_path}\n\n{} · 双击放入场景 · 拖到视口放到鼠标位置",
        kind.label()
    );
    let _ = filter;
    response.on_hover_text(hover).context_menu(|ui| {
        if ui
            .add_enabled(
                kind.is_draggable_into_scene() && state.is_editing(),
                egui::Button::new("放入场景"),
            )
            .clicked()
        {
            actions.push(Action::Place(full_path.to_string()));
            ui.close_menu();
        }
        if ui.button("复制路径").clicked() {
            ui.ctx().copy_text(full_path.to_string());
            ui.close_menu();
        }
        if let Some(guid) = state.project.asset_guid(full_path) {
            if ui.button("复制 GUID").clicked() {
                ui.ctx().copy_text(guid.to_string());
                ui.close_menu();
            }
        }
    });
}

// ---------------------------------------------------------------------------
// 详情
// ---------------------------------------------------------------------------

fn details(state: &mut EditorState, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let Some(relative) = state.asset_selected.clone() else {
        ui.label(theme::dim_note("选中一个资产查看详情与导入设置"));
        return;
    };
    let Some(kind) = AssetKind::from_path(&relative) else {
        return;
    };

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(12.0, theme::ROW_HEIGHT),
            egui::Sense::hover(),
        );
        icons::paint(ui.painter(), rect, kind_icon(kind), kind_color(kind));
        ui.label(
            egui::RichText::new(kind.label())
                .family(theme::bold_family())
                .color(theme::colors::TEXT_PRIMARY),
        );
    });
    ui.label(
        egui::RichText::new(&relative)
            .monospace()
            .size(11.0)
            .color(theme::colors::TEXT_SECONDARY),
    );
    if let Some(guid) = state.project.asset_guid(&relative) {
        let full = guid.to_string();
        ui.label(
            egui::RichText::new(format!("GUID {}", &full[..8]))
                .monospace()
                .size(11.0)
                .color(theme::colors::TEXT_DIM),
        )
        .on_hover_text(full);
    }

    // 导入设置：只对该类型有意义的字段才显示（模型没有"纹理过滤"，
    // 把不适用的开关画出来只会让人以为它有效）。
    let mut import = state.asset_import(&relative);
    let before = import;
    ui.add_space(4.0);
    ui.add_enabled_ui(state.is_editing(), |ui| match kind {
        AssetKind::Model => {
            ui.horizontal(|ui| {
                ui.label(theme::dim_note("缩放"));
                ui.add(
                    egui::DragValue::new(&mut import.model_scale)
                        .speed(0.01)
                        .range(0.001..=1000.0)
                        .fixed_decimals(2),
                );
            });
        }
        AssetKind::Texture => {
            ui.checkbox(&mut import.texture_srgb, "sRGB（颜色贴图）");
            ui.checkbox(&mut import.texture_nearest, "最近邻过滤（像素风）");
        }
        AssetKind::Audio => {
            ui.checkbox(&mut import.audio_streaming, "流式播放（长音频）");
        }
        _ => {
            ui.label(theme::dim_note("该类型没有可调导入参数"));
        }
    });
    if import != before {
        state.set_asset_import(&relative, import);
    }

    if kind.is_draggable_into_scene() {
        ui.add_space(4.0);
        let label = if kind == AssetKind::Prefab {
            "实例化到场景"
        } else {
            "放入场景"
        };
        if ui
            .add_enabled(
                state.is_editing(),
                egui::Button::new(label).min_size(egui::vec2(ui.available_width(), 20.0)),
            )
            .clicked()
        {
            actions.push(Action::Place(relative));
        }
    }
}

// ---------------------------------------------------------------------------
// 类型 → 图标 / 颜色
// ---------------------------------------------------------------------------

fn kind_icon(kind: AssetKind) -> icons::Icon {
    match kind {
        AssetKind::Model => icons::Icon::Cube,
        AssetKind::Texture => icons::Icon::Image,
        AssetKind::Audio => icons::Icon::Wave,
        AssetKind::Script => icons::Icon::Script,
        AssetKind::Prefab => icons::Icon::Diamond,
        AssetKind::Shader => icons::Icon::Component,
        AssetKind::Scene | AssetKind::Other => icons::Icon::Folder,
    }
}

/// 颜色只取主题里已有的强调色，不就地造 RGB —— 与全仓库的取色纪律一致。
fn kind_color(kind: AssetKind) -> egui::Color32 {
    match kind {
        AssetKind::Model => theme::colors::ACCENT_LIGHT,
        AssetKind::Texture => theme::colors::BADGE_PHYSICS,
        AssetKind::Audio => theme::colors::BADGE_SCRIPT,
        AssetKind::Script => theme::colors::BADGE_SCRIPT,
        AssetKind::Prefab => theme::colors::BADGE_PREFAB,
        AssetKind::Shader => theme::colors::TEXT_ACCENT,
        AssetKind::Scene | AssetKind::Other => theme::colors::TEXT_DIM,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 目录树必须把**完整路径**还原出来 —— 两个目录下的同名文件不能串。
    ///
    /// 第一版用 `ends_with` 反查完整路径，`Resources/A/Box.glb` 与
    /// `Resources/B/Box.glb` 会互相命中（点谁都是同一个）。
    #[test]
    fn tree_rebuilds_unambiguous_full_paths() {
        let entries = vec![
            ("Resources/A/Box.glb".to_string(), AssetKind::Model),
            ("Resources/B/Box.glb".to_string(), AssetKind::Model),
            ("Scripts/a.lua".to_string(), AssetKind::Script),
        ];
        let tree = build_tree(&entries);

        assert_eq!(
            tree.dirs.keys().cloned().collect::<Vec<_>>(),
            vec!["Resources".to_string(), "Scripts".to_string()]
        );

        let mut rebuilt = Vec::new();
        collect_paths(&tree, "", &mut rebuilt);
        rebuilt.sort();
        let mut expected: Vec<String> = entries.iter().map(|(p, _)| p.clone()).collect();
        expected.sort();
        assert_eq!(rebuilt, expected, "树里点到的路径必须与资产清单一致");
    }

    /// 递归收集（与 `render_node` 同一套拼接规则）。
    fn collect_paths(node: &DirNode, prefix: &str, out: &mut Vec<String>) {
        for (name, child) in &node.dirs {
            collect_paths(child, &join_dir(prefix, name), out);
        }
        for (file, _) in &node.files {
            out.push(join_path(prefix, file));
        }
    }

    /// 顶层文件（不在任何子目录）也要能拼对。
    #[test]
    fn top_level_files_have_no_prefix() {
        assert_eq!(join_path("", "Box.glb"), "Box.glb");
        assert_eq!(join_path("Resources/", "Box.glb"), "Resources/Box.glb");
        assert_eq!(join_dir("", "Resources"), "Resources/");
        assert_eq!(join_dir("Resources/", "Audio"), "Resources/Audio/");
    }
}
