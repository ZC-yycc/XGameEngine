//! 脚本侧 UI 布局 / 文本 / 样式 setter 与旧端语义对齐。
//!
//! 重点钉住三件事：
//! 1. `ui_set_size` 保持 `offset_min` 不动（血条 bug 的根因）；
//! 2. 同帧 `ui_set_size` + `ui_set_pos` 互相看见对方的本帧写入（顺序无关）；
//! 3. 颜色 sRGB→线性、字号 / 圆角钳制、对齐字符串映射、getter 同帧可见。

use xgc_world::{Name, Script, Transform, UiElement, World};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_ui_{label}_{}_{}",
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
    let path = dir.join("ui.lua");
    std::fs::write(&path, format!("function update(ctx) {body} end\n")).unwrap();
    path
}

fn spawn(world: &mut World) -> xgc_base::EntityKey {
    world.spawn((
        Transform::default(),
        Name {
            value: "bar".to_string(),
        },
        UiElement {
            offset_min: [2.0, 2.0],
            offset_max: [2.0, 2.0],
            ..Default::default()
        },
        Script {
            path: "ui.lua".to_string(),
        },
    ))
}

fn run(dir: &std::path::Path) -> (World, xgc_base::EntityKey, Option<String>) {
    let mut world = World::new();
    let key = spawn(&mut world);
    let mut runtime = xgc_script::ScriptRuntime::new(dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    let error = runtime.take_error();
    (world, key, error)
}

fn assert_close(actual: f32, expected: f32, label: &str) {
    assert!(
        (actual - expected).abs() < 1e-4,
        "{label}: 期望 {expected}，实际 {actual}"
    );
}

#[test]
fn ui_set_size_keeps_the_top_left_corner() {
    let dir = temp_dir("size");
    write_script(
        &dir,
        r#"local id = entity_id(ctx, "bar") ui_set_size(ctx, id, 48.17, 14.0)"#,
    );
    let (world, key, error) = run(&dir);
    assert!(error.is_none(), "脚本报错：{error:?}");
    let ui = world.get::<UiElement>(key).unwrap();
    assert_eq!(ui.offset_min, [2.0, 2.0], "左上角必须不动");
    assert_close(ui.offset_max[0], 50.17, "offset_max.x");
    assert_close(ui.offset_max[1], 16.0, "offset_max.y");
}

#[test]
fn same_frame_size_and_position_compose_in_either_order() {
    for (label, body) in [
        (
            "size_then_pos",
            r#"local id = entity_id(ctx, "bar") ui_set_size(ctx, id, 10.0, 6.0) ui_set_pos(ctx, id, 5.0, 7.0)"#,
        ),
        (
            "pos_then_size",
            r#"local id = entity_id(ctx, "bar") ui_set_pos(ctx, id, 5.0, 7.0) ui_set_size(ctx, id, 10.0, 6.0)"#,
        ),
    ] {
        let dir = temp_dir(label);
        write_script(&dir, body);
        let (world, key, error) = run(&dir);
        assert!(error.is_none(), "{label} 脚本报错：{error:?}");
        let ui = world.get::<UiElement>(key).unwrap();
        assert_close(ui.offset_min[0], 5.0, &format!("{label} min.x"));
        assert_close(ui.offset_min[1], 7.0, &format!("{label} min.y"));
        assert_close(ui.offset_max[0], 15.0, &format!("{label} max.x"));
        assert_close(ui.offset_max[1], 13.0, &format!("{label} max.y"));
    }
}

#[test]
fn text_style_and_state_setters_match_old_semantics() {
    let dir = temp_dir("style");
    write_script(
        &dir,
        r#"local id = entity_id(ctx, "bar")
            ui_set_text_size(ctx, id, 0.2)
            ui_set_radius(ctx, id, -5.0)
            ui_set_enabled(ctx, id, false)
            ui_set_visible(ctx, id, false)
            ui_set_text_color(ctx, id, 0.5, 0.0, 0.0, 1.0)
            ui_set_text_align(ctx, id, "Centre")
            ui_set_text_align(ctx, id, "bogus")"#,
    );
    let (world, key, error) = run(&dir);
    assert!(error.is_none(), "脚本报错：{error:?}");
    let ui = world.get::<UiElement>(key).unwrap();
    assert_close(ui.text_size, 1.0, "字号下限钳到 1");
    assert_close(ui.corner_radius, 0.0, "负圆角钳到 0");
    assert!(!ui.interactable, "禁用交互");
    assert!(!ui.visible, "隐藏");
    assert_close(ui.text_color[0], 0.214_041_14, "sRGB 0.5 → 线性");
    assert_eq!(ui.text_align, "center", "centre 映射 center，bogus 忽略");
}

#[test]
fn getters_see_same_frame_writes_and_absent_elements_are_no_ops() {
    let dir = temp_dir("getters");
    write_script(
        &dir,
        r#"local id = entity_id(ctx, "bar")
            if not ui_has(ctx, id) then error("ui_has") end
            ui_set_text(ctx, id, "Ping")
            if ui_text(ctx, id) ~= "Ping" then error("ui_text") end
            ui_set_visible(ctx, id, false)
            if ui_visible(ctx, id) ~= false then error("ui_visible") end
            ui_set_enabled(ctx, id, false)
            if ui_enabled(ctx, id) ~= false then error("ui_enabled") end
            local missing = 999999
            if ui_has(ctx, missing) ~= false then error("ui_has missing") end
            ui_set_size(ctx, missing, 10.0, 10.0)
            ui_set_text(ctx, missing, "ignored")"#,
    );
    let (world, key, error) = run(&dir);
    assert!(error.is_none(), "脚本报错：{error:?}");
    let ui = world.get::<UiElement>(key).unwrap();
    assert_eq!(ui.text, "Ping");
    assert!(!ui.visible);
    assert!(!ui.interactable);
}
