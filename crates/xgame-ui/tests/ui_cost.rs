//! UI 布局 + 合批的 CPU 成本基准（默认 `#[ignore]`）。
//!
//! ```sh
//! cargo test -p xgame-ui --release --test ui_cost -- --ignored --nocapture
//! ```
//!
//! # 为什么这条基准必须存在
//!
//! UI 每帧都要重算：锚点布局、命中测试、文本排版、合批。它是**每帧恒定**
//! 的成本（不像 3D 有指纹门控可以整帧跳过），所以一旦退化成"每个元素都
//! 分配一次"或"每个字形都重新光栅化"，编辑器会稳定地慢下去。
//!
//! 三个数字各有含义：
//! - `build`：布局 + 合批（不含 GPU 上传）；
//! - `batches`：UI crate 侧的"命令段"数（矩形 + 每段文本各一段）。
//!   渲染器会再按纹理把相邻段合并成一次 draw（矩形与字形共用图集），
//!   所以真实 draw call 通常远小于这个数；
//! - `glyphs`：本帧的字形精灵数（文本排版的工作量）。
//!
//! 参考值（2026-10-20，本机 release）：
//! 900 元素 / 3190 字形 ≈ **0.75 ms/帧**（占 60fps 预算 4.5%），
//! 命令段 1200 段 → 渲染器合并成 **1 次 draw**（示例 HUD 实测也是 1）。

use std::time::Instant;

use xgame_core::{Entity, Scene, UiElement};
use xgame_math::glam::Vec2;
use xgame_ui::Painter;

/// 造一个"HUD 规模"的场景：`count` 个面板 + 同数量的标签 + 同数量的按钮。
fn hud_scene(count: usize) -> Scene {
    let mut scene = Scene::new("ui-cost");
    let mut x = 8.0f32;
    let mut y = 8.0f32;
    for i in 0..count {
        let mut panel = Entity::new(format!("Panel{i}"));
        panel.ui = Some(UiElement::panel(120.0, 28.0).at([0.0, 0.0], x, y, 120.0, 28.0));
        scene.entities.push(panel);

        let mut label = Entity::new(format!("Label{i}"));
        label.ui = Some(
            UiElement::text(format!("血量 {i} / 100"))
                .at([0.0, 0.0], x + 4.0, y + 2.0, 112.0, 24.0),
        );
        scene.entities.push(label);

        let mut button = Entity::new(format!("Button{i}"));
        button.ui = Some(
            UiElement::button("确定")
                .at([0.0, 0.0], x + 130.0, y, 80.0, 28.0),
        );
        scene.entities.push(button);

        x += 220.0;
        if x > 1100.0 {
            x = 8.0;
            y += 40.0;
        }
    }
    scene
}

#[test]
#[ignore = "基准：cargo test -p xgame-ui --release --test ui_cost -- --ignored --nocapture"]
fn hud_build_cost() {
    let count = 300;
    let scene = hud_scene(count);
    let viewport = Vec2::new(1280.0, 720.0);

    let mut painter = Painter::new();
    if !painter.text().has_font() {
        eprintln!("跳过：本机没有可用字体（文本成本无从测量）");
        return;
    }

    // 预热：首帧要光栅化全部字形（一次性成本，之后命中缓存）。
    let draw = painter.build(&scene, viewport);
    let elements = draw.element_count;
    let quads = draw.quads.len();
    let glyphs = draw.sprites.len();
    let draws = draw.commands.len();
    assert_eq!(elements, count * 3, "每个实体都该参与布局");

    let frames = 200;
    let start = Instant::now();
    let mut last = 0usize;
    for _ in 0..frames {
        let draw = painter.build(&scene, viewport);
        last = draw.sprites.len();
    }
    let elapsed = start.elapsed();
    let per_frame_us = elapsed.as_secs_f64() * 1e6 / frames as f64;

    println!(
        "ui build: {elements} elements / {quads} quads / {glyphs} glyphs / {draws} batches \
         → {per_frame_us:.1} µs/frame（占 60fps 预算 {:.2}%）",
        per_frame_us / (1e6 / 60.0) * 100.0
    );
    assert_eq!(last, glyphs, "稳态帧的字形数应与预热帧一致（缓存生效）");
    assert!(
        per_frame_us < 8000.0,
        "900 元素的 UI 每帧不该超过 8ms（实测 {per_frame_us:.1} µs）"
    );
}

/// 空场景快路径：没有 UI 实体时**一次布局都不该发生**。
#[test]
fn empty_scene_is_cheap() {
    let mut scene = Scene::new("empty");
    for i in 0..1000 {
        scene
            .entities
            .push(Entity::new(format!("Mesh{i}")).with_model("builtin://cube"));
    }
    let mut painter = Painter::new();
    let draw = painter.build(&scene, Vec2::new(1280.0, 720.0));
    assert_eq!(draw.element_count, 0);
    assert!(draw.commands.is_empty());
    assert!(draw.quads.is_empty());
}
