//! 形变目标（morph target / blend shape）：顶点级形变的 CPU 实现。
//!
//! # 它和骨骼动画的分工
//!
//! 骨骼动画用**骨头**带动顶点（蒙皮权重决定"谁跟着哪根骨走"），适合身体；
//! 形变目标让**顶点自己**存一份位移增量、按权重线性混合，适合表情、口型、
//! 肌肉鼓起这类局部精细变形。两者可以同时存在：先形变、再蒙皮。
//!
//! `final = base + Σ wᵢ · (targetᵢ − base)`
//!
//! # 为什么是 CPU（而不是着色器里累加）
//!
//! 商业引擎把增量塞进 storage buffer、在顶点着色器里累加。这里 v1 走 CPU
//! 重写顶点缓冲，理由与 CPU 粒子同构：
//!
//! - 只有**权重变化时**才重算；静止的模型零开销（`apply` 里比对上次权重）；
//! - 面部网格量级很小（几千顶点 × 每顶点 76 字节左右），一帧一次
//!   `write_buffer` 远没到 PCIe 的痛处；
//! - 换来的是**零着色器改动、零新增绑定、零槽位分配** —— GPU 侧三处改动
//!   换成一个纯函数，而这个纯函数能被单测逐位钉死（CI 没有显卡）。
//!
//! 万级顶点 / 几十个角色同屏时再上 GPU：增量数据本来就是按
//! target-major 平铺的（`positions[t * vertex_count + v]`），搬进 storage
//! buffer 不需要改数据布局，只需要加一次上传与一段 WGSL。
//!
//! # 刻意不做的
//!
//! - **tangent 增量**：引擎的 PBR 不算法线贴图 TBN，收了也没有消费者。
//! - **sparse accessor 之外的格式兼容**：交给 gltf crate 的 `Iter` 统一处理。
//! - **负权重 / 超过 1 的权重**：按 `clamp(0, 1)` 处理（Unity 的 blendshape
//!   同款语义）。真实资产偶尔会写 1.0000001 这种浮点毛刺，钳住比不钳稳。

use wgpu::util::DeviceExt;

use crate::gltf_loader::GltfVertex;

/// 一个图元的全部形变目标。
///
/// 只有**带 morph 的图元**才持有一份，其余图元不为它多付内存。
pub struct MorphTargets {
    /// 目标名（glTF `mesh.extras.targetNames`；资产没写就用 `Target N`）。
    names: Vec<String>,
    /// mesh 自带的默认权重（glTF `mesh.weights`），长度对齐 `names`。
    defaults: Vec<f32>,
    /// 顶点位置增量，**target-major**：`positions[t * vertex_count + v]`。
    ///
    /// 已乘 `model_scale`（与顶点位置同一套烘焙规则，否则改导入缩放会让
    /// 形变幅度与模型尺寸脱钩）。
    positions: Vec<[f32; 3]>,
    /// 顶点法线增量，布局同上。资产没给就整段为空（只变形不重算法线）。
    normals: Vec<[f32; 3]>,
    /// 未变形的原始顶点 —— 每帧形变的基准。
    base: Vec<GltfVertex>,
}

/// 一个实体、一个图元独占的形变顶点缓冲。
///
/// 网格与形变增量仍可共享，但这份 GPU 顶点数据不能共享：两个实体摆不同
/// 表情时，若写入同一块缓冲，后写的实体就会覆盖先写的实体。
pub struct MorphBuffer {
    pub buffer: wgpu::Buffer,
    state: MorphState,
}

#[derive(Default)]
struct MorphState {
    /// 上次写进 GPU 的权重（空 = 还没应用过）。
    applied: Vec<f32>,
    /// 与目标数对齐之后的权重（`applied` 的候选，比对用）。
    resolved: Vec<f32>,
    /// 复用缓冲：避免每帧为几千个顶点新分配一次。
    scratch: Vec<GltfVertex>,
}

