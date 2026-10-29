//! Inspector 的「UI」区块：屏幕空间 UI 元素的锚点 / 样式 / 文本 / 图片。
//!
//! # 为什么单独一个文件
//!
//! `inspector.rs` 已经有一千八百多行（Transform / Mesh / Animation / Scripts /
//! Behaviors / Physics / Character / Audio / Material / Parent / Prefab）。
//! UI 区块有二十来行控件，再塞进去只会让那个文件更难读。这里复用
//! `theme` 的排版件（`section_bar` / `field_row` / `drag_field_w`），
//! 与其余区块的**视觉规则完全一致**（行高、标签列宽、数值框宽度）。
//!
//! # 锚点预设
//!
//! 手填四个归一化坐标 + 四个偏移太反人类。这里给一个预设下拉：选中即套用
//! 锚点并**保持当前尺寸**（居中的预设会把偏移重算成 `±size/2`）。手填仍然
//! 保留 —— 预设覆盖不到的组合（比如"距右边缘 20px、垂直拉伸"）还得靠数值。

use xgame_core::{Entity, UiAlign, UiElement, UiKind, UiVAlign};

use crate::panels::theme;

/// 带范围与步长的数值框（`theme::drag_field_w` 没有 range 参数）。
///
/// Inspector 里凡是"物理上不可能为负"的量（半径 / 边框 / 内边距 / 字号）
/// 都要给范围：拖成负数不会崩，但会得到"圆角内凹""字号 0"这类诡异画面。
fn drag(
    ui: &mut egui::Ui,
    width: f32,
    value: &mut f32,
    speed: f64,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    ui.add_sized(
        egui::vec2(width, theme::FIELD_HEIGHT),
        egui::DragValue::new(value).speed(speed).range(range),
    )
    .changed()
}

/// 与 Inspector 其余非向量行对齐（补掉轴标那一截）。
fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    theme::field_row(ui, label, |ui| {
        ui.add_space(theme::AXIS_TAG_WIDTH + ui.spacing().item_spacing.x);
        add(ui)
    })
}

