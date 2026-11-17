//! 基色贴图数据：glTF 的 `baseColorTexture` 解成 RGBA8，供 GPU 采样。

use std::sync::Arc;

/// 8-bit sRGB 的 RGBA8 贴图（每像素 4 字节，按行优先）。
#[derive(Clone, Debug, PartialEq)]
pub struct TextureData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl TextureData {
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        debug_assert_eq!(rgba.len(), (width * height * 4) as usize);
        Self {
            width,
            height,
            rgba,
        }
    }

    /// 1×1 白纹理：没有贴图的模型也走同一条采样路径，值恒为 1。
    pub fn white() -> Arc<Self> {
        Arc::new(Self::from_rgba(1, 1, vec![255; 4]))
    }

    /// 从 PNG 字节解码（glTF 的 `image.uri` / GLB 内嵌 image 都走这里）。
    ///
    /// 与 `image.rs` 相同，只认 8-bit；gRPC 素材（Fox 的 Texture.png）就是 8-bit RGB。
    pub fn from_png(bytes: &[u8]) -> Result<Self, String> {
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder
            .read_info()
            .map_err(|err| format!("PNG 头解析失败：{err}"))?;
        let mut buffer = vec![0; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut buffer)
            .map_err(|err| format!("PNG 解码失败：{err}"))?;
        if info.bit_depth != png::BitDepth::Eight {
            return Err(format!("PNG 只支持 8-bit，得到 {:?}", info.bit_depth));
        }
        let pixels = &buffer[..info.buffer_size()];
        let mut rgba = Vec::with_capacity((info.width * info.height * 4) as usize);
        match info.color_type {
            png::ColorType::Rgba => rgba.extend_from_slice(pixels),
            png::ColorType::Rgb => {
                for pixel in pixels.chunks_exact(3) {
                    rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
                }
            }
            png::ColorType::Grayscale => {
                for value in pixels {
                    rgba.extend_from_slice(&[*value, *value, *value, 255]);
                }
            }
            other => return Err(format!("PNG 颜色类型不支持：{other:?}")),
        }
        Ok(Self::from_rgba(info.width, info.height, rgba))
    }
}