impl MorphTargets {
    /// 构造。`positions` / `normals` 必须已按 target-major 平铺。
    ///
    /// 长度不自洽（`positions.len() != names.len() * base.len()`）时返回
    /// `None`：宁可这个图元当静态网格渲染，也不要拿错位的增量去挪顶点
    /// —— 那会画出一团谁都不认识的几何，且比"没有表情"难查得多。
    pub fn new(
        names: Vec<String>,
        defaults: Vec<f32>,
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        base: Vec<GltfVertex>,
    ) -> Option<Self> {
        let target_count = names.len();
        if target_count == 0 || base.is_empty() {
            return None;
        }
        if positions.len() != target_count * base.len() {
            return None;
        }
        if !normals.is_empty() && normals.len() != positions.len() {
            return None;
        }
        let mut defaults = defaults;
        defaults.resize(target_count, 0.0);
        Some(Self {
            names,
            defaults,
            positions,
            normals,
            base,
        })
    }

    /// 形变目标个数。
    pub fn target_count(&self) -> usize {
        self.names.len()
    }

    /// 顶点个数（= 每次重写的顶点数）。
    pub fn vertex_count(&self) -> usize {
        self.base.len()
    }

    /// 目标名（Inspector 的滑条标题、脚本按名查找的唯一来源）。
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// 资产自带的默认权重。
    pub fn defaults(&self) -> &[f32] {
        &self.defaults
    }

    /// 位置增量（target-major 平铺：`[target * vertex_count + vertex]`）。
    ///
    /// 渲染路径不需要它（`apply_source` 内部用），留给**测试与诊断**：
    /// "导入缩放有没有乘进增量""增量有没有串到别的目标上"这类问题，
    /// 只能靠逐元素读数判断。
    pub fn position_deltas(&self) -> &[[f32; 3]] {
        &self.positions
    }

    /// 实体侧权重 → 与目标数对齐的最终权重。
    ///
    /// 规则（与动画的优先级一起构成完整语义，见 `scene_pipeline::render`）：
    /// - 实体没写（空）→ 资产默认；
    /// - 实体写得比目标少 → 缺的用资产默认补齐；
    /// - 实体写得比目标多（换了模型）→ 多出来的丢弃。
    ///
    /// 三条合起来保证"换模型不会让权重错位到别的表情上"——这是唯一安全的
    /// 解释，因为 morph 目标只按**下标**标识，下标跨模型没有意义。
    pub fn resolve_into(&self, entity: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.reserve(self.names.len());
        for i in 0..self.names.len() {
            let value = entity
                .get(i)
                .copied()
                .unwrap_or_else(|| self.defaults.get(i).copied().unwrap_or(0.0));
            out.push(sanitize_weight(value));
        }
    }

    /// 所有合法权重组合的保守盒；导入时计算一次，避免剔除先于权重采样而漏画。
    pub fn conservative_bounds(&self) -> crate::gltf_loader::Bounds {
        use crate::gltf_loader::Bounds;
        use xgame_math::glam::Vec3;

        let mut bounds = Bounds::EMPTY;
        for (v, vertex) in self.base.iter().enumerate() {
            let mut min = Vec3::from(vertex.position);
            let mut max = min;
            for t in 0..self.target_count() {
                let delta = Vec3::from(self.positions[t * self.vertex_count() + v]);
                // 每个权重独立落在 0..=1；不能只并各个单目标的终点。
                min += delta.min(Vec3::ZERO);
                max += delta.max(Vec3::ZERO);
            }
            bounds = bounds.union(&Bounds { min, max });
        }
        bounds
    }

