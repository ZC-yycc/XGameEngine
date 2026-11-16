//! 动画叠加层：按权重累加平移。

use xgc_sim::{AnimationClip, AnimationLayerSystem, AnimationLibrary, System, TranslationKey};
use xgc_world::{AnimationLayers, Animator, Name, Transform, World};

#[test]
fn layer_weight_blends_translation() {
    let mut library = AnimationLibrary::new();
    library.insert(AnimationClip::new(
        "add",
        1.0,
        vec![TranslationKey {
            time: 0.0,
            value: [1.0, 0.0, 0.0],
        }],
    ));
    let mut world = World::new();
    let key = world.spawn((
        AnimationLayers {
            layers: "[(clip: \"add\", weight: 0.5)]".to_string(),
        },
        Animator::default(),
        Transform::default(),
    ));
    let mut system = AnimationLayerSystem::new(library);
    system.run(&mut world, 0.1);
    assert_eq!(
        world.get::<Transform>(key).unwrap().position,
        [0.5, 0.0, 0.0]
    );
}

#[test]
fn layer_mask_limits_to_named_entities() {
    let mut library = AnimationLibrary::new();
    library.insert(AnimationClip::new(
        "add",
        1.0,
        vec![TranslationKey {
            time: 0.0,
            value: [2.0, 0.0, 0.0],
        }],
    ));
    let mut world = World::new();
    let upper = world.spawn((
        Name {
            value: "upper".to_string(),
        },
        AnimationLayers {
            layers: "[(clip: \"add\", weight: 1.0, mask: [\"upper\"])]".to_string(),
        },
        Animator::default(),
        Transform::default(),
    ));
    let other = world.spawn((
        Name {
            value: "other".to_string(),
        },
        AnimationLayers {
            layers: "[(clip: \"add\", weight: 1.0, mask: [\"upper\"])]".to_string(),
        },
        Animator::default(),
        Transform::default(),
    ));
    let mut system = AnimationLayerSystem::new(library);
    system.run(&mut world, 0.1);
    assert_eq!(world.get::<Transform>(upper).unwrap().position[0], 2.0);
    assert_eq!(world.get::<Transform>(other).unwrap().position[0], 0.0);
}
