//! 保守的时域硬件遮挡剔除。
//!
//! # 为什么不是“本帧先画全几何深度，再同帧查询”
//!
//! 那条路线虽然绝不会漏画，但被挡几何已经在深度预通道支付了一遍完整顶点与
//! 光栅成本。RTX 2060 / 2048 立方体实测，即使只剩 64 个可见实例，GPU 仍从
//! 62.8 增到 90.5 µs。结构性重复不能靠调阈值补救。
//!
//! 当前路线把查询对象换成每组实例的世界 AABB（固定 12 个三角形），并在主通道
//! 完成后、对最终不透明深度做查询。结果供下一帧使用，但只有在以下输入与上一帧
//! **逐字节相同**时才复用：相机矩阵、相机位置、实例数据、骨骼矩阵、批次身份与
//! 分组包围盒。任一变化都完整绘制并仅刷新历史，因此相机 / 实体 / 遮挡物运动时
//! 宁可多画一帧，也不会拿旧可见性漏掉刚露出的物体。morph 顶点缓冲不在 CPU
//! 实例束里，主通道出现 morph 不透明批次时整条时域路径保守关闭。
//!
//! 查询结果还会异步读回一个很小的布尔数组：只有被挡索引工作至少占全帧三分之一
//! 才启用重映射管线。读回只做非阻塞 `Poll`；尚未就绪或映射失败都完整绘制。

use bytemuck::{Pod, Zeroable};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use wgpu::util::DeviceExt;
use xgame_math::glam::{Mat4, Vec3};

use crate::gltf_loader::Bounds;

/// 一次查询覆盖的连续实例上限。
pub(crate) const QUERY_GROUP_SIZE: u32 = 128;
/// 小于此值的组直接保留，避免为零碎尾段支付硬件查询固定税。
pub(crate) const MIN_QUERY_GROUP_SIZE: u32 = 64;
/// 低面网格的查询固定税高于省下的绘制；只让至少这个索引数的图元参与。
const MIN_INDICES_PER_INSTANCE: u32 = 512;
/// 即使单体够复杂，整帧候选工作不足时也不值得开 query pass / compute pass。
/// 该门槛由 `occlusion_cost` 的低面 / 高面对照约束，不是画质参数。
const MIN_TOTAL_CANDIDATE_INDICES: u64 = 1_000_000;
/// 重映射管线自身有固定税；查询结果至少挡掉全帧三分之一的索引工作才启用。
const MIN_CULLED_WORK_DIVISOR: u64 = 3;
/// `DrawIndexedIndirectArgs` = 5 个 32 位字段。
pub(crate) const DRAW_INDEXED_INDIRECT_STRIDE: u64 = 20;
const ALWAYS_VISIBLE: u32 = u32::MAX;
const READBACK_PENDING: u8 = 0;
const READBACK_READY: u8 = 1;
const READBACK_FAILED: u8 = 2;

/// 场景管线交给遮挡模块的一批实例。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BatchInput {
    pub first: u32,
    pub count: u32,
    pub index_count: u32,
    /// 图元加载代际 ID（全局单调递增）；模型热重载必定换 ID，让历史确定失效。
    pub primitive_key: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QueryChunk {
    pub batch: u32,
    pub first: u32,
    pub count: u32,
    pub query: Option<u32>,
}

/// 收集阶段用同一条门槛决定是否值得额外计算遮挡 AABB，避免低面批次虽然最终
/// 不发查询，却先在 CPU 上白算一遍包围盒。
pub(crate) fn primitive_is_candidate(index_count: u32) -> bool {
    index_count >= MIN_INDICES_PER_INSTANCE
}

fn has_enough_candidate_work(batches: &[BatchInput]) -> bool {
    batches
        .iter()
        .filter(|batch| {
            batch.count >= MIN_QUERY_GROUP_SIZE && primitive_is_candidate(batch.index_count)
        })
        .map(|batch| batch.index_count as u64 * batch.count as u64)
        .sum::<u64>()
        >= MIN_TOTAL_CANDIDATE_INDICES
}