    /// 为一个实体创建独立的 GPU 顶点缓冲，初值是未变形的原始顶点。
    pub fn create_buffer(&self, device: &wgpu::Device) -> MorphBuffer {
        MorphBuffer {
            buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("morph_instance_vertices"),
                contents: bytemuck::cast_slice(&self.base),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            }),
            state: MorphState::default(),
        }
    }

    /// 把权重形变后的结果写进该实体自己的缓冲；与上次相同就跳过。
    pub fn apply_source(
        &self,
        queue: &wgpu::Queue,
        output: &mut MorphBuffer,
        source: &[f32],
    ) -> bool {
        if !output.state.prepare(self, source) {
            return false;
        }
        queue.write_buffer(
            &output.buffer,
            0,
            bytemuck::cast_slice(&output.state.scratch),
        );
        true
    }
}

impl MorphState {
    /// CPU 部分与 GPU 写入分开：既复用各自的权重缓存，也能在无显卡 CI
    /// 验证两个实例交替更新时不会互相覆盖。
    fn prepare(&mut self, morph: &MorphTargets, source: &[f32]) -> bool {
        let mut resolved = std::mem::take(&mut self.resolved);
        morph.resolve_into(source, &mut resolved);
        if self.applied == resolved {
            self.resolved = resolved;
            return false;
        }
        morph_vertices(
            &morph.base,
            &morph.positions,
            &morph.normals,
            morph.names.len(),
            &resolved,
            &mut self.scratch,
        );
        self.resolved = std::mem::replace(&mut self.applied, resolved);
        true
    }
}

