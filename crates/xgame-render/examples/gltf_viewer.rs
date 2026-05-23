//! glTF model viewer with orbit camera and Blinn-Phong directional light.
//!
//! Run with:  cargo run --example gltf_viewer -- path/to/model.glb
//!
//! Controls:
//!   Left-drag  → rotate camera
//!   Scroll     → zoom in/out
//!   Escape     → exit

use anyhow::Context;
use std::borrow::Cow;
use std::path::PathBuf;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

use xgame_render::{
    camera::{CameraUniform, DirectionalLight, LightUniform, OrbitCamera},
    gltf_loader::{GltfMesh, GltfVertex},
    RenderContext,
};

// ---------------------------------------------------------------------------
// Shader — Blinn-Phong directional light
// ---------------------------------------------------------------------------

const SHADER: &str = r#"
struct CameraUniform {
    view_proj: mat4x4<f32>,
    position: vec4<f32>,
}

struct LightUniform {
    direction: vec4<f32>,
    color: vec4<f32>,
    ambient: vec4<f32>,
}

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var<uniform> light: LightUniform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec3<f32>,
}

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    out.position = camera.view_proj * vec4<f32>(pos, 1.0);
    out.world_pos = pos;
    out.world_normal = normal;
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let V = normalize(camera.position.xyz - in.world_pos);
    let L = normalize(-light.direction.xyz);
    let N = normalize(in.world_normal);
    let H = normalize(L + V);

    let ambient = light.ambient.xyz * 0.5;
    let diff = max(dot(N, L), 0.0) * light.color.xyz * light.color.w;
    let spec = pow(max(dot(N, H), 0.0), 32.0) * light.color.xyz * light.color.w * 0.4;

    let lit = ambient + diff + spec;
    let col = lit * (in.color * 0.4 + 0.6);
    return vec4<f32>(col, 1.0);
}
"#;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct State {
    model_path: PathBuf,
    window: Option<Window>,
    render_ctx: Option<RenderContext>,
    depth_texture: Option<wgpu::Texture>,
    depth_view: Option<wgpu::TextureView>,
    gltf_mesh: Option<GltfMesh>,
    pipeline: Option<wgpu::RenderPipeline>,
    camera_bind_group: Option<wgpu::BindGroup>,
    light_bind_group: Option<wgpu::BindGroup>,
    camera_buffer: Option<wgpu::Buffer>,
    light_buffer: Option<wgpu::Buffer>,
    orbit: Option<OrbitCamera>,
    light: DirectionalLight,

    mouse_pressed: bool,
    last_mouse: (f64, f64),
    size: (u32, u32),
}

impl State {
    fn new(model_path: PathBuf) -> Self {
        Self {
            model_path,
            window: None,
            render_ctx: None,
            depth_texture: None,
            depth_view: None,
            gltf_mesh: None,
            pipeline: None,
            camera_bind_group: None,
            light_bind_group: None,
            camera_buffer: None,
            light_buffer: None,
            orbit: None,
            light: DirectionalLight::default(),
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
        let (depth_tex, depth_view) = ctx.create_depth_texture("Viewer depth");
        self.depth_texture = Some(depth_tex);
        self.depth_view = Some(depth_view);

        // Load glTF model (or fallback cube)
        let model_path = self.model_path.clone();
        let gltf_mesh = GltfMesh::from_file(&ctx.device, &model_path)
            .with_context(|| format!("Failed to load {}", model_path.display()))
            .unwrap_or_else(|e| {
                log::warn!("{e}: falling back to built-in cube");
                GltfMesh::fallback_cube(&ctx.device)
            });
        self.gltf_mesh = Some(gltf_mesh);

        // --- Camera uniform buffer ---
        let camera_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Camera uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.camera_buffer = Some(camera_buffer);

        // --- Light uniform buffer ---
        let light_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Light uniform"),
            size: std::mem::size_of::<LightUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Upload initial light data
        ctx.queue.write_buffer(
            &light_buffer,
            0,
            bytemuck::bytes_of(&self.light.uniform()),
        );
        self.light_buffer = Some(light_buffer);

        // --- Bind group layout (camera @group 0) ---
        let camera_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Camera BGL"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let camera_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Camera BG"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.camera_buffer.as_ref().unwrap().as_entire_binding(),
            }],
        });
        self.camera_bind_group = Some(camera_bind_group);

        // --- Bind group layout (light @group 1) ---
        let light_bgl = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Light BGL"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let light_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Light BG"),
            layout: &light_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.light_buffer.as_ref().unwrap().as_entire_binding(),
            }],
        });
        self.light_bind_group = Some(light_bind_group);

        // --- Pipeline ---
        let pipeline_layout =
            ctx.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("Viewer pipeline layout"),
                    bind_group_layouts: &[&camera_bgl, &light_bgl],
                    push_constant_ranges: &[],
                });

        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Viewer shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
            });

        let pipeline = ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Viewer pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    compilation_options: Default::default(),
                    buffers: &[GltfVertex::layout()],
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
        ctx.queue.write_buffer(
            self.camera_buffer.as_ref().unwrap(),
            0,
            bytemuck::bytes_of(&uniform),
        );
    }

    fn render(&mut self) {
        let ctx = self.render_ctx.as_ref().unwrap();
        let pipeline = self.pipeline.as_ref().unwrap();
        let mesh = self.gltf_mesh.as_ref().unwrap();
        let camera_bg = self.camera_bind_group.as_ref().unwrap();
        let light_bg = self.light_bind_group.as_ref().unwrap();
        let depth_view = self.depth_view.as_ref().unwrap();

        let output = ctx.surface.get_current_texture().expect("get_current_texture");
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Viewer encoder"),
            });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Viewer pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.02,
                            g: 0.02,
                            b: 0.04,
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
            rpass.set_bind_group(0, camera_bg, &[]);
            rpass.set_bind_group(1, light_bg, &[]);

            for prim in &mesh.primitives {
                rpass.set_vertex_buffer(0, prim.vertex_buffer.slice(..));
                rpass.set_index_buffer(prim.index_buffer.slice(..), prim.index_format);
                rpass.draw_indexed(0..prim.num_indices, 0, 0..1);
            }
        }

        ctx.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        ctx.device.poll(wgpu::Maintain::Poll);
    }

    fn recreate_depth(&mut self) {
        let ctx = self.render_ctx.as_ref().unwrap();
        let (tex, view) = ctx.create_depth_texture("Viewer depth");
        self.depth_texture = Some(tex);
        self.depth_view = Some(view);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    let model_path = if args.len() >= 2 {
        PathBuf::from(&args[1])
    } else {
        anyhow::bail!(
            "Usage: {} <path/to/model.glb>\n\nDownload a sample model from:\n  https://github.com/KhronosGroup/glTF-Sample-Models",
            args[0]
        );
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut state = State::new(model_path);
    event_loop.run_app(&mut state)?;
    Ok(())
}

impl ApplicationHandler for State {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let attr = Window::default_attributes()
                .with_title("XGameEngine — glTF Viewer (Blinn-Phong)")
                .with_inner_size(winit::dpi::LogicalSize::new(1000, 700));
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
            WindowEvent::CloseRequested => event_loop.exit(),
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
                        orbit.camera.aspect =
                            physical_size.width as f32 / physical_size.height as f32;
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
