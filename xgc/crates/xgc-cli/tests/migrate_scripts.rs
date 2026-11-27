//! 旧项目脚本与预制体迁移：GUID 脚本可装载、预制体可直接迁移运行。

use std::fs;
use std::path::Path;
use std::process::Command;

use xgc_asset::scan;
use xgc_migrate::migrate_str;
use xgc_script::ScriptRuntime;
use xgc_sim::Simulation;
use xgc_world::{Component, Name, Script, Transform, World};

#[test]
fn migrate_cli_keeps_all_lua_attachments_in_each_mode() {
    let scratch = std::env::temp_dir().join(format!(
        "xgc-cli-migrate-attachments-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let source = scratch.join("source");
    let scenes = source.join("Scenes");
    let scripts = source.join("Scripts");
    fs::create_dir_all(&scenes).unwrap();
    fs::create_dir(&scripts).unwrap();
    fs::create_dir(source.join("Prefabs")).unwrap();
    fs::write(
        scripts.join("second.lua"),
        "function update(ctx)\n  xgc.set(self_id(ctx), 'core.name', 'value', 'second ran')\n  set_pos(ctx, 7, 0, 0)\nend\n",
    )
    .unwrap();
    fs::write(
        scripts.join("first.lua"),
        "function update(ctx) set_pos(ctx, 3, 0, 0) end\n",
    )
    .unwrap();
    let old_scene = scenes.join("a.ron");
    fs::write(
        &old_scene,
        r#"(
            format_version: 2,
            entities: [(
                id: (17),
                name: "Before Lua",
                transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0),
                            scale: (1.0, 1.0, 1.0)),
                components: (entries: [
                    (data: Lua(path: "Scripts/second.lua")),
                    (data: Lua(path: "Scripts/first.lua")),
                ]),
            )],
        )"#,
    )
    .unwrap();
    fs::write(
        source.join("project.ron"),
        r#"(
            format_version: 1,
            name: "Attachments",
            entry_scene: "Scenes/a.ron",
            build: (scenes: ["Scenes/a.ron"]),
        )"#,
    )
    .unwrap();

    let run_migrate = |input: &Path, output: &Path, flags: &[&str]| {
        let process = Command::new(env!("CARGO_BIN_EXE_xgc"))
            .arg("migrate")
            .arg(input)
            .arg(output)
            .args(flags)
            .output()
            .unwrap();
        assert!(
            process.status.success(),
            "{}: {}\n{}",
            input.display(),
            process.status,
            String::from_utf8_lossy(&process.stderr)
        );
        String::from_utf8(process.stdout).unwrap()
    };
    let single = scratch.join("single.json");
    let flat = scratch.join("flat");
    let project = scratch.join("project");
    let dry = run_migrate(&source, &project, &["--dry-run"]);
    assert!(
        dry.contains("asset_references=2 entity_references=0 unmatched=0"),
        "{dry}"
    );
    assert!(dry.contains("dry-run"), "{dry}");
    assert!(!project.exists());
    let project_stdout = run_migrate(&source, &project, &[]);
    assert!(project_stdout.contains("asset_references=2 entity_references=0 unmatched=0"));
    assert!(project_stdout
        .lines()
        .nth(1)
        .unwrap()
        .contains("entity ID map:"));
    for stdout in [
        run_migrate(&old_scene, &single, &[]),
        run_migrate(&scenes, &flat, &[]),
    ] {
        assert!(stdout.lines().next().unwrap().starts_with("migrate: "));
        assert_eq!(stdout.lines().count(), 1, "{stdout}");
        assert!(stdout.contains("1 个组件已映射"), "{stdout}");
    }
    let project_scene = project.join("scenes/a.json");
    for target in [&single, &flat.join("a.json"), &project_scene] {
        let data = xgc_store::load_world(target).unwrap();
        let mut world = World::new();
        world.restore_world_data(&data).unwrap();
        let key = world.key(data.entities[0].id).unwrap();
        assert_eq!(
            world.get::<Script>(key).unwrap().paths,
            ["Scripts/second.lua", "Scripts/first.lua"],
            "{}",
            target.display()
        );
    }

    let snapshot = scratch.join("after.json");
    let run = Command::new(env!("CARGO_BIN_EXE_xgc"))
        .arg("run-scripts")
        .args([&project_scene, &project])
        .arg("1")
        .arg(&snapshot)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let after = xgc_store::load_world(&snapshot).unwrap();
    let mut world = World::new();
    world.restore_world_data(&after).unwrap();
    let key = world.key(after.entities[0].id).unwrap();
    // 第一项的独立写入证明两项都执行，位置被第二项覆盖证明执行顺序。
    assert_eq!(world.get::<Name>(key).unwrap().value, "second ran");
    assert_eq!(
        world.get::<Transform>(key).unwrap().position,
        [3.0, 0.0, 0.0]
    );
    fs::remove_dir_all(scratch).unwrap();
}

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
        .filter(|component| component.component == Script::ID)
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
