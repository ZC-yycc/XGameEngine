//! 从项目文件到资产点选的回归；确认浏览不会生成元数据或修改场景。

use std::path::{Path, PathBuf};

use eframe::egui::{self, epaint::Shape};
use xgc_store::{save_project, ProjectContent, ProjectManifest};

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
    project.write("Assets/nested\\fake.prefab.ron");
    project.write("Assets/nested/fake.prefab.ron");
    project.write("Assets/NUL.lua");
    let assets = project.app().state.assets();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].path, "Assets/nested/fake.prefab.ron");
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
    let prefab = "Assets/Pillar.prefab.ron";
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../projects/XGame/Prefabs/Pillar.prefab.ron"),
        project.0.join(prefab),
    )
    .unwrap();
    let mut probe = project.app();
    probe
        .state
        .instantiate_prefab_file(&project.0.join(prefab))
        .unwrap();
    assert!(
        !probe.state.world().is_empty(),
        "夹具必须能实例化，避免解析失败造成假绿"
    );
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
    assert_eq!(Path::new(&app.prefab_path), project.0.join(prefab));
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