/// 画 UI 区块。`touched` 由调用方汇总成"这一帧改过东西"。
pub fn show(ui: &mut egui::Ui, entity: &mut Entity, touched: &mut bool) {
    theme::section_bar(ui, "UI");

    if entity.ui.is_none() {
        ui.indent("ui_empty", |ui| {
            if ui
                .button("Add UI Element")
                .on_hover_text("给这个实体挂一个屏幕空间 UI 元素（锚点布局，忽略 Transform）")
                .clicked()
            {
                entity.ui = Some(UiElement::panel(320.0, 180.0));
                *touched = true;
            }
            ui.label(theme::dim_note(
                "UI 元素的位置由锚点决定；父子关系决定绘制顺序。",
            ));
        });
        return;
    }

    ui.indent("ui_section", |ui| {
        let element = entity.ui.as_mut().expect("刚判过 Some");
        let mut changed = false;

        // ── 类型 ──
        row(ui, "Type", |ui| {
            egui::ComboBox::from_id_salt("ui_kind")
                .width(ui.available_width().min(theme::FIELD_WIDTH * 2.0))
                .selected_text(element.kind.label())
                .show_ui(ui, |ui| {
                    for kind in UiKind::ALL {
                        if ui
                            .selectable_value(&mut element.kind, kind, kind.label())
                            .clicked()
                        {
                            changed = true;
                        }
                    }
                });
        });

        // ── 显隐与交互 ──
        row(ui, "Visible", |ui| {
            changed |= ui.checkbox(&mut element.visible, "").changed();
        });
        row(ui, "Enabled", |ui| {
            changed |= ui
                .checkbox(&mut element.interactable, "")
                .on_hover_text("关掉 = 不接受点击（按钮会显示为半透明禁用态）")
                .changed();
        });
        row(ui, "Clip", |ui| {
            changed |= ui
                .checkbox(&mut element.clip_children, "")
                .on_hover_text("把后代裁到本元素矩形内（自身完整绘制；命中测试同样生效）")
                .changed();
        });

        ui.add_space(theme::PANEL_MARGIN);

        // ── 锚点预设 ──
        row(ui, "Preset", |ui| {
            egui::ComboBox::from_id_salt("ui_anchor_preset")
                .width(ui.available_width().min(theme::FIELD_WIDTH * 2.5))
                .selected_text("Apply…")
                .show_ui(ui, |ui| {
                    for preset in AnchorPreset::ALL {
                        if ui.selectable_label(false, preset.label()).clicked() {
                            preset.apply(element);
                            changed = true;
                        }
                    }
                });
        });

        let field_w = theme::axis_field_width(ui);
        // 标签列只有 58px，`Anchor Min` / `Offset Max` 这类长标签会被截断成
        // `Anchor ...`（实测截图里四个标签全是省略号）。改成"分组说明 + min/max"：
        // 说明行走 `dim_note`，两行的标签就是 `Min` / `Max`，短且不重复。
        ui.label(theme::dim_note("Anchor（0..1，父矩形）"));
        row(ui, "Min", |ui| {
            changed |=
                theme::drag_field_w(ui, field_w, &mut element.anchor_min[0], 0.01, "").changed();
            changed |=
                theme::drag_field_w(ui, field_w, &mut element.anchor_min[1], 0.01, "").changed();
        });
        row(ui, "Max", |ui| {
            changed |=
                theme::drag_field_w(ui, field_w, &mut element.anchor_max[0], 0.01, "").changed();
            changed |=
                theme::drag_field_w(ui, field_w, &mut element.anchor_max[1], 0.01, "").changed();
        });
        ui.label(theme::dim_note("Offset（像素）"));
        row(ui, "Min", |ui| {
            changed |=
                theme::drag_field_w(ui, field_w, &mut element.offset_min[0], 1.0, "").changed();
            changed |=
                theme::drag_field_w(ui, field_w, &mut element.offset_min[1], 1.0, "").changed();
        });
        row(ui, "Max", |ui| {
            changed |=
                theme::drag_field_w(ui, field_w, &mut element.offset_max[0], 1.0, "").changed();
            changed |=
                theme::drag_field_w(ui, field_w, &mut element.offset_max[1], 1.0, "").changed();
        });
        row(ui, "Size", |ui| {
            let size = element.size();
            ui.label(
                egui::RichText::new(format!("{:.0} × {:.0}", size[0], size[1]))
                    .color(theme::colors::TEXT_DIM)
                    .size(11.0),
            );
        });

        ui.add_space(theme::PANEL_MARGIN);

        // ── 背景 / 边框 ──
        //
        // 标签列 58px：`Background` / `Border Color` 这类长标签会被截成省略号
        // （实测过）。这里统一用 4 字以内的短标签，语义由分组说明承担。
        ui.label(theme::dim_note("Style（线性）"));
        row(ui, "Bg", |ui| {
            changed |= color_edit(ui, &mut element.style.background);
        });
        row(ui, "Radius", |ui| {
            changed |= drag(
                ui,
                theme::FIELD_WIDTH,
                &mut element.style.corner_radius,
                0.5,
                0.0..=512.0,
            );
        });
        row(ui, "Width", |ui| {
            changed |= drag(
                ui,
                theme::FIELD_WIDTH,
                &mut element.style.border_width,
                0.1,
                0.0..=64.0,
            );
        });
        row(ui, "Border", |ui| {
            changed |= color_edit(ui, &mut element.style.border_color);
        });
        row(ui, "Padding", |ui| {
            changed |= drag(
                ui,
                theme::FIELD_WIDTH,
                &mut element.style.padding,
                0.5,
                0.0..=256.0,
            );
        });

        ui.add_space(theme::PANEL_MARGIN);

        // ── 文本 ──
        ui.label(theme::dim_note("Text"));
        row(ui, "Content", |ui| {
            let resp = ui.add_sized(
                egui::vec2(ui.available_width(), theme::FIELD_HEIGHT + 2.0),
                egui::TextEdit::singleline(&mut element.text.content),
            );
            changed |= resp.changed();
        });
        row(ui, "Size", |ui| {
            changed |= drag(
                ui,
                theme::FIELD_WIDTH,
                &mut element.text.size,
                0.5,
                1.0..=512.0,
            );
        });
        row(ui, "Color", |ui| {
            changed |= color_edit(ui, &mut element.text.color);
        });
        row(ui, "Align", |ui| {
            egui::ComboBox::from_id_salt("ui_align")
                .width(theme::FIELD_WIDTH)
                .selected_text(element.text.align.label())
                .show_ui(ui, |ui| {
                    for align in [UiAlign::Left, UiAlign::Center, UiAlign::Right] {
                        if ui
                            .selectable_value(&mut element.text.align, align, align.label())
                            .clicked()
                        {
                            changed = true;
                        }
                    }
                });
        });
        row(ui, "V Align", |ui| {
            egui::ComboBox::from_id_salt("ui_valign")
                .width(theme::FIELD_WIDTH)
                .selected_text(element.text.valign.label())
                .show_ui(ui, |ui| {
                    for valign in [UiVAlign::Top, UiVAlign::Middle, UiVAlign::Bottom] {
                        if ui
                            .selectable_value(&mut element.text.valign, valign, valign.label())
                            .clicked()
                        {
                            changed = true;
                        }
                    }
                });
        });
        row(ui, "Flags", |ui| {
            changed |= ui.checkbox(&mut element.text.wrap, "Wrap").changed();
            changed |= ui.checkbox(&mut element.text.bold, "Bold").changed();
        });

        ui.add_space(theme::PANEL_MARGIN);

        // ── 图片 ──
        let has_image = element.image.is_some();
        ui.label(theme::dim_note("Image"));
        row(ui, "Image", |ui| {
            let mut enabled = has_image;
            if ui
                .checkbox(&mut enabled, "")
                .on_hover_text("勾上即给元素加一张图片（血条 / 图标 / 背景图）")
                .changed()
            {
                element.image = if enabled {
                    Some(xgame_core::UiImageStyle::default())
                } else {
                    None
                };
                changed = true;
            }
        });
        if let Some(image) = element.image.as_mut() {
            row(ui, "Path", |ui| {
                let resp = ui.add_sized(
                    egui::vec2(ui.available_width(), theme::FIELD_HEIGHT + 2.0),
                    egui::TextEdit::singleline(&mut image.path).hint_text("Resources/xxx.png"),
                );
                changed |= resp.changed();
            });
            row(ui, "Tint", |ui| {
                changed |= color_edit(ui, &mut image.tint);
            });
            row(ui, "Fill", |ui| {
                changed |= ui
                    .add_sized(
                        egui::vec2(ui.available_width(), theme::FIELD_HEIGHT),
                        egui::Slider::new(&mut image.fill, 0.0..=1.0).show_value(true),
                    )
                    .on_hover_text("从左往右填充比例：血条 / 进度条直接用它")
                    .changed();
            });
            row(ui, "Aspect", |ui| {
                changed |= ui.checkbox(&mut image.keep_aspect, "").changed();
            });
        }

        ui.add_space(theme::PANEL_MARGIN);

        // ── 运行态（只读）──
        //
        // 摆 UI 的时候最常问的是"为什么点不到"：多半是 interactable 关了、
        // 或者被更上层的元素盖住（hovered 落在别人身上）。把状态摆出来比
        // 让作者去猜便宜得多。
        let state = element.state;
        ui.label(theme::dim_note(format!(
            "hovered: {}   pressed: {}   clicked: {}",
            state.hovered, state.pressed, state.clicked
        )));

        *touched |= changed;
    });
}

