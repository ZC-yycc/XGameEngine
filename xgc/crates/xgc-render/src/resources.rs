//! 渲染资源描述：纹理 / 缓冲的格式、尺寸与用途。这里只做 CPU 侧声明，
//! wgpu 资源创建与 barrier 在执行层接。

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Texture2D,
    Buffer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceUsage {
    Sampled,
    ColorAttachment,
    DepthAttachment,
    Uniform,
    Storage,
}

impl ResourceUsage {
    pub fn readable(self) -> bool {
        matches!(self, Self::Sampled | Self::Uniform | Self::Storage)
    }

    pub fn writable(self) -> bool {
        matches!(
            self,
            Self::ColorAttachment | Self::DepthAttachment | Self::Storage
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceDesc {
    pub name: String,
    pub kind: ResourceKind,
    pub format: String,
    /// 纹理宽度；buffer 为 0。
    pub width: u32,
    /// 纹理高度；buffer 为 0。
    pub height: u32,
    /// buffer 字节数；texture 为 0。
    pub size: u64,
    pub usage: Vec<ResourceUsage>,
}

impl ResourceDesc {
    pub fn texture2d(
        name: impl Into<String>,
        format: impl Into<String>,
        width: u32,
        height: u32,
        usage: Vec<ResourceUsage>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: ResourceKind::Texture2D,
            format: format.into(),
            width,
            height,
            size: 0,
            usage,
        }
    }

    pub fn buffer(
        name: impl Into<String>,
        format: impl Into<String>,
        size: u64,
        usage: Vec<ResourceUsage>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: ResourceKind::Buffer,
            format: format.into(),
            width: 0,
            height: 0,
            size,
            usage,
        }
    }

    pub fn readable(&self) -> bool {
        self.usage.iter().any(|usage| usage.readable())
    }

    pub fn writable(&self) -> bool {
        self.usage.iter().any(|usage| usage.writable())
    }
}

#[derive(Clone, Debug, Default)]
pub struct ResourceTable {
    entries: Vec<ResourceDesc>,
}

impl ResourceTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, resource: ResourceDesc) -> Result<(), String> {
        if self.get(&resource.name).is_some() {
            return Err(format!("渲染资源重名：{}", resource.name));
        }
        self.entries.push(resource);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ResourceDesc> {
        self.entries.iter().find(|resource| resource.name == name)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ResourceDesc> {
        self.entries.iter()
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Texture2D => f.write_str("texture2d"),
            Self::Buffer => f.write_str("buffer"),
        }
    }
}
