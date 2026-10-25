//! 共享材质资产（`.mat`）的运行时库：解析 → 贴图 → 绑定组。
//!
//! # 为什么材质绑定组要单独一张表
//!
//! 模型自带的材质绑定组按 `GltfPrimitive::material_id` 缓存，生命周期跟着
//! 模型缓存走。共享材质资产是**跨模型**的：同一份 `Wood.mat` 会被箱子、
//! 地板、柱子的不同图元引用，而且同一个图元也可能被两个实体分别指定不同
//! 材质。所以资产材质需要自己的 key 空间（[`MaterialKey::Asset`]），
//! 批次键也必须是「图元地址 + 材质 key」——否则两个相邻的箱子一个木质、
//! 一个金属，会被错误地合进同一批，一个盖掉另一个的颜色。
//!
//! # 缓存与失效
//!
//! 加载是**每帧热路径**上的查询：`HashMap` 命中就返回 key，不读盘。
//! 失败也缓存（负缓存），避免一个坏 `.mat` 每帧刷一条 warn 加一次磁盘 IO。
//! 编辑器保存材质 / 改导入设置后调用 [`MaterialLibrary::invalidate`]，
//! 下一帧重新加载；独立游戏只在启动时设置一次项目，不需要失效。
//!
//! # 贴图共享
//!
//! 多份材质引用同一张 `albedo` 时只上传一次 GPU 纹理（按解析后的项目相对
//! 路径缓存）。材质里的 `albedo` 在加载边界已经从 GUID 解码成路径
//! （见 `ProjectHandle::load_material`），所以缓存键就是可以直接打开的路径。

use std::collections::HashMap;
use std::sync::Arc;

use wgpu::BindGroupLayout;
use xgame_core::{read_import_settings, MaterialAsset, ProjectHandle};

use crate::texture::{Texture, TextureOptions};

/// 一次 draw 用哪套材质绑定组。
///
/// `Model` 是 glTF / 内置图元自带纹理（旧路径，按材质槽位 ID 缓存）；
/// `Asset` 是项目里的 `.mat`（按资产路径缓存，贴图可以替换模型贴图）。
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MaterialKey {
    /// 模型自带材质（`GltfPrimitive::material_id`）。
    Model(u64),
    /// 共享材质资产（本库内部自增 ID）。
    Asset(u64),
}

impl Default for MaterialKey {
    fn default() -> Self {
        Self::Model(0)
    }
}

/// 一份已加载的资产材质（GPU 资源另存，见 [`MaterialLibrary`]）。
struct LoadedMaterial {
    asset: Arc<MaterialAsset>,
}

/// 材质资产库。
pub struct MaterialLibrary {
    /// 材质绑定组的布局。放在这里而不是 `ScenePipeline`：所有绑定组都由
    /// 本库创建，管线侧不需要再持有第二份句柄。
    material_bgl: BindGroupLayout,
    project: Option<ProjectHandle>,

    /// 资产引用（路径或 `guid://`）→ key。`None` 是负缓存（读不出来）。
    by_reference: HashMap<String, Option<MaterialKey>>,
    /// `Asset(id)` 的 id → 已加载资产。
    slots: Vec<Option<LoadedMaterial>>,
    next_id: u64,

    /// 模型自带的材质绑定组（key 是 `GltfPrimitive::material_id`）。
    model_bind_groups: HashMap<u64, wgpu::BindGroup>,
    /// 资产材质的绑定组（key 是 `Asset(id)` 的 id）。
    asset_bind_groups: HashMap<u64, wgpu::BindGroup>,
    /// 共享贴图：项目相对路径 → GPU 纹理。空串不用这里（走 1×1 白）。
    textures: HashMap<String, Texture>,
    /// 1×1 白色回退（资产没写 albedo 时使用）。惰性创建。
    white: Option<Texture>,

    /// 失效代数。编辑器把它混进渲染指纹：`invalidate` 之后必须重画。
    revision: u64,
}

impl MaterialLibrary {
    /// 用场景管线的材质绑定组布局建库。
    pub fn new(material_bgl: BindGroupLayout) -> Self {
        Self {
            material_bgl,
            project: None,
            by_reference: HashMap::new(),
            slots: Vec::new(),
            next_id: 1,
            model_bind_groups: HashMap::new(),
            asset_bind_groups: HashMap::new(),
            textures: HashMap::new(),
            white: None,
            revision: 0,
        }
    }

    /// 设置项目句柄（编辑器 / 游戏 / 无头截图在启动时各调一次）。
    ///
    /// 重复设置同一个项目也会清缓存：调用方通常是在项目资产刚变化后调，
    /// 省一次"是否同一个 root"的比较不值得让缓存有机会变脏。
    pub fn set_project(&mut self, project: Option<ProjectHandle>) {
        self.project = project;
        self.invalidate();
    }

