use super::{scene, staging_of, standard_project, write, write_meta};
use xgc_migrate::migrate_old_project;
use xgc_store::{load_prefab, load_world};
use xgc_world::{Component, Script, World, WorldData};

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

fn assert_scripts(data: &WorldData, expected: &[&str]) {
    let entity = &data.entities[0];
    assert_eq!(
        entity
            .components
            .iter()
            .filter(|component| component.component == Script::ID)
            .count(),
        1
    );
    let mut world = World::new();
    world.restore_world_data(data).unwrap();
    let key = world.key(entity.id).unwrap();
    assert_eq!(world.get::<Script>(key).unwrap().paths, expected);
}

#[test]
fn preserves_all_lua_attachments_in_scenes_and_prefabs() {
    let (root, out) = standard_project("lua-attachments");
    let scene_paths = [r".\Scripts\a.lua", "guid://abcd1234", "Scripts/main.lua"];
    let prefab_paths = ["Scripts/main.lua", "guid://abcd1234"];
    let source = scene(&scripted_entity(41, &scene_paths));
    let prefab = scene(&scripted_entity(71, &prefab_paths));
    write(&root, "Scenes/a.ron", &source);
    write(&root, "Prefabs/P.prefab.ron", &prefab);
    write(&root, "Scripts/a.lua", "function start(ctx) end\r\n");
    write(&root, "Scripts/b.lua", "-- GUID 指向此脚本\n");
    write_meta(&root, "Scripts/b.lua", "abcd1234");

    let dry = migrate_old_project(&root, &out, true).unwrap();
    assert!(!out.exists());
    assert!(!staging_of(&out).exists());
    assert!(dry.unmatched.is_empty(), "{}", dry.render());
    assert_eq!(dry.asset_references, 5);

    let live = migrate_old_project(&root, &out, false).unwrap();
    assert_eq!(live.unmatched, dry.unmatched);
    assert_eq!(live.copied_files, dry.copied_files);
    assert_eq!(live.components, dry.components);
    assert_eq!(live.asset_references, dry.asset_references);
    assert_scripts(
        &load_world(&out.join("scenes/a.json")).unwrap(),
        &scene_paths,
    );
    assert_scripts(
        &load_prefab(&out.join("prefabs/P.prefab.json")).unwrap(),
        &prefab_paths,
    );
    assert_eq!(
        std::fs::read(root.join("Scenes/a.ron")).unwrap(),
        source.as_bytes()
    );
    assert_eq!(
        std::fs::read(root.join("Prefabs/P.prefab.ron")).unwrap(),
        prefab.as_bytes(),
    );
    for path in [
        "Scripts/a.lua",
        "Scripts/b.lua",
        "Scripts/b.lua.meta",
        "Scripts/main.lua",
    ] {
        assert_eq!(
            std::fs::read(root.join(path)).unwrap(),
            std::fs::read(out.join(path)).unwrap(),
        );
    }
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(out).unwrap();
}
