//! Coloured cube with orbit camera and depth testing.
//!
//! Run with:  cargo run --example cube
//!
//! Controls:
//!   Left-drag  → rotate camera
//!   Scroll     → zoom in/out
//!   Escape     → exit

use std::borrow::Cow;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

use xgame_render::{
    camera::{CameraUniform, OrbitCamera},
    mesh::{create_coloured_cube, Mesh},
    RenderContext,
};

// ---------------------------------------------------------------------------
// Shader
// ---------------------------------------------------------------------------

const SHADER: &str = r#"
struct CameraUniform {
    view_proj: mat4x4<f32>,
    position: vec4<f32>,
}

@group(0) @binding(0) var<uniform> camera: CameraUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
}

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) col: vec3<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    out.position = camera.view_proj * vec4<f32>(pos, 1.0);
    out.color = col;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct State {
    window: Option<Window>,
    render_ctx: Option<RenderContext>,
    depth_texture: Option<wgpu::Texture>,
    depth_view: Option<wgpu::TextureView>,
    mesh: Option<Mesh>,
    pipeline: Option<wgpu::RenderPipeline>,
    bind_group: Option<wgpu::BindGroup>,
    camera_buffer: Option<wgpu::Buffer>,
    orbit: Option<OrbitCamera>,

    mouse_pressed: bool,
    last_mouse: (f64, f64),
    size: (u32, u32),
}

impl State {
    fn new() -> Self {
        Self {
            window: None,
            render_ctx: None,
            depth_texture: None,
            depth_view: None,
            mesh: None,
            pipeline: None,
            bind_group: None,
            camera_buffer: None,
            orbit: None,
            mouse_pressed: false,
            last_mouse: (0.0, 0.0),
            size: (800, 600),
        }
    }

    fn init_gpu(&mut self) {
        let window = self.window.as_ref().unwrap();
        let ctx = pollster::block_on(RenderContext::new(window));
        self.size = ctx.size;

        // Depth
        let (depth_tex, depth_view) = ctx.create_depth_texture("Cube depth");
        self.depth_texture = Some(depth_tex);
        self.depth_view = Some(depth_view);

        // Mesh — unit cube with per-face colours
        let (vtx_bytes, vtx_size, indices) = create_coloured_cube();
        let mesh = Mesh::new(&ctx.device, "Cube", &vtx_bytes, &indices, vtx_size);
        self.mesh = Some(mesh);

        // Camera uniform buffer
        let camera_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Camera uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.camera_buffer = Some(camera_buffer);

        // Bind group layout
        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("Camera BGL"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Camera BG"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.camera_buffer.as_ref().unwrap().as_entire_binding(),
            }],
        });
        self.bind_group = Some(bind_group);

        // Pipeline layout
        let pipeline_layout =
            ctx.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("Cube pipeline layout"),
                    bind_group_layouts: &[&bind_group_layout],
                    push_constant_ranges: &[],
                });

        // Shader
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Cube shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
            });

        let pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Cube pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 24, // position (f32×3) + color (f32×3) = 24 bytes
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: ctx.config.format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
        self.pipeline = Some(pipeline);

        // Orbit camera
        let aspect = self.size.0 as f32 / self.size.1 as f32;
        self.orbit = Some(OrbitCamera::new(aspect));

        self.render_ctx = Some(ctx);
    }

    fn update_camera_uniform(&self) {
        let ctx = self.render_ctx.as_ref().unwrap();
        let orbit = self.orbit.as_ref().unwrap();
        let uniform = orbit.camera.uniform();
        ctx.queue
            .write_buffer(self.camera_buffer.as_ref().unwrap(), 0, bytemuck::bytes_of(&uniform));
    }

    fn render(&mut self) {
        let ctx = self.render_ctx.as_ref().unwrap();
        let pipeline = self.pipeline.as_ref().unwrap();
        let mesh = self.mesh.as_ref().unwrap();
        let bind_group = self.bind_group.as_ref().unwrap();
        let depth_view = self.depth_view.as_ref().unwrap();

        let output = ctx.surface.get_current_texture().expect("get_current_texture");
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Cube encoder"),
            });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Cube pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.1,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, bind_group, &[]);
            rpass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            rpass.set_index_buffer(mesh.index_buffer.slice(..), mesh.index_format);
            rpass.draw_indexed(0..mesh.num_indices, 0, 0..1);
        }

        ctx.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        ctx.device.poll(wgpu::Maintain::Poll);
    }

    fn recreate_depth(&mut self) {
        let ctx = self.render_ctx.as_ref().unwrap();
        let (tex, view) = ctx.create_depth_texture("Cube depth");
        self.depth_texture = Some(tex);
        self.depth_view = Some(view);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut state = State::new();
    event_loop.run_app(&mut state).unwrap();
}

impl ApplicationHandler for State {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let attr = Window::default_attributes()
                .with_title("XGameEngine — Cube")
                .with_inner_size(winit::dpi::LogicalSize::new(800, 600));
            let window = event_loop.create_window(attr).unwrap();
            self.window = Some(window);
            self.init_gpu();
            self.window.as_ref().unwrap().request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::Escape)
                {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(physical_size) => {
                if let Some(ref mut ctx) = self.render_ctx {
                    ctx.resize((physical_size.width, physical_size.height));
                    self.size = (physical_size.width, physical_size.height);
                    self.recreate_depth();
                    if let Some(ref mut orbit) = self.orbit {
                        let aspect = physical_size.width as f32 / physical_size.height as f32;
                        orbit.camera.aspect = aspect;
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left {
                    self.mouse_pressed = state == ElementState::Pressed;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.mouse_pressed {
                    if let Some(ref mut orbit) = self.orbit {
                        let dx = position.x - self.last_mouse.0;
                        let dy = position.y - self.last_mouse.1;
                        orbit.rotate(dx as f32, dy as f32);
                    }
                }
                self.last_mouse = (position.x, position.y);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(ref mut orbit) = self.orbit {
                    let scroll = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(pos) => pos.y as f32 * 0.1,
                    };
                    orbit.zoom(scroll);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(ref mut orbit) = self.orbit {
                    orbit.update();
                }
                self.update_camera_uniform();
                self.render();
                self.window.as_ref().unwrap().request_redraw();
            }
            _ => {}
        }
    }
}
