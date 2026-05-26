//! 类 Unity 暗色编辑器主题。
//!
//! 在启动时（或每帧）调用 `install(ctx)` 以应用深色背景、蓝色强调色和锐利边角，
//! 仿照 Unity 编辑器的配色方案。

use egui::Color32;

/// Unity dark theme colour constants.
pub mod colors {
    use egui::Color32;

    pub const WINDOW_BG: Color32 = Color32::from_rgb(26, 26, 26);
    pub const PANEL_BG: Color32 = Color32::from_rgb(34, 34, 34);
    pub const PANEL_HEADER_BG: Color32 = Color32::from_rgb(40, 40, 40);
    pub const INPUT_BG: Color32 = Color32::from_rgb(48, 48, 48);
    pub const BUTTON_BG: Color32 = Color32::from_rgb(55, 55, 55);
    pub const BUTTON_HOVER: Color32 = Color32::from_rgb(70, 70, 70);
    pub const BUTTON_ACTIVE: Color32 = Color32::from_rgb(85, 85, 85);

    pub const ACCENT_BLUE: Color32 = Color32::from_rgb(0, 122, 204);
    pub const ACCENT_LIGHT: Color32 = Color32::from_rgb(64, 150, 220);
    pub const SELECTION_BG: Color32 = Color32::from_rgb(30, 80, 120);

    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(200, 200, 200);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(140, 140, 140);
    pub const TEXT_ACCENT: Color32 = Color32::from_rgb(77, 156, 230);

    pub const SEPARATOR: Color32 = Color32::from_rgb(50, 50, 50);
}

/// Build the Unity-dark egui `Visuals`.
pub fn unity_dark_visuals() -> egui::Visuals {
    use colors::*;

    egui::Visuals {
        dark_mode: true,

        window_rounding: egui::Rounding::ZERO,
        window_fill: WINDOW_BG,
        window_stroke: egui::Stroke::new(1.0, SEPARATOR),

        panel_fill: PANEL_BG,
        faint_bg_color: PANEL_HEADER_BG,
        extreme_bg_color: Color32::from_rgb(20, 20, 20),
        code_bg_color: Color32::from_rgb(30, 30, 35),

        override_text_color: Some(TEXT_PRIMARY),
        hyperlink_color: TEXT_ACCENT,
        warn_fg_color: Color32::YELLOW,
        error_fg_color: Color32::RED,

        selection: egui::style::Selection {
            bg_fill: SELECTION_BG,
            stroke: egui::Stroke::new(1.0, ACCENT_BLUE),
        },

        widgets: egui::style::Widgets {
            noninteractive: egui::style::WidgetVisuals {
                bg_fill: PANEL_BG,
                weak_bg_fill: INPUT_BG,
                bg_stroke: egui::Stroke::new(1.0, SEPARATOR),
                fg_stroke: egui::Stroke::new(1.0, TEXT_SECONDARY),
                rounding: egui::Rounding::ZERO,
                expansion: 0.0,
            },
            inactive: egui::style::WidgetVisuals {
                bg_fill: BUTTON_BG,
                weak_bg_fill: INPUT_BG,
                bg_stroke: egui::Stroke::new(1.0, SEPARATOR),
                fg_stroke: egui::Stroke::new(1.5, TEXT_PRIMARY),
                rounding: egui::Rounding::ZERO,
                expansion: 0.0,
            },
            hovered: egui::style::WidgetVisuals {
                bg_fill: BUTTON_HOVER,
                weak_bg_fill: Color32::from_rgb(50, 50, 60),
                bg_stroke: egui::Stroke::new(1.0, ACCENT_BLUE),
                fg_stroke: egui::Stroke::new(2.0, TEXT_PRIMARY),
                rounding: egui::Rounding::ZERO,
                expansion: 1.0,
            },
            active: egui::style::WidgetVisuals {
                bg_fill: ACCENT_BLUE,
                weak_bg_fill: SELECTION_BG,
                bg_stroke: egui::Stroke::new(1.0, ACCENT_LIGHT),
                fg_stroke: egui::Stroke::new(2.0, Color32::WHITE),
                rounding: egui::Rounding::ZERO,
                expansion: 1.0,
            },
            open: egui::style::WidgetVisuals {
                bg_fill: BUTTON_ACTIVE,
                weak_bg_fill: INPUT_BG,
                bg_stroke: egui::Stroke::new(1.0, ACCENT_BLUE),
                fg_stroke: egui::Stroke::new(2.0, TEXT_PRIMARY),
                rounding: egui::Rounding::ZERO,
                expansion: 0.0,
            },
        },

        ..Default::default()
    }
}

/// Apply the Unity dark theme to the egui context.
/// Safe to call every frame.
pub fn install(ctx: &egui::Context) {
    let style = egui::Style {
        spacing: egui::style::Spacing {
            item_spacing: egui::Vec2::new(4.0, 2.0),
            button_padding: egui::Vec2::new(6.0, 2.0),
            indent: 12.0,
            ..Default::default()
        },
        interaction: egui::style::Interaction {
            resize_grab_radius_side: 4.0,
            resize_grab_radius_corner: 6.0,
            ..Default::default()
        },
        visuals: unity_dark_visuals(),
        ..Default::default()
    };

    ctx.set_style(style);
}
