//! Draw list 抽取回归：收集带 GlobalTransform 的渲染实体，按 EntityId 稳定排序。

use xgc_render::{build_draw_list, scene_bounds, Bounds, MeshCache};
use xgc_world::{GlobalTransform, LodGroup, LodLevel, Material, MeshRenderer, Transform, World};

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
            ..Material::default()
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
            ..Material::default()
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

#[test]
fn draw_item_carries_global_transform() {
    let mut world = World::new();
    world.spawn((
        GlobalTransform {
            position: [2.0, 3.0, 4.0],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
    ));
    let list = build_draw_list(&world);
    assert_eq!(list.items[0].transform[12], 2.0);
    assert_eq!(list.items[0].transform[13], 3.0);
    assert_eq!(list.items[0].transform[14], 4.0);
}

#[test]
fn first_lod_supplies_geometry_and_bounds_with_no_base_mesh() {
    let mut world = World::new();
    let lod = LodGroup {
        levels: vec![
            LodLevel::default(),
            LodLevel {
                model: "builtin://cube".into(),
                max_distance: 8.0,
            },
            LodLevel {
                model: "builtin://plane".into(),
                max_distance: 0.0,
            },
        ],
    };
    let only_lod = world.spawn((GlobalTransform::default(), lod.clone()));
    let with_base = world.spawn((
        GlobalTransform {
            position: [2.0, 0.0, 0.0],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "../unused-base.glb".into(),
        },
        lod,
    ));
    let empty_lod = world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://quad".into(),
        },
        LodGroup {
            levels: vec![LodLevel::default()],
        },
    ));

    let list = build_draw_list(&world);
    let models: Vec<_> = list
        .items
        .iter()
        .map(|item| (item.entity, item.model.as_str()))
        .collect();
    assert_eq!(
        models,
        [
            (world.entity(only_lod).unwrap(), "builtin://cube"),
            (world.entity(with_base).unwrap(), "builtin://cube"),
            (world.entity(empty_lod).unwrap(), "builtin://quad"),
        ]
    );
    assert_eq!(
        scene_bounds(&world, &mut MeshCache::new(".")).unwrap(),
        Some(Bounds {
            min: [-0.5; 3],
            max: [2.5, 0.5, 0.5],
        })
    );
}
