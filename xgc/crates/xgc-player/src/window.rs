//! 图形播放器窗口：每帧仿真 + 脚本 + wgpu 视口。

use std::path::{Path, PathBuf};

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use xgc_host::ScriptedRunner;
use xgc_render::{GpuContext, MeshCache, Renderer};
use xgc_world::World;

use crate::input::PlayerInput;

struct Viewport {
    texture: wgpu::Texture,
    id: egui::TextureId,
    renderer: Renderer,
    context: GpuContext,
}

struct PlayerApp {
    dir: PathBuf,
    runner: Option<ScriptedRunner>,
    failed: bool,
    meshes: Option<MeshCache>,
    input: PlayerInput,
    scene: Option<String>,
    error: Option<String>,
    viewport: Option<Viewport>,
    frames: u64,
}

impl PlayerApp {
    fn new(path: &Path, scene: Option<&str>) -> Self {
        let mut app = Self {
            dir: path.to_path_buf(),
            runner: None,
            failed: false,
            meshes: None,
            input: PlayerInput::default(),
            scene: scene.map(str::to_string),
            error: None,
            viewport: None,
            frames: 0,
        };
        if let Err(err) = app.load(path, scene) {
            app.error = Some(err);
        }
        app
    }

    fn load(&mut self, path: &Path, scene: Option<&str>) -> Result<(), String> {
        let loaded = crate::load_scene(path, scene)?;
        let (root, input) = match &loaded.project {
            Some(project) => {
                let path = project
                    .input_path()
                    .map_err(|err| format!("读取项目输入映射失败：{err}"))?;
                (project.root(), PlayerInput::load(path.as_deref())?)
            }
            None => (
                path,
                PlayerInput::load(Some(&path.join("input.ron"))).unwrap_or_default(),
            ),
        };
        let mut world = World::new();
        world
            .restore_world_data(&loaded.data)
            .map_err(|err| format!("恢复世界失败：{err}"))?;
        let (runner, meshes) = ScriptedRunner::with_assets(world, Some(root));
        self.dir = root.to_path_buf();
        self.input = input;
        self.meshes = Some(meshes);
        self.runner = Some(runner);
        Ok(())
    }

    fn ensure_viewport(&mut self, render_state: &RenderState) {
        let Some(meshes) = self.meshes.take() else {
            return;
        };
        let size = [1280, 720];
        let context = GpuContext::from_arc(
            render_state.device.clone(),
            render_state.queue.clone(),
            render_state.adapter.get_info(),
            xgc_render::preferred_sample_count(&render_state.adapter),
        );
        let texture = context.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("xgc-player-viewport"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let id = render_state.renderer.write().register_native_texture(
            context.device(),
            &view,
            wgpu::FilterMode::Linear,
        );
        self.viewport = Some(Viewport {
            texture,
            id,
            renderer: Renderer::new(meshes),
            context,
        });
    }

    fn step(&mut self) {
        let Some(runner) = self.runner.as_mut().filter(|_| !self.failed) else {
            return;
        };
        match runner.step() {
            Ok(_) => self.frames += 1,
            Err(err) => {
                self.failed = true;
                self.error = Some(err);
            }
        }
    }

    fn render_viewport(&mut self) {
        let (Some(runner), Some(viewport)) = (&self.runner, &mut self.viewport) else {
            return;
        };
        viewport
            .renderer
            .set_particles(runner.particle_quads().to_vec());
        if let Err(err) =
            viewport
                .renderer
                .render(&viewport.context, &viewport.texture, runner.world())
        {
            self.error = Some(format!("渲染失败：{err}"));
        }
    }
}

impl eframe::App for PlayerApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(runner) = &mut self.runner {
            self.input.apply(ctx, runner);
        }
        self.step();
        if let Some(render_state) = frame.wgpu_render_state() {
            self.ensure_viewport(render_state);
            self.render_viewport();
        }
        egui::TopBottomPanel::top("xgc-player-status").show(ctx, |ui| {
            let entities = self
                .runner
                .as_ref()
                .map_or(0, |runner| runner.world().len());
            let scene = self
                .scene
                .as_deref()
                .map_or("入口场景".to_string(), |scene| format!("场景 {scene}"));
            ui.label(format!(
                "XGC Player · {} · {} · {} 帧 · {} 实体",
                self.dir.display(),
                scene,
                self.frames,
                entities
            ));
            if let Some(error) = &self.error {
                ui.colored_label(egui::Color32::RED, error);
            }
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(viewport) = &self.viewport {
                ui.image((viewport.id, ui.available_size()));
            } else {
                ui.label("等待 GPU 视口…");
            }
        });
        ctx.request_repaint();
    }
}

