//! glTF 贴图：`TEXCOORD_0` 与 `baseColorTexture` 的解析。
//!
//! 只认第一图元的基色贴图；外部 `uri` 相对模型目录解析，GLB 内嵌 image 从
//! `bufferView` 取字节。这两个来源之外（data URI / 多 buffer）直接报不支持，
//! 不做静默降级 —— 贴图静默丢失比报错难查。

use std::path::Path;

use super::{accessor_layout, read_f32, MeshError, Primitive, Root};

/// glTF `textures[i]` 的最小投影（本模块专用，`Root` 也用它）。
#[derive(Debug, serde::Deserialize)]
pub(super) struct TextureDef {
    #[serde(default)]
    pub(super) source: Option<usize>,
}

/// glTF `images[i]` 的最小投影：外部 uri 或 GLB 内嵌 bufferView。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImageDef {
    #[serde(default)]
    pub(super) uri: Option<String>,
    #[serde(default)]
    pub(super) buffer_view: Option<usize>,
}
use crate::texture_data::TextureData;

/// 解析 `TEXCOORD_0`；没有该属性返回空（着色器用 (0,0) 采样）。
pub(super) fn read_uvs(
    root: &Root,
    accessor: usize,
    bin: &[u8],
) -> Result<Vec<[f32; 2]>, MeshError> {
    let accessor = root
        .accessors
        .get(accessor)
        .ok_or_else(|| MeshError::Missing("TEXCOORD_0 accessor".into()))?;
    if accessor.component_type != 5126 || accessor.ty != "VEC2" {
        return Err(MeshError::Unsupported(format!(
            "TEXCOORD_0 需要 float VEC2，实际 type {} component {}",
            accessor.ty, accessor.component_type
        )));
    }
    let (base, stride) = accessor_layout(root, accessor, 8)?;
    let mut uvs = Vec::with_capacity(accessor.count);
    for index in 0..accessor.count {
        let offset = base + index * stride;
        uvs.push([read_f32(bin, offset)?, read_f32(bin, offset + 4)?]);
    }
    Ok(uvs)
}

/// 第一图元的基色贴图；没有材质 / 没有贴图返回 `None`（纯色）。
pub(super) fn base_color(
    root: &Root,
    primitive: &Primitive,
    bin: &[u8],
    base_dir: Option<&Path>,
) -> Result<Option<std::sync::Arc<TextureData>>, MeshError> {
    let Some(material) = primitive.material else {
        return Ok(None);
    };
    let material = root
        .materials
        .get(material)
        .ok_or_else(|| MeshError::Missing(format!("material {material}")))?;
    let Some(texture) = material.pbr_metallic_roughness.base_color_texture.as_ref() else {
        return Ok(None);
    };
    let texture = root
        .textures
        .get(texture.index)
        .ok_or_else(|| MeshError::Missing(format!("textures[{}]", texture.index)))?;
    let source = texture
        .source
        .ok_or_else(|| MeshError::Missing("texture.source".into()))?;
    let image = root
        .images
        .get(source)
        .ok_or_else(|| MeshError::Missing(format!("images[{source}]")))?;
    let bytes = read_image(root, image, bin, base_dir)?;
    let data = TextureData::from_png(&bytes).map_err(MeshError::Unsupported)?;
    Ok(Some(std::sync::Arc::new(data)))
}

fn read_image(
    root: &Root,
    image: &ImageDef,
    bin: &[u8],
    base_dir: Option<&Path>,
) -> Result<Vec<u8>, MeshError> {
    if let Some(uri) = image.uri.as_deref() {
        if uri.starts_with("data:") {
            return Err(MeshError::Unsupported("data URI 贴图".into()));
        }
        let base_dir = base_dir
            .ok_or_else(|| MeshError::Unsupported("模型没有文件目录，无法解析贴图 uri".into()))?;
        let uri = uri.strip_prefix("file://").unwrap_or(uri);
        return std::fs::read(base_dir.join(uri)).map_err(|err| MeshError::Io(err.to_string()));
    }
    if let Some(view) = image.buffer_view {
        let view = root
            .buffer_views
            .get(view)
            .ok_or_else(|| MeshError::Missing("image.bufferView".into()))?;
        let end = view
            .byte_offset
            .checked_add(view.byte_length)
            .ok_or_else(|| MeshError::Glb("image.bufferView 偏移溢出".into()))?;
        let bytes = bin
            .get(view.byte_offset..end)
            .ok_or_else(|| MeshError::Glb("image 超出 BIN chunk".into()))?;
        return Ok(bytes.to_vec());
    }
    Err(MeshError::Missing("image.uri / image.bufferView".into()))
}