/// 颜色编辑。
///
/// `color_edit_button_rgba_unmultiplied` 收的就是**线性**非预乘 RGBA
/// （egui 内部自己转 sRGB 显示）—— 与 Material 区块直接传 `base_color`
/// 一致。别在这里再套一次 `to_srgb()`：那会让色块比实际画面亮一大截，
/// 作者按色块调出来的颜色上屏就变暗。
fn color_edit(ui: &mut egui::Ui, color: &mut [f32; 4]) -> bool {
    ui.color_edit_button_rgba_unmultiplied(color).changed()
}

/// 锚点预设：选中即套用，**保持当前尺寸**。
///
/// "保持尺寸"是这里唯一有点绕的地方：锚点从 (0,0) 换到 (0.5,0.5) 之后，
/// 偏移的含义从"距左上角"变成"距中心"，不重算偏移元素就会跳走半个屏幕。
#[derive(Clone, Copy, PartialEq, Eq)]
enum AnchorPreset {
    TopLeft,
    TopCenter,
    TopRight,
    Center,
    BottomLeft,
    BottomRight,
    Stretch,
}

impl AnchorPreset {
    const ALL: [AnchorPreset; 7] = [
        AnchorPreset::TopLeft,
        AnchorPreset::TopCenter,
        AnchorPreset::TopRight,
        AnchorPreset::Center,
        AnchorPreset::BottomLeft,
        AnchorPreset::BottomRight,
        AnchorPreset::Stretch,
    ];

