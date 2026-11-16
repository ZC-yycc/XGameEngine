//! 关节轨道：clip 可驱动骨骼实体本地变换。

use xgc_sim::{
    AnimationClip, AnimationLibrary, AnimationPlaybackSystem, JointTrack, System, TranslationKey,
};
use xgc_world::{Animator, Bone, Parent, Transform, World};

#[test]
fn joint_track_drives_bone_transform() {
    let clip = AnimationClip::new(
        "walk",
        1.0,
        vec![TranslationKey {
            time: 0.0,
            value: [0.0, 0.0, 0.0],
        }],
    )
    .with_joints(vec![JointTrack {
        joint: 0,
        translation: vec![TranslationKey {
            time: 0.0,
            value: [5.0, 0.0, 0.0],
        }],
        rotation: Vec::new(),
        scale: Vec::new(),
    }]);
    let mut library = AnimationLibrary::new();
    library.insert(clip);
    let mut world = World::new();
    let root = world.spawn((
        Animator {
            clip: "walk".to_string(),
            ..Default::default()
        },
        Transform::default(),
    ));
    let root_id = world.entity(root).unwrap();
    let bone = world.spawn((
        Bone { index: 0 },
        Parent { entity: root_id },
        Transform::default(),
    ));
    let mut playback = AnimationPlaybackSystem { library };
    playback.run(&mut world, 0.0);
    assert_eq!(world.get::<Transform>(bone).unwrap().position[0], 5.0);
}

#[test]
fn parses_joint_tracks_from_ron() {
    let clip = xgc_sim::load_clip_ron(
        r#"(name: "j", duration: 1.0, joints: [(joint: 2, translation: [(time: 0.0, value: (1.0, 2.0, 3.0))])])"#,
    )
    .unwrap();
    let (position, _, scale) = clip.sample_joint_looped(2, 0.0).unwrap();
    assert_eq!(position, [1.0, 2.0, 3.0]);
    assert_eq!(scale, [1.0; 3]);
}
