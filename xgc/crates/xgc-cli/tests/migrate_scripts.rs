//! 旧项目脚本与预制体迁移：GUID 脚本可装载、预制体可直接迁移运行。

use std::path::Path;

use xgc_asset::scan;
use xgc_migrate::migrate_str;
use xgc_script::ScriptRuntime;
use xgc_sim::Simulation;
use xgc_world::World;

const SCRIPT_COMPONENT: u32 = 0x0301;

#[test]
fn character_scene_scripts_resolve_and_load() {
    let root = Path::new("../../../projects/XGame");
    let scene = root.join("Scenes/character.ron");
    if !scene.exists() {
        eprintln!("skip: 没有旧项目 character.ron");
        return;
    }
    let text = std::fs::read_to_string(&scene).unwrap();
    let (data, _report) = migrate_str(&text).unwrap();
    let scripts = data
        .entities
        .iter()
        .flat_map(|entity| &entity.components)
        .filter(|component| component.component.0 == SCRIPT_COMPONENT)
        .count();
    assert!(scripts > 0, "character 场景应有脚本");
    let catalog = scan(root).unwrap();
    let mut world = World::new();
    world.restore_world_data(&data).unwrap();
    let mut simulation = Simulation::new(world);
    let mut runtime = ScriptRuntime::with_catalog(root, catalog);
    for _ in 0..30 {
        simulation
            .step_with(Vec::new(), |world| runtime.run_frame(world, 1.0 / 60.0))
            .unwrap();
    }
    assert_eq!(runtime.entity_count(), scripts);
}

#[test]
fn prefab_migrates_as_world_data() {
    let prefab = Path::new("../../../projects/XGame/Prefabs/Pillar.prefab.ron");
    if !prefab.exists() {
        eprintln!("skip: 没有旧项目 Pillar 预制体");
        return;
    }
    let text = std::fs::read_to_string(prefab).unwrap();
    let (data, report) = migrate_str(&text).unwrap();
    assert_eq!(report.entities, 3);
    assert!(report.mapped_components > 0);
    assert_eq!(report.missing_parents, 0);
    let mut world = World::new();
    world.restore_world_data(&data).unwrap();
    let mut simulation = Simulation::new(world);
    simulation.step(Vec::new()).unwrap();
}
