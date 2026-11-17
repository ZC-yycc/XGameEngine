//! 最小 GLB 解析：取第一个 mesh/primitive 的几何、蒙皮与 PBR 基色因子。
//!
//! 只支持 `.glb` 的 JSON + BIN chunk、单个 buffer、triangles；贴图与多 primitive
//! 合并仍由后续切片接入。

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use serde::Deserialize;

use self::material::MaterialDef;
use super::mesh::{Mesh, SkinData};

mod material;

const GLB_MAGIC: u32 = 0x4654_6C67;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;
const MODE_TRIANGLES: u32 = 4;

#[derive(Debug)]
pub enum MeshError {
    Io(String),
    Glb(String),
    Json(String),
    Unsupported(String),
    Missing(String),
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(f, "读取网格失败：{message}"),
            Self::Glb(message) => write!(f, "GLB 解析失败：{message}"),
            Self::Json(message) => write!(f, "glTF JSON 解析失败：{message}"),
            Self::Unsupported(message) => write!(f, "不支持的网格：{message}"),
            Self::Missing(message) => write!(f, "网格缺少数据：{message}"),
        }
    }
}

impl std::error::Error for MeshError {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Root {
    #[serde(default)]
    meshes: Vec<MeshDef>,
    #[serde(default)]
    accessors: Vec<Accessor>,
    #[serde(default)]
    buffer_views: Vec<BufferView>,
    #[serde(default)]
    buffers: Vec<BufferDef>,
    #[serde(default)]
    nodes: Vec<NodeDef>,
    #[serde(default)]
    skins: Vec<SkinDef>,
    #[serde(default)]
    materials: Vec<MaterialDef>,
}

#[derive(Debug, Deserialize)]
struct BufferDef {
    #[serde(default)]
    uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NodeDef {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    children: Vec<usize>,
    #[serde(default)]
    mesh: Option<usize>,
    #[serde(default)]
    skin: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkinDef {
    #[serde(default)]
    joints: Vec<usize>,
    inverse_bind_matrices: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct MeshDef {
    #[serde(default)]
    primitives: Vec<Primitive>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Primitive {
    #[serde(default)]
    attributes: HashMap<String, usize>,
    indices: Option<usize>,
    material: Option<usize>,
    #[serde(default = "default_mode")]
    mode: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Accessor {
    buffer_view: Option<usize>,
    #[serde(default)]
    byte_offset: usize,
    component_type: u32,
    count: usize,
    #[serde(rename = "type")]
    ty: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BufferView {
    buffer: usize,
    #[serde(default)]
    byte_offset: usize,
    byte_length: usize,
    byte_stride: Option<usize>,
}

fn default_mode() -> u32 {
    MODE_TRIANGLES
}

pub fn load_glb(path: &Path) -> Result<Mesh, MeshError> {
    let bytes = std::fs::read(path).map_err(|err| MeshError::Io(err.to_string()))?;
    load_glb_bytes(&bytes)
}

/// `.gltf`：JSON + 外部 `.bin`，先拼成内存 GLB 再走同一条解析。
pub fn load_gltf(path: &Path) -> Result<Mesh, MeshError> {
    let text = std::fs::read_to_string(path).map_err(|err| MeshError::Io(err.to_string()))?;
    let root: Root = serde_json::from_str(&text).map_err(|err| MeshError::Json(err.to_string()))?;
    let uri = root
        .buffers
        .first()
        .and_then(|buffer| buffer.uri.as_deref())
        .ok_or_else(|| MeshError::Missing("buffers[0].uri".into()))?;
    if uri.starts_with("data:") {
        return Err(MeshError::Unsupported("data URI buffer".into()));
    }
    let uri = uri.strip_prefix("file://").unwrap_or(uri);
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let bin = std::fs::read(base.join(uri)).map_err(|err| MeshError::Io(err.to_string()))?;
    let glb = encode_glb(&text, &bin);
    load_glb_bytes(&glb)
}

/// 按扩展名分派 `.glb` / `.gltf`。
pub fn load_model(path: &Path) -> Result<Mesh, MeshError> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "gltf" {
        load_gltf(path)
    } else {
        load_glb(path)
    }
}

fn encode_glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut json_bytes = json.as_bytes().to_vec();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    glb.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    glb.extend_from_slice(&json_bytes);
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(&CHUNK_BIN.to_le_bytes());
    glb.extend_from_slice(bin);
    glb
}

fn load_glb_bytes(bytes: &[u8]) -> Result<Mesh, MeshError> {
    let (json, bin) = split_glb(bytes)?;
    let root: Root =
        serde_json::from_slice(json).map_err(|err| MeshError::Json(err.to_string()))?;
    let mesh = root
        .meshes
        .first()
        .ok_or(MeshError::Missing("meshes".into()))?;
    let primitive = mesh
        .primitives
        .first()
        .ok_or(MeshError::Missing("primitives".into()))?;
    if primitive.mode != MODE_TRIANGLES {
        return Err(MeshError::Unsupported(format!(
            "只支持 triangles，实际 mode {}",
            primitive.mode
        )));
    }
    let position_accessor = primitive
        .attributes
        .get("POSITION")
        .copied()
        .ok_or(MeshError::Missing("POSITION".into()))?;
    let positions = read_positions(&root, position_accessor, bin)?;
    let indices = match primitive.indices {
        Some(index) => read_indices(&root, index, bin)?,
        None => Vec::new(),
    };
    let mut mesh = Mesh::new(positions, indices);
    if let Some(material) = primitive.material {
        mesh.base_color_factor = root
            .materials
            .get(material)
            .ok_or_else(|| MeshError::Missing(format!("material {material}")))?
            .pbr_metallic_roughness
            .base_color_factor;
    }
    if let (Some(joints), Some(weights)) = (
        primitive.attributes.get("JOINTS_0").copied(),
        primitive.attributes.get("WEIGHTS_0").copied(),
    ) {
        let joints = read_joints(&root, joints, bin)?;
        let weights = read_weights(&root, weights, bin)?;
        if joints.len() != mesh.positions.len() || weights.len() != mesh.positions.len() {
            return Err(MeshError::Glb(
                "JOINTS_0 / WEIGHTS_0 数量与 POSITION 不一致".into(),
            ));
        }
        mesh.skin = Some(SkinData {
            joints,
            weights,
            joint_nodes: Vec::new(),
            inverse_bind: Vec::new(),
        });
        if let Some(skin_index) = root
            .nodes
            .iter()
            .find(|node| node.mesh == Some(0))
            .and_then(|node| node.skin)
        {
            if let Some(skin) = root.skins.get(skin_index) {
                if let Some(mut data) = mesh.skin.take() {
                    data.joint_nodes = skin.joints.clone();
                    if let Some(accessor) = skin.inverse_bind_matrices {
                        data.inverse_bind = read_mat4(&root, accessor, bin)?;
                    }
                    mesh.skin = Some(data);
                }
            }
        }
    }
    Ok(mesh)
}

fn split_glb(bytes: &[u8]) -> Result<(&[u8], &[u8]), MeshError> {
    if bytes.len() < 12 {
        return Err(MeshError::Glb("header 太短".into()));
    }
    let magic = read_u32(bytes, 0)?;
    let version = read_u32(bytes, 4)?;
    let length = read_u32(bytes, 8)? as usize;
    if magic != GLB_MAGIC {
        return Err(MeshError::Glb("magic 不是 glTF".into()));
    }
    if version != 2 {
        return Err(MeshError::Unsupported(format!("GLB 版本 {version}")));
    }
    if length > bytes.len() {
        return Err(MeshError::Glb("声明长度超过文件".into()));
    }

    let mut json = None;
    let mut bin = None;
    let mut offset = 12usize;
    while offset + 8 <= length {
        let chunk_len = read_u32(bytes, offset)? as usize;
        let chunk_type = read_u32(bytes, offset + 4)?;
        let start = offset + 8;
        let end = start
            .checked_add(chunk_len)
            .filter(|end| *end <= length)
            .ok_or_else(|| MeshError::Glb("chunk 越界".into()))?;
        match chunk_type {
            CHUNK_JSON => json = Some(&bytes[start..end]),
            CHUNK_BIN => bin = Some(&bytes[start..end]),
            _ => {}
        }
        offset = end.div_ceil(4) * 4;
    }
    let json = json.ok_or(MeshError::Missing("JSON chunk".into()))?;
    let bin = bin.ok_or(MeshError::Missing("BIN chunk".into()))?;
    Ok((json, bin))
}

fn read_positions(root: &Root, accessor: usize, bin: &[u8]) -> Result<Vec<[f32; 3]>, MeshError> {
    let accessor = root
        .accessors
        .get(accessor)
        .ok_or(MeshError::Missing("POSITION accessor".into()))?;
    if accessor.component_type != 5126 || accessor.ty != "VEC3" {
        return Err(MeshError::Unsupported(format!(
            "POSITION 需要 float VEC3，实际 type {} component {}",
            accessor.ty, accessor.component_type
        )));
    }
    let (base, stride) = accessor_layout(root, accessor, 12)?;
    let needed = base
        .checked_add(stride.saturating_mul(accessor.count.saturating_sub(1)))
        .and_then(|offset| offset.checked_add(12))
        .ok_or_else(|| MeshError::Glb("POSITION 偏移溢出".into()))?;
    if needed > bin.len() {
        return Err(MeshError::Glb("POSITION 超出 BIN chunk".into()));
    }
    let mut positions = Vec::with_capacity(accessor.count);
    for index in 0..accessor.count {
        let offset = base + index * stride;
        positions.push([
            read_f32(bin, offset)?,
            read_f32(bin, offset + 4)?,
            read_f32(bin, offset + 8)?,
        ]);
    }
    Ok(positions)
}

fn read_indices(root: &Root, accessor: usize, bin: &[u8]) -> Result<Vec<u32>, MeshError> {
    let accessor = root
        .accessors
        .get(accessor)
        .ok_or(MeshError::Missing("indices accessor".into()))?;
    let size = match accessor.component_type {
        5121 => 1,
        5123 => 2,
        5125 => 4,
        other => {
            return Err(MeshError::Unsupported(format!(
                "索引 componentType {other}"
            )));
        }
    };
    let (base, stride) = accessor_layout(root, accessor, size)?;
    let needed = base
        .checked_add(stride.saturating_mul(accessor.count.saturating_sub(1)))
        .and_then(|offset| offset.checked_add(size))
        .ok_or_else(|| MeshError::Glb("索引偏移溢出".into()))?;
    if needed > bin.len() {
        return Err(MeshError::Glb("索引超出 BIN chunk".into()));
    }
    let mut indices = Vec::with_capacity(accessor.count);
    for index in 0..accessor.count {
        let offset = base + index * stride;
        indices.push(match size {
            1 => u32::from(bin[offset]),
            2 => u32::from(read_u16(bin, offset)?),
            4 => read_u32(bin, offset)?,
            _ => unreachable!(),
        });
    }
    Ok(indices)
}

fn accessor_layout(
    root: &Root,
    accessor: &Accessor,
    element_size: usize,
) -> Result<(usize, usize), MeshError> {
    let view_index = accessor
        .buffer_view
        .ok_or_else(|| MeshError::Unsupported("sparse accessor".into()))?;
    let view = root
        .buffer_views
        .get(view_index)
        .ok_or(MeshError::Missing("bufferView".into()))?;
    if view.buffer != 0 {
        return Err(MeshError::Unsupported(format!(
            "多 buffer，buffer {}",
            view.buffer
        )));
    }
    if accessor.byte_offset >= view.byte_length {
        return Err(MeshError::Glb("accessor 超出 bufferView".into()));
    }
    let base = view.byte_offset + accessor.byte_offset;
    let stride = view.byte_stride.unwrap_or(element_size);
    if stride < element_size {
        return Err(MeshError::Glb("byteStride 小于单元素".into()));
    }
    Ok((base, stride))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, MeshError> {
    let chunk = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| MeshError::Glb("读取 u16 越界".into()))?;
    Ok(u16::from_le_bytes([chunk[0], chunk[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, MeshError> {
    let chunk = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| MeshError::Glb("读取 u32 越界".into()))?;
    Ok(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
}

fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, MeshError> {
    Ok(f32::from_bits(read_u32(bytes, offset)?))
}

fn read_joints(root: &Root, accessor: usize, bin: &[u8]) -> Result<Vec<[u16; 4]>, MeshError> {
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

fn read_weights(root: &Root, accessor: usize, bin: &[u8]) -> Result<Vec<[f32; 4]>, MeshError> {
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

fn read_mat4(root: &Root, accessor: usize, bin: &[u8]) -> Result<Vec<[f32; 16]>, MeshError> {
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

#[derive(Clone, Debug, PartialEq)]
pub struct SkeletonJoint {
    pub name: String,
    pub parent: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkeletonGraph {
    pub joints: Vec<SkeletonJoint>,
}

/// 读取 GLB 第一个 skin 的关节层级；名字缺失时回退 `joint_<node>`。
pub fn load_glb_skeleton(path: &Path) -> Result<SkeletonGraph, MeshError> {
    let bytes = std::fs::read(path).map_err(|err| MeshError::Io(err.to_string()))?;
    let (json, _bin) = split_glb(&bytes)?;
    let root: Root =
        serde_json::from_slice(json).map_err(|err| MeshError::Json(err.to_string()))?;
    let skin = root
        .skins
        .first()
        .ok_or(MeshError::Missing("skins".into()))?;
    if skin.joints.is_empty() {
        return Err(MeshError::Missing("skin.joints".into()));
    }
    let mut parents = vec![None; root.nodes.len()];
    for (index, node) in root.nodes.iter().enumerate() {
        for child in &node.children {
            if let Some(slot) = parents.get_mut(*child) {
                *slot = Some(index);
            }
        }
    }
    let position: HashMap<usize, usize> = skin
        .joints
        .iter()
        .enumerate()
        .map(|(slot, node)| (*node, slot))
        .collect();
    let joints = skin
        .joints
        .iter()
        .map(|node| {
            let name = root
                .nodes
                .get(*node)
                .and_then(|node| node.name.clone())
                .unwrap_or_else(|| format!("joint_{node}"));
            let parent = parents
                .get(*node)
                .and_then(|parent| *parent)
                .and_then(|parent| position.get(&parent).copied());
            SkeletonJoint { name, parent }
        })
        .collect();
    Ok(SkeletonGraph { joints })
}