/// 权重合法化：非有限值当 0，其余钳到 `[0, 1]`。
pub fn sanitize_weight(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// 纯函数：把形变目标按权重累加到 `base` 上，结果写进 `out`。
///
/// 拆出来是为了能在**没有 GPU** 的环境里测（CI 的 runner 没有显卡）：
/// 这里错一位，屏幕上就是一张歪脸，而歪脸很难从截图里看出"是哪一步错了"。
///
/// 语义细节：
/// - 权重为 0 的目标整段跳过（面部资产常带 20+ 目标，同帧只有几个非 0）；
/// - 有法线增量时**先累加再归一化**；归一化后长度为 0（增量正好抵消）时
///   保留基准法线，不写 NaN；
/// - `out` 先被清空再填，调用方可以复用同一个 Vec。
pub fn morph_vertices(
    base: &[GltfVertex],
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    target_count: usize,
    weights: &[f32],
    out: &mut Vec<GltfVertex>,
) {
    out.clear();
    out.extend_from_slice(base);
    let vertex_count = base.len();
    if vertex_count == 0 {
        return;
    }
    let has_normals = normals.len() == positions.len();
    let mut normals_dirty = false;
    for (target, raw) in weights.iter().take(target_count).enumerate() {
        let weight = sanitize_weight(*raw);
        if weight == 0.0 {
            continue;
        }
        let Some(deltas) = positions.get(target * vertex_count..(target + 1) * vertex_count) else {
            continue;
        };
        for (vertex, delta) in out.iter_mut().zip(deltas) {
            vertex.position[0] += weight * delta[0];
            vertex.position[1] += weight * delta[1];
            vertex.position[2] += weight * delta[2];
        }
        if has_normals {
            let Some(deltas) = normals.get(target * vertex_count..(target + 1) * vertex_count)
            else {
                continue;
            };
            for (vertex, delta) in out.iter_mut().zip(deltas) {
                vertex.normal[0] += weight * delta[0];
                vertex.normal[1] += weight * delta[1];
                vertex.normal[2] += weight * delta[2];
            }
            normals_dirty = true;
        }
    }
    // 只有**真的加过法线增量**才归一化：权重全 0 时连最后一位都不该动，
    // 否则"关掉表情"与"从来没有表情"在截图里会差出浮点级噪声。
    if normals_dirty {
        for vertex in out.iter_mut() {
            let (x, y, z) = (vertex.normal[0], vertex.normal[1], vertex.normal[2]);
            let length_sq = x * x + y * y + z * z;
            if length_sq > 1e-12 {
                let inv = 1.0 / length_sq.sqrt();
                vertex.normal = [x * inv, y * inv, z * inv];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertex(position: [f32; 3]) -> GltfVertex {
        GltfVertex {
            position,
            normal: [0.0, 1.0, 0.0],
            color: [1.0, 1.0, 1.0],
            tex_coord: [0.0, 0.0],
            joints: [0.0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }

    /// 两个顶点、一个目标：目标把两个顶点各抬起 +2。
    fn simple(base: Vec<GltfVertex>) -> MorphTargets {
        let n = base.len();
        MorphTargets::new(
            vec!["Lift".into()],
            vec![0.0],
            vec![[0.0, 2.0, 0.0]; n],
            Vec::new(),
            base,
        )
        .expect("形状自洽")
    }

    #[test]
    fn morph_bounds_cover_mixed_sign_targets_and_fractional_weights() {
        let morph = MorphTargets::new(
            vec!["A".into(), "B".into()],
            vec![0.0; 2],
            vec![[2.0, -3.0, 1.0], [-4.0, 2.0, -2.0]],
            Vec::new(),
            vec![vertex([1.0, 2.0, 3.0])],
        )
        .unwrap();
        let bounds = morph.conservative_bounds();
        assert_eq!(bounds.min.to_array(), [-3.0, -1.0, 1.0]);
        assert_eq!(bounds.max.to_array(), [3.0, 4.0, 4.0]);
        let mut output = Vec::new();
        for a in [0.0, 0.25, 0.5, 1.0] {
            for b in [0.0, 0.25, 0.5, 1.0] {
                morph_vertices(&morph.base, &morph.positions, &[], 2, &[a, b], &mut output);
                let p = xgame_math::glam::Vec3::from(output[0].position);
                assert!(p.cmpge(bounds.min).all() && p.cmple(bounds.max).all());
            }
        }
    }

    #[test]
    fn independent_instances_keep_their_own_weights_and_vertices() {
        let morph = simple(vec![vertex([0.0, 0.0, 0.0])]);
        let mut first = MorphState::default();
        let mut second = MorphState::default();
        assert!(first.prepare(&morph, &[0.0]));
        assert!(second.prepare(&morph, &[1.0]));
        assert_eq!(first.scratch[0].position[1], 0.0);
        assert_eq!(second.scratch[0].position[1], 2.0);
        // 交错更新后第一个实例仍按自己的缓存跳过，不读取第二个实例的权重。
        assert!(!second.prepare(&morph, &[1.0]));
        assert!(!first.prepare(&morph, &[0.0]));
        assert_eq!(first.scratch[0].position[1], 0.0);
    }

    #[test]
    fn zero_weight_keeps_base_vertices_untouched() {
        let base = vec![vertex([0.0, 0.0, 0.0]), vertex([1.0, 0.0, 0.0])];
        let morph = simple(base.clone());
        let mut out = Vec::new();
        morph_vertices(
            &base,
            &morph.positions,
            &morph.normals,
            morph.target_count(),
            &[0.0],
            &mut out,
        );
        // 逐位相等：权重 0 必须是"什么都没发生"，不是"差不多没发生"。
        assert_eq!(out[0].position, base[0].position);
        assert_eq!(out[1].position, base[1].position);
    }

    #[test]
    fn full_weight_reaches_the_target_exactly() {
        let base = vec![vertex([0.0, 0.0, 0.0])];
        let morph = simple(base.clone());
        let mut out = Vec::new();
        morph_vertices(
            &base,
            &morph.positions,
            &morph.normals,
            morph.target_count(),
            &[1.0],
            &mut out,
        );
        assert_eq!(out[0].position, [0.0, 2.0, 0.0]);
    }

    #[test]
    fn half_weight_lands_halfway() {
        let base = vec![vertex([0.0, 0.0, 0.0])];
        let morph = simple(base.clone());
        let mut out = Vec::new();
        morph_vertices(
            &base,
            &morph.positions,
            &morph.normals,
            morph.target_count(),
            &[0.5],
            &mut out,
        );
        assert!((out[0].position[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn targets_accumulate_additively() {
        let base = vec![vertex([0.0, 0.0, 0.0])];
        let morph = MorphTargets::new(
            vec!["A".into(), "B".into()],
            vec![0.0, 0.0],
            // A 抬起 +2，B 再抬起 +4 → 两个都 1.0 时是 +6
            vec![[0.0, 2.0, 0.0], [0.0, 4.0, 0.0]],
            Vec::new(),
            base.clone(),
        )
        .expect("形状自洽");
        let mut out = Vec::new();
        morph_vertices(
            &base,
            &morph.positions,
            &morph.normals,
            morph.target_count(),
            &[1.0, 1.0],
            &mut out,
        );
        assert_eq!(out[0].position[1], 6.0);
    }

    #[test]
    fn weights_are_clamped_and_nan_is_treated_as_zero() {
        assert_eq!(sanitize_weight(1.5), 1.0);
        assert_eq!(sanitize_weight(-0.3), 0.0);
        assert_eq!(sanitize_weight(f32::NAN), 0.0);
        assert_eq!(sanitize_weight(f32::INFINITY), 0.0);
    }

    #[test]
    fn normals_are_renormalised_after_accumulation() {
        let base = vec![vertex([0.0, 0.0, 0.0])];
        let morph = MorphTargets::new(
            vec!["Tilt".into()],
            vec![0.0],
            vec![[0.0, 0.0, 0.0]],
            // 基准法线 (0,1,0) + 增量 (1,0,0) → 斜 45°，归一化后长度必须是 1
            vec![[1.0, 0.0, 0.0]],
            base.clone(),
        )
        .expect("形状自洽");
        let mut out = Vec::new();
        morph_vertices(
            &base,
            &morph.positions,
            &morph.normals,
            morph.target_count(),
            &[1.0],
            &mut out,
        );
        let [x, y, z] = out[0].normal;
        assert!((x * x + y * y + z * z - 1.0).abs() < 1e-6);
        assert!((x - y).abs() < 1e-6, "45° 的两个分量应相等");
    }

    #[test]
    fn target_count_beyond_weights_is_ignored() {
        let base = vec![vertex([0.0, 0.0, 0.0])];
        let morph = simple(base.clone());
        let mut out = Vec::new();
        // 权重表比目标多：多的部分不该越界读增量。
        morph_vertices(
            &base,
            &morph.positions,
            &morph.normals,
            morph.target_count(),
            &[1.0, 3.0, 4.0],
            &mut out,
        );
        assert_eq!(out[0].position, [0.0, 2.0, 0.0]);
    }

    #[test]
    fn resolve_pads_with_defaults_and_drops_extra() {
        let base = vec![vertex([0.0, 0.0, 0.0])];
        let morph = MorphTargets::new(
            vec!["A".into(), "B".into()],
            vec![0.1, 0.2],
            vec![[0.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            Vec::new(),
            base,
        )
        .expect("形状自洽");
        let mut out = Vec::new();

        morph.resolve_into(&[], &mut out);
        assert_eq!(out, vec![0.1, 0.2], "空 = 用资产默认");

        morph.resolve_into(&[0.7], &mut out);
        assert_eq!(out, vec![0.7, 0.2], "短了用默认补齐");

        morph.resolve_into(&[0.7, 0.8, 0.9], &mut out);
        assert_eq!(out, vec![0.7, 0.8], "长了丢弃多余");
    }

    #[test]
    fn mismatched_delta_count_is_rejected() {
        let base = vec![vertex([0.0, 0.0, 0.0]), vertex([1.0, 0.0, 0.0])];
        // 两个顶点、一个目标 ⇒ 需要 2 段增量，只给 1 段必须被拒。
        let built = MorphTargets::new(
            vec!["Broken".into()],
            vec![0.0],
            vec![[0.0, 1.0, 0.0]],
            Vec::new(),
            base,
        );
        assert!(built.is_none(), "错位增量会把顶点挪到不存在的位置");
    }
}
