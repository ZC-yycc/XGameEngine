//! 图形播放器窗口：每帧仿真 + 脚本 + wgpu 视口。

use std::path::{Path, PathBuf};

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use xgc_render::{GpuContext, MeshCache, Renderer};
use xgc_script::ScriptRuntime;
use xgc_sim::Simulation;
use xgc_world::World;

struct Viewport {
    texture: wgpu::Texture,
    id: egui::TextureId,
    renderer: Renderer,
    context: GpuContext,
}

struct PlayerApp {
    dir: PathBuf,
    simulation: Option<Simulation>,
    runtime: Option<ScriptRuntime>,
    error: Option<String>,
    viewport: Option<Viewport>,
    frames: u64,
}

impl PlayerApp {
    fn new(dir: &Path) -> Self {
        let mut app = Self {
            dir: dir.to_path_buf(),
            simulation: None,
            runtime: None,
            error: None,
            viewport: None,
            frames: 0,
        };
        match crate::load_world_dir(dir) {
            Ok(data) => {
                let mut world = World::new();
                if let Err(err) = world.restore_world_data(&data) {
                    app.error = Some(format!("恢复世界失败：{err}"));
                    return app;
                }
                app.simulation = Some(Simulation::new(world));
                app.runtime = Some(crate::runtime_for(dir));
            }
            Err(err) => app.error = Some(err),
        }
        app
    }

    fn ensure_viewport(&mut self, render_state: &RenderState) {
        if self.viewport.is_some() {
            return;
        }
        let size = [1280, 720];
        let context = GpuContext::from_arc(
            render_state.device.clone(),
            render_state.queue.clone(),
            render_state.adapter.get_info(),
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
            renderer: Renderer::new(MeshCache::new(self.dir.clone())),
            context,
        });
    }

    fn step(&mut self) {
        let (Some(simulation), Some(runtime)) = (&mut self.simulation, &mut self.runtime) else {
            return;
        };
        if let Err(err) =
            simulation.step_with(Vec::new(), |world| runtime.run_frame(world, 1.0 / 60.0))
        {
            self.error = Some(format!("运行失败：{err:?}"));
        } else {
            self.frames += 1;
        }
    }

    fn render_viewport(&mut self) {
        let (Some(simulation), Some(viewport)) = (&self.simulation, &mut self.viewport) else {
            return;
        };
        if let Err(err) =
            viewport
                .renderer
                .render(&viewport.context, &viewport.texture, simulation.world())
        {
            self.error = Some(format!("渲染失败：{err}"));
        }
    }
}

impl eframe::App for PlayerApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.step();
        if let Some(render_state) = frame.wgpu_render_state() {
            self.ensure_viewport(render_state);
            self.render_viewport();
        }
        egui::TopBottomPanel::top("xgc-player-status").show(ctx, |ui| {
            let entities = self.simulation.as_ref().map_or(0, |sim| sim.world().len());
            ui.label(format!(
                "XGC Player · {} · {} 帧 · {} 实体",
                self.dir.display(),
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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "XGC Player",
        options,
        Box::new(move |_cc| Ok(Box::new(PlayerApp::new(dir)))),
    )
}
