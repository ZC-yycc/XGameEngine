//! UI 运行时端到端测试：指针 → 布局 / 命中 → `UiState` → Lua 脚本。
//!
//! 这里刻意走**真脚本**（临时目录里的 `.lua`），而不是直接构造 `GameCtx`：
//! 上一轮的角色控制器测试证明，只测方法层会漏掉"转发壳没注册 / 名字拼错"
//! 这类错误 —— 脚本里能写出来才算 API 真的存在。

use std::path::{Path, PathBuf};

use xgame_core::{Entity, Scene, UiElement};
use xgame_input::MouseButton;
use xgame_math::glam::Vec2;
use xgame_runtime::{World, WorldConfig};

/// 临时项目根（脚本按 `<root>/Scripts/...` 解析）。
fn temp_root(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "xgame_ui_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("Scripts")).expect("建临时项目目录");
    base
}

fn write_script(root: &Path, name: &str, body: &str) {
    std::fs::write(root.join("Scripts").join(name), body).expect("写脚本");
}

/// 一帧"按下"，一帧"松开"。
fn click_at(world: &mut World, pos: Vec2) {
    world.set_ui_pointer(Some(pos));
    world
        .input_mut()
        .state_mut()
        .handle_mouse_button(MouseButton::Left, true);
    world.update(1.0 / 60.0);

    world
        .input_mut()
        .state_mut()
        .handle_mouse_button(MouseButton::Left, false);
    world.update(1.0 / 60.0);
}

fn button_scene(script: &str) -> (Scene, xgame_core::EntityId) {
    let mut scene = Scene::new("ui-runtime");
    let mut button = Entity::new("button");
    button.ui = Some(UiElement::button("OK").at([0.0, 0.0], 10.0, 10.0, 120.0, 40.0));
    button.scripts.push(script.to_owned());
    let id = button.id;
    scene.add_entity(button);
    (scene, id)
}

fn text_of(world: &World, id: xgame_core::EntityId) -> String {
    let index = world.scene().find_entity(id).expect("按钮还在");
    world.scene().entities[index]
        .ui
        .as_ref()
        .expect("还是 UI 元素")
        .text
        .content
        .clone()
}

/// 完整链路：指针落在按钮上 → 按下 → 松开 → 脚本收到 `ui_clicked`。
#[test]
fn lua_script_sees_a_click() {
    let root = temp_root("click");
    write_script(
        &root,
        "Click.lua",
        r#"
function update(ctx)
    if ui_clicked(ctx, self_id(ctx)) then
        ui_set_text(ctx, self_id(ctx), "clicked")
    end
end
"#,
    );
    let (scene, id) = button_scene("Scripts/Click.lua");
    let mut world = World::new(scene, root.clone(), WorldConfig::default());
    world.set_ui_viewport((800, 600));

    // 第一帧：仅仅悬停，不该算点击。
    world.set_ui_pointer(Some(Vec2::new(50.0, 30.0)));
    world.update(1.0 / 60.0);
    assert_eq!(text_of(&world, id), "OK", "只悬停不该触发点击");

    click_at(&mut world, Vec2::new(50.0, 30.0));
    assert_eq!(text_of(&world, id), "clicked");

    let errors: Vec<_> = world
        .take_logs()
        .into_iter()
        .filter(|log| matches!(log.level, xgame_script::LogLevel::Error))
        .collect();
    assert!(errors.is_empty(), "脚本不该报错: {errors:?}");

    let _ = std::fs::remove_dir_all(&root);
}

/// 点在按钮外：不触发。
#[test]
fn click_outside_does_not_fire() {
    let root = temp_root("outside");
    write_script(
        &root,
        "Click.lua",
        r#"
function update(ctx)
    if ui_clicked(ctx, self_id(ctx)) then
        ui_set_text(ctx, self_id(ctx), "clicked")
    end
end
"#,
    );
    let (scene, id) = button_scene("Scripts/Click.lua");
    let mut world = World::new(scene, root.clone(), WorldConfig::default());
    world.set_ui_viewport((800, 600));

    click_at(&mut world, Vec2::new(600.0, 400.0));
    assert_eq!(text_of(&world, id), "OK");

    let _ = std::fs::remove_dir_all(&root);
}

/// 没有视口（无头运行 / 编辑器未上屏）时整段 UI 交互不跑 ——
/// 既不算点击，也不会因为"指针在 (0,0)"误触左上角的元素。
#[test]
fn no_viewport_disables_interaction() {
    let root = temp_root("novp");
    write_script(
        &root,
        "Click.lua",
        r#"
function update(ctx)
    if ui_clicked(ctx, self_id(ctx)) then
        ui_set_text(ctx, self_id(ctx), "clicked")
    end
end
"#,
    );
    let (scene, id) = button_scene("Scripts/Click.lua");
    let mut world = World::new(scene, root.clone(), WorldConfig::default());
    // 刻意不设置视口。
    world.set_ui_pointer(Some(Vec2::new(50.0, 30.0)));

    world
        .input_mut()
        .state_mut()
        .handle_mouse_button(MouseButton::Left, true);
    world.update(1.0 / 60.0);
    world
        .input_mut()
        .state_mut()
        .handle_mouse_button(MouseButton::Left, false);
    world.update(1.0 / 60.0);

    assert_eq!(text_of(&world, id), "OK");

    let _ = std::fs::remove_dir_all(&root);
}

