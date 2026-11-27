//! 旧端 `ui_spawn_*`（运行时创建 UI 实体）语义对齐。
//!
//! 旧端 `xgame-script/src/ui_ctx.rs`：
//! - `ui_ctx.ui_spawn` 直接建实体、发 `EntitySpawned`，返回 `id.raw()`；
//! - `ui_spawn_label(text,x,y,size)`：文本元素，宽 = `max(字数×size, size)`、高 = `size*1.4`；
//! - `ui_spawn_button(text,x,y,w,h)`：蓝底 + 1px 边框 + 圆角 6 + 内边距 6、文字居中、可交互；
//! - `ui_spawn_panel(x,y,w,h)`：深灰底 `rgba8(28,30,36,216)` + 内边距 6、不可交互；
//! - `ui_spawn_image(path,x,y,w,h)`：透明底、`keep_aspect = false`。
//!
//! XGC 侧走 `Command::Spawn`（帧末结算），`ScriptView::pending_spawns` 让同一帧就能
//! 接着调 setter / 焦点。这里既断言**同帧可见**（Lua `assert`），也断言结算后的世界字段。

use xgc_base::EntityKey;
use xgc_world::{
    GlobalTransform, Name, Script, Transform, UiElement, UiInteraction, UiLayout, World,
};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_spawn_{label}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("spawn.lua");
    std::fs::write(&path, format!("function update(ctx) {body} end\n")).unwrap();
    path
}

/// 只有一台驱动脚本的世界（没有预置 UI），跑 `frames` 帧。
fn run(body: &str, frames: usize) -> (World, Option<String>) {
    let dir = temp_dir("world");
    write_script(&dir, body);
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        Name {
            value: "driver".to_string(),
        },
        Script {
            paths: vec!["spawn.lua".into()],
        },
    ));
    let mut runtime = xgc_script::ScriptRuntime::new(&dir);
    for _ in 0..frames {
        runtime.run_frame(&mut world, 1.0 / 60.0);
    }
    let error = runtime.take_error();
    (world, error)
}

fn by_name(world: &World, name: &str) -> EntityKey {
    let mut found = None;
    world.for_each::<Name>(|key, value| {
        if value.value == name {
            found = Some(key);
        }
    });
    found.unwrap_or_else(|| panic!("世界里没有叫 {name} 的实体"))
}

fn element<'a>(world: &'a World, name: &str) -> &'a UiElement {
    world.get::<UiElement>(by_name(world, name)).unwrap()
}

#[test]
fn ui_spawn_creates_the_six_components_like_the_old_engine() {
    let (world, error) = run(
        r#"
        ui_spawn_label(ctx, "你好", 10, 20, 30)
        ui_spawn_button(ctx, "开始", 40, 50, 100, 30)
        ui_spawn_panel(ctx, 0, 0, 200, 120)
        ui_spawn_image(ctx, "Resources/Textures/bar.png", 8, 8, 64, 32)
        "#,
        1,
    );
    assert!(error.is_none(), "脚本报错：{error:?}");

    for name in ["Label", "Button", "Panel", "Image"] {
        let key = by_name(&world, name);
        for present in [
            world.has::<Transform>(key),
            world.has::<GlobalTransform>(key),
            world.has::<UiLayout>(key),
            world.has::<UiInteraction>(key),
            world.has::<UiElement>(key),
        ] {
            assert!(present, "{name} 缺组件");
        }
    }

    let label = element(&world, "Label");
    assert_eq!(label.kind, "text");
    assert_eq!(label.text, "你好");
    assert_eq!(label.text_size, 30.0);
    assert_eq!(label.offset_min, [10.0, 20.0]);
    assert_eq!(
        label.offset_max,
        [10.0 + 60.0, 20.0 + 42.0],
        "宽 = 字数×字号，高 = 字号×1.4"
    );
    assert!(!label.interactable, "旧端文本元素不可交互");

    let button = element(&world, "Button");
    assert_eq!(button.kind, "button");
    assert_eq!(button.text, "开始");
    assert!(button.interactable, "旧端按钮可交互");
    assert_eq!(button.border_width, 1.0);
    assert_eq!(button.corner_radius, 6.0);
    assert_eq!(button.padding, 6.0);
    assert_eq!(button.text_align, "center");
    assert_eq!(button.text_valign, "middle");

    let panel = element(&world, "Panel");
    assert_eq!(panel.kind, "panel");
    assert_eq!(panel.padding, 6.0);
    assert!(!panel.interactable);
    assert!(
        (panel.background[3] - 216.0 / 255.0).abs() < 1e-6,
        "面板底不透明度"
    );

    let image = element(&world, "Image");
    assert_eq!(image.kind, "image");
    assert_eq!(image.image_path, "Resources/Textures/bar.png");
    assert_eq!(
        image.image_mode, "stretch",
        "旧端 ui_spawn_image 固定 keep_aspect = false"
    );

    // 四个实体 + 一台驱动脚本。
    assert_eq!(world.len(), 5, "只该多出四个 UI 实体");
}