    fn label(self) -> &'static str {
        match self {
            AnchorPreset::TopLeft => "Top Left",
            AnchorPreset::TopCenter => "Top Center",
            AnchorPreset::TopRight => "Top Right",
            AnchorPreset::Center => "Center",
            AnchorPreset::BottomLeft => "Bottom Left",
            AnchorPreset::BottomRight => "Bottom Right",
            AnchorPreset::Stretch => "Stretch (fill parent)",
        }
    }

    fn apply(self, element: &mut UiElement) {
        let size = element.size();
        let (anchor, offset) = match self {
            AnchorPreset::TopLeft => ([0.0, 0.0], [0.0, 0.0]),
            AnchorPreset::TopCenter => ([0.5, 0.0], [-size[0] * 0.5, 0.0]),
            AnchorPreset::TopRight => ([1.0, 0.0], [-size[0], 0.0]),
            AnchorPreset::Center => ([0.5, 0.5], [-size[0] * 0.5, -size[1] * 0.5]),
            AnchorPreset::BottomLeft => ([0.0, 1.0], [0.0, -size[1]]),
            AnchorPreset::BottomRight => ([1.0, 1.0], [-size[0], -size[1]]),
            AnchorPreset::Stretch => {
                element.anchor_min = [0.0, 0.0];
                element.anchor_max = [1.0, 1.0];
                let inset = element.offset_min;
                element.offset_min = [inset[0], inset[1]];
                element.offset_max = [-inset[0], -inset[1]];
                return;
            }
        };
        element.anchor_min = anchor;
        element.anchor_max = anchor;
        element.offset_min = offset;
        element.offset_max = [offset[0] + size[0], offset[1] + size[1]];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 预设换锚点时必须保持元素当前尺寸，否则"点一下预设，元素就变形了"。
    #[test]
    fn anchor_presets_keep_size() {
        let mut element = UiElement::panel(1.0, 1.0).at([0.0, 0.0], 12.0, 34.0, 200.0, 60.0);
        for preset in AnchorPreset::ALL {
            if preset == AnchorPreset::Stretch {
                continue;
            }
            preset.apply(&mut element);
            assert_eq!(
                element.size(),
                [200.0, 60.0],
                "{} 改动了尺寸",
                preset.label()
            );
        }
    }

    /// Center 预设：锚点 (0.5,0.5)、偏移 ±size/2 —— 元素中心落在父矩形中心。
    #[test]
    fn center_preset_centers_offsets() {
        let mut element = UiElement::panel(1.0, 1.0).at([0.0, 0.0], 0.0, 0.0, 200.0, 60.0);
        AnchorPreset::Center.apply(&mut element);
        assert_eq!(element.anchor_min, [0.5, 0.5]);
        assert_eq!(element.offset_min, [-100.0, -30.0]);
        assert_eq!(element.offset_max, [100.0, 30.0]);
    }

    /// Stretch 预设：四边拉伸、偏移取当前内边距的对称值。
    #[test]
    fn stretch_preset_pins_all_sides() {
        let mut element = UiElement::panel(1.0, 1.0).with_offset([8.0, 8.0], [8.0, 8.0]);
        AnchorPreset::Stretch.apply(&mut element);
        assert_eq!(element.anchor_min, [0.0, 0.0]);
        assert_eq!(element.anchor_max, [1.0, 1.0]);
        assert_eq!(element.offset_min, [8.0, 8.0]);
        assert_eq!(element.offset_max, [-8.0, -8.0]);
    }
}
