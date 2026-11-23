//! 图片三件套（`ui_set_image` / `ui_set_image_tint` / `ui_set_fill`）与旧端语义对齐。
//!
//! 旧端 `crates/xgame-script/src/ui_ctx.rs`：
//! - `ui_set_image` 换路径（不存在 image 时创建）；
//! - `ui_set_image_tint` 收 **sRGB** 颜色，组件里存线性（`color_srgb` 唯一转换点）；
//! - `ui_set_fill` 把填充比例 clamp 到 `0..=1`。
//!
//! XGC 侧三个函数写 `ui.element` 的 `image_path` / `image_tint` / `image_fill`，
//! 随脚本命令在帧末统一结算；这里用世界状态断言最终结果。

use xgc_world::{Name, Script, Transform, UiElement, World};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_image_{label}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 一个「图片元素 bar」+「没有 ui.element 的 plain」+「驱动脚本」的世界。
fn run(
    body: &str,
) -> (
    World,
    xgc_base::EntityKey,
    xgc_base::EntityKey,
    Option<String>,
) {
    let dir = temp_dir("world");
    std::fs::write(
        dir.join("image.lua"),
        format!("function update(ctx) {body} end\n"),
    )
    .unwrap();
    let mut world = World::new();
    let image = world.spawn((
        Transform::default(),
        Name {
            value: "bar".to_string(),
        },
        UiElement {
            kind: "image".to_string(),
            ..Default::default()
        },
    ));
    let plain = world.spawn((
        Transform::default(),
        Name {
            value: "plain".to_string(),
        },
    ));
    world.spawn((
        Transform::default(),
        Name {
            value: "driver".to_string(),
        },
        Script {
            path: "image.lua".to_string(),
        },
    ));
    let mut runtime = xgc_script::ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    let error = runtime.take_error();
    (world, image, plain, error)
}

fn element(world: &World, key: xgc_base::EntityKey) -> UiElement {
    world.get::<UiElement>(key).cloned().unwrap()
}

#[test]
fn ui_image_setters_write_the_component_fields() {
    let (world, key, _plain, error) = run(r#"
        local bar = entity_id(ctx, "bar")
        ui_set_image(ctx, bar, "Resources/Textures/bar.png")
        ui_set_image_tint(ctx, bar, 0.5, 0.25, 0.0, 1.0)
        ui_set_fill(ctx, bar, 0.25)
        "#);
    assert!(error.is_none(), "脚本报错：{error:?}");
    let ui = element(&world, key);
    assert_eq!(ui.image_path, "Resources/Textures/bar.png");
    // 0.5 / 0.25 走 sRGB→线性（≈0.21404 / 0.050876）；直接搬原值会差 0.28 / 0.20，1e-3 容差足够分辨。
    assert!(
        (ui.image_tint[0] - 0.21404).abs() < 1e-3,
        "{:?}",
        ui.image_tint
    );
    assert!(
        (ui.image_tint[1] - 0.050876).abs() < 1e-3,
        "{:?}",
        ui.image_tint
    );
    assert_eq!(ui.image_tint[2], 0.0);
    assert_eq!(ui.image_tint[3], 1.0);
    assert_eq!(ui.image_fill, 0.25);
}

#[test]
fn ui_set_fill_clamps_to_the_unit_range() {
    let (world, key, _plain, error) = run(r#"
        local bar = entity_id(ctx, "bar")
        ui_set_fill(ctx, bar, 2.0)
        "#);
    assert!(error.is_none(), "脚本报错：{error:?}");
    assert_eq!(element(&world, key).image_fill, 1.0, "上溢要钳到 1");

    let (world, key, _plain, error) = run(r#"
        local bar = entity_id(ctx, "bar")
        ui_set_fill(ctx, bar, -3.0)
        "#);
    assert!(error.is_none(), "脚本报错：{error:?}");
    assert_eq!(element(&world, key).image_fill, 0.0, "下溢要钳到 0");
}

#[test]
fn ui_image_setters_ignore_entities_without_a_ui_element() {
    // 没有 `ui.element` 的实体：三个 setter 必须彻底 no-op（不写半截命令、不报错）。
    let (world, _key, plain, error) = run(r#"
        local plain = entity_id(ctx, "plain")
        ui_set_image(ctx, plain, "Resources/Textures/bar.png")
        ui_set_image_tint(ctx, plain, 1.0, 0.0, 0.0, 1.0)
        ui_set_fill(ctx, plain, 0.5)
        ui_set_image(ctx, 99999, "Resources/Textures/bar.png")
        "#);
    assert!(error.is_none(), "脚本报错：{error:?}");
    assert!(
        world.get::<UiElement>(plain).is_none(),
        "不该凭空加 UI 组件"
    );
}
