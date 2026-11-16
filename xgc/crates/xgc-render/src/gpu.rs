//! wgpu 资源创建层：把 CPU 侧的 `ResourceDesc` 变成真实纹理 / 缓冲。
//!
//! 这一层只负责设备与资源；pass 录制 / 管线 / 着色器在下一刀。无 GPU 的环境
//! 里 `GpuContext::headless()` 返回错误，调用方自行降级，测试里按"无适配器则跳过"处理。

use std::collections::HashMap;
use std::fmt;

use super::{ResourceDesc, ResourceKind, ResourceTable, ResourceUsage};

#[derive(Debug)]
pub enum GpuError {
    NoAdapter(String),
    RequestDevice(String),
    UnsupportedFormat(String),
    UnsupportedUsage(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter(message) => write!(f, "没有可用的 GPU 适配器：{message}"),
            Self::RequestDevice(message) => write!(f, "创建 GPU 设备失败：{message}"),
            Self::UnsupportedFormat(format) => write!(f, "不支持的渲染格式：{format}"),
            Self::UnsupportedUsage(resource) => {
                write!(f, "资源 {resource} 的用途无法映射到 wgpu")
            }
        }
    }
}

impl std::error::Error for GpuError {}

pub struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_info: wgpu::AdapterInfo,
}

impl GpuContext {
    pub fn headless() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| GpuError::NoAdapter("没有匹配的适配器".to_string()))?;
        let adapter_info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("xgc-device"),
                ..Default::default()
            },
            None,
        ))
        .map_err(|err| GpuError::RequestDevice(err.to_string()))?;
        Ok(Self {
            device,
            queue,
            adapter_info,
        })
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn create_resource(&self, desc: &ResourceDesc) -> Result<GpuResource, GpuError> {
        if !desc.readable() && !desc.writable() {
            return Err(GpuError::UnsupportedUsage(desc.name.clone()));
        }
        match desc.kind {
            ResourceKind::Texture2D => {
                let format = parse_format(&desc.format)?;
                let usage = texture_usage(desc)?;
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(&desc.name),
                    size: wgpu::Extent3d {
                        width: desc.width.max(1),
                        height: desc.height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                });
                Ok(GpuResource::Texture(texture))
            }
            ResourceKind::Buffer => {
                let usage = buffer_usage(desc)?;
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&desc.name),
                    size: desc.size.max(4),
                    usage,
                    mapped_at_creation: false,
                });
                Ok(GpuResource::Buffer(buffer))
            }
        }
    }

    pub fn create_resources(&self, table: &ResourceTable) -> Result<GpuResourceTable, GpuError> {
        let mut resources = HashMap::with_capacity(table.len());
        for desc in table.iter() {
            resources.insert(desc.name.clone(), self.create_resource(desc)?);
        }
        Ok(GpuResourceTable { resources })
    }
}

pub enum GpuResource {
    Texture(wgpu::Texture),
    Buffer(wgpu::Buffer),
}

pub struct GpuResourceTable {
    resources: HashMap<String, GpuResource>,
}

impl GpuResourceTable {
    pub fn get(&self, name: &str) -> Option<&GpuResource> {
        self.resources.get(name)
    }

    pub fn len(&self) -> usize {
        self.resources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}

fn parse_format(text: &str) -> Result<wgpu::TextureFormat, GpuError> {
    match text {
        "rgba8unorm" => Ok(wgpu::TextureFormat::Rgba8Unorm),
        "rgba8unorm-srgb" => Ok(wgpu::TextureFormat::Rgba8UnormSrgb),
        "rgba16float" => Ok(wgpu::TextureFormat::Rgba16Float),
        "rgba32float" => Ok(wgpu::TextureFormat::Rgba32Float),
        "bgra8unorm" => Ok(wgpu::TextureFormat::Bgra8Unorm),
        "depth32float" => Ok(wgpu::TextureFormat::Depth32Float),
        "depth24plus" => Ok(wgpu::TextureFormat::Depth24Plus),
        "r32float" => Ok(wgpu::TextureFormat::R32Float),
        other => Err(GpuError::UnsupportedFormat(other.to_string())),
    }
}

fn texture_usage(desc: &ResourceDesc) -> Result<wgpu::TextureUsages, GpuError> {
    let mut usage = wgpu::TextureUsages::COPY_DST;
    let mut mapped = false;
    for usage_flag in &desc.usage {
        match usage_flag {
            ResourceUsage::Sampled => {
                usage |= wgpu::TextureUsages::TEXTURE_BINDING;
                mapped = true;
            }
            ResourceUsage::ColorAttachment | ResourceUsage::DepthAttachment => {
                usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
                mapped = true;
            }
            ResourceUsage::Storage => {
                usage |= wgpu::TextureUsages::STORAGE_BINDING;
                mapped = true;
            }
            ResourceUsage::Uniform => {}
        }
    }
    if mapped {
        Ok(usage)
    } else {
        Err(GpuError::UnsupportedUsage(desc.name.clone()))
    }
}

fn buffer_usage(desc: &ResourceDesc) -> Result<wgpu::BufferUsages, GpuError> {
    let mut usage = wgpu::BufferUsages::COPY_DST;
    let mut mapped = false;
    for usage_flag in &desc.usage {
        match usage_flag {
            ResourceUsage::Uniform => {
                usage |= wgpu::BufferUsages::UNIFORM;
                mapped = true;
            }
            ResourceUsage::Storage => {
                usage |= wgpu::BufferUsages::STORAGE;
                mapped = true;
            }
            ResourceUsage::Sampled
            | ResourceUsage::ColorAttachment
            | ResourceUsage::DepthAttachment => {}
        }
    }
    if mapped {
        Ok(usage)
    } else {
        Err(GpuError::UnsupportedUsage(desc.name.clone()))
    }
}