fn should_query_group(index_count: u32, count: u32, bounds: Bounds, camera_pos: Vec3) -> bool {
    let camera_inside = !bounds.is_empty()
        && camera_pos.cmpge(bounds.min).all()
        && camera_pos.cmple(bounds.max).all();
    primitive_is_candidate(index_count)
        && count >= MIN_QUERY_GROUP_SIZE
        && !bounds.is_empty()
        && !camera_inside
}

fn evaluate_visibility(
    batches: &[BatchInput],
    chunks: &[QueryChunk],
    query_bytes: &[u8],
) -> (usize, bool) {
    let mut visible_instances = 0usize;
    let mut culled_work = 0u64;
    for chunk in chunks {
        let visible = chunk.query.is_none_or(|query| {
            let offset = query as usize * wgpu::QUERY_SIZE as usize;
            query_bytes
                .get(offset..offset + wgpu::QUERY_SIZE as usize)
                .and_then(|bytes| bytes.try_into().ok())
                .map(u64::from_le_bytes)
                .is_none_or(|samples| samples != 0)
        });
        if visible {
            visible_instances += chunk.count as usize;
        } else if let Some(batch) = batches.get(chunk.batch as usize) {
            culled_work += batch.index_count as u64 * chunk.count as u64;
        }
    }
    let total_work = batches
        .iter()
        .map(|batch| batch.index_count as u64 * batch.count as u64)
        .sum::<u64>();
    let profitable = culled_work > 0 && culled_work >= total_work.div_ceil(MIN_CULLED_WORK_DIVISOR);
    (visible_instances, profitable)
}

/// compute 端每批的压缩说明。
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct BatchMeta {
    first: u32,
    index_count: u32,
    group_start: u32,
    group_count: u32,
}

