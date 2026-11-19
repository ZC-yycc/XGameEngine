//! glTF 顶点属性：`NORMAL` 与蒙皮属性（`JOINTS_0` / `WEIGHTS_0` + 逆绑定矩阵）。
//!
//! 从 `glb.rs` 拆出：`glb.rs` 已贴着 600 行软上限，而"怎么把属性挂到 `Mesh` 上"
//! 与"怎么从 GLB 字节里读出几何"是两件事，分开以后两边都还能长。
//!
//! 缺 `NORMAL` 不算错：片元着色会退回屏幕导数叉积。但**有** `NORMAL` 就必须条数
//! 与 `POSITION` 一致——少一条会让顶点缓冲错位，比没有法线难查得多。

use super::normals::read_normals;
use super::{accessor_layout, read_f32, read_u16, MeshError, Primitive, Root};
use crate::camera::IDENTITY;
use crate::mesh::{Mesh, SkinData};

/// 把本图元的法线与蒙皮属性挂到 `mesh` 上。
pub(super) fn attach(
    root: &Root,
    primitive: &Primitive,
    bin: &[u8],
    mesh: &mut Mesh,
) -> Result<(), MeshError> {
    if let Some(accessor) = primitive.attributes.get("NORMAL").copied() {
        mesh.normals = read_normals(root, accessor, bin)?;
        if mesh.normals.len() != mesh.positions.len() {
            return Err(MeshError::Glb("NORMAL 数量与 POSITION 不一致".into()));
        }
    }
    if let (Some(joints), Some(weights)) = (
        primitive.attributes.get("JOINTS_0").copied(),
        primitive.attributes.get("WEIGHTS_0").copied(),
    ) {
        let joints = read_joints(root, joints, bin)?;
        let weights = read_weights(root, weights, bin)?;
        if joints.len() != mesh.positions.len() || weights.len() != mesh.positions.len() {
            return Err(MeshError::Glb(
                "JOINTS_0 / WEIGHTS_0 数量与 POSITION 不一致".into(),
            ));
        }
        let mut skin = SkinData {
            joints,
            weights,
            joint_nodes: Vec::new(),
            inverse_bind: Vec::new(),
        };
        // 逆绑定矩阵挂在 skin 上（不是 primitive 上）：同一个 skin 可以被多个网格共用。
        if let Some(skin_index) = root
            .nodes
            .iter()
            .find(|node| node.mesh == Some(0))
            .and_then(|node| node.skin)
        {
            if let Some(def) = root.skins.get(skin_index) {
                skin.joint_nodes = def.joints.clone();
                skin.inverse_bind = match def.inverse_bind_matrices {
                    Some(accessor) => read_mat4(root, accessor, bin)?,
                    // glTF 2.0：未提供 inverseBindMatrices 时每个关节都按单位矩阵。
                    None => vec![IDENTITY; skin.joint_nodes.len()],
                };
            }
        }
        mesh.skin = Some(skin);
    }
    Ok(())
}

pub(super) fn read_joints(
    root: &Root,
    accessor: usize,
    bin: &[u8],
) -> Result<Vec<[u16; 4]>, MeshError> {
    let accessor = root
        .accessors
        .get(accessor)
        .ok_or(MeshError::Missing("JOINTS_0 accessor".into()))?;
    let size = match accessor.component_type {
        5121 => 1,
        5123 => 2,
        other => {
            return Err(MeshError::Unsupported(format!(
                "JOINTS_0 componentType {other}"
            )));
        }
    };
    if accessor.ty != "VEC4" {
        return Err(MeshError::Unsupported(format!(
            "JOINTS_0 需要 VEC4，实际 {}",
            accessor.ty
        )));
    }
    let (base, stride) = accessor_layout(root, accessor, size * 4)?;
    let needed = base
        .checked_add(stride.saturating_mul(accessor.count.saturating_sub(1)))
        .and_then(|offset| offset.checked_add(size * 4))
        .ok_or_else(|| MeshError::Glb("JOINTS_0 偏移溢出".into()))?;
    if needed > bin.len() {
        return Err(MeshError::Glb("JOINTS_0 超出 BIN chunk".into()));
    }
    let mut joints = Vec::with_capacity(accessor.count);
    for index in 0..accessor.count {
        let offset = base + index * stride;
        let mut value = [0u16; 4];
        for (slot, joint) in value.iter_mut().enumerate() {
            let at = offset + slot * size;
            *joint = match size {
                1 => u16::from(bin[at]),
                2 => read_u16(bin, at)?,
                _ => unreachable!(),
            };
        }
        joints.push(value);
    }
    Ok(joints)
}

pub(super) fn read_weights(
    root: &Root,
    accessor: usize,
    bin: &[u8],
) -> Result<Vec<[f32; 4]>, MeshError> {
    let accessor = root
        .accessors
        .get(accessor)
        .ok_or(MeshError::Missing("WEIGHTS_0 accessor".into()))?;
    if accessor.component_type != 5126 || accessor.ty != "VEC4" {
        return Err(MeshError::Unsupported(format!(
            "WEIGHTS_0 需要 float VEC4，实际 type {} component {}",
            accessor.ty, accessor.component_type
        )));
    }
    let (base, stride) = accessor_layout(root, accessor, 16)?;
    let needed = base
        .checked_add(stride.saturating_mul(accessor.count.saturating_sub(1)))
        .and_then(|offset| offset.checked_add(16))
        .ok_or_else(|| MeshError::Glb("WEIGHTS_0 偏移溢出".into()))?;
    if needed > bin.len() {
        return Err(MeshError::Glb("WEIGHTS_0 超出 BIN chunk".into()));
    }
    let mut weights = Vec::with_capacity(accessor.count);
    for index in 0..accessor.count {
        let offset = base + index * stride;
        weights.push([
            read_f32(bin, offset)?,
            read_f32(bin, offset + 4)?,
            read_f32(bin, offset + 8)?,
            read_f32(bin, offset + 12)?,
        ]);
    }
    Ok(weights)
}

pub(super) fn read_mat4(
    root: &Root,
    accessor: usize,
    bin: &[u8],
) -> Result<Vec<[f32; 16]>, MeshError> {
    let accessor = root
        .accessors
        .get(accessor)
        .ok_or(MeshError::Missing("inverseBindMatrices accessor".into()))?;
    if accessor.component_type != 5126 || accessor.ty != "MAT4" {
        return Err(MeshError::Unsupported(format!(
            "inverseBindMatrices 需要 float MAT4，实际 type {} component {}",
            accessor.ty, accessor.component_type
        )));
    }
    let (base, stride) = accessor_layout(root, accessor, 64)?;
    let needed = base
        .checked_add(stride.saturating_mul(accessor.count.saturating_sub(1)))
        .and_then(|offset| offset.checked_add(64))
        .ok_or_else(|| MeshError::Glb("inverseBindMatrices 偏移溢出".into()))?;
    if needed > bin.len() {
        return Err(MeshError::Glb("inverseBindMatrices 超出 BIN chunk".into()));
    }
    let mut matrices = Vec::with_capacity(accessor.count);
    for index in 0..accessor.count {
        let offset = base + index * stride;
        let mut matrix = [0.0f32; 16];
        for (slot, value) in matrix.iter_mut().enumerate() {
            *value = read_f32(bin, offset + slot * 4)?;
        }
        matrices.push(matrix);
    }
    Ok(matrices)
}
