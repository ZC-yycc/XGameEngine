//! 动画混合层：层栈状态机 / 时钟 / 淡入推进与按名改权重。
//!
//! 姿势合成在渲染侧（`glb::layers`），这里只钉模拟层的账：转移按层自己的
//! 机器与参数走、转移没写 duration 时用 `fade_time` 兜底、旧状态时钟定格、
//! 非 autoplay 不推进、坏 RON 静默跳过。

use xgc_sim::{set_layer_weight, AnimationLayerSystem, System};
use xgc_world::{
    layers_to_ron, parse_layers, set_layer_parameter, AnimationLayers, Animator, World,
};

const MACHINE_LAYERS: &str = r#"[(name:"upper", mode:Override, clip:"Walk", weight:0.0, speed:1.0, looping:true, machine:Some((initial:"Natural", parameters:{"survey":0.0}, states:[(name:"Natural", clip:"Walk"), (name:"Survey", clip:"Survey")], transitions:[(from:Some("Natural"), to:"Survey", when:Some(AtLeast(parameter:"survey", value:0.5)), duration:0.35), (from:Some("Survey"), to:"Natural", when:Some(Below(parameter:"survey", value:0.5)), duration:0.15)])), mask:["b_Spine01_02"])]"#;

fn spawn(world: &mut World, layers: &str, autoplay: bool) -> xgc_base::EntityKey {
    world.spawn((
        AnimationLayers {
            layers: layers.to_string(),
        },
        Animator {
            autoplay,
            ..Animator::default()
        },
    ))
}

fn read_layers(world: &World, key: xgc_base::EntityKey) -> Vec<xgc_world::AnimLayer> {
    let component = world.get::<AnimationLayers>(key).expect("组件在");
    parse_layers(&component.layers).expect("层栈可解析")
}

fn set_parameters(world: &mut World, key: xgc_base::EntityKey, survey: f32) {
    let component = world.get::<AnimationLayers>(key).expect("组件在");
    let mut layers = parse_layers(&component.layers).expect("层栈可解析");
    set_layer_parameter(&mut layers[0], "survey", survey);
    let text = layers_to_ron(&layers);
    world
        .get_mut::<AnimationLayers>(key)
        .expect("组件在")
        .layers = text;
}

#[test]
fn layer_machine_switches_and_freezes_the_old_clock() {
    let mut world = World::new();
    let key = spawn(&mut world, MACHINE_LAYERS, true);
    let mut system = AnimationLayerSystem;
    system.run(&mut world, 0.1);
    let layer = read_layers(&world, key).pop().unwrap();
    assert_eq!(layer.state, "Natural");
    assert!((layer.time - 0.1).abs() < 1e-6, "Natural 走了 0.1s");

    set_parameters(&mut world, key, 1.0);
    system.run(&mut world, 0.1);
    let layer = read_layers(&world, key).pop().unwrap();
    assert_eq!(layer.state, "Survey");
    // 转移没写 duration 时用 `fade_time`（默认 0.25）？这里写了 0.35，以它为准。
    assert!((layer.fade_duration - 0.35).abs() < 1e-6);
    assert_eq!(layer.fade_from, "Walk");
    // 旧端切换当帧已给旧时钟再推一帧（`prev.t += dt * prev.speed`）：0.1 + 0.1。
    assert!(
        (layer.fade_from_time - 0.2).abs() < 1e-6,
        "旧时钟按旧状态速度继续走一帧"
    );
    assert!((layer.time - 0.1).abs() < 1e-6, "新状态从切换帧起算");
    assert!((layer.fade_elapsed - 0.1).abs() < 1e-6);
}

#[test]
fn layer_fade_ends_after_its_duration() {
    let mut world = World::new();
    let key = spawn(&mut world, MACHINE_LAYERS, true);
    let mut system = AnimationLayerSystem;
    system.run(&mut world, 0.1);
    set_parameters(&mut world, key, 1.0);
    system.run(&mut world, 0.1);
    system.run(&mut world, 0.3);
    let layer = read_layers(&world, key).pop().unwrap();
    assert!(layer.fade_from.is_empty(), "淡入结束应清掉旧半边");
    assert_eq!(layer.fade_duration, 0.0);
}

#[test]
fn layer_weight_is_clamped_and_missing_names_are_silent() {
    let mut world = World::new();
    let key = spawn(&mut world, MACHINE_LAYERS, true);
    let mut layers = read_layers(&world, key);
    assert!(set_layer_weight(&mut layers, "upper", 2.0));
    assert!((layers[0].weight - 1.0).abs() < 1e-6);
    assert!(set_layer_weight(&mut layers, "upper", -1.0));
    assert!(layers[0].weight.abs() < 1e-6);
    assert!(!set_layer_weight(&mut layers, "missing", 0.5));
    assert!(!set_layer_weight(&mut layers, "upper", f32::NAN));
    assert!(layers[0].weight.abs() < 1e-6, "非有限值不该改权重");
}

#[test]
fn non_autoplay_layers_do_not_advance() {
    let mut world = World::new();
    let key = spawn(&mut world, MACHINE_LAYERS, false);
    let mut system = AnimationLayerSystem;
    system.run(&mut world, 0.1);
    let layer = read_layers(&world, key).pop().unwrap();
    // 非 autoplay：状态名仍按机器 entry 初始化，但时钟 / 转移一律不推进。
    assert_eq!(layer.state, "Natural");
    assert_eq!(layer.time, 0.0);
    assert!(layer.fade_from.is_empty());
}

#[test]
fn broken_ron_is_skipped_without_touching_the_component() {
    let mut world = World::new();
    let key = spawn(&mut world, "not a ron list", true);
    let mut system = AnimationLayerSystem;
    system.run(&mut world, 0.1);
    let component = world.get::<AnimationLayers>(key).unwrap();
    assert_eq!(component.layers, "not a ron list");
    assert!(world.get::<Animator>(key).unwrap().autoplay);
}

#[test]
fn layer_transition_fields_are_optional_in_ron() {
    let layers = parse_layers(
        r#"[(name:"upper", machine:Some((initial:"idle", transitions:[
            (from:None, to:"run", when:Some(AtLeast(parameter:"speed", value:0.5))),
            (to:"idle"),
        ])))]"#,
    )
    .unwrap();
    let transitions = &layers[0].machine.as_ref().unwrap().transitions;
    assert_eq!(transitions[0].duration, 0.0);
    assert!(transitions[0].from.is_none());
    assert!(transitions[1].when.is_none());
    assert!(transitions[1].from.is_none());
}

#[test]
fn layer_snapshot_roundtrips_through_ron() {
    let mut layers = parse_layers(MACHINE_LAYERS).unwrap();
    layers[0].weight = 0.6;
    let text = layers_to_ron(&layers);
    let parsed = parse_layers(&text).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].name, "upper");
    assert!((parsed[0].weight - 0.6).abs() < 1e-6);
    assert_eq!(parsed[0].mask, vec!["b_Spine01_02".to_string()]);
    assert!(parsed[0].machine.is_some());
}