/// 脚本动态生成的 UI 元素下一帧就能被布局 / 命中（`ui_spawn_button`）。
#[test]
fn script_spawned_ui_becomes_hittable() {
    let root = temp_root("spawn");
    write_script(
        &root,
        "Spawn.lua",
        r#"
local spawned = 0

function start(ctx)
    if self_name(ctx) == "spawner" then
        spawned = ui_spawn_button(ctx, "OK", 10.0, 10.0, 120.0, 40.0)
    end
end

function update(ctx)
    if self_name(ctx) == "spawner" then
        -- 生成之后把坐标写进日志，测试靠它确认 id 有效。
        if spawned ~= 0 then
            log(ctx, "spawned " .. spawned)
            spawned = 0
        end
    end
end
"#,
    );
    let mut scene = Scene::new("spawn");
    let mut spawner = Entity::new("spawner");
    spawner.scripts.push("Scripts/Spawn.lua".to_owned());
    scene.add_entity(spawner);

    let mut world = World::new(scene, root.clone(), WorldConfig::default());
    world.set_ui_viewport((800, 600));
    world.set_ui_pointer(Some(Vec2::new(400.0, 300.0)));
    world.update(1.0 / 60.0);

    let spawned = world
        .scene()
        .entities
        .iter()
        .find(|entity| entity.name == "Button")
        .map(|entity| entity.id)
        .expect("脚本应当生成了一个按钮");
    // 第二帧：布局把新元素算进去，点击应当能命中。
    click_at(&mut world, Vec2::new(30.0, 20.0));
    let index = world.scene().find_entity(spawned).expect("按钮还在");
    assert!(
        world.scene().entities[index]
            .ui
            .as_ref()
            .expect("UI 元素")
            .state
            .clicked,
        "运行时生成的 UI 元素应当可点"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// 隐藏的 UI 不吃点击（`visible = false` 整棵子树消失）。
#[test]
fn hidden_element_is_not_clickable() {
    let root = temp_root("hidden");
    write_script(
        &root,
        "Click.lua",
        r#"
function update(ctx)
    if ui_clicked(ctx, self_id(ctx)) then
        ui_set_text(ctx, self_id(ctx), "clicked")
    end
end
"#,
    );
    let (mut scene, id) = button_scene("Scripts/Click.lua");
    if let Some(ui) = scene.entities[0].ui.as_mut() {
        ui.visible = false;
    }
    let mut world = World::new(scene, root.clone(), WorldConfig::default());
    world.set_ui_viewport((800, 600));

    click_at(&mut world, Vec2::new(50.0, 30.0));
    assert_eq!(text_of(&world, id), "OK");

    let _ = std::fs::remove_dir_all(&root);
}

/// 在视口外松手：捕获必须收尾，按钮不能卡在按下态。
///
/// 这就是 `UiPointer` 把"位置"与"按键"分开的直接理由 —— 旧实现用
/// `pointer = None` 表示"指针不在视口"，于是那一帧的 `released` 边沿被整条
/// 丢掉，捕获永远留在按钮上：玩家把鼠标拖出视口松手，回来时按钮还亮着。
#[test]
fn releasing_outside_the_viewport_clears_the_button() {
    let mut scene = Scene::new("ui-runtime");
    let mut button = Entity::new("button");
    button.ui = Some(UiElement::button("OK").at([0.0, 0.0], 10.0, 10.0, 120.0, 40.0));
    let id = button.id;
    scene.add_entity(button);

    let root = temp_root("release-outside");
    let mut world = World::new(scene, root.clone(), WorldConfig::default());
    world.set_ui_viewport((800, 600));

    let pressed = |world: &World| -> bool {
        let index = world.scene().find_entity(id).expect("按钮还在");
        world.scene().entities[index]
            .ui
            .as_ref()
            .expect("还是 UI 元素")
            .state
            .pressed
    };

    // 在按钮上按下。
    world.set_ui_pointer(Some(Vec2::new(50.0, 30.0)));
    world
        .input_mut()
        .state_mut()
        .handle_mouse_button(MouseButton::Left, true);
    world.update(1.0 / 60.0);
    assert!(pressed(&world), "按住时按钮应显示按下");

    // 指针移出视口（鼠标去点编辑器别的面板），然后在视口外松手。
    world.set_ui_pointer(None);
    world
        .input_mut()
        .state_mut()
        .handle_mouse_button(MouseButton::Left, false);
    world.update(1.0 / 60.0);
    assert!(
        world.ui_pressed().is_none(),
        "视口外松手那一帧就必须清掉捕获，而不是等指针回到视口再兜底"
    );

    // 回到视口内、不再按着。
    world.set_ui_pointer(Some(Vec2::new(50.0, 30.0)));
    world.update(1.0 / 60.0);
    assert!(!pressed(&world), "视口外松手后不该再显示按下");

    let _ = std::fs::remove_dir_all(&root);
}
