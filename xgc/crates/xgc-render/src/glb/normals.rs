//! glTF `NORMAL`：每顶点法线（模型空间）。
//!
//! 只接受 float VEC3，与 `TEXCOORD_0` 的读法一致；不归一化 —— 归一化留到 GPU
//! （蒙皮 / 缩放后再归一才准，提前归一会被非均匀缩放带偏）。

use super::{accessor_layout, read_f32, MeshError, Root};

pub(super) fn read_normals(
    root: &Root,
    accessor: usize,
    bin: &[u8],
) -> Result<Vec<[f32; 3]>, MeshError> {
    let accessor = root
        .accessors
        .get(accessor)
        .ok_or_else(|| MeshError::Missing("NORMAL accessor".into()))?;
    if accessor.component_type != 5126 || accessor.ty != "VEC3" {
        return Err(MeshError::Unsupported(format!(
            "NORMAL 需要 float VEC3，实际 type {} component {}",
            accessor.ty, accessor.component_type
        )));
    }
    let (base, stride) = accessor_layout(root, accessor, 12)?;
    // 与 WEIGHTS_0 / inverseBindMatrices 同一套越界守卫：`accessor.count` 来自不可信
    // 的 JSON，直接拿去乘 stride 可能在 32 位机器上溢出、`Vec::with_capacity` 还可能
    // 直接 abort。先算出最后一条法线的末尾，超出 BIN chunk 就报错。
    let needed = base
        .checked_add(stride.saturating_mul(accessor.count.saturating_sub(1)))
        .and_then(|offset| offset.checked_add(12))
        .ok_or_else(|| MeshError::Glb("NORMAL 偏移溢出".into()))?;
    if needed > bin.len() {
        return Err(MeshError::Glb("NORMAL 超出 BIN chunk".into()));
    }
    let mut normals = Vec::with_capacity(accessor.count);
    for index in 0..accessor.count {
        let offset = base + index * stride;
        normals.push([
            read_f32(bin, offset)?,
            read_f32(bin, offset + 4)?,
            read_f32(bin, offset + 8)?,
        ]);
    }
    Ok(normals)
}
