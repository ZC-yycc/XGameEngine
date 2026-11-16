//! 最小 eframe 编辑器：顶部工具栏、左侧层级、中央 Inspector。

use std::path::PathBuf;

mod toolbar;

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use xgc_sim::{InputState, KeyCode, MouseButton};

use xgc_render::{GpuContext, MeshCache, Renderer};

use crate::gizmo::{distance_to_segment, project};
use crate::transform_ops::snap;
use crate::{EditorState, FieldRow};
use xgc_base::EntityId;
use xgc_reflect::Value;
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
    script_root: String,
}

impl EditorApp {
    pub fn new(state: EditorState) -> Self {
        Self {
            state,
            status: None,
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
            script_root: String::new(),
        }
    }

    fn ensure_viewport(&mut self, render_state: &RenderState) {
        let size = [800, 450];
        if self
            .viewport
            .as_ref()
            .is_some_and(|viewport| viewport.size == size)
        {
            return;
        }
        if let Some(old) = self.viewport.take() {
            render_state.renderer.write().free_texture(&old.id);
        }
        let context = GpuContext::from_arc(
            render_state.device.clone(),
            render_state.queue.clone(),
            render_state.adapter.get_info(),
        );
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
            renderer: Renderer::new(MeshCache::new(".")),
            context,
        });
    }

    fn render_viewport(&mut self) {
        let Some(viewport) = &mut self.viewport else {
            return;
        };
        if let Err(err) = viewport.renderer.render_selected(
            &viewport.context,
            &viewport.texture,
            self.state.world(),
            self.state.selected(),
        ) {
            self.status = Some(format!("视口渲染失败：{err}"));
        }
    }

    fn paint_gizmo(
        &mut self,
        _ui: &mut egui::Ui,
        painter: &egui::Painter,
        image_rect: egui::Rect,
        response: &egui::Response,
    ) {
        let Some(entity) = self.state.selected() else {
            return;
        };
        let Some(position) = self.state.entity_position(entity) else {
            return;
        };
        let size = [image_rect.width(), image_rect.height()];
        let Some(mvp) = camera_view_projection(self.state.world(), 800, 450) else {
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
        let selected = self.state.selected();
        let rows = self.state.entities();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for row in rows {
                let label = match row.parent {
                    Some(parent) => format!("{}  (parent {})", row.name, parent.raw()),
                    None => row.name,
                };
                if ui
                    .selectable_label(
                        selected == Some(row.id),
                        format!("{}  {}", row.id.raw(), label),
                    )
                    .clicked()
                {
                    self.state.select(Some(row.id));
                }
            }
        });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("Inspector");
        let Some(id) = self.state.selected() else {
            ui.label("未选择实体");
            return;
        };
        ui.label(format!("Entity {}", id.raw()));
        let names = self.state.component_names();
        let mut chosen = None;
        egui::ComboBox::from_id_salt("xgc-add-component")
            .selected_text("Add Component")
            .show_ui(ui, |ui| {
                for name in &names {
                    if ui.selectable_label(false, name).clicked() {
                        chosen = Some(name.clone());
                    }
                }
            });
        if let Some(name) = chosen {
            self.status = Some(match self.state.add_component(id, &name) {
                Ok(()) => format!("已添加 {name}"),
                Err(err) => err,
            });
        }
        let components = self.state.inspector();
        for component in components {
            let mut remove = false;
            ui.horizontal(|ui| {
                ui.strong(&component.name);
                if ui.small_button("Remove").clicked() {
                    remove = true;
                }
            });
            if remove {
                self.status = Some(match self.state.remove_component(id, component.id) {
                    Ok(()) => format!("已移除 {}", component.name),
                    Err(err) => err,
                });
                continue;
            }
            for field in &component.fields {
                field_row(ui, &mut self.state, id, component.id, field);
            }
            ui.separator();
        }
    }
}

fn field_row(
    ui: &mut egui::Ui,
    state: &mut EditorState,
    entity: xgc_base::EntityId,
    component: u32,
    field: &FieldRow,
) {
    ui.horizontal(|ui| {
        ui.label(format!("{} ({})", field.name, field.ty));
        let Some(edited) = value_editor(ui, &field.value, &field.ty) else {
            return;
        };
        if edited != field.value {
            if let Err(err) = state.edit_field(entity, component, &field.name, edited) {
                ui.colored_label(egui::Color32::RED, err);
            }
        }
    });
}

