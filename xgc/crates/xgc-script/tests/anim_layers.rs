//! 按名 setter 与动态字段写入按脚本调用顺序落到同一层栈。

use xgc_base::EntityKey;
use xgc_world::{AnimLayer, AnimationLayers, Animator, Script, StateMachine, Transform, World};

fn run_script(body: &str) -> Vec<AnimLayer> {
    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_layers_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("layers.lua"),
        format!("function update(ctx) {body} end\n"),
    )
    .unwrap();
    let mut world = World::new();
    let key: EntityKey = world.spawn((
        Transform::default(),
        Animator::default(),
        AnimationLayers {
            layers: vec![
                AnimLayer {
                    name: "upper".into(),
                    weight: 0.0,
                    machine: Some(StateMachine::default()),
                    parameters: [("survey".into(), 0.0)].into(),
                    ..AnimLayer::default()
                },
                AnimLayer {
                    name: "single".into(),
                    ..AnimLayer::default()
                },
            ],
        },
        Script {
            paths: vec!["layers.lua".into()],
        },
    ));
    let mut runtime = xgc_script::ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(runtime.take_error(), None);
    world.get::<AnimationLayers>(key).unwrap().layers.clone()
}

#[test]
fn script_writes_layer_weight_and_parameters() {
    let layers = run_script(
        r#"
        set_anim_layer_weight(ctx, "upper", 2.0)
        set_anim_layer_param(ctx, "upper", "survey", 1.0)
        set_anim_layer_param(ctx, "upper", "new", 0.25)
    "#,
    );
    assert_eq!(layers[0].weight, 1.0);
    assert_eq!(layers[0].parameters["survey"], 1.0);
    assert_eq!(layers[0].parameters["new"], 0.25);
}

#[test]
fn named_setters_respect_dynamic_paths_in_call_order() {
    let layers = run_script(
        r#"
        local id = self_id(ctx)
        set_anim_layer_weight(ctx, "upper", 0.75)
        xgc.set(id, 'animation.layers', 'layers[0].weight', 0.25)
        set_anim_layer_param(ctx, "upper", "survey", 0.5)
        xgc.set(id, 'animation.layers', 'layers[0].parameters["survey"]', 0.125)
        xgc.set(id, 'animation.layers', 'layers[0].name', 'renamed')
        set_anim_layer_param(ctx, "renamed", "second", 0.75)
    "#,
    );
    assert_eq!(layers[0].name, "renamed");
    assert_eq!(layers[0].weight, 0.25);
    assert_eq!(layers[0].parameters["survey"], 0.125);
    assert_eq!(layers[0].parameters["second"], 0.75);
}

#[test]
fn missing_layer_disabled_machine_and_non_finite_values_are_silent() {
    let layers = run_script(
        r#"
        set_anim_layer_weight(ctx, "missing", 0.5)
        set_anim_layer_weight(ctx, "upper", 0.0/0.0)
        set_anim_layer_param(ctx, "upper", "survey", 0.0/0.0)
        set_anim_layer_param(ctx, "single", "survey", 1.0)
    "#,
    );
    assert_eq!(layers[0].weight, 0.0);
    assert_eq!(layers[0].parameters["survey"], 0.0);
    assert!(layers[1].parameters.is_empty());
}
