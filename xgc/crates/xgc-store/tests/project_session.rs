//! 项目清单会话：只按登记过的相对路径读场景。

use xgc_store::{
    load_project, save_project, save_world, ProjectContent, ProjectManifest, ProjectSession,
    PROJECT_FORMAT_VERSION,
};
use xgc_world::{Name, Transform, World};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xgc_session_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        name: "demo".to_string(),
        entry_scene: "scenes/main.json".to_string(),
        scenes: vec!["scenes/main.json".to_string()],
        content: ProjectContent {
            models: "Resources".to_string(),
            textures: "Resources".to_string(),
            audio: "Resources".to_string(),
            shaders: None,
            prefabs: "Prefabs".to_string(),
            scripts: "Scripts".to_string(),
        },
        search_paths: Vec::new(),
        input: None,
        render: Default::default(),
        quality: Default::default(),
    }
}

#[test]
fn opens_a_directory_and_rejects_unregistered_scenes() {
    let dir = temp("open");
    let mut world = World::new();
    world.spawn((
        Name {
            value: "hero".to_string(),
        },
        Transform::default(),
    ));
    save_world(&dir.join("scenes/main.json"), &world.world_data()).unwrap();
    save_project(&dir.join("project.xgc"), &manifest()).unwrap();

    let session = ProjectSession::open(&dir).unwrap();
    assert_eq!(session.manifest().name, "demo");
    assert_eq!(session.root(), dir.as_path());
    assert_eq!(session.load_entry().unwrap().entities.len(), 1);

    let err = session.load_scene("scenes/other.json").unwrap_err();
    assert!(err.to_string().contains("没有登记"), "{err}");
    let err = session.scene_path("../escape.json").unwrap_err();
    assert!(err.to_string().contains("不合法"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn manifest_roundtrips_through_disk() {
    let dir = temp("roundtrip");
    let manifest = manifest();
    save_project(&dir.join("project.xgc"), &manifest).unwrap();
    assert_eq!(load_project(&dir.join("project.xgc")).unwrap(), manifest);
    std::fs::remove_dir_all(&dir).unwrap();
}
