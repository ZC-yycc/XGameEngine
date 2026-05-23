use wgpu::util::DeviceExt;

/// A GPU-resident mesh with vertex and index buffers.
pub struct Mesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub num_indices: u32,
    pub index_format: wgpu::IndexFormat,
}

impl Mesh {
    /// Upload vertex and index data to the GPU.
    pub fn new(
        device: &wgpu::Device,
        label: &str,
        vertices: &[u8],
        indices: &[u32],
        _vertex_size: u64,
    ) -> Self {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{} vertex buffer", label)),
            contents: vertices,
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_data: &[u8] = bytemuck::cast_slice(indices);
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{} index buffer", label)),
            contents: index_data,
            usage: wgpu::BufferUsages::INDEX,
        });

        Self {
            vertex_buffer,
            index_buffer,
            num_indices: indices.len() as u32,
            index_format: wgpu::IndexFormat::Uint32,
        }
    }
}

/// Create a unit-sized coloured cube mesh.
///
/// Returns (vertex bytes, vertex size in bytes, index array).
pub fn create_coloured_cube() -> (Vec<u8>, u64, Vec<u32>) {
    #[repr(C)]
    #[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
    struct Vertex {
        position: [f32; 3],
        color: [f32; 3],
    }

    let vertices: &[Vertex] = &[
        // +X face (red)
        Vertex { position: [ 0.5,  0.5,  0.5], color: [1.0, 0.0, 0.0] },
        Vertex { position: [ 0.5, -0.5,  0.5], color: [1.0, 0.0, 0.0] },
        Vertex { position: [ 0.5, -0.5, -0.5], color: [1.0, 0.0, 0.0] },
        Vertex { position: [ 0.5,  0.5, -0.5], color: [1.0, 0.0, 0.0] },
        // -X face (green)
        Vertex { position: [-0.5,  0.5, -0.5], color: [0.0, 1.0, 0.0] },
        Vertex { position: [-0.5, -0.5, -0.5], color: [0.0, 1.0, 0.0] },
        Vertex { position: [-0.5, -0.5,  0.5], color: [0.0, 1.0, 0.0] },
        Vertex { position: [-0.5,  0.5,  0.5], color: [0.0, 1.0, 0.0] },
        // +Y face (blue)
        Vertex { position: [-0.5,  0.5, -0.5], color: [0.0, 0.0, 1.0] },
        Vertex { position: [-0.5,  0.5,  0.5], color: [0.0, 0.0, 1.0] },
        Vertex { position: [ 0.5,  0.5,  0.5], color: [0.0, 0.0, 1.0] },
        Vertex { position: [ 0.5,  0.5, -0.5], color: [0.0, 0.0, 1.0] },
        // -Y face (yellow)
        Vertex { position: [-0.5, -0.5,  0.5], color: [1.0, 1.0, 0.0] },
        Vertex { position: [-0.5, -0.5, -0.5], color: [1.0, 1.0, 0.0] },
        Vertex { position: [ 0.5, -0.5, -0.5], color: [1.0, 1.0, 0.0] },
        Vertex { position: [ 0.5, -0.5,  0.5], color: [1.0, 1.0, 0.0] },
        // +Z face (cyan)
        Vertex { position: [-0.5,  0.5,  0.5], color: [0.0, 1.0, 1.0] },
        Vertex { position: [-0.5, -0.5,  0.5], color: [0.0, 1.0, 1.0] },
        Vertex { position: [ 0.5, -0.5,  0.5], color: [0.0, 1.0, 1.0] },
        Vertex { position: [ 0.5,  0.5,  0.5], color: [0.0, 1.0, 1.0] },
        // -Z face (magenta)
        Vertex { position: [ 0.5,  0.5, -0.5], color: [1.0, 0.0, 1.0] },
        Vertex { position: [ 0.5, -0.5, -0.5], color: [1.0, 0.0, 1.0] },
        Vertex { position: [-0.5, -0.5, -0.5], color: [1.0, 0.0, 1.0] },
        Vertex { position: [-0.5,  0.5, -0.5], color: [1.0, 0.0, 1.0] },
    ];

    let indices: &[u32] = &[
        // +X       -X       +Y        -Y        +Z        -Z
         0,1,2,    4,5,6,   8,9,10,   12,13,14, 16,17,18, 20,21,22,
         0,2,3,    4,6,7,   8,10,11,  12,14,15, 16,18,19, 20,22,23,
    ];

    (bytemuck::cast_slice(vertices).to_vec(), std::mem::size_of::<Vertex>() as u64, indices.to_vec())
}
