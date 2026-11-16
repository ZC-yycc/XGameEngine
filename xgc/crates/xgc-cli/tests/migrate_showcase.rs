//! 旧项目 showcase 端到端：迁移 / 恢复 / 跑 60 帧 / GUID 资产可解析。
//! 旧项目不在 xgc workspace 内时跳过。

use std::path::Path;

use xgc_migrate::migrate_str;
use xgc_render::{load_model, MeshError};
use xgc_sim::Simulation;
use xgc_world::World;

#[test]
fn showcase_scene_migrates_and_runs() {
    let root = Path::new("../../../projects/XGame");
    let scene = root.join("Scenes/showcase.ron");
    if !scene.exists() {
        eprintln!("skip: 没有旧项目 showcase.ron");
        return;
    }
    let text = std::fs::read_to_string(&scene).unwrap();
    let (data, report) = migrate_str(&text).unwrap();
    assert!(report.entities > 0);
    assert!(report.mapped_components > 0);
    assert_eq!(report.missing_parents, 0);
    let mut world = World::new();
    world.restore_world_data(&data).unwrap();
    let mut simulation = Simulation::new(world);
    for _ in 0..60 {
        simulation.step(Vec::new()).unwrap();
    }
    let world = simulation.world();
    assert_ne!(world.len(), 0);
    assert_ne!(world.state_hash(), 0);

    let catalog = xgc_asset::scan(root).unwrap();
    let mut models = 0;
    for entity in &data.entities {
        for component in &entity.components {
            if component.component.0 != 0x0201 {
                continue;
            }
            let model = component
                .fields
                .iter()
                .find(|(name, _)| name == "model")
                .map(|(_, value)| match value {
                    xgc_world::Value::String(value) => value.clone(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            if !model.starts_with("guid://") {
                continue;
            }
            let path = catalog.resolve(&model).expect("GUID 应可解析");
            match load_model(path) {
                Ok(mesh) => assert!(!mesh.positions.is_empty()),
                Err(MeshError::Unsupported(message)) => panic!("{model}: {message}"),
                Err(err) => panic!("{model}: {err}"),
            }
            models += 1;
        }
    }
    assert!(models > 0, "showcase 应有 GUID 网格");
}
