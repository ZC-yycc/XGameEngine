use anyhow::{Context, Result};
use std::path::Path;
use wgpu::util::DeviceExt;

/// One draw-call from a glTF mesh primitive.
pub struct GltfPrimitive {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub num_indices: u32,
    pub index_format: wgpu::IndexFormat,
    pub num_vertices: u32,
}

/// A collection of GPU-resident primitives loaded from a glTF file.
pub struct GltfMesh {
    pub primitives: Vec<GltfPrimitive>,
    /// Vertex stride in bytes (uniform across all primitives).
    pub vertex_size: u64,
}

// ---------------------------------------------------------------------------
// Host-side vertex (position + normal + colour)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GltfVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

impl GltfVertex {
    pub const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x3, // position
        1 => Float32x3, // normal
        2 => Float32x3, // colour
    ];
    
    pub const STRIDE: wgpu::BufferAddress = std::mem::size_of::<Self>() as wgpu::BufferAddress;

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: Self::STRIDE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: Self::ATTRIBUTES,
        }
    }
}

// ---------------------------------------------------------------------------
// Face-normal computation (used when glTF has no NORMAL attribute)
// ---------------------------------------------------------------------------

/// Accumulate per-face normals and average them into per-vertex smooth normals.
fn compute_face_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let vertex_count = positions.len();
    let mut normals = vec![[0.0f32; 3]; vertex_count];

    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            break;
        }
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);

        let (ax, ay, az) = (
            positions[i1][0] - positions[i0][0],
            positions[i1][1] - positions[i0][1],
            positions[i1][2] - positions[i0][2],
        );
        let (bx, by, bz) = (
            positions[i2][0] - positions[i0][0],
            positions[i2][1] - positions[i0][1],
            positions[i2][2] - positions[i0][2],
        );

        // cross product
        let (nx, ny, nz) = (
            ay * bz - az * by,
            az * bx - ax * bz,
            ax * by - ay * bx,
        );

        normals[i0][0] += nx;
        normals[i0][1] += ny;
        normals[i0][2] += nz;
        normals[i1][0] += nx;
        normals[i1][1] += ny;
        normals[i1][2] += nz;
        normals[i2][0] += nx;
        normals[i2][1] += ny;
        normals[i2][2] += nz;
    }

    // Normalize all vertex normals
    for n in &mut normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n[0] /= len;
            n[1] /= len;
            n[2] /= len;
        } else {
            *n = [0.0, 0.0, 1.0];
        }
    }

    normals
}

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

impl GltfMesh {
    /// Load a `.gltf` or `.glb` file and upload all mesh primitives to the GPU.
    ///
    /// Missing vertex attributes are filled with sensible defaults:
    ///   - normal → (0, 0, 1)   [pointing +Z]
    ///   - colour → white
    pub fn from_file(device: &wgpu::Device, path: impl AsRef<Path>) -> Result<Self> {
        let (document, buffers, _images) =
            gltf::import(path.as_ref()).context("Failed to parse glTF file")?;

        let mut primitives = Vec::new();

        for mesh in document.meshes() {
            for primitive in mesh.primitives() {
                let reader = primitive.reader(|buf| Some(&buffers[buf.index()]));

                // --- positions (required by glTF spec) ---
                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .context("Mesh primitive has no POSITION attribute")?
                    .collect();

                let vertex_count = positions.len();

                // --- indices (read first — needed for normal computation) ---
                let indices: Vec<u32> = reader
                    .read_indices()
                    .map(|iter| iter.into_u32().collect())
                    .unwrap_or_else(|| (0..vertex_count as u32).collect());

                // --- normals (compute from faces if missing) ---
                let normals: Vec<[f32; 3]> = match reader.read_normals() {
                    Some(iter) => iter.collect(),
                    None => {
                        log::info!("No NORMAL attribute — computing flat normals from faces");
                        compute_face_normals(&positions, &indices)
                    }
                };

                // --- vertex colours (optional, default: white) ---
                let colors: Vec<[f32; 3]> = reader
                    .read_colors(0)
                    .map(|iter| iter.into_rgb_f32().collect())
                    .unwrap_or_else(|| vec![[1.0, 1.0, 1.0]; vertex_count]);

                // Interleave into GltfVertex array
                let vertices: Vec<GltfVertex> = (0..vertex_count)
                    .map(|i| GltfVertex {
                        position: positions[i],
                        normal: normals[i],
                        color: colors[i],
                    })
                    .collect();

                let vertex_bytes: &[u8] = bytemuck::cast_slice(&vertices);

                let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("glTF vertex buffer"),
                    contents: vertex_bytes,
                    usage: wgpu::BufferUsages::VERTEX,
                });

                let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("glTF index buffer"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

                primitives.push(GltfPrimitive {
                    vertex_buffer,
                    index_buffer,
                    num_indices: indices.len() as u32,
                    index_format: wgpu::IndexFormat::Uint32,
                    num_vertices: vertex_count as u32,
                });
            }
        }

