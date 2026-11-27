use eframe::egui;
use egui::epaint::Shape;
use xgc_base::EntityId;
use xgc_world::{Name, Parent, World};

use super::{EditorApp, EditorState};

struct HierarchyUi {
    app: EditorApp,
    ctx: egui::Context,
    width: u32,
}

impl HierarchyUi {
    fn new(world: World, width: u32) -> Self {
        let ctx = egui::Context::default();
        super::install_fonts(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);
        Self {
            app: EditorApp::new(EditorState::new(world)),
            ctx,
            width,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        self.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(self.width as f32, 720.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::top("xgc-toolbar").show(ctx, |ui| self.app.toolbar(ui));
                egui::SidePanel::left("xgc-hierarchy")
                    .default_width(260.0)
                    .show(ctx, |ui| self.app.hierarchy(ui));
                egui::SidePanel::right("xgc-inspector")
                    .default_width(360.0)
                    .width_range(280.0..=460.0)
                    .show(ctx, |ui| {
                        ui.set_min_width(ui.available_width());
                        self.app.inspector(ui);
                    });
                egui::CentralPanel::default().show(ctx, |_| {});
            },
        )
    }

    fn button(&mut self, pos: egui::Pos2, pressed: bool) -> egui::FullOutput {
        self.frame(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ])
    }

    fn click(&mut self, pos: egui::Pos2) -> egui::FullOutput {
        self.button(pos, true);
        self.button(pos, false)
    }

    fn scroll_to(&mut self, target: &str, delta: f32) -> egui::FullOutput {
        for _ in 0..12 {
            let output = self.frame(vec![
                egui::Event::PointerMoved(egui::pos2(100.0, 350.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, delta),
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            if visible_text(&output, target).is_some() {
                return output;
            }
        }
        panic!("Hierarchy 滚动后未显示：{target}");
    }
}

fn visible_text(output: &egui::FullOutput, text: &str) -> Option<egui::Pos2> {
    output.shapes.iter().find_map(|item| match &item.shape {
        Shape::Text(shape) if shape.galley.text() == text => {
            let pos = shape.pos + shape.galley.size() * 0.5;
            item.clip_rect.contains(pos).then_some(pos)
        }
        _ => None,
    })
}

fn text(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    visible_text(output, label).unwrap_or_else(|| panic!("缺少可见文本：{label}"))
}

fn name(value: &str) -> Name {
    Name {
        value: value.into(),
    }
}

fn seed() -> (World, EntityId, EntityId) {
    let mut world = World::new();
    let root = world.spawn(());
    let root = world.entity(root).unwrap();
    let first = world.spawn((name("first child"), Parent { entity: root }));
    let first = world.entity(first).unwrap();
    world.spawn((name(&"long hierarchy name ".repeat(12)),));
    for index in 3..200 {
        world.spawn((name(&format!("row {index}")),));
    }
    let last = world.spawn((name("last child"), Parent { entity: root }));
    let last = world.entity(last).unwrap();
    (world, first, last)
}

fn first_label(first: EntityId) -> String {
    format!("{}  first child  (parent 0)", first.raw())
}

fn last_label(last: EntityId) -> String {
    format!("{}  last child  (parent 0)", last.raw())
}

#[test]
fn hierarchy_release_does_not_select_a_replacement_row() {
    let (world, _, last) = seed();
    let mut ui = HierarchyUi::new(world, 1280);
    ui.frame(Vec::new());
    let label = last_label(last);
    let output = ui.scroll_to(&label, -10_000.0);
    let pos = text(&output, &label);
    ui.button(pos, true);
    let world = ui.app.state.world_mut().unwrap();
    let key = world.key(last).unwrap();
    assert!(world.despawn(key));
    let replacement = world.spawn((
        name("new child"),
        Parent {
            entity: EntityId::from_raw(0),
        },
    ));
    assert_eq!(replacement.index(), key.index());
    let replacement = world.entity(replacement).unwrap();
    let label = format!("{}  new child  (parent 0)", replacement.raw());
    let output = ui.button(pos, false);
    assert_eq!(ui.app.state.selected(), None, "松手不能选择复用行的实体");
    ui.click(text(&output, &label));
    assert_eq!(ui.app.state.selected(), Some(replacement));
}

#[test]
fn hierarchy_scrolls_and_selects_in_edit_and_play() {
    let (world, first, last) = seed();
    let mut ui = HierarchyUi::new(world, 1280);
    let output = ui.frame(Vec::new());
    let unnamed = text(&output, "0  #0");
    let first_label = first_label(first);
    let first_pos = text(&output, &first_label);
    assert!(unnamed.y < first_pos.y);
    assert!(visible_text(&output, &last_label(last)).is_none());
    ui.click(first_pos);
    assert_eq!(ui.app.state.selected(), Some(first));
    let output = ui.frame(Vec::new());
    text(&output, &format!("Entity {}", first.raw()));
    ui.click(text(&output, "Play"));
    assert!(ui.app.state.is_playing());

    let last_label = last_label(last);
    let output = ui.scroll_to(&last_label, -10_000.0);
    assert!(visible_text(&output, &first_label).is_none());
    ui.click(text(&output, &last_label));
    assert_eq!(ui.app.state.selected(), Some(last));
    let output = ui.frame(Vec::new());
    text(&output, &format!("Entity {}", last.raw()));
    ui.click(text(&output, "Stop"));
    assert!(!ui.app.state.is_playing());
    assert_eq!(ui.app.state.selected(), Some(first));

    let hash = ui.app.state.world().state_hash();
    let output = ui.scroll_to(&first_label, 10_000.0);
    assert!(visible_text(&output, &last_label).is_none());
    assert_eq!(ui.app.state.world().state_hash(), hash);
    assert!(!ui.app.state.can_undo());
}

#[test]
#[ignore = "用实际 egui GPU 合成输出 1280 / 1440 / 1920 的面板截图"]
fn hierarchy_panel_screenshots() {
    use eframe::{egui_wgpu, icon_data::IconDataExt, wgpu};
    use xgc_render::GpuContext;

    let root = std::env::temp_dir().join("xgc-s6-hierarchy");
    std::fs::create_dir_all(&root).unwrap();
    let gpu = GpuContext::headless().unwrap();
    let (device, queue) = (gpu.device(), gpu.queue());
    let format = wgpu::TextureFormat::Rgba8Unorm;
    for width in [1280, 1440, 1920] {
        let (world, first, _) = seed();
        let mut ui = HierarchyUi::new(world, width);
        ui.app.state.select(Some(first));
        let output = ui.frame(Vec::new());
        text(&output, "Hierarchy");
        text(&output, &first_label(first));
        text(&output, "Inspector");
        text(&output, &format!("Entity {}", first.raw()));
        let height = 720;
        let dimensions = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point: output.pixels_per_point,
        };
        let mut renderer = egui_wgpu::Renderer::new(device, format, None, 1, false);
        for (id, delta) in &output.textures_delta.set {
            renderer.update_texture(device, queue, *id, delta);
        }
        let primitives = ui.ctx.tessellate(output.shapes, output.pixels_per_point);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Hierarchy screenshot"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let cmd = renderer.update_buffers(device, queue, &mut encoder, &primitives, &dimensions);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            renderer.render(&mut pass.forget_lifetime(), &primitives, &dimensions);
        }
        queue.submit(cmd.into_iter().chain([encoder.finish()]));
        let rgba = gpu.read_texture_rgba8(&target, width, height).unwrap();
        let png = egui::IconData {
            rgba,
            width,
            height,
        }
        .to_png_bytes()
        .unwrap();
        let path = root.join(format!("hierarchy-{width}x{height}.png"));
        std::fs::write(&path, png).unwrap();
        println!("Hierarchy 面板截图：{}", path.display());
    }
}
