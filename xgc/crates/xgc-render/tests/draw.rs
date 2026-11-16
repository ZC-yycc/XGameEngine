//! Draw list 抽取回归：只收带 GlobalTransform 的 render.mesh，按 EntityId 稳定排序。

use xgc_render::build_draw_list;
use xgc_world::{GlobalTransform, Material, MeshRenderer, Transform, World};

#[test]
fn draw_list_extracts_mesh_material_and_skips_missing_global() {
    let mut world = World::new();
    let first = world.spawn((
        Transform::default(),
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://cube".to_string(),
        },
        Material {
            base_color: [1.0, 0.0, 0.0, 1.0],
            roughness: 0.2,
        },
    ));
    let second = world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://sphere".to_string(),
        },
        Material {
            base_color: [0.0, 0.0, 1.0, 1.0],
            roughness: 0.8,
        },
    ));
    world.spawn((MeshRenderer {
        model: "no-global".to_string(),
    },));

    let list = build_draw_list(&world);
    assert_eq!(list.len(), 2);
    assert_eq!(list.items[0].entity, world.entity(first).unwrap());
    assert_eq!(list.items[1].entity, world.entity(second).unwrap());
    assert_eq!(list.items[0].model, "builtin://cube");
    assert_eq!(list.items[0].color, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(list.items[1].roughness, 0.8);
}

#[test]
fn empty_world_has_empty_draw_list() {
    let world = World::new();
    assert!(build_draw_list(&world).is_empty());
}
