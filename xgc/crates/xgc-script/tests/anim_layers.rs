//! 脚本侧混合层：`set_anim_layer_weight` / `set_anim_layer_param` 读当前层栈、
//! 改值后写回 `animation.layers` 组件；层名不存在 / 非有限值静默 no-op。

use xgc_world::{
    find_layer, layer_parameters, parse_layers, AnimationLayers, Animator, Script, Transform, World,
};

const LAYERS: &str = r#"[(name:"upper", mode:Override, clip:"Walk", weight:0.0, speed:1.0, looping:true, machine:Some((initial:"Natural", parameters:{"survey":0.0}, states:[(name:"Natural", clip:"Walk"), (name:"Survey", clip:"Survey")], transitions:[(from:Some("Natural"), to:"Survey", when:Some(AtLeast(parameter:"survey", value:0.5)), duration:0.35)])), mask:["b_Spine01_02"])]"#;

fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_layers_{label}_{}_{}",
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
    let path = dir.join("layers.lua");
    std::fs::write(&path, format!("function update(ctx) {body} end\n")).unwrap();
    path
}

fn spawn(world: &mut World) -> xgc_base::EntityKey {
    world.spawn((
        Transform::default(),
        Animator::default(),
        AnimationLayers {
            layers: LAYERS.to_string(),
        },
        Script {
            path: "layers.lua".to_string(),
        },
    ))
}

fn layers(world: &World, key: xgc_base::EntityKey) -> Vec<xgc_world::AnimLayer> {
    parse_layers(&world.get::<AnimationLayers>(key).unwrap().layers).unwrap()
}

#[test]
fn script_writes_layer_weight_and_parameters() {
    let dir = temp_dir("weight");
    write_script(
        &dir,
        r#"set_anim_layer_weight(ctx, "upper", 2.0) set_anim_layer_param(ctx, "upper", "survey", 1.0)"#,
    );
    let mut world = World::new();
    let key = spawn(&mut world);
    let mut runtime = xgc_script::ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);

    let parsed = layers(&world, key);
    let layer = find_layer(&mut parsed.clone(), "upper").cloned().unwrap();
    assert!((layer.weight - 1.0).abs() < 1e-6, "超界权重钳到 1");
    let parameters = layer_parameters(&layer);
    assert!((parameters["survey"] - 1.0).abs() < 1e-6, "参数写进层机器");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn missing_layer_and_non_finite_values_are_silent() {
    let dir = temp_dir("silent");
    write_script(
        &dir,
        r#"set_anim_layer_weight(ctx, "missing", 0.5) set_anim_layer_weight(ctx, "upper", 0.0/0.0)"#,
    );
    let mut world = World::new();
    let key = spawn(&mut world);
    let mut runtime = xgc_script::ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);

    let parsed = layers(&world, key);
    let layer = find_layer(&mut parsed.clone(), "upper").cloned().unwrap();
    assert!(layer.weight.abs() < 1e-6, "缺层 / NaN 都不改权重");

    std::fs::remove_dir_all(&dir).unwrap();
}
