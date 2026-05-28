//! XGameEngine 独立游戏二进制。
//!
//! 在无编辑器的情况下运行 3D 场景——使用 `GameClock` 驱动固定步长物理模拟，
//! 通过 `ScenePipeline` 进行 wgpu 渲染。
//!
//! # 运行
//!
//! ```sh
//! cargo run -p xgame-game
//! ```

use std::collections::HashMap;

use winit::{
    event::{Event, WindowEvent},
    event_loop::EventLoop,
};

use xgame_math::glam::Vec3;
use xgame_math::projection;
use xgame_core::{GameClock, Scene};
use xgame_physics::{Collider, PhysicsConfig, PhysicsWorld, RigidBody};
use xgame_render::gltf_loader::GltfMesh;
use xgame_render::scene_pipeline::ScenePipeline;

// ---------------------------------------------------------------------------
// 应用状态
// ---------------------------------------------------------------------------

struct GameApp {
    scene: Scene,
    physics: PhysicsWorld,
    entity_to_body: HashMap<usize, u64>,
    clock: GameClock,
    pipeline: ScenePipeline,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    depth_texture: wgpu::Texture,
    model_cache: HashMap<String, GltfMesh>,
}

impl GameApp {
    async fn new(window: &winit::window::Window) -> Self {
        // -- wgpu 初始化 --
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::empty(),
            ..Default::default()
        });

        let surface = instance.create_surface(window).unwrap();
        // SAFETY: the window outlives the surface.
        let surface: wgpu::Surface<'static> = unsafe { std::mem::transmute(surface) };

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find a GPU adapter");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("XGameEngine Game Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("Failed to create wgpu device");

        let surface_caps = surface.get_capabilities(&adapter);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_caps.formats[0],
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // -- Depth texture --
        let (depth_texture, depth_view) = Self::create_depth_texture(
            &device,
            size.width.max(1),
            size.height.max(1),
        );

        // -- Scene pipeline --
        let pipeline = ScenePipeline::new(&device, config.format);

        // -- Load scene --
        let scene = load_scene("Scenes/scene.ron")
            .unwrap_or_else(|| Scene::new("Untitled"));

        // -- Physics --
        let mut physics = PhysicsWorld::new(PhysicsConfig::default());
        let mut entity_to_body = HashMap::new();

        for (i, entity) in scene.entities.iter().enumerate() {
            if entity.model.is_empty() {
                continue;
            }
            let body = RigidBody::dynamic_at(entity.transform);
            let body_id = physics.add_body(body);

            let half_extents = (entity.transform.scale * 0.5).max(Vec3::splat(0.01));
            let collider = Collider::cuboid(half_extents);
            physics.add_collider(body_id, collider);

            entity_to_body.insert(i, body_id);
        }

        Self {
            scene,
            physics,
            entity_to_body,
            clock: GameClock::new().with_fixed_dt(1.0 / 60.0),
            pipeline,
            device,
            queue,
            surface,
            config,
            depth_view,
            depth_texture,
            model_cache: HashMap::new(),
        }
    }

    fn create_depth_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("game_depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);

        let (tex, view) = Self::create_depth_texture(&self.device, width, height);
        self.depth_texture = tex;
        self.depth_view = view;
    }

    fn update(&mut self, dt: f32) {
        self.clock.tick(dt);
        while self.clock.consume_fixed_update() {
            self.physics.step(self.clock.fixed_dt());
        }
        self.clock.reset_step_counter();

        // Sync physics -> entity transforms
        self.sync_transforms();
    }

    fn sync_transforms(&mut self) {
        let mapping: Vec<(usize, u64)> = self
            .entity_to_body
            .iter()
            .map(|(&i, &b)| (i, b))
            .collect();

        for (entity_idx, body_id) in mapping {
            if let Some(phys_transform) = self.physics.body_transform(body_id) {
                if let Some(entity) = self.scene.entities.get_mut(entity_idx) {
                    entity.transform.position = phys_transform.position;
                    entity.transform.rotation = phys_transform.rotation;
                }
            }
        }
    }

    fn render(&mut self) {
        // -- Camera --
        let aspect = self.config.width as f32 / self.config.height as f32;
        let eye = Vec3::new(5.0, 5.0, 5.0);
        let target = Vec3::ZERO;
        let view = projection::look_at(eye, target, Vec3::Y);
        let proj = projection::perspective(60f32.to_radians(), aspect, 0.1, 100.0);
        let view_proj = proj * view;

        // -- Ensure models are loaded --
        for entity in &self.scene.entities {
            if !entity.model.is_empty() && !self.model_cache.contains_key(&entity.model) {
                match GltfMesh::from_file(&self.device, &self.queue, &entity.model) {
                    Ok(m) => {
                        log::info!(
                            "Loaded model '{}' ({} primitives)",
                            entity.model,
                            m.primitives.len()
                        );
                        self.model_cache.insert(entity.model.clone(), m);
                    }
                    Err(e) => log::warn!("Failed to load model '{}': {:#}", entity.model, e),
                }
            }
        }

        // -- Get swapchain texture --
        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(e) => {
                log::warn!("Surface error: {e}");
                return;
            }
        };
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // -- Render --
        let mut encoder =
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("game_encoder"),
                });

        self.pipeline.render(
            &self.queue,
            &mut encoder,
            &frame_view,
            &self.depth_view,
            &self.scene.entities,
            &self.model_cache,
            &view_proj,
            &eye,
        );

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}

// ---------------------------------------------------------------------------
// 场景加载
// ---------------------------------------------------------------------------

fn load_scene(path: &str) -> Option<Scene> {
    let content = std::fs::read_to_string(path).ok()?;
    ron::from_str(&content).ok()
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

fn main() {
    env_logger::init();

    let event_loop = EventLoop::new().unwrap();
    let window = event_loop.create_window(winit::window::Window::default_attributes()).unwrap();
    window.set_title("XGameEngine");

    let mut app = pollster::block_on(GameApp::new(&window));

    #[allow(deprecated)]
    event_loop
        .run(move |event, elwt| {
            match event {
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => {
                    elwt.exit();
                }
                Event::WindowEvent {
                    event: WindowEvent::Resized(size),
                    ..
                } => {
                    app.resize(size.width, size.height);
                }
                Event::WindowEvent {
                    event: WindowEvent::KeyboardInput {
                        event:
                            winit::event::KeyEvent {
                                logical_key: winit::keyboard::Key::Named(
                                    winit::keyboard::NamedKey::Escape,
                                ),
                                state: winit::event::ElementState::Pressed,
                                ..
                            },
                        ..
                    },
                    ..
                } => {
                    elwt.exit();
                }
                Event::AboutToWait => {
                    let dt = 1.0 / 60.0; // Fixed timestep target
                    app.update(dt);
                    app.render();
                }
                _ => {}
            }
        })
        .unwrap();
}