#[test]
fn spawned_elements_are_visible_to_setters_and_getters_in_the_same_frame() {
    let (world, error) = run(
        r#"
        local id = ui_spawn_button(ctx, "开始", 40, 50, 100, 30)
        assert(id > 0, "spawn 应返回有效 id")
        assert(ui_has(ctx, id), "同帧必须能查到刚生成的实体")
        assert(entity_id(ctx, "Button") == id, "同帧按名字也能查到")
        ui_set_text(ctx, id, "已改")
        assert(ui_text(ctx, id) == "已改", "同帧 setter 必须对新实体生效")
        local img = ui_spawn_image(ctx, "Resources/a.png", 0, 0, 32, 32)
        ui_set_image_tint(ctx, img, 1.0, 0.0, 0.0, 1.0)
        ui_set_fill(ctx, img, 0.5)
        "#,
        1,
    );
    assert!(error.is_none(), "脚本报错：{error:?}");
    assert_eq!(element(&world, "Button").text, "已改");
    let image = element(&world, "Image");
    assert!((image.image_fill - 0.5).abs() < 1e-6);
    assert!(
        image.image_tint[0] > 0.9 && image.image_tint[1] < 0.01,
        "红色 tint 的线性值"
    );
}

#[test]
fn a_spawned_button_can_take_focus_on_the_same_frame() {
    let (world, error) = run(
        r#"
        local id = ui_spawn_button(ctx, "开始", 40, 50, 100, 30)
        ui_set_focus(ctx, id)
        assert(ui_focused(ctx, id), "同帧设置焦点后立即可读")
        "#,
        1,
    );
    assert!(error.is_none(), "脚本报错：{error:?}");
    let interaction = world
        .get::<UiInteraction>(by_name(&world, "Button"))
        .unwrap();
    assert!(interaction.focused, "帧末结算后组件上的焦点为真");
}

#[test]
fn spawned_entity_ids_are_distinct_and_reproducible() {
    let body = r#"
        local a = ui_spawn_label(ctx, "A", 0, 0, 20)
        local b = ui_spawn_label(ctx, "B", 0, 40, 20)
        assert(a > 0 and b > 0 and a ~= b, "两次 spawn 的 id 必须不同且有效")
        "#;
    let (first, error) = run(body, 1);
    assert!(error.is_none(), "脚本报错：{error:?}");
    let (second, error) = run(body, 1);
    assert!(error.is_none(), "脚本报错：{error:?}");

    let ids = |world: &World| -> Vec<u64> {
        let mut ids = Vec::new();
        world.for_each::<UiElement>(|key, _| {
            if let Some(id) = world.entity(key) {
                ids.push(id.raw());
            }
        });
        ids.sort_unstable();
        ids
    };
    assert_eq!(
        ids(&first),
        ids(&second),
        "同输入两次运行的 id 序列必须一致"
    );
    assert_eq!(ids(&first).len(), 2);
    assert_eq!(first.len(), 3, "驱动 + 两个标签");
    assert_ne!(ids(&first)[0], ids(&first)[1]);
}

#[test]
fn spawned_entities_enter_the_next_frame_snapshot() {
    let (world, error) = run(
        r#"
        if time(ctx) < 0.02 then
            ui_spawn_panel(ctx, 0, 0, 200, 120)
        else
            local id = entity_id(ctx, "Panel")
            assert(id > 0, "下一帧快照里必须有生成的实体")
            assert(ui_has(ctx, id), "下一帧仍能按 id 查到")
        end
        "#,
        2,
    );
    assert!(error.is_none(), "脚本报错：{error:?}");
    assert_eq!(world.len(), 2, "驱动 + 一个面板");
}