/// 一组连续实例；`query_index == u32::MAX` 表示无条件保留。
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GroupMeta {
    first: u32,
    count: u32,
    query_index: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CompactParams {
    batch_count: u32,
    _pad: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ProxyBounds {
    center: [f32; 4],
    extent: [f32; 4],
}

impl From<Bounds> for ProxyBounds {
    fn from(bounds: Bounds) -> Self {
        let center = bounds.center();
        let extent = bounds.half_extent();
        Self {
            center: [center.x, center.y, center.z, 1.0],
            extent: [extent.x, extent.y, extent.z, 0.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ProxyVertex {
    position: [f32; 3],
}

impl ProxyVertex {
    const ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

const PROXY_VERTICES: [ProxyVertex; 36] = {
    const fn v(x: f32, y: f32, z: f32) -> ProxyVertex {
        ProxyVertex {
            position: [x, y, z],
        }
    }
    [
        // -Z / +Z
        v(-1.0, -1.0, -1.0),
        v(1.0, 1.0, -1.0),
        v(1.0, -1.0, -1.0),
        v(-1.0, -1.0, -1.0),
        v(-1.0, 1.0, -1.0),
        v(1.0, 1.0, -1.0),
        v(-1.0, -1.0, 1.0),
        v(1.0, -1.0, 1.0),
        v(1.0, 1.0, 1.0),
        v(-1.0, -1.0, 1.0),
        v(1.0, 1.0, 1.0),
        v(-1.0, 1.0, 1.0),
        // -X / +X
        v(-1.0, -1.0, -1.0),
        v(-1.0, -1.0, 1.0),
        v(-1.0, 1.0, 1.0),
        v(-1.0, -1.0, -1.0),
        v(-1.0, 1.0, 1.0),
        v(-1.0, 1.0, -1.0),
        v(1.0, -1.0, -1.0),
        v(1.0, 1.0, 1.0),
        v(1.0, -1.0, 1.0),
        v(1.0, -1.0, -1.0),
        v(1.0, 1.0, -1.0),
        v(1.0, 1.0, 1.0),
        // -Y / +Y
        v(-1.0, -1.0, -1.0),
        v(1.0, -1.0, -1.0),
        v(1.0, -1.0, 1.0),
        v(-1.0, -1.0, -1.0),
        v(1.0, -1.0, 1.0),
        v(-1.0, -1.0, 1.0),
        v(-1.0, 1.0, -1.0),
        v(-1.0, 1.0, 1.0),
        v(1.0, 1.0, 1.0),
        v(-1.0, 1.0, -1.0),
        v(1.0, 1.0, 1.0),
        v(1.0, 1.0, -1.0),
    ]
};

const COMPACT_WGSL: &str = r#"
struct BatchMeta {
    first: u32,
    index_count: u32,
    group_start: u32,
    group_count: u32,
}
struct GroupMeta {
    first: u32,
    count: u32,
    query_index: u32,
    _pad: u32,
}
struct DrawIndexedIndirectArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: u32,
    first_instance: u32,
}
struct CompactParams {
    batch_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<storage, read> query_words: array<u32>;
@group(0) @binding(1) var<storage, read> batches: array<BatchMeta>;
@group(0) @binding(2) var<storage, read> groups: array<GroupMeta>;
@group(0) @binding(3) var<storage, read_write> visible_indices: array<u32>;
@group(0) @binding(4) var<storage, read_write> indirect: array<DrawIndexedIndirectArgs>;
@group(0) @binding(5) var<uniform> params: CompactParams;

fn query_visible(index: u32) -> bool {
    return (query_words[index * 2u] | query_words[index * 2u + 1u]) != 0u;
}

@compute @workgroup_size(64)
fn cs_compact(@builtin(global_invocation_id) gid: vec3<u32>) {
    let batch_index = gid.x;
    if (batch_index >= params.batch_count) { return; }
    let batch_info = batches[batch_index];
    var output_count = 0u;
    for (var g = 0u; g < batch_info.group_count; g = g + 1u) {
        let group = groups[batch_info.group_start + g];
        let visible = group.query_index == 0xffffffffu || query_visible(group.query_index);
        if (visible) {
            for (var i = 0u; i < group.count; i = i + 1u) {
                visible_indices[batch_info.first + output_count] = group.first + i;
                output_count = output_count + 1u;
            }
        }
    }
    indirect[batch_index] = DrawIndexedIndirectArgs(
        batch_info.index_count,
        output_count,
        0u,
        0u,
        0u,
    );
}
"#;

const PROXY_WGSL: &str = r#"
struct CameraUniform {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
}
struct ProxyBounds { center: vec4<f32>, extent: vec4<f32> }
@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(0) @binding(1) var<storage, read> proxies: array<ProxyBounds>;

@vertex
fn vs_proxy(
    @location(0) position: vec3<f32>,
    @builtin(instance_index) instance_index: u32,
) -> @builtin(position) vec4<f32> {
    let proxy = proxies[instance_index];
    let world = proxy.center.xyz + position * proxy.extent.xyz;
    return camera.view_proj * vec4<f32>(world, 1.0);
}
"#;

#[derive(Default)]
struct HistorySnapshot {
    camera_bits: [u32; 20],
    draws: Vec<u8>,
    bones: Vec<u8>,
    batches: Vec<BatchInput>,
    bounds: Vec<[u32; 6]>,
}

fn bounds_bits(bounds: &Bounds) -> [u32; 6] {
    [
        bounds.min.x.to_bits(),
        bounds.min.y.to_bits(),
        bounds.min.z.to_bits(),
        bounds.max.x.to_bits(),
        bounds.max.y.to_bits(),
        bounds.max.z.to_bits(),
    ]
}

impl HistorySnapshot {
    fn matches(
        &self,
        camera_bits: &[u32; 20],
        draws: &[u8],
        bones: &[u8],
        batches: &[BatchInput],
        bounds: &[Bounds],
    ) -> bool {
        self.camera_bits == *camera_bits
            && self.draws == draws
            && self.bones == bones
            && self.batches == batches
            && self.bounds.len() == bounds.len()
            && self
                .bounds
                .iter()
                .zip(bounds)
                .all(|(snapshot, current)| *snapshot == bounds_bits(current))
    }

    fn copy_from(
        &mut self,
        camera_bits: [u32; 20],
        draws: &[u8],
        bones: &[u8],
        batches: &[BatchInput],
        bounds: &[Bounds],
    ) {
        self.camera_bits = camera_bits;
        self.draws.clear();
        self.draws.extend_from_slice(draws);
        self.bones.clear();
        self.bones.extend_from_slice(bones);
        self.batches.clear();
        self.batches.extend_from_slice(batches);
        self.bounds.clear();
        self.bounds.extend(bounds.iter().map(bounds_bits));
    }
}

pub(crate) struct OcclusionCuller {
    enabled: bool,
    supported: bool,
    compact_pipeline: wgpu::ComputePipeline,
    compact_bind_group: wgpu::BindGroup,
    proxy_pipeline: wgpu::RenderPipeline,
    proxy_bind_group: wgpu::BindGroup,
    proxy_vertex_buffer: wgpu::Buffer,
    query_set: wgpu::QuerySet,
    query_buffer: wgpu::Buffer,
    query_readback: wgpu::Buffer,
    readback_status: Arc<AtomicU8>,
    readback_needs_map: bool,
    readback_mapping: bool,
    readback_query_count: u32,
    batch_buffer: wgpu::Buffer,
    group_buffer: wgpu::Buffer,
    proxy_buffer: wgpu::Buffer,
    visible_indices: wgpu::Buffer,
    indirect_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    chunks: Vec<QueryChunk>,
    batch_staging: Vec<BatchMeta>,
    group_staging: Vec<GroupMeta>,
    proxy_staging: Vec<ProxyBounds>,
    history: HistorySnapshot,
    history_ready: bool,
    visibility_profitable: Option<bool>,
    visible_instances: Option<usize>,
    queries_pending: bool,
    last_compacted: bool,
    query_count: u32,
    batch_count: u32,
}

impl OcclusionCuller {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        device: &wgpu::Device,
        camera_buffer: &wgpu::Buffer,
        sample_count: u32,
        max_draws: usize,
        query_resolve_on_queue: bool,
    ) -> Self {
        let max_queries = (max_draws as u32 / MIN_QUERY_GROUP_SIZE).max(1);
        let max_batches = max_draws.max(1) as u64;

        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("scene_occlusion_queries"),
            ty: wgpu::QueryType::Occlusion,
            count: max_queries,
        });
        let query_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_occlusion_query_sb"),
            size: max_queries as u64 * wgpu::QUERY_SIZE as u64,
            usage: wgpu::BufferUsages::QUERY_RESOLVE
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let query_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_occlusion_query_readback"),
            size: max_queries as u64 * wgpu::QUERY_SIZE as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let batch_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_occlusion_batch_sb"),
            size: max_batches * std::mem::size_of::<BatchMeta>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_occlusion_group_sb"),
            size: max_batches * std::mem::size_of::<GroupMeta>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let proxy_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_occlusion_proxy_sb"),
            size: max_queries as u64 * std::mem::size_of::<ProxyBounds>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let visible_indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_occlusion_visible_indices_sb"),
            size: max_draws.max(1) as u64 * 4,
            // compute 写、主 pass 作为 instance-rate u32 顶点流切片读。indirect
            // 固定从实例 0 起步，不依赖后端对非零 first_instance 的 builtin 支持。
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        let indirect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_occlusion_indirect_sb"),
            size: max_batches * DRAW_INDEXED_INDIRECT_STRIDE,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_occlusion_params_ub"),
            size: std::mem::size_of::<CompactParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let compact_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_occlusion_compact_bgl"),
            entries: &[
                storage_entry(0, true, wgpu::ShaderStages::COMPUTE),
                storage_entry(1, true, wgpu::ShaderStages::COMPUTE),
                storage_entry(2, true, wgpu::ShaderStages::COMPUTE),
                storage_entry(3, false, wgpu::ShaderStages::COMPUTE),
                storage_entry(4, false, wgpu::ShaderStages::COMPUTE),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let compact_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_occlusion_compact_bg"),
            layout: &compact_bgl,
            entries: &[
                buffer_entry(0, &query_buffer),
                buffer_entry(1, &batch_buffer),
                buffer_entry(2, &group_buffer),
                buffer_entry(3, &visible_indices),
                buffer_entry(4, &indirect_buffer),
                buffer_entry(5, &params_buffer),
            ],
        });
        let compact_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_occlusion_compact_cs"),
            source: wgpu::ShaderSource::Wgsl(COMPACT_WGSL.into()),
        });
        let compact_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_occlusion_compact_pl"),
            bind_group_layouts: &[&compact_bgl],
            push_constant_ranges: &[],
        });
        let compact_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("scene_occlusion_compact_pipeline"),
            layout: Some(&compact_layout),
            module: &compact_shader,
            entry_point: Some("cs_compact"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let proxy_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_occlusion_proxy_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage_entry(1, true, wgpu::ShaderStages::VERTEX),
            ],
        });
        let proxy_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_occlusion_proxy_bg"),
            layout: &proxy_bgl,
            entries: &[
                buffer_entry(0, camera_buffer),
                buffer_entry(1, &proxy_buffer),
            ],
        });
        let proxy_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_occlusion_proxy_vs"),
            source: wgpu::ShaderSource::Wgsl(PROXY_WGSL.into()),
        });
        let proxy_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_occlusion_proxy_pl"),
            bind_group_layouts: &[&proxy_bgl],
            push_constant_ranges: &[],
        });
        let proxy_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene_occlusion_proxy_pipeline"),
            layout: Some(&proxy_layout),
            vertex: wgpu::VertexState {
                module: &proxy_shader,
                entry_point: Some("vs_proxy"),
                buffers: &[ProxyVertex::layout()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count.max(1),
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });
        let proxy_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_occlusion_proxy_vertices"),
            contents: bytemuck::cast_slice(&PROXY_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Self {
            enabled: true,
            supported: query_resolve_on_queue,
            compact_pipeline,
            compact_bind_group,
            proxy_pipeline,
            proxy_bind_group,
            proxy_vertex_buffer,
            query_set,
            query_buffer,
            query_readback,
            readback_status: Arc::new(AtomicU8::new(READBACK_PENDING)),
            readback_needs_map: false,
            readback_mapping: false,
            readback_query_count: 0,
            batch_buffer,
            group_buffer,
            proxy_buffer,
            visible_indices,
            indirect_buffer,
            params_buffer,
            chunks: Vec::new(),
            batch_staging: Vec::new(),
            group_staging: Vec::new(),
            proxy_staging: Vec::new(),
            history: HistorySnapshot::default(),
            history_ready: false,
            visibility_profitable: None,
            visible_instances: None,
            queries_pending: false,
            last_compacted: false,
            query_count: 0,
            batch_count: 0,
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.history_ready = false;
            self.queries_pending = false;
            self.last_compacted = false;
            self.visibility_profitable = None;
            self.visible_instances = None;
            self.query_count = 0;
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn supported(&self) -> bool {
        self.supported
    }

    pub(crate) fn wants_bounds(&self) -> bool {
        self.enabled && self.supported
    }

    fn readback_busy(&self) -> bool {
        self.readback_needs_map || self.readback_mapping
    }

    /// 推进查询结果的小读回，但绝不等待 GPU。
    ///
    /// `map_async` 的回调只写原子状态；真正读取映射区、计算收益与 `unmap` 都留在
    /// 渲染线程。这样既没有通道分配，也不会把回调线程引进渲染状态。
    fn poll_readback(&mut self, device: &wgpu::Device) {
        if self.readback_needs_map && !self.readback_mapping {
            let size = self.readback_query_count as u64 * wgpu::QUERY_SIZE as u64;
            if size == 0 {
                self.readback_needs_map = false;
            } else {
                self.readback_status
                    .store(READBACK_PENDING, Ordering::Release);
                let status = Arc::clone(&self.readback_status);
                self.query_readback
                    .slice(0..size)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        status.store(
                            if result.is_ok() {
                                READBACK_READY
                            } else {
                                READBACK_FAILED
                            },
                            Ordering::Release,
                        );
                    });
                self.readback_needs_map = false;
                self.readback_mapping = true;
            }
        }

        if !self.readback_mapping {
            return;
        }

        // `Poll` 只让已经完成的回调前进；与 `Wait` 不同，它不会把 CPU 卡在 GPU 上。
        let _ = device.poll(wgpu::Maintain::Poll);
        match self.readback_status.load(Ordering::Acquire) {
            READBACK_READY => {
                let size = self.readback_query_count as u64 * wgpu::QUERY_SIZE as u64;
                let mapped = self.query_readback.slice(0..size).get_mapped_range();
                let (visible, profitable) =
                    evaluate_visibility(&self.history.batches, &self.chunks, &mapped);
                drop(mapped);
                self.query_readback.unmap();
                self.readback_mapping = false;
                self.readback_query_count = 0;
                self.visible_instances = Some(visible);
                self.visibility_profitable = Some(profitable);
            }
            READBACK_FAILED => {
                // 映射失败不能猜“不可见”。废弃这份历史，下一帧重新查询；在此之前
                // 始终完整绘制。
                self.readback_mapping = false;
                self.readback_query_count = 0;
                self.history_ready = false;
                self.visibility_profitable = None;
                self.visible_instances = None;
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        batches: &[BatchInput],
        slot_bounds: &[Bounds],
        render_view_proj: Mat4,
        camera_pos: Vec3,
        draw_bytes: &[u8],
        bone_bytes: &[u8],
        dynamic_geometry: bool,
    ) -> bool {
        self.poll_readback(device);
        self.batch_count = batches.len() as u32;
        self.queries_pending = false;
        self.last_compacted = false;

        let mut camera_bits = [0u32; 20];
        for (dst, value) in camera_bits[..16]
            .iter_mut()
            .zip(render_view_proj.to_cols_array())
        {
            *dst = value.to_bits();
        }
        camera_bits[16] = camera_pos.x.to_bits();
        camera_bits[17] = camera_pos.y.to_bits();
        camera_bits[18] = camera_pos.z.to_bits();
        camera_bits[19] = 1.0f32.to_bits();
        let same_as_history = self.history_ready
            && self
                .history
                .matches(&camera_bits, draw_bytes, bone_bytes, batches, slot_bounds);

        // 持久 readback 缓冲在 map 期间不能再接收 COPY_DST。若输入恰在此时变化，
        // 保留旧快照直到回调收尾，但绝不复用旧可见性；新查询延到缓冲可写的帧。
        if self.readback_busy() {
            if !same_as_history {
                self.visibility_profitable = None;
                self.visible_instances = None;
            }
            return false;
        }

        if !self.enabled || !self.supported || dynamic_geometry {
            self.query_count = 0;
            self.history_ready = false;
            self.visibility_profitable = None;
            self.visible_instances = None;
            return false;
        }

        if !has_enough_candidate_work(batches) {
            self.query_count = 0;
            self.history_ready = false;
            self.visibility_profitable = None;
            self.visible_instances = None;
            return false;
        }

        if same_as_history {
            // 输入逐字节未变时，深度与 AABB 查询结果也不可能变化。继续消费上一份
            // query buffer 即可；但只有异步收益判定通过后才支付重映射固定税。
            let active = self.visibility_profitable == Some(true);
            self.last_compacted = active;
            return active;
        }

        self.visibility_profitable = None;
        self.visible_instances = None;
        self.chunks.clear();
        self.batch_staging.clear();
        self.group_staging.clear();
        self.proxy_staging.clear();
        self.query_count = 0;

        for (batch_index, batch) in batches.iter().copied().enumerate() {
            let group_start = self.group_staging.len() as u32;
            let mut offset = 0u32;
            while offset < batch.count {
                let count = (batch.count - offset).min(QUERY_GROUP_SIZE);
                let first = batch.first + offset;
                let bounds = slot_bounds
                    .get(first as usize..(first + count) as usize)
                    .unwrap_or(&[])
                    .iter()
                    .fold(Bounds::EMPTY, |union, bounds| union.union(bounds));
                let query = if should_query_group(batch.index_count, count, bounds, camera_pos) {
                    let query = self.query_count;
                    self.query_count += 1;
                    self.proxy_staging.push(bounds.into());
                    Some(query)
                } else {
                    None
                };
                self.chunks.push(QueryChunk {
                    batch: batch_index as u32,
                    first,
                    count,
                    query,
                });
                self.group_staging.push(GroupMeta {
                    first,
                    count,
                    query_index: query.unwrap_or(ALWAYS_VISIBLE),
                    _pad: 0,
                });
                offset += count;
            }
            self.batch_staging.push(BatchMeta {
                first: batch.first,
                index_count: batch.index_count,
                group_start,
                group_count: self.group_staging.len() as u32 - group_start,
            });
        }

        if self.query_count == 0 {
            self.history_ready = false;
            return false;
        }

        queue.write_buffer(
            &self.batch_buffer,
            0,
            bytemuck::cast_slice(&self.batch_staging),
        );
        queue.write_buffer(
            &self.group_buffer,
            0,
            bytemuck::cast_slice(&self.group_staging),
        );
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::bytes_of(&CompactParams {
                batch_count: self.batch_count,
                _pad: [0; 3],
            }),
        );
        queue.write_buffer(
            &self.proxy_buffer,
            0,
            bytemuck::cast_slice(&self.proxy_staging),
        );
        self.history
            .copy_from(camera_bits, draw_bytes, bone_bytes, batches, slot_bounds);
        self.queries_pending = true;
        false
    }

    pub(crate) fn query_count(&self) -> u32 {
        self.query_count
    }

    pub(crate) fn visible_indices(&self) -> &wgpu::Buffer {
        &self.visible_indices
    }

    pub(crate) fn indirect_buffer(&self) -> &wgpu::Buffer {
        &self.indirect_buffer
    }

    pub(crate) fn compact(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("scene_occlusion_compact_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.compact_pipeline);
        pass.set_bind_group(0, &self.compact_bind_group, &[]);
        pass.dispatch_workgroups(self.batch_count.div_ceil(64), 1, 1);
    }

    /// 在主通道完成后，用廉价 AABB 对最终不透明深度发查询，供下一稳定帧使用。
    pub(crate) fn record_queries(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
    ) {
        if !self.queries_pending {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene_occlusion_proxy_query_pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: Some(&self.query_set),
        });
        pass.set_pipeline(&self.proxy_pipeline);
        pass.set_bind_group(0, &self.proxy_bind_group, &[]);
        pass.set_vertex_buffer(0, self.proxy_vertex_buffer.slice(..));
        for chunk in &self.chunks {
            let Some(query) = chunk.query else {
                continue;
            };
            pass.begin_occlusion_query(query);
            pass.draw(0..PROXY_VERTICES.len() as u32, query..query + 1);
            pass.end_occlusion_query();
        }
        drop(pass);
        encoder.resolve_query_set(&self.query_set, 0..self.query_count, &self.query_buffer, 0);
        let readback_size = self.query_count as u64 * wgpu::QUERY_SIZE as u64;
        encoder.copy_buffer_to_buffer(
            &self.query_buffer,
            0,
            &self.query_readback,
            0,
            readback_size,
        );
        self.readback_query_count = self.query_count;
        self.readback_needs_map = true;
        self.history_ready = true;
        self.queries_pending = false;
    }

    /// 最近一份已完成查询的可见实例数；尚未异步读回时返回 `None`，从不阻塞。
    pub(crate) fn visible_instances(&self) -> Option<usize> {
        self.visible_instances
    }
}

