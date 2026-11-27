//! eframe 编辑器窗口：工具栏、层级、Inspector 和 3D 视口。

use std::path::PathBuf;
use std::time::{Duration, Instant};

mod assets_panel;
mod inspector;
mod toolbar;

#[cfg(test)]
mod editor_city_tests;
#[cfg(test)]
mod hierarchy_tests;
#[cfg(test)]
mod hud_tests;
#[cfg(test)]
mod reload_tests;

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use xgc_host::window::{fit_viewport, WindowInput};
use xgc_host::WindowAudio;
use xgc_sim::DEFAULT_UI_VIEWPORT;

use xgc_render::{GpuContext, MeshCache, Renderer};

use crate::gizmo::{distance_to_segment, project};
use crate::transform_ops::snap;
use crate::{AssetEntry, EditorState};
use xgc_base::{EntityId, EntityKey};
use xgc_render::camera_view_projection;

struct Viewport {
    size: [u32; 2],
    texture: wgpu::Texture,
    id: egui::TextureId,
    renderer: Renderer,
    context: GpuContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GizmoMode {
    Translate,
    Rotate,
    Scale,
}

struct GizmoDrag {
    entity: EntityId,
    axis: [f32; 3],
    start: [f32; 3],
    start_scale: [f32; 3],
    direction: egui::Vec2,
    pixels_per_unit: f32,
    mode: GizmoMode,
    last_amount: f32,
}

pub struct EditorApp {
    state: EditorState,
    status: Option<String>,
    fonts_installed: bool,
    viewport: Option<Viewport>,
    yaw: f32,
    pitch: f32,
    distance: f32,
    gizmo: Option<GizmoDrag>,
    gizmo_mode: GizmoMode,
    snap: bool,
    grid: f32,
    angle_step: f32,
    scale_step: f32,
    prefab_path: String,
    prefab_position: [f32; 3],
    last_play_update: Option<Instant>,
    next_script_scan: Option<Instant>,
    play_input: WindowInput,
    play_audio: WindowAudio,
    assets: Option<Vec<AssetEntry>>,
    selected_asset: Option<String>,
    hierarchy_keys: Vec<(EntityId, EntityKey)>,
}

impl EditorApp {
    pub fn new(state: EditorState) -> Self {
        Self {
            state,
            status: None,
            fonts_installed: false,
            viewport: None,
            yaw: 0.0,
            pitch: 0.0,
            distance: 5.0,
            gizmo: None,
            gizmo_mode: GizmoMode::Translate,
            snap: true,
            grid: 0.5,
            angle_step: 15.0,
            scale_step: 0.1,
            prefab_path: String::new(),
            prefab_position: [0.0; 3],
            last_play_update: None,
            next_script_scan: None,
            play_input: WindowInput::default(),
            play_audio: WindowAudio::default(),
            assets: None,
            selected_asset: None,
            hierarchy_keys: Vec::new(),
        }
    }

    fn viewport_meshes(&self) -> MeshCache {
        MeshCache::with_assets(self.state.asset_paths())
    }

