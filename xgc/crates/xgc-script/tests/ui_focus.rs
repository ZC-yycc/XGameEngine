//! 焦点三件套（`ui_focused` / `ui_set_focus` / `ui_clear_focus`）与旧端语义对齐。
//!
//! 旧端 `crates/xgame-script/src/ui_ctx.rs`：
//! - `ui_set_focus` 只认 `kind == Button && visible && interactable`，否则彻底 no-op；
//! - 成功时清其它元素、置目标，**同帧**就能被 `ui_focused` 读到 `true`；
//! - `ui_clear_focus` 清所有焦点标志。
//!
//! XGC 侧写 `ui.interaction.focused`（comp `0x0603`），每个 UI 实体一条 `SetField`
//! 随脚本命令在帧末统一结算；这里用世界状态断言最终结果，用 `assert` 钉同帧可读。

use xgc_world::{Name, Script, Transform, UiElement, UiInteraction, World};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_focus_{label}_{}_{}",
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
    let path = dir.join("focus.lua");
    std::fs::write(&path, format!("function update(ctx) {body} end\n")).unwrap();
    path
}

fn spawn_ui(
    world: &mut World,
    name: &str,
    kind: &str,
    visible: bool,
    interactable: bool,
    focused: bool,
) -> xgc_base::EntityKey {
    world.spawn((
        Transform::default(),
        Name {
            value: name.to_string(),
        },
        UiElement {
            kind: kind.to_string(),
            visible,
            interactable,
            ..Default::default()
        },
        UiInteraction {
            focused,
            ..Default::default()
        },
    ))
}

/// 一个「按钮 a 带脚本」+「按钮 b（初始有焦点）」+「文本 c」+「隐藏按钮 d」+「禁用按钮 e」
/// +「没挂 ui.interaction 的按钮 f」的世界。
fn run(body: &str) -> (World, Vec<xgc_base::EntityKey>, Option<String>) {
    let dir = temp_dir("world");
    write_script(&dir, body);
    let mut world = World::new();
    let a = spawn_ui(&mut world, "a", "button", true, true, false);
    let b = spawn_ui(&mut world, "b", "button", true, true, true);
    let c = spawn_ui(&mut world, "c", "text", true, true, false);
    let d = spawn_ui(&mut world, "d", "button", false, true, false);
    let e = spawn_ui(&mut world, "e", "button", true, false, false);
    // 没挂 `ui.interaction` 的按钮：焦点命令不能写到它（否则整批命令计划无效）。
    let f = world.spawn((
        Transform::default(),
        Name {
            value: "f".to_string(),
        },
        UiElement {
            kind: "button".to_string(),
            ..Default::default()
        },
    ));
    world.spawn((
        Transform::default(),
        Name {
            value: "driver".to_string(),
        },
        Script {
            paths: vec!["focus.lua".into()],
        },
    ));
    let mut runtime = xgc_script::ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    let error = runtime.take_error();
    (world, vec![a, b, c, d, e, f], error)
}

fn focused(world: &World, key: xgc_base::EntityKey) -> bool {
    world.get::<UiInteraction>(key).unwrap().focused
}

#[test]
fn ui_set_focus_switches_the_focus_and_reads_back_in_the_same_frame() {
    let (world, keys, error) = run(r#"
        local a = entity_id(ctx, "a")
        local b = entity_id(ctx, "b")
        ui_set_focus(ctx, a)
        assert(ui_focused(ctx, a), "同帧必须读到刚设置的焦点")
        assert(not ui_focused(ctx, b), "旧焦点必须被同一帧清掉")
        "#);
    assert!(error.is_none(), "脚本报错：{error:?}");
    assert!(focused(&world, keys[0]), "目标按钮拿到焦点");
    assert!(!focused(&world, keys[1]), "旧焦点被清掉");
    assert!(!focused(&world, keys[2]), "文本没有焦点");
}

#[test]
fn ui_set_focus_ignores_non_buttons_hidden_and_disabled_elements() {
    let (world, keys, error) = run(r#"
        ui_set_focus(ctx, entity_id(ctx, "c"))
        ui_set_focus(ctx, entity_id(ctx, "d"))
        ui_set_focus(ctx, entity_id(ctx, "e"))
        ui_set_focus(ctx, entity_id(ctx, "f"))
        assert(not ui_focused(ctx, entity_id(ctx, "c")))
        assert(not ui_focused(ctx, entity_id(ctx, "f")))
        assert(not ui_focused(ctx, entity_id(ctx, "d")))
        assert(not ui_focused(ctx, entity_id(ctx, "e")))
        "#);
    assert!(error.is_none(), "脚本报错：{error:?}");
    assert!(!focused(&world, keys[2]), "文本不能被聚焦");
    assert!(!focused(&world, keys[3]), "隐藏按钮不能被聚焦");
    assert!(!focused(&world, keys[4]), "禁用按钮不能被聚焦");
    assert!(focused(&world, keys[1]), "失败请求不该清掉已有焦点");
}

#[test]
fn ui_set_focus_survives_into_the_next_frame_view() {
    let dir = temp_dir("next_frame");
    write_script(
        &dir,
        r#"
        local a = entity_id(ctx, "a")
        if time(ctx) < 0.02 then
            ui_set_focus(ctx, a)
        else
            assert(ui_focused(ctx, a), "下一帧的帧首快照必须带着上一帧结算的焦点")
        end
        "#,
    );
    let mut world = World::new();
    let a = spawn_ui(&mut world, "a", "button", true, true, false);
    spawn_ui(&mut world, "b", "button", true, true, true);
    world.spawn((
        Transform::default(),
        Name {
            value: "driver".to_string(),
        },
        Script {
            paths: vec!["focus.lua".into()],
        },
    ));
    let mut runtime = xgc_script::ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert!(runtime.take_error().is_none(), "第一帧不该报错");
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert!(
        runtime.take_error().is_none(),
        "第二帧的帧首快照必须看得到焦点"
    );
    assert!(focused(&world, a), "焦点标志落在组件上");
}

#[test]
fn ui_clear_focus_clears_every_element() {
    let (world, keys, error) = run(r#"
        ui_clear_focus(ctx)
        assert(not ui_focused(ctx, entity_id(ctx, "b")))
        "#);
    assert!(error.is_none(), "脚本报错：{error:?}");
    for key in keys {
        let state = world.get::<UiInteraction>(key);
        assert!(
            state.is_none_or(|state| !state.focused),
            "清焦点后所有元素都必须无焦点"
        );
    }
}
