use super::{scene, standard_project, write};
use xgc_migrate::{migrate_old_project, migrate_str};
use xgc_store::{load_prefab, load_world};
use xgc_world::{Component, Script, Value, WorldData};

fn scripted_entity(id: u64, paths: &[&str]) -> String {
    let entries = paths
        .iter()
        .map(|path| format!("(data: Lua(path: {path:?}))"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "(id: ({id}), transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), \
         scale: (1.0, 1.0, 1.0)), components: (entries: [{entries}]))"
    )
}

fn assert_script(data: &WorldData, retained: &str) {
    let scripts: Vec<_> = data.entities[0]
        .components
        .iter()
        .filter(|component| component.component == Script::ID)
        .collect();
    assert_eq!(scripts.len(), 1);
    assert_eq!(
        scripts[0].fields,
        [("path".to_string(), Value::String(retained.to_string()))]
    );
}

#[test]
fn reports_unattached_lua_scripts_in_scenes_and_prefabs() {
    let (root, out) = standard_project("lua-attachments");
    let paths = ["Scripts/a.lua", "Scripts/b.lua", "Scripts/main.lua"];
    let source = scene(&scripted_entity(41, &paths));
    let prefab = scene(&scripted_entity(71, &paths[1..]));
    write(&root, "Scenes/a.ron", &source);
    write(&root, "Prefabs/P.prefab.ron", &prefab);
    for path in &paths[..2] {
        write(&root, path, "function start(ctx) end\n");
    }

    let dry = migrate_old_project(&root, &out, true).unwrap();
    assert!(!out.exists());
    assert_eq!(dry.unmatched.len(), 2, "{}", dry.render());
    let scene_note = dry
        .unmatched
        .iter()
        .find(|note| note.contains("[migration.lua.attachment] Scenes/a.ron 实体 41"))
        .expect("应点名旧场景和实体");
    assert!(scene_note.contains("保留 \"Scripts/main.lua\""));
    assert!(scene_note.contains("未挂载 [\"Scripts/a.lua\", \"Scripts/b.lua\"]"));
    assert!(dry.unmatched.iter().any(|note| note
        .contains("[migration.lua.attachment] Prefabs/P.prefab.ron 实体 71")
        && note.contains("未挂载 [\"Scripts/b.lua\"]")));
    assert_eq!(dry.asset_references, 2);

    let (world, single) = migrate_str(&source).unwrap();
    assert_eq!(single.mapped_components, 1);
    assert_eq!(
        single.unmatched("Scenes/a.ron").collect::<Vec<_>>(),
        std::slice::from_ref(scene_note)
    );
    assert_script(&world, paths[2]);

    let live = migrate_old_project(&root, &out, false).unwrap();
    assert_eq!(live.unmatched, dry.unmatched);
    assert_script(&load_world(&out.join("scenes/a.json")).unwrap(), paths[2]);
    assert_script(
        &load_prefab(&out.join("prefabs/P.prefab.json")).unwrap(),
        paths[2],
    );
    assert_eq!(
        std::fs::read_to_string(root.join("Scenes/a.ron")).unwrap(),
        source
    );
    assert_eq!(
        std::fs::read_to_string(root.join("Prefabs/P.prefab.ron")).unwrap(),
        prefab,
    );
    for path in paths {
        assert_eq!(
            std::fs::read(root.join(path)).unwrap(),
            std::fs::read(out.join(path)).unwrap(),
        );
    }
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(out).unwrap();
}
