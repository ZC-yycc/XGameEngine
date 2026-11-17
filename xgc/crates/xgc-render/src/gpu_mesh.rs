//! GPU 网格缓存：模型字符串 → 顶点 / UV / 索引缓冲 + 贴图绑定组，跨帧复用。

use std::collections::HashMap;

use super::gpu::GpuContext;
use super::mesh::Mesh;
use super::texture_bind::TextureBinding;

pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub uv_buffer: wgpu::Buffer,
    pub index_buffer: Option<wgpu::Buffer>,
    /// 按模型上传时建好的贴图绑定组（没贴图时指向白纹理）。
    pub texture_group: wgpu::BindGroup,
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
    pub fn upload(
        &mut self,
        context: &GpuContext,
        key: &str,
        mesh: &Mesh,
        textures: &TextureBinding,
    ) -> &GpuMesh {
        if !self.meshes.contains_key(key) {
            let vertex_bytes = positions_bytes(&mesh.positions);
            let uv_bytes = uvs_bytes(&mesh.positions, &mesh.uvs);
            let vertex_buffer = create_vertex_buffer(context, "xgc-mesh-vertices", &vertex_bytes);
            let uv_buffer = create_vertex_buffer(context, "xgc-mesh-uvs", &uv_bytes);
            let index_buffer = if mesh.indices.is_empty() {
                None
            } else {
                let bytes: Vec<u8> = mesh
                    .indices
                    .iter()
                    .flat_map(|index| index.to_le_bytes())
                    .collect();
                let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
                    label: Some("xgc-mesh-indices"),
                    size: bytes.len() as u64,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                context.queue().write_buffer(&buffer, 0, &bytes);
                Some(buffer)
            };
            let texture_group = textures.group(context, mesh.texture.as_deref());
            self.meshes.insert(
                key.to_string(),
                GpuMesh {
                    vertex_buffer,
                    uv_buffer,
                    index_buffer,
                    texture_group,
                    vertices: mesh.positions.len() as u32,
                    indices: mesh.indices.len() as u32,
                },
            );
        }
        self.meshes.get(key).unwrap()
    }

    /// 蒙皮顶点每帧覆盖；顶点数不变时只写位置缓冲（UV 不变）。
    pub fn upload_deformed(
        &mut self,
        context: &GpuContext,
        key: &str,
        source: &Mesh,
        positions: &[[f32; 3]],
        textures: &TextureBinding,
    ) -> &GpuMesh {
        if let Some(mesh) = self.meshes.get(key) {
            if mesh.vertices as usize == positions.len() {
                context
                    .queue()
                    .write_buffer(&mesh.vertex_buffer, 0, &positions_bytes(positions));
                return self.meshes.get(key).unwrap();
            }
        }
        self.meshes.remove(key);
        let mut deformed = source.clone();
        deformed.positions = positions.to_vec();
        self.upload(context, key, &deformed, textures)
    }
}

fn positions_bytes(positions: &[[f32; 3]]) -> Vec<u8> {
    positions
        .iter()
        .flat_map(|position| position.iter().flat_map(|value| value.to_le_bytes()))
        .collect()
}

fn uvs_bytes(positions: &[[f32; 3]], uvs: &[[f32; 2]]) -> Vec<u8> {
    if uvs.is_empty() {
        return vec![0; positions.len() * 8];
    }
    uvs.iter()
        .flat_map(|uv| uv.iter().flat_map(|value| value.to_le_bytes()))
        .collect()
}

fn create_vertex_buffer(context: &GpuContext, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    let buffer = context.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (bytes.len() as u64).max(4),
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if !bytes.is_empty() {
        context.queue().write_buffer(&buffer, 0, bytes);
    }
    buffer
}