pub fn run(dir: &Path) -> eframe::Result {
    run_scene(dir, None)
}

/// 打开窗口并加载指定场景；`None` 走项目入口。
pub fn run_scene(dir: &Path, scene: Option<&str>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };
    let dir = dir.to_path_buf();
    let scene = scene.map(str::to_string);
    eframe::run_native(
        "XGC Player",
        options,
        Box::new(move |_cc| Ok(Box::new(PlayerApp::new(&dir, scene.as_deref())))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgc_store::{save_project, save_world, ProjectContent, ProjectManifest};
    use xgc_world::{Script, Transform};

    #[test]
    fn window_uses_only_the_manifest_input_map() {
        let root = std::env::temp_dir().join(format!(
            "xgc_player_manifest_input_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("Config")).unwrap();
        std::fs::write(
            root.join("move.lua"),
            "function update(ctx) if action_pressed(ctx, 'Jump') then set_pos(ctx, 2, 0, 0) end end",
        )
        .unwrap();
        std::fs::write(
            root.join("Config/controls.ron"),
            r#"InputMap(actions: {"Jump": [Key(Space)]})"#,
        )
        .unwrap();
        std::fs::write(root.join("input.ron"), "InputMap(actions: {})").unwrap();
        let mut world = World::new();
        let key = world.spawn((
            Transform::default(),
            Script {
                path: "move.lua".into(),
            },
        ));
        let id = world.entity(key).unwrap();
        save_world(&root.join("scene.json"), &world.world_data()).unwrap();
        let manifest_path = root.join("project.xgc");
        let mut manifest = ProjectManifest {
            format_version: xgc_store::PROJECT_FORMAT_VERSION,
            name: "controls".into(),
            entry_scene: "scene.json".into(),
            scenes: vec!["scene.json".into()],
            content: ProjectContent {
                models: "Assets".into(),
                textures: "Assets".into(),
                audio: "Assets".into(),
                shaders: None,
                prefabs: "Assets".into(),
                scripts: "Scripts".into(),
            },
            search_paths: Vec::new(),
            input: Some("./Config\\controls.ron".into()),
        };
        let play_one_frame = |path: &Path| {
            let mut app = PlayerApp::new(path, None);
            assert!(app.error.is_none(), "{:?}", app.error);
            let ctx = egui::Context::default();
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::Space,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| app.input.apply(ctx, app.runner.as_mut().unwrap()),
            );
            app.step();
            let world = app.runner.as_ref().unwrap().world();
            world
                .get::<Transform>(world.key(id).unwrap())
                .unwrap()
                .position[0]
        };
        save_project(&manifest_path, &manifest).unwrap();
        assert_eq!(play_one_frame(&root), 2.0);
        assert_eq!(play_one_frame(&manifest_path), 2.0);

        std::fs::copy(root.join("Config/controls.ron"), root.join("input.ron")).unwrap();
        std::fs::remove_file(root.join("Config/controls.ron")).unwrap();
        let missing = PlayerApp::new(&root, None);
        assert!(missing.error.as_deref().unwrap().contains("项目输入映射"));
        assert!(missing.runner.is_none());

        manifest.input = None;
        save_project(&manifest_path, &manifest).unwrap();
        assert_eq!(play_one_frame(&root), 0.0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
