//! GPU 网格缓存：模型字符串 → 顶点 / 索引缓冲，跨帧复用。

use std::collections::HashMap;

use super::gpu::GpuContext;
use super::mesh::Mesh;

pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: Option<wgpu::Buffer>,
    pub vertices: u32,
    pub indices: u32,
}

#[derive(Default)]
pub struct GpuMeshCache {
    meshes: HashMap<String, GpuMesh>,
}

impl GpuMeshCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.meshes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&GpuMesh> {
        self.meshes.get(key)
    }

    /// 首次上传后复用缓冲；同一模型字符串只建一次。
    pub fn upload(&mut self, context: &GpuContext, key: &str, mesh: &Mesh) -> &GpuMesh {
        if !self.meshes.contains_key(key) {
            let vertex_bytes: Vec<u8> = mesh
                .positions
                .iter()
                .flat_map(|position| position.iter().flat_map(|value| value.to_le_bytes()))
                .collect();
            let vertex_buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("xgc-mesh-vertices"),
                size: vertex_bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            context
                .queue()
                .write_buffer(&vertex_buffer, 0, &vertex_bytes);
            let index_buffer = if mesh.indices.is_empty() {
                None
            } else {
                let index_bytes: Vec<u8> = mesh
                    .indices
                    .iter()
                    .flat_map(|index| index.to_le_bytes())
                    .collect();
                let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
                    label: Some("xgc-mesh-indices"),
                    size: index_bytes.len() as u64,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                context.queue().write_buffer(&buffer, 0, &index_bytes);
                Some(buffer)
            };
            self.meshes.insert(
                key.to_string(),
                GpuMesh {
                    vertex_buffer,
                    index_buffer,
                    vertices: mesh.positions.len() as u32,
                    indices: mesh.indices.len() as u32,
                },
            );
        }
        self.meshes.get(key).unwrap()
    }
}
