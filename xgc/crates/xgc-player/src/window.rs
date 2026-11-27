//! 图形播放器窗口：每帧仿真 + 脚本 + wgpu 视口。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use xgc_host::window::{fit_viewport, WindowInput};
use xgc_host::{FixedStepClock, ScriptedRunner, WindowAudio};
use xgc_render::{GpuContext, MeshCache, RenderQuality, RenderSettings, Renderer};
use xgc_sim::{InputMap, DEFAULT_UI_VIEWPORT};

use crate::SceneSource;

struct Viewport {
    texture: wgpu::Texture,
    id: egui::TextureId,
    renderer: Renderer,
    context: GpuContext,
}

struct PlayerApp {
    dir: PathBuf,
    runner: Option<ScriptedRunner>,
    clock: Option<FixedStepClock>,
    last_update: Option<Instant>,
    failed: bool,
    meshes: Option<MeshCache>,
    render_settings: (RenderSettings, RenderQuality),
    input_map: Option<InputMap>,
    input: WindowInput,
    audio: WindowAudio,
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
            clock: None,
            last_update: None,
            failed: false,
            meshes: None,
            render_settings: Default::default(),
            input_map: None,
            input: WindowInput::default(),
            audio: WindowAudio::default(),
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
        let input = match &loaded.source {
            SceneSource::Project(project) => project
                .input_path()
                .map_err(|err| format!("读取项目输入映射失败：{err}"))?
                .as_deref()
                .map(InputMap::load)
                .transpose()?,
            SceneSource::Cooked {
                root,
                runtime: Some(runtime),
            } => runtime
                .input
                .as_ref()
                .map(|input| InputMap::load(&root.join(input)))
                .transpose()?,
            SceneSource::Cooked {
                root,
                runtime: None,
            } => InputMap::load(&root.join("input.ron")).ok(),
        };
        let (runner, meshes) = loaded.runner()?;
        self.dir = loaded.source.root().to_path_buf();
        self.render_settings = loaded.source.render_settings();
        self.input_map = input;
        self.meshes = Some(meshes);
        self.clock = Some(FixedStepClock::new(runner.fixed_dt()));
        self.runner = Some(runner);
        Ok(())
    }

    fn ensure_viewport(&mut self, render_state: &RenderState) {
        let Some(meshes) = self.meshes.take() else {
            return;
        };
        let size = DEFAULT_UI_VIEWPORT;
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
        let mut renderer = Renderer::new(meshes);
        let (render, quality) = self.render_settings;
        renderer.apply_project_settings(&context, render, quality);
        self.viewport = Some(Viewport {
            texture,
            id,
            renderer,
            context,
        });
    }

    fn step(&mut self) {
        let Some(runner) = self.runner.as_mut().filter(|_| !self.failed) else {
            return;
        };
        match runner.step_window(self.input_map.as_ref(), &mut self.input) {
            Ok(_) => {
                self.frames += 1;
                self.audio.submit(runner.audio_samples());
            }
            Err(err) => {
                self.audio.clear();
                self.failed = true;
                self.error = Some(err);
            }
        }
    }

    fn tick(&mut self, elapsed: Duration) -> Option<Duration> {
        let clock = self.clock.as_mut().filter(|_| !self.failed)?;
        let (steps, wait) = clock.advance(elapsed);
        for _ in 0..steps {
            self.step();
            if self.failed {
                return None;
            }
        }
        Some(wait)
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
        let now = Instant::now();
        let elapsed = self
            .last_update
            .replace(now)
            .map_or(Duration::ZERO, |previous| {
                now.saturating_duration_since(previous)
            });
        if let Some(render_state) = frame.wgpu_render_state() {
            self.ensure_viewport(render_state);
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
        let wait = egui::CentralPanel::default()
            .show(ctx, |ui| {
                let image_rect = fit_viewport(ui.available_rect_before_wrap(), DEFAULT_UI_VIEWPORT)
                    .filter(|_| self.viewport.is_some());
                self.input.sample(ctx, image_rect);
                let was_running = !self.failed;
                let wait = self.tick(elapsed);
                self.render_viewport();
                if was_running && self.failed {
                    ctx.request_repaint();
                }
                if let (Some(viewport), Some(rect)) = (&self.viewport, image_rect) {
                    ui.put(rect, egui::Image::new((viewport.id, rect.size())));
                } else if self.viewport.is_none() {
                    ui.label("等待 GPU 视口…");
                }
                wait
            })
            .inner;
        if let Some(wait) = wait {
            ctx.request_repaint_after(wait.saturating_sub(now.elapsed()));
        }
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
    use xgc_world::{Script, Transform, World};

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
                paths: vec!["move.lua".into()],
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
            layers: Default::default(),
            input: Some("./Config\\controls.ron".into()),
            render: RenderSettings {
                exposure: 2.25,
                ..Default::default()
            },
            quality: RenderQuality {
                msaa: Some(1),
                shadow_resolution: Some(1024),
                ..Default::default()
            },
        };
        let render_settings = (manifest.render, manifest.quality);
        let play_one_frame = |path: &Path| {
            let mut app = PlayerApp::new(path, None);
            assert!(app.error.is_none(), "{:?}", app.error);
            assert_eq!(app.render_settings, render_settings);
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
                |ctx| app.input.sample(ctx, None),
            );
            let step = Duration::from_secs_f32(app.runner.as_ref().unwrap().fixed_dt());
            let short_redraw = Duration::from_millis(2);
            app.tick(Duration::ZERO);
            app.tick(short_redraw);
            assert_eq!(app.frames, 0, "重绘频率不能推进固定步");
            app.tick(step - short_redraw);
            assert_eq!(app.frames, 1);
            let world = app.runner.as_ref().unwrap().world();
            world
                .get::<Transform>(world.key(id).unwrap())
                .unwrap()
                .position[0]
        };
        let cooked = root.join("cooked");
        let pack = |manifest: &ProjectManifest| {
            xgc_asset::cook_project_with_runtime(
                &root,
                &cooked,
                xgc_asset::CookRuntime {
                    search_paths: manifest.search_paths.clone(),
                    input: manifest.input.clone(),
                    render: manifest.render,
                    quality: manifest.quality,
                    layers: manifest.layers.clone(),
                },
                None,
            )
            .unwrap();
            save_world(&cooked.join("world.json"), &world.world_data()).unwrap();
        };
        save_project(&manifest_path, &manifest).unwrap();
        assert_eq!(play_one_frame(&root), 2.0);
        assert_eq!(play_one_frame(&manifest_path), 2.0);
        pack(&manifest);
        assert_eq!(play_one_frame(&cooked), 2.0);

        std::fs::copy(root.join("Config/controls.ron"), root.join("input.ron")).unwrap();
        std::fs::remove_file(root.join("Config/controls.ron")).unwrap();
        let missing = PlayerApp::new(&root, None);
        assert!(missing.error.as_deref().unwrap().contains("项目输入映射"));
        assert!(missing.runner.is_none());

        manifest.input = None;
        save_project(&manifest_path, &manifest).unwrap();
        assert_eq!(play_one_frame(&root), 0.0);
        pack(&manifest);
        assert_eq!(play_one_frame(&cooked), 0.0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
