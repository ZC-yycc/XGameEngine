//! DrawList 蒙皮姿态：网格沿 Parent 找到 Skeleton 根，读取 animation.pose。

use xgc_render::build_draw_list;
use xgc_world::{GlobalTransform, Material, MeshRenderer, Parent, Skeleton, SkeletonPose, World};

fn translation(x: f32) -> [f32; 16] {
    let mut matrix = [0.0; 16];
    matrix[0] = 1.0;
    matrix[5] = 1.0;
    matrix[10] = 1.0;
    matrix[15] = 1.0;
    matrix[12] = x;
    matrix
}

#[test]
fn draw_item_finds_ancestor_skeleton_pose() {
    let mut world = World::new();
    let skeleton = world.spawn((
        Skeleton { joint_count: 1 },
        SkeletonPose {
            matrices: ron::ser::to_string(&vec![translation(3.0)]).unwrap(),
        },
    ));
    let skeleton_id = world.entity(skeleton).unwrap();
    world.spawn((
        Parent {
            entity: skeleton_id,
        },
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material::default(),
        GlobalTransform::default(),
    ));
    let list = build_draw_list(&world);
    assert_eq!(list.items.len(), 1);
    let pose = list.items[0].pose.as_ref().expect("应读取到骨架姿态");
    assert_eq!(pose.len(), 1);
    assert_eq!(pose[0][12], 3.0);
}
