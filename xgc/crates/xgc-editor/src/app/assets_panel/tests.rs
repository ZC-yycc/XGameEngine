//! 从项目文件到资产点选的回归；确认浏览不会生成元数据或修改场景。

use std::path::{Path, PathBuf};

use eframe::egui::{self, epaint::Shape};
use xgc_store::{save_prefab, save_project, ProjectContent, ProjectManifest};
use xgc_world::{Name, Parent, PrefabInstance, Transform, World};

use super::super::EditorApp;
use crate::{assets_ops::MAX_ASSETS, AssetKind, EditorState};

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "xgc_asset_panel_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("Assets/nested")).unwrap();
        xgc_store::save_world(
            &root.join("main.json"),
            &xgc_world::World::new().world_data(),
        )
        .unwrap();
        save_project(
            &root.join("project.xgc"),
            &ProjectManifest {
                format_version: xgc_store::PROJECT_FORMAT_VERSION,
                name: "assets".into(),
                entry_scene: "main.json".into(),
                scenes: vec!["main.json".into()],
                content: ProjectContent {
                    models: "./Assets//".into(),
                    textures: "Assets/nested".into(),
                    audio: "Assets".into(),
                    shaders: None,
                    prefabs: "Assets".into(),
                    scripts: "Assets".into(),
                },
                search_paths: Vec::new(),
                input: None,
            },
        )
        .unwrap();
        Self(root)
    }

    fn write(&self, path: &str) {
        std::fs::write(self.0.join(path), b"fixture").unwrap();
    }

    fn app(&self) -> EditorApp {
        EditorApp::new(EditorState::open(&self.0).unwrap())
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn render(app: &mut EditorApp, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(260.0, 720.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.assets_panel(ui);
            });
        },
    )
}

fn text_position(output: &egui::FullOutput, text: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(shape) if shape.galley.text().starts_with(text) => {
                Some(shape.pos + shape.galley.size() * 0.5)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("缺少可点击文本：{text}"))
}