fn storage_entry(
    binding: u32,
    read_only: bool,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn buffer_entry<'a>(binding: u32, buffer: &'a wgpu::Buffer) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_struct_strides_match_wgsl_arrays() {
        assert_eq!(std::mem::size_of::<BatchMeta>(), 16);
        assert_eq!(std::mem::size_of::<GroupMeta>(), 16);
        assert_eq!(std::mem::size_of::<CompactParams>(), 16);
        assert_eq!(std::mem::size_of::<ProxyBounds>(), 32);
        assert_eq!(DRAW_INDEXED_INDIRECT_STRIDE, 20);
    }

    #[test]
    fn history_snapshot_is_exact_not_hash_based() {
        let batches = [BatchInput {
            first: 0,
            count: 64,
            index_count: 36,
            primitive_key: 7,
        }];
        let bounds = [Bounds {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        }];
        let mut snapshot = HistorySnapshot::default();
        snapshot.copy_from([1; 20], &[1, 2], &[3, 4], &batches, &bounds);
        assert!(snapshot.matches(&[1; 20], &[1, 2], &[3, 4], &batches, &bounds));
        assert!(!snapshot.matches(&[2; 20], &[1, 2], &[3, 4], &batches, &bounds));
        assert!(!snapshot.matches(&[1; 20], &[1, 9], &[3, 4], &batches, &bounds));
        assert!(!snapshot.matches(&[1; 20], &[1, 2], &[3, 8], &batches, &bounds));
        let mut reloaded = batches;
        reloaded[0].primitive_key += 1;
        assert!(!snapshot.matches(&[1; 20], &[1, 2], &[3, 4], &reloaded, &bounds));
        let mut moved = bounds;
        moved[0].max.x = 2.0;
        assert!(!snapshot.matches(&[1; 20], &[1, 2], &[3, 4], &batches, &moved));

        let signed_zero = [Bounds {
            min: Vec3::new(-0.0, -1.0, -1.0),
            max: Vec3::ONE,
        }];
        snapshot.copy_from([1; 20], &[1, 2], &[3, 4], &batches, &signed_zero);
        let positive_zero = [Bounds {
            min: Vec3::new(0.0, -1.0, -1.0),
            max: Vec3::ONE,
        }];
        assert!(!snapshot.matches(&[1; 20], &[1, 2], &[3, 4], &batches, &positive_zero));
    }

    #[test]
    fn proxy_cube_has_twelve_triangles() {
        assert_eq!(PROXY_VERTICES.len(), 36);
        assert!(PROXY_VERTICES
            .iter()
            .all(|vertex| vertex.position.iter().all(|value| value.abs() == 1.0)));
    }

    #[test]
    fn cost_gate_rejects_low_poly_and_small_workloads() {
        let low_poly = [BatchInput {
            first: 0,
            count: 100_000,
            index_count: MIN_INDICES_PER_INSTANCE - 1,
            primitive_key: 1,
        }];
        assert!(!has_enough_candidate_work(&low_poly));

        let too_small = [BatchInput {
            first: 0,
            count: MIN_QUERY_GROUP_SIZE,
            index_count: MIN_INDICES_PER_INSTANCE,
            primitive_key: 2,
        }];
        assert!(!has_enough_candidate_work(&too_small));

        let enough = [BatchInput {
            first: 0,
            count: MIN_TOTAL_CANDIDATE_INDICES.div_ceil(MIN_INDICES_PER_INSTANCE as u64) as u32,
            index_count: MIN_INDICES_PER_INSTANCE,
            primitive_key: 3,
        }];
        assert!(has_enough_candidate_work(&enough));
    }

    #[test]
    fn visibility_gate_only_enables_when_culled_work_is_large_enough() {
        let batches = [BatchInput {
            first: 0,
            count: 256,
            index_count: 1024,
            primitive_key: 1,
        }];
        let chunks = [
            QueryChunk {
                batch: 0,
                first: 0,
                count: 128,
                query: Some(0),
            },
            QueryChunk {
                batch: 0,
                first: 128,
                count: 128,
                query: Some(1),
            },
        ];

        let hidden = [1u64, 0];
        assert_eq!(
            evaluate_visibility(&batches, &chunks, bytemuck::cast_slice(&hidden)),
            (128, true),
        );

        let open = [1u64, 1];
        assert_eq!(
            evaluate_visibility(&batches, &chunks, bytemuck::cast_slice(&open)),
            (256, false),
        );

        // 缺失或越界的查询结果一律按可见处理，绝不能因诊断数据不完整而漏画。
        assert_eq!(evaluate_visibility(&batches, &chunks, &[]), (256, false));
    }

    #[test]
    fn small_tail_and_camera_inside_are_always_visible() {
        let bounds = Bounds {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        };
        assert!(should_query_group(
            MIN_INDICES_PER_INSTANCE,
            MIN_QUERY_GROUP_SIZE,
            bounds,
            Vec3::splat(2.0),
        ));
        assert!(!should_query_group(
            MIN_INDICES_PER_INSTANCE,
            MIN_QUERY_GROUP_SIZE - 1,
            bounds,
            Vec3::splat(2.0),
        ));
        assert!(!should_query_group(
            MIN_INDICES_PER_INSTANCE,
            MIN_QUERY_GROUP_SIZE,
            bounds,
            Vec3::ZERO,
        ));
        assert!(!should_query_group(
            MIN_INDICES_PER_INSTANCE,
            MIN_QUERY_GROUP_SIZE,
            bounds,
            bounds.max,
        ));
    }
}
