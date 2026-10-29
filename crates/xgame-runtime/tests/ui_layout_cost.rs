//! UI 布局与命中测试的**成本**（默认 `#[ignore]`，手动跑）。
//!
//! 这条基准回答一个问题：一张 900 元素的 HUD 每帧到底花多少时间，其中
//! "建树 / 解矩形 / 命中测试"各占多少。它不设阈值（不同机器差太多），
//! 只打印分段读数 —— 改动前后同机对比用。
//!
//! ```text
//! cargo test -p xgame-runtime --release --test ui_layout_cost -- --ignored --nocapture
//! ```

use std::time::Instant;

use xgame_core::{Entity, Scene, UiElement};
use xgame_math::glam::Vec2;
use xgame_runtime::{World, WorldConfig};

/// 900 个文本元素 + 100 个可点按钮（近似一个复杂 HUD）。
fn hud_scene() -> Scene {
    let mut scene = Scene::new("hud");
    let mut panel = Entity::new("panel");
    panel.ui = Some(UiElement::panel(1280.0, 720.0).at(
        [0.0, 0.0],
        0.0,
        0.0,
        1280.0,
        720.0,
    ));
    let panel_id = panel.id;
    scene.add_entity(panel);

    for i in 0..900 {
        let mut e = Entity::new(format!("label{i}"));
        e.parent = Some(panel_id);
        let ui = UiElement::text(format!("Score {:>6}", i * 7)).at(
            [0.0, 0.0],
            8.0 + (i % 30) as f32 * 42.0,
            8.0 + (i / 30) as f32 * 24.0,
            120.0,
            20.0,
        );
        e.ui = Some(ui);
        scene.add_entity(e);
    }
    for i in 0..100 {
        let mut e = Entity::new(format!("button{i}"));
        e.parent = Some(panel_id);
        let mut ui = UiElement::button(format!("Go {i}"));
        ui = ui.at(
            [0.0, 0.0],
            8.0 + (i % 10) as f32 * 120.0,
            600.0 + (i / 10) as f32 * 26.0,
            110.0,
            22.0,
        );
        e.ui = Some(ui);
        scene.add_entity(e);
    }
    scene
}

#[test]
#[ignore = "基准：手动跑，数据写进 PLAN"]
fn ui_layout_cost() {
    let root = std::env::temp_dir();
    let mut world = World::new(hud_scene(), &root, WorldConfig::default());
    world.set_ui_viewport((1280, 720));
    world.set_ui_pointer(Some(Vec2::new(640.0, 360.0)));

    // 预热：让脚本 / 物理 / 缓存进入稳态。
    for _ in 0..60 {
        world.update(1.0 / 60.0);
    }

    const FRAMES: usize = 600;
    let start = Instant::now();
    for _ in 0..FRAMES {
        world.update(1.0 / 60.0);
    }
    let total = start.elapsed();
    let per_frame = total.as_secs_f64() * 1e6 / FRAMES as f64;

    // 单独量"建树 + 解矩形"（把 UI 视口关掉会跳过整段，所以这里直接调布局 API）。
    let scene = world.scene().clone();
    let viewport = Vec2::new(1280.0, 720.0);
    let mut tree = xgame_ui::UiTree::new();
    let mut items = Vec::new();
    let rounds = 600;
    let build_start = Instant::now();
    for _ in 0..rounds {
        tree.build(&scene);
    }
    let build_us = build_start.elapsed().as_secs_f64() * 1e6 / rounds as f64;

    let layout_start = Instant::now();
    for _ in 0..rounds {
        items.clear();
        xgame_ui::layout::visit(&scene, &tree, viewport, &mut items);
    }
    let layout_us = layout_start.elapsed().as_secs_f64() * 1e6 / rounds as f64;

    println!(
        "UI: {} 元素 / {} 帧\n  整帧 world.update      {per_frame:8.1} µs\n  建树 UiTree::build     {build_us:8.1} µs\n  解矩形 layout::visit   {layout_us:8.1} µs\n  条目数 {}",
        scene.entities.len(),
        FRAMES,
        items.len()
    );
}