        Ok(Self {
            primitives,
            vertex_size: GltfVertex::STRIDE,
        })
    }

    /// Build a unit cube as a fallback when glTF loading fails.
    pub fn fallback_cube(device: &wgpu::Device) -> Self {
        let (verts, _vtx_size, indices) = crate::mesh::create_coloured_cube();

        // Old format: [pos(f32×3), col(f32×3)] = 24 bytes
        // New format: [pos(f32×3), normal(f32×3), col(f32×3)] = 36 bytes
        // Default normal: forward (+Z) = (0, 0, 1)
        let default_normal = f32::to_le_bytes(0.0)
            .into_iter()
            .chain(f32::to_le_bytes(0.0))
            .chain(f32::to_le_bytes(1.0))
            .collect::<Vec<u8>>();

        let old_vertex_count = verts.len() / 24;
        let mut new_verts: Vec<u8> = Vec::with_capacity(old_vertex_count * 36);

        for i in 0..old_vertex_count {
            let base = i * 24;
            new_verts.extend_from_slice(&verts[base..base + 12]); // position
            new_verts.extend_from_slice(&default_normal); // normal = (0,0,1)
            new_verts.extend_from_slice(&verts[base + 12..base + 24]); // colour
        }

        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fallback cube VB"),
            contents: &new_verts,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fallback cube IB"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        GltfMesh {
            primitives: vec![GltfPrimitive {
                vertex_buffer: vb,
                index_buffer: ib,
                num_indices: indices.len() as u32,
                index_format: wgpu::IndexFormat::Uint32,
                num_vertices: old_vertex_count as u32,
            }],
            vertex_size: GltfVertex::STRIDE,
        }
    }
}

// ---------------------------------------------------------------------------
// Face-normal computation (used when glTF has no NORMAL attribute)
// ---------------------------------------------------------------------------

/// Accumulate per-face normals and average them into per-vertex smooth normals.
fn compute_face_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let vertex_count = positions.len();
    let mut normals = vec![[0.0f32; 3]; vertex_count];

    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            break;
        }
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);

        let (ax, ay, az) = (
            positions[i1][0] - positions[i0][0],
            positions[i1][1] - positions[i0][1],
            positions[i1][2] - positions[i0][2],
        );
        let (bx, by, bz) = (
            positions[i2][0] - positions[i0][0],
            positions[i2][1] - positions[i0][1],
            positions[i2][2] - positions[i0][2],
        );

        // cross product
        let (nx, ny, nz) = (
            ay * bz - az * by,
            az * bx - ax * bz,
            ax * by - ay * bx,
        );

        normals[i0][0] += nx;
        normals[i0][1] += ny;
        normals[i0][2] += nz;
        normals[i1][0] += nx;
        normals[i1][1] += ny;
        normals[i1][2] += nz;
        normals[i2][0] += nx;
        normals[i2][1] += ny;
        normals[i2][2] += nz;
    }

    // Normalize all vertex normals
    for n in &mut normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n[0] /= len;
            n[1] /= len;
            n[2] /= len;
        } else {
            *n = [0.0, 0.0, 1.0];
        }
    }

    normals
}