fn value_editor(ui: &mut egui::Ui, value: &Value, ty: &str) -> Option<Value> {
    match value {
        Value::Bool(current) => {
            let mut edited = *current;
            ui.checkbox(&mut edited, "");
            edited.ne(current).then_some(Value::Bool(edited))
        }
        Value::I64(current) => {
            let mut edited = *current;
            ui.add(egui::DragValue::new(&mut edited).speed(1.0));
            edited.ne(current).then_some(Value::I64(edited))
        }
        Value::F64(current) => {
            let mut edited = *current;
            ui.add(egui::DragValue::new(&mut edited).speed(0.01));
            edited.ne(current).then_some(Value::F64(edited))
        }
        Value::String(current) => {
            let mut edited = current.clone();
            ui.text_edit_singleline(&mut edited);
            edited.ne(current).then_some(Value::String(edited))
        }
        Value::Entity(current) => {
            ui.label(format!("entity {current}"));
            None
        }
        Value::Vec2(current) => {
            let mut edited = *current;
            for component in &mut edited {
                ui.add(egui::DragValue::new(component).speed(0.01));
            }
            edited.ne(current).then_some(Value::Vec2(edited))
        }
        Value::Vec3(current) => {
            let mut edited = *current;
            for component in &mut edited {
                ui.add(egui::DragValue::new(component).speed(0.01));
            }
            edited.ne(current).then_some(Value::Vec3(edited))
        }
        Value::Vec4(current) | Value::Quat(current) => {
            let mut edited = *current;
            for component in &mut edited {
                ui.add(egui::DragValue::new(component).speed(0.01));
            }
            let next = if ty == "quat" {
                Value::Quat(edited)
            } else {
                Value::Vec4(edited)
            };
            (edited != *current).then_some(next)
        }
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.state.apply_input(&input_state(ctx));
        let (undo, redo) = ctx.input(|input| {
            let command = input.modifiers.command;
            (
                command && input.key_pressed(egui::Key::Z) && !input.modifiers.shift,
                command
                    && (input.key_pressed(egui::Key::Y)
                        || (input.modifiers.shift && input.key_pressed(egui::Key::Z))),
            )
        });
        if undo {
            self.status = Some(match self.state.undo() {
                Ok(true) => "已撤销".to_string(),
                Ok(false) => "没有可撤销操作".to_string(),
                Err(err) => err,
            });
        }
        if redo {
            self.status = Some(match self.state.redo() {
                Ok(true) => "已重做".to_string(),
                Ok(false) => "没有可重做操作".to_string(),
                Err(err) => err,
            });
        }
        if let Some(render_state) = frame.wgpu_render_state() {
            self.ensure_viewport(render_state);
            self.render_viewport();
        }
        egui::TopBottomPanel::top("xgc-toolbar").show(ctx, |ui| self.toolbar(ui));
        egui::SidePanel::left("xgc-hierarchy")
            .default_width(260.0)
            .show(ctx, |ui| self.hierarchy(ui));
        egui::SidePanel::right("xgc-inspector")
            .default_width(380.0)
            .show(ctx, |ui| self.inspector(ui));
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(viewport) = &self.viewport else {
                ui.label("没有 wgpu 渲染后端，视口不可用");
                return;
            };
            let rect = ui.available_rect_before_wrap();
            let image_rect = egui::Rect::from_min_size(
                rect.min,
                egui::vec2(viewport.size[0] as f32, viewport.size[1] as f32),
            );
            let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
            let painter = ui.painter_at(rect);
            painter.image(
                viewport.id,
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            self.paint_gizmo(ui, &painter, image_rect, &response);
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

fn input_state(ctx: &egui::Context) -> InputState {
    let mut state = InputState::default();
    ctx.input(|input| {
        let mut key = |code: egui::Key, mapped: KeyCode| {
            if input.key_down(code) {
                state.keys.insert(mapped);
            }
        };
        key(egui::Key::Space, KeyCode::Space);
        key(egui::Key::W, KeyCode::KeyW);
        key(egui::Key::A, KeyCode::KeyA);
        key(egui::Key::S, KeyCode::KeyS);
        key(egui::Key::D, KeyCode::KeyD);
        key(egui::Key::E, KeyCode::KeyE);
        key(egui::Key::Q, KeyCode::KeyQ);
        key(egui::Key::R, KeyCode::KeyR);
        key(egui::Key::P, KeyCode::KeyP);
        key(egui::Key::ArrowUp, KeyCode::ArrowUp);
        key(egui::Key::ArrowDown, KeyCode::ArrowDown);
        key(egui::Key::ArrowLeft, KeyCode::ArrowLeft);
        key(egui::Key::ArrowRight, KeyCode::ArrowRight);
        if input.modifiers.shift {
            state.keys.insert(KeyCode::ShiftLeft);
        }
        if input.pointer.primary_down() {
            state.mouse.insert(MouseButton::Left);
        }
        if input.pointer.secondary_down() {
            state.mouse.insert(MouseButton::Right);
        }
        if input.pointer.middle_down() {
            state.mouse.insert(MouseButton::Middle);
        }
    });
    state
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
