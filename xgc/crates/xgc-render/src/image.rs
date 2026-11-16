//! 极简图片读取：只支持二进制 PPM（P6），用于 UI 图片调试。

#[derive(Clone, Debug, PartialEq)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<[u8; 4]>,
}

impl ImageData {
    pub fn load_ppm(path: &std::path::Path) -> Result<Self, String> {
        let data = std::fs::read(path)
            .map_err(|err| format!("读取图片 {} 失败：{err}", path.display()))?;
        parse_ppm(&data)
    }

    /// 按扩展名读取 `.png` / `.ppm`。
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        match path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => load_png(path),
            _ => Self::load_ppm(path),
        }
    }

    /// 把图片按 `cols × rows` 取平均色；用于无纹理管线时的块状预览。
    pub fn cells(&self, cols: u32, rows: u32) -> Vec<[f32; 4]> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut colors = Vec::with_capacity((cols * rows) as usize);
        for row in 0..rows {
            for col in 0..cols {
                let x0 = self.width * col / cols;
                let x1 = (self.width * (col + 1) / cols).max(x0 + 1).min(self.width);
                let y0 = self.height * row / rows;
                let y1 = (self.height * (row + 1) / rows)
                    .max(y0 + 1)
                    .min(self.height);
                let mut sum = [0u32; 4];
                let mut count = 0u32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let pixel = self.rgba[(y * self.width + x) as usize];
                        for channel in 0..4 {
                            sum[channel] += u32::from(pixel[channel]);
                        }
                        count += 1;
                    }
                }
                let count = count.max(1);
                colors.push([
                    sum[0] as f32 / count as f32 / 255.0,
                    sum[1] as f32 / count as f32 / 255.0,
                    sum[2] as f32 / count as f32 / 255.0,
                    sum[3] as f32 / count as f32 / 255.0,
                ]);
            }
        }
        colors
    }
}

fn parse_ppm(data: &[u8]) -> Result<ImageData, String> {
    let mut pos = 0;
    if token(data, &mut pos) != Some(b"P6") {
        return Err("只支持二进制 PPM（P6）".to_string());
    }
    let width = parse_u32(token(data, &mut pos).ok_or("PPM 缺少宽度")?)?;
    let height = parse_u32(token(data, &mut pos).ok_or("PPM 缺少高度")?)?;
    let max = parse_u32(token(data, &mut pos).ok_or("PPM 缺少最大值")?)?;
    if max != 255 {
        return Err(format!("PPM 最大值只支持 255，得到 {max}"));
    }
    while pos < data.len() && data[pos].is_ascii_whitespace() {
        pos += 1;
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or("PPM 尺寸溢出")?;
    let pixels = data
        .get(pos..pos + expected)
        .ok_or_else(|| format!("PPM 像素数据不足：需要 {expected} 字节"))?;
    let mut rgba = Vec::with_capacity((width * height) as usize);
    for chunk in pixels.chunks_exact(3) {
        rgba.push([chunk[0], chunk[1], chunk[2], 255]);
    }
    Ok(ImageData {
        width,
        height,
        rgba,
    })
}

fn token<'a>(data: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    loop {
        while *pos < data.len() && data[*pos].is_ascii_whitespace() {
            *pos += 1;
        }
        if *pos < data.len() && data[*pos] == b'#' {
            while *pos < data.len() && data[*pos] != b'\n' {
                *pos += 1;
            }
            continue;
        }
        break;
    }
    let start = *pos;
    while *pos < data.len() && !data[*pos].is_ascii_whitespace() {
        *pos += 1;
    }
    (*pos > start).then_some(&data[start..*pos])
}

fn parse_u32(token: &[u8]) -> Result<u32, String> {
    std::str::from_utf8(token)
        .map_err(|_| "PPM 头不是 ASCII".to_string())?
        .parse()
        .map_err(|err| format!("PPM 头解析失败：{err}"))
}

fn load_png(path: &std::path::Path) -> Result<ImageData, String> {
    let file = std::fs::File::open(path)
        .map_err(|err| format!("读取 PNG {} 失败：{err}", path.display()))?;
    let decoder = png::Decoder::new(file);
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
    let mut rgba = Vec::with_capacity((info.width * info.height) as usize);
    match info.color_type {
        png::ColorType::Rgba => {
            for pixel in buffer[..info.buffer_size()].chunks_exact(4) {
                rgba.push([pixel[0], pixel[1], pixel[2], pixel[3]]);
            }
        }
        png::ColorType::Rgb => {
            for pixel in buffer[..info.buffer_size()].chunks_exact(3) {
                rgba.push([pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for value in &buffer[..info.buffer_size()] {
                rgba.push([*value, *value, *value, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in buffer[..info.buffer_size()].chunks_exact(2) {
                rgba.push([pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        other => return Err(format!("PNG 不支持的颜色类型：{other:?}")),
    }
    Ok(ImageData {
        width: info.width,
        height: info.height,
        rgba,
    })
}