    fn ensure_viewport(&mut self, render_state: &RenderState) {
        let size = if self.state.is_playing() {
            DEFAULT_UI_VIEWPORT
        } else {
            [800, 450]
        };
        if self
            .viewport
            .as_ref()
            .is_some_and(|viewport| viewport.size == size)
        {
            return;
        }
        let (renderer, context) = match self.viewport.take() {
            Some(old) => {
                render_state.renderer.write().free_texture(&old.id);
                (old.renderer, old.context)
            }
            None => {
                let context = GpuContext::from_arc(
                    render_state.device.clone(),
                    render_state.queue.clone(),
                    render_state.adapter.get_info(),
                    xgc_render::preferred_sample_count(&render_state.adapter),
                );
                let mut renderer = Renderer::new(self.viewport_meshes());
                if let Some(project) = &self.state.project {
                    let manifest = project.manifest();
                    renderer.apply_project_settings(&context, manifest.render, manifest.quality);
                }
                (renderer, context)
            }
        };
        let texture = context.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("xgc-editor-viewport"),
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
            size,
            texture,
            id,
            renderer,
            context,
        });
    }

    fn render_viewport(&mut self) {
        let particles = self.state.particle_quads().to_vec();
        let Some(viewport) = &mut self.viewport else {
            return;
        };
        viewport.renderer.set_particles(particles);
        let result = if self.state.is_playing() {
            viewport
                .renderer
                .render(&viewport.context, &viewport.texture, self.state.world())
        } else {
            viewport.renderer.render_selected(
                &viewport.context,
                &viewport.texture,
                self.state.world(),
                self.state.selected(),
            )
        };
        if let Err(err) = result {
            self.status = Some(format!("视口渲染失败：{err}"));
        }
    }

    fn paint_gizmo(
        &mut self,
        painter: &egui::Painter,
        image_rect: egui::Rect,
        response: &egui::Response,
        render_size: [u32; 2],
    ) {
        let Some(entity) = self.state.selected() else {
            return;
        };
        let Some(position) = self.state.entity_position(entity) else {
            return;
        };
        let size = [image_rect.width(), image_rect.height()];
        let Some(mvp) = camera_view_projection(self.state.world(), render_size[0], render_size[1])
        else {
            return;
        };
        let Some(origin) =
            project(mvp, position, size).map(|point| point + image_rect.min.to_vec2())
        else {
            return;
        };
        let axes = [
            ([1.0, 0.0, 0.0], egui::Color32::from_rgb(230, 80, 80)),
            ([0.0, 1.0, 0.0], egui::Color32::from_rgb(80, 210, 90)),
            ([0.0, 0.0, 1.0], egui::Color32::from_rgb(80, 130, 230)),
        ];
        let mut handles = Vec::new();
        for (axis, color) in axes {
            let target = [
                position[0] + axis[0],
                position[1] + axis[1],
                position[2] + axis[2],
            ];
            let Some(end) =
                project(mvp, target, size).map(|point| point + image_rect.min.to_vec2())
            else {
                continue;
            };
            painter.line_segment([origin, end], egui::Stroke::new(2.0_f32, color));
            painter.circle_filled(end, 3.0, color);
            handles.push((axis, end));
        }

        if response.drag_started() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let mut best: Option<([f32; 3], egui::Pos2, f32)> = None;
                for (axis, end) in &handles {
                    let distance = distance_to_segment(pointer, origin, *end);
                    if distance <= 12.0
                        && best
                            .as_ref()
                            .is_none_or(|(_, _, current)| distance < *current)
                    {
                        best = Some((*axis, *end, distance));
                    }
                }
                if let Some((axis, end, _)) = best {
                    let direction = (end - origin).normalized();
                    let pixels_per_unit = (end - origin).length().max(1.0);
                    self.state.checkpoint();
                    self.gizmo = Some(GizmoDrag {
                        entity,
                        axis,
                        start: position,
                        start_scale: self.state.entity_scale(entity).unwrap_or([1.0; 3]),
                        direction,
                        pixels_per_unit,
                        mode: self.gizmo_mode,
                        last_amount: 0.0,
                    });
                }
            }
        }
        if response.dragged() {
            if let Some(drag) = &mut self.gizmo {
                let delta = response.drag_delta();
                let amount = delta.dot(drag.direction) / drag.pixels_per_unit;
                match drag.mode {
                    GizmoMode::Translate => {
                        let mut next = [
                            drag.start[0] + drag.axis[0] * amount,
                            drag.start[1] + drag.axis[1] * amount,
                            drag.start[2] + drag.axis[2] * amount,
                        ];
                        if self.snap {
                            for value in &mut next {
                                *value = snap(*value, self.grid);
                            }
                        }
                        if let Err(err) = self.state.set_entity_position(drag.entity, next) {
                            self.status = Some(err);
                        }
                    }
                    GizmoMode::Scale => {
                        let axis = drag
                            .axis
                            .iter()
                            .position(|value| value.abs() > 0.5)
                            .unwrap_or(0);
                        let mut scale = drag.start_scale;
                        scale[axis] = (scale[axis] + amount).max(0.01);
                        if self.snap {
                            scale[axis] = snap(scale[axis], self.scale_step).max(0.01);
                        }
                        if let Err(err) = self.state.set_entity_scale(drag.entity, scale) {
                            self.status = Some(err);
                        }
                    }
                    GizmoMode::Rotate => {
                        let mut degrees = (amount - drag.last_amount) * 90.0;
                        if self.snap {
                            degrees = snap(degrees, self.angle_step);
                        }
                        drag.last_amount = amount;
                        if let Err(err) = self.state.rotate_entity(drag.entity, drag.axis, degrees)
                        {
                            self.status = Some(err);
                        }
                    }
                }
            }
        }
        if response.drag_stopped() {
            self.gizmo = None;
        }
    }

    fn hierarchy(&mut self, ui: &mut egui::Ui) {
        ui.heading("Hierarchy");
        let scenes = self.state.scenes().to_vec();
        if !scenes.is_empty() {
            ui.label("Scenes");
            let current = self.state.current_scene().map(str::to_owned);
            for scene in &scenes {
                if ui
                    .selectable_label(current.as_deref() == Some(scene.as_str()), scene)
                    .clicked()
                {
                    match self.state.open_scene(scene) {
                        Ok(()) => self.status = Some(format!("已打开场景 {scene}")),
                        Err(err) => self.status = Some(err),
                    }
                }
            }
            ui.separator();
        }
        self.assets_panel(ui);
        let selected = self.state.selected();
        self.state.collect_entity_keys(&mut self.hierarchy_keys);
        let row_height = (ui.text_style_height(&egui::TextStyle::Button)
            + 2.0 * ui.spacing().button_padding.y)
            .max(ui.spacing().interact_size.y);
        egui::ScrollArea::vertical()
            // 可见名称变短时，滚轮命中区仍覆盖整个列表宽度。
            .auto_shrink([false, true])
            .show_rows(ui, row_height, self.hierarchy_keys.len(), |ui, rows| {
                // 行高固定，长名称截断，悬停仍可看到完整名称和父 ID。
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                for &(id, key) in &self.hierarchy_keys[rows] {
                    let row = self.state.entity_row(id, key);
                    let label = match row.parent {
                        Some(parent) => format!("{id}  {}  (parent {parent})", row.name),
                        None => format!("{id}  {}", row.name),
                    };
                    let response = ui
                        .push_id(id, |ui| ui.selectable_label(selected == Some(id), &label))
                        .inner
                        .on_hover_text(label);
                    if response.clicked() {
                        self.state.select(Some(id));
                    }
                }
            });
    }
    fn poll_play_scripts(&mut self, ctx: &egui::Context, now: Instant) {
        const INTERVAL: Duration = Duration::from_secs(1);
        if !self.state.is_playing() {
            self.next_script_scan = None;
            return;
        }
        let next = *self.next_script_scan.get_or_insert(now + INTERVAL);
        if now < next {
            ctx.request_repaint_after(next - now);
            return;
        }
        self.next_script_scan = Some(now + INTERVAL);
        match self.state.poll_script_reload() {
            Ok(0) => {}
            Ok(count) => {
                self.status = Some(format!("已重载 {count} 个脚本挂载"));
                ctx.request_repaint();
            }
            Err(err) => {
                self.status = Some(format!("脚本重载失败：{err}"));
                ctx.request_repaint();
            }
        }
        ctx.request_repaint_after(INTERVAL);
    }

    fn tick_play(&mut self, ctx: &egui::Context, now: Instant, viewport: Option<egui::Rect>) {
        if !self.state.is_playing() {
            if self.last_play_update.take().is_some() {
                self.play_audio.clear();
            }
            self.play_input = WindowInput::default();
            return;
        }
        self.play_input.sample(ctx, viewport);
        if self.state.is_paused() {
            if self.last_play_update.take().is_some() {
                self.play_audio.clear();
            }
            self.play_input.discard_edges();
            return;
        }
        let elapsed = self
            .last_play_update
            .replace(now)
            .map_or(Duration::ZERO, |previous| {
                now.saturating_duration_since(previous)
            });
        match self
            .state
            .advance_play_with_io(elapsed, Some(&mut self.play_input), |samples| {
                self.play_audio.submit(samples)
            }) {
            Ok(Some(wait)) => ctx.request_repaint_after(wait.saturating_sub(now.elapsed())),
            Ok(None) => self.last_play_update = None,
            Err(err) => {
                self.last_play_update = None;
                self.play_input.discard_edges();
                self.play_audio.clear();
                self.status = Some(err);
                ctx.request_repaint();
            }
        }
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !self.fonts_installed {
            install_fonts(ctx);
            self.fonts_installed = true;
        }
        let (undo, redo) = ctx.input(|input| {
            let command = input.modifiers.command;
            (
                command && input.key_pressed(egui::Key::Z) && !input.modifiers.shift,
                command
                    && (input.key_pressed(egui::Key::Y)
                        || (input.modifiers.shift && input.key_pressed(egui::Key::Z))),
            )
        });
        if undo && !self.state.is_playing() {
            self.status = Some(match self.state.undo() {
                Ok(true) => "已撤销".to_string(),
                Ok(false) => "没有可撤销操作".to_string(),
                Err(err) => err,
            });
        }
        if redo && !self.state.is_playing() {
            self.status = Some(match self.state.redo() {
                Ok(true) => "已重做".to_string(),
                Ok(false) => "没有可重做操作".to_string(),
                Err(err) => err,
            });
        }
        let now = Instant::now();
        // 工具栏可以执行暂停单步，先处理到期的脚本修改再画按钮。
        self.poll_play_scripts(ctx, now);
        egui::TopBottomPanel::top("xgc-toolbar").show(ctx, |ui| self.toolbar(ui));
        if let Some(render_state) = frame.wgpu_render_state() {
            self.ensure_viewport(render_state);
        }
        egui::TopBottomPanel::bottom("xgc-status").show(ctx, |ui| {
            let mode = if self.state.is_paused() {
                "PAUSED"
            } else if self.state.is_playing() {
                "PLAY"
            } else {
                "EDIT"
            };
            ui.horizontal(|ui| {
                ui.strong(mode);
                if self.state.is_dirty() {
                    ui.label("未保存");
                }
                ui.separator();
                let path = self.state.path().map(|path| path.to_string_lossy());
                let text = self
                    .status
                    .as_deref()
                    .or(path.as_deref())
                    .unwrap_or("未打开场景");
                ui.add(egui::Label::new(text).truncate())
                    .on_hover_text(text);
            });
        });
        egui::SidePanel::left("xgc-hierarchy")
            .default_width(260.0)
            .show(ctx, |ui| self.hierarchy(ui));
        egui::SidePanel::right("xgc-inspector")
            .default_width(360.0)
            .width_range(280.0..=460.0)
            .show(ctx, |ui| {
                ui.set_min_width(ui.available_width());
                self.inspector(ui);
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            let image_rect = self
                .viewport
                .as_ref()
                .and_then(|viewport| fit_viewport(ui.available_rect_before_wrap(), viewport.size));
            self.tick_play(ctx, now, image_rect);
            self.render_viewport();
            let Some(viewport) = &self.viewport else {
                ui.label("没有 wgpu 渲染后端，视口不可用");
                return;
            };
            let Some(image_rect) = image_rect else {
                return;
            };
            let render_size = viewport.size;
            let response = ui.allocate_rect(image_rect, egui::Sense::click_and_drag());
            let painter = ui.painter_at(image_rect);
            painter.image(
                viewport.id,
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            if self.state.is_playing() {
                self.gizmo = None;
                return;
            }
            self.paint_gizmo(&painter, image_rect, &response, render_size);
            if response.hovered() {
                let scroll = ui.input(|input| input.raw_scroll_delta.y);
                if scroll != 0.0 {
                    self.distance = (self.distance * (1.0 - scroll * 0.001)).clamp(0.5, 50.0);
                    if let Err(err) =
                        self.state
                            .set_camera_orbit(self.yaw, self.pitch, self.distance)
                    {
                        self.status = Some(err);
                    }
                }
            }
        });
    }
}

fn install_fonts(ctx: &egui::Context) {
    let Some(bytes) = xgc_render::load_system_cjk_font() else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    let name = "system-cjk";
    fonts
        .font_data
        .insert(name.to_owned(), egui::FontData::from_owned(bytes).into());
    // 保留西文和图标的内置字体，只有缺字时才访问系统字体。
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(name.to_owned());
    }
    ctx.set_fonts(fonts);
}

pub fn run(path: Option<PathBuf>) -> eframe::Result {
    let state = match path {
        Some(path) => match EditorState::open(&path) {
            Ok(state) => state,
            Err(err) => {
                eprintln!("{err}");
                return Ok(());
            }
        },
        None => EditorState::demo(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "XGC Editor",
        options,
        Box::new(|_cc| Ok(Box::new(EditorApp::new(state)))),
    )
}
