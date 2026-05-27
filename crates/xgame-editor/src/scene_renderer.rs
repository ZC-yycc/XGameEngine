//! 编辑器 3D 场景渲染器。
//!
//! 将场景实体渲染到离屏 wgpu 纹理，然后将该纹理注册到 egui-wgpu，
//! 供场景视图面板显示。
//!
//! 底层渲染逻辑由 `xgame_render::scene_pipeline::ScenePipeline` 提供，
//! 此模块仅负责离屏纹理管理和 egui 集成。

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec3};

use xgame_render::gltf_loader::GltfMesh;
use xgame_render::scene_pipeline::ScenePipeline;
use xgame_core::Entity;

// ---------------------------------------------------------------------------
// SceneRenderer
// ---------------------------------------------------------------------------

pub struct SceneRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    /// egui-wgpu renderer for registering native textures.
    egui_renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    surface_format: wgpu::TextureFormat,

    /// 共享渲染管线。
    pipeline: ScenePipeline,

    // Render target (recreated when viewport size changes)
    viewport_size: (u32, u32),
    render_view: Option<wgpu::TextureView>,
    depth_view: Option<wgpu::TextureView>,
    /// Keep old render-target textures alive so egui doesn't dangle.
    _stale_textures: Vec<wgpu::Texture>,
    texture_id: Option<egui::TextureId>,

    // Model cache (path -> GPU mesh)
    model_cache: HashMap<String, GltfMesh>,
}

impl SceneRenderer {
    pub fn new(
        device: &Arc<wgpu::Device>,
        queue: &Arc<wgpu::Queue>,
        egui_renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let device = Arc::clone(device);
        let queue = Arc::clone(queue);

        let pipeline = ScenePipeline::new(&device, surface_format);

        Self {
            device,
            queue,
            egui_renderer,
            surface_format,
            pipeline,
            viewport_size: (0, 0),
            render_view: None,
            depth_view: None,
            _stale_textures: Vec::new(),
            texture_id: None,
            model_cache: HashMap::new(),
        }
    }

    // -- Viewport management --

    pub fn set_viewport_size(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || self.viewport_size == (width, height) {
            return;
        }
        self.viewport_size = (width, height);
        self.recreate_targets();
    }

    fn recreate_targets(&mut self) {
        let (w, h) = self.viewport_size;

        // Push old textures to stale list — keep them alive so egui-wgpu
        // doesn't access freed GPU memory.
        if let Some(_old) = self.render_view.take() {
            self._stale_textures.push(
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("stale_placeholder"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.surface_format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                }),
            );
        }
        self.texture_id = None;

        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };

        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("editor_scene_color"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.surface_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());

        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("editor_scene_depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

        // Register with egui-wgpu
        let tid = self
            .egui_renderer
            .write()
            .register_native_texture(&self.device, &color_view, wgpu::FilterMode::Linear);

        self.render_view = Some(color_view);
        self.depth_view = Some(depth_view);
        self.texture_id = Some(tid);
        self._stale_textures.push(color);
        self._stale_textures.push(depth);
        if self._stale_textures.len() > 24 {
            self._stale_textures.drain(..12);
        }
    }

    pub fn texture_id(&self) -> Option<egui::TextureId> {
        self.texture_id
    }

    // -- Rendering --

    pub fn render(&mut self, entities: &[Entity], view_proj: &Mat4, camera_pos: &Vec3) {
        let (w, h) = self.viewport_size;
        if w == 0 || h == 0 {
            return;
        }

        // Phase 1: ensure all models are loaded (mutable borrow on model_cache)
        for entity in entities {
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
                    Err(e) => {
                        log::warn!("Failed to load model '{}': {:#}", entity.model, e)
                    }
                }
            }
        }

        // Phase 2: render via shared pipeline
        let Some(ref color_view) = self.render_view else {
            return;
        };
        let Some(ref depth_view) = self.depth_view else {
            return;
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("editor_scene_encoder"),
            });

        self.pipeline.render(
            &self.queue,
            &mut encoder,
            color_view,
            depth_view,
            entities,
            &self.model_cache,
            view_proj,
            camera_pos,
        );

        self.queue.submit(std::iter::once(encoder.finish()));
    }
}