    /// 丢掉所有资产材质 / 贴图 / 绑定组（模型材质保持）。
    ///
    /// 模型材质的 key 随模型缓存一起换（重新加载模型会分配新 id），
    /// 不需要在这里清；资产材质则必须清，因为 `.mat` 的内容可能刚被编辑。
    pub fn invalidate(&mut self) {
        self.by_reference.clear();
        self.slots.clear();
        self.next_id = 1;
        self.asset_bind_groups.clear();
        self.textures.clear();
        self.white = None;
        self.revision = self.revision.wrapping_add(1);
    }

    /// 失效代数（只增，回绕可忽略；用于渲染指纹）。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 确保模型自带材质的绑定组存在。
    pub fn bind_model(&mut self, device: &wgpu::Device, material_id: u64, texture: &Texture) {
        if self.model_bind_groups.contains_key(&material_id) {
            return;
        }
        let bind_group = self.create_bind_group(device, texture, "scene_model_material_bg");
        self.model_bind_groups.insert(material_id, bind_group);
    }

    /// 确保某个资产材质的绑定组存在，返回它的 key。
    ///
    /// 第一次调用读 `.mat` + 贴图并上传；之后每帧只查表。读不出来（文件
    /// 缺失 / RON 坏 / 没有项目上下文）返回 `None` 并负缓存 —— 调用方退回
    /// 模型自带材质，而不是整帧失败。
    pub fn bind_asset(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        reference: &str,
    ) -> Option<MaterialKey> {
        if let Some(cached) = self.by_reference.get(reference) {
            return *cached;
        }

        let loaded = match self.load_asset(device, queue, reference) {
            Some((asset, bind_group)) => {
                let id = self.next_id;
                self.next_id += 1;
                self.slots.push(Some(LoadedMaterial {
                    asset: Arc::new(asset),
                }));
                self.asset_bind_groups.insert(id, bind_group);
                Some(MaterialKey::Asset(id))
            }
            None => None,
        };
        self.by_reference.insert(reference.to_string(), loaded);
        loaded
    }

    /// 按 key 取已加载的资产参数。
    ///
    /// 返回 `Arc` 的引用而不是 `MaterialAsset` 的引用：调用方在收集循环里
    /// 还要继续向本库借可变引用（建绑定组），直接借用会打架。
    pub fn asset(&self, key: MaterialKey) -> Option<&Arc<MaterialAsset>> {
        let MaterialKey::Asset(id) = key else {
            return None;
        };
        let index = usize::try_from(id).ok()?.checked_sub(1)?;
        self.slots.get(index)?.as_ref().map(|s| &s.asset)
    }

    /// 按 key 取绑定组（绘制阶段只读）。
    pub fn bind_group(&self, key: MaterialKey) -> Option<&wgpu::BindGroup> {
        match key {
            MaterialKey::Model(id) => self.model_bind_groups.get(&id),
            MaterialKey::Asset(id) => self.asset_bind_groups.get(&id),
        }
    }

    fn load_asset(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        reference: &str,
    ) -> Option<(MaterialAsset, wgpu::BindGroup)> {
        let Some(project) = self.project.clone() else {
            log::warn!("材质 '{reference}' 无法加载：渲染器没有绑定项目");
            return None;
        };
        let asset = match project.load_material(reference) {
            Ok(asset) => asset,
            Err(e) => {
                log::warn!("材质 '{reference}' 加载失败：{e}");
                return None;
            }
        };

        // albedo 为空 = 资产就是纯色，用白纹理乘 base_color；有贴图就按
        // `.meta` 的导入设置上传（sRGB / 最近邻）。贴图坏了退回白色，
        // 但材质参数仍然生效 —— 一个坏贴图不该让整个物体消失。
        let texture_path = (!asset.albedo.is_empty()).then(|| asset.albedo.clone());
        if let Some(path) = &texture_path {
            if !self.textures.contains_key(path) {
                let import = read_import_settings(project.root(), path);
                match Texture::from_file(
                    device,
                    queue,
                    project.path(path),
                    TextureOptions {
                        srgb: import.texture_srgb,
                        nearest: import.texture_nearest,
                    },
                    path,
                ) {
                    Ok(texture) => {
                        self.textures.insert(path.clone(), texture);
                    }
                    Err(e) => {
                        log::warn!("材质 '{reference}' 的贴图加载失败：{e}");
                    }
                }
            }
        }

        let bind_group = match texture_path.as_deref().and_then(|p| self.textures.get(p)) {
            Some(texture) => self.create_bind_group(device, texture, "scene_asset_material_bg"),
            None => {
                if self.white.is_none() {
                    self.white = Some(Texture::white(device, queue));
                }
                let texture = self.white.as_ref().expect("刚创建的白纹理");
                self.create_bind_group(device, texture, "scene_asset_material_bg")
            }
        };
        Some((asset, bind_group))
    }

    fn create_bind_group(
        &self,
        device: &wgpu::Device,
        texture: &Texture,
        label: &str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.material_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&texture.sampler),
                },
            ],
        })
    }
}