fn click(app: &mut EditorApp, ctx: &egui::Context, pos: egui::Pos2) {
    for pressed in [true, false] {
        render(
            app,
            ctx,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

#[test]
fn assets_scan_is_bounded_deterministic_and_read_only() {
    let project = Project::new();
    project.write("Assets/.hidden.lua");
    project.write("Assets/model.GLB.META");
    std::fs::create_dir(project.0.join("Assets/.cache")).unwrap();
    project.write("Assets/.cache/hidden.glb");
    // 倒序创建让文件系统枚举顺序不能冒充排序；最后两条必须被截断。
    for index in (0..MAX_ASSETS + 2).rev() {
        project.write(&format!("Assets/model-{index:04}.GLB"));
    }
    let sample = project.0.join("Assets/model-0000.GLB");
    let modified = sample.metadata().unwrap().modified().unwrap();
    let app = project.app();
    let assets = app.state.assets();
    assert_eq!(assets.len(), MAX_ASSETS);
    assert_eq!(assets[0].path, "Assets/model-0000.GLB");
    assert_eq!(assets[MAX_ASSETS - 1].path, "Assets/model-1999.GLB");
    assert!(assets.iter().all(|asset| asset.kind == AssetKind::Model));
    assert_eq!(assets, app.state.assets());
    assert_eq!(sample.metadata().unwrap().modified().unwrap(), modified);
    assert!(!sample.with_extension("GLB.meta").exists());
    assert_eq!(std::fs::read(sample).unwrap(), b"fixture");
}

#[cfg(unix)]
#[test]
fn asset_names_cannot_normalize_into_a_different_file() {
    let project = Project::new();
    project.write("Assets/nested\\fake.prefab.json");
    project.write("Assets/nested/fake.prefab.json");
    project.write("Assets/NUL.lua");
    let assets = project.app().state.assets();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].path, "Assets/nested/fake.prefab.json");
}

#[cfg(windows)]
#[test]
fn assets_skip_windows_hidden_files_directories_and_content_roots() {
    let project = Project::new();
    project.write("Assets/visible.lua");
    project.write("Assets/hidden.lua");
    project.write("Assets/nested/model.glb");
    let hide = |path: &Path| {
        assert!(std::process::Command::new("attrib")
            .arg("+H")
            .arg(path)
            .status()
            .unwrap()
            .success());
    };
    hide(&project.0.join("Assets/hidden.lua"));
    hide(&project.0.join("Assets/nested"));
    let app = project.app();
    let assets = app.state.assets();
    assert_eq!(assets.len(), 1, "Windows 隐藏属性也必须排除");
    assert_eq!(assets[0].path, "Assets/visible.lua");
    hide(&project.0.join("Assets"));
    assert!(app.state.assets().is_empty(), "内容根不能绕过隐藏检查");
}

#[test]
fn assets_expand_cache_refresh_and_prefab_selection_do_not_edit_the_scene() {
    let project = Project::new();
    let prefab = "Assets/Pillar.prefab.json";
    let mut asset = World::new();
    asset.spawn((Transform::default(),));
    save_prefab(&project.0.join(prefab), &asset.world_data()).unwrap();
    let mut probe = project.app();
    let root = probe
        .state
        .instantiate_prefab_file(Path::new("./Assets//Pillar.prefab.json"))
        .unwrap();
    let world = probe.state.world();
    let instance = world
        .get::<PrefabInstance>(world.key(root).unwrap())
        .unwrap();
    assert_eq!(instance.source, prefab);
    assert_eq!(probe.state.revert_selected_prefab().unwrap(), 1);
    let mut app = project.app();
    let ctx = egui::Context::default();
    ctx.style_mut(|style| style.animation_time = 0.0);

    render(&mut app, &ctx, Vec::new());
    let output = render(&mut app, &ctx, Vec::new());
    assert!(app.assets.is_none(), "折叠的资产面板不能扫盘");
    click(&mut app, &ctx, text_position(&output, "Assets"));
    let output = render(&mut app, &ctx, Vec::new());
    assert_eq!(app.assets.as_ref().unwrap().len(), 1);
    click(&mut app, &ctx, text_position(&output, "[prefab]"));
    assert_eq!(app.selected_asset.as_deref(), Some(prefab));
    assert_eq!(app.prefab_path, prefab);
    assert_eq!(app.state.world().len(), 0, "选择预制体不能自动实例化");
    assert!(!app.state.is_dirty());

    std::fs::remove_file(project.0.join(prefab)).unwrap();
    project.write("Assets/move.lua");
    let output = render(&mut app, &ctx, Vec::new());
    assert_eq!(app.assets.as_ref().unwrap()[0].path, prefab, "重绘使用缓存");
    click(&mut app, &ctx, text_position(&output, "Refresh"));
    assert_eq!(app.assets.as_ref().unwrap()[0].path, "Assets/move.lua");
    assert_eq!(app.selected_asset, None);
    assert!(!app.state.is_dirty());
}

#[test]
fn prefab_revert_resolves_guid_and_targets_only_the_selected_instance() {
    let project = Project::new();
    let path = "Assets/Pillar.prefab.json";
    let moved = "Assets/nested/Renamed.prefab.json";
    let guid = "0123456789abcdef0123456789abcdef";
    let mut asset = World::new();
    let asset_root = asset.spawn((
        Name {
            value: "Pillar".into(),
        },
        Transform::default(),
    ));
    let asset_child = asset.spawn((
        Name {
            value: "Child".into(),
        },
        Parent {
            entity: asset.entity(asset_root).unwrap(),
        },
    ));
    let mut app = project.app();
    save_prefab(&project.0.join(path), &asset.world_data()).unwrap();
    let source = |state: &EditorState, id| {
        let world = state.world();
        world
            .get::<PrefabInstance>(world.key(id).unwrap())
            .unwrap()
            .source
            .clone()
    };
    std::fs::write(
        project.0.join(format!("{path}.meta")),
        format!("(guid: \"{guid}\")"),
    )
    .unwrap();
    let first = app.state.instantiate_prefab_file(Path::new(path)).unwrap();
    let second = app
        .state
        .instantiate_prefab_file(&project.0.join(path))
        .unwrap();
    assert_eq!(source(&app.state, first), format!("guid://{guid}"));
    assert_eq!(source(&app.state, first), source(&app.state, second));
    let child = |state: &EditorState, root| {
        let world = state.world();
        let mut child = None;
        world.for_each::<Parent>(|key, parent| {
            if parent.entity == root {
                child = world.entity(key);
            }
        });
        child.unwrap()
    };
    let first_child = child(&app.state, first);
    let second_child = child(&app.state, second);
    app.state
        .set_entity_position(second, [7.0, 8.0, 9.0])
        .unwrap();
    app.state.save().unwrap();
    app = project.app();
    std::fs::rename(project.0.join(path), project.0.join(moved)).unwrap();
    std::fs::rename(
        project.0.join(format!("{path}.meta")),
        project.0.join(format!("{moved}.meta")),
    )
    .unwrap();
    asset.get_mut::<Name>(asset_child).unwrap().value = "Renamed Child".into();
    save_prefab(&project.0.join(moved), &asset.world_data()).unwrap();

    app.state.select(Some(second_child));
    assert!(app.prefab_path.is_empty());
    let ctx = egui::Context::default();
    let render_toolbar = |app: &mut EditorApp, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.toolbar(ui));
            },
        )
    };
    let output = render_toolbar(&mut app, Vec::new());
    let pos = text_position(&output, "Revert");
    for pressed in [true, false] {
        render_toolbar(
            &mut app,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert_eq!(app.status.as_deref(), Some("已还原 2 个实体"));
    assert_eq!(app.state.selected(), Some(second_child));
    let name = |state: &EditorState, child| {
        let world = state.world();
        world
            .get::<Name>(world.key(child).unwrap())
            .unwrap()
            .value
            .clone()
    };
    assert_eq!(name(&app.state, second_child), "Renamed Child");
    assert_eq!(name(&app.state, first_child), "Child");
    assert_eq!(app.state.entity_position(second), Some([7.0, 8.0, 9.0]));
    assert!(app.state.is_dirty());
    assert!(app.state.undo().unwrap());
    assert_eq!(name(&app.state, second_child), "Child");
    assert!(app.state.redo().unwrap());
    assert_eq!(name(&app.state, second_child), "Renamed Child");
}
