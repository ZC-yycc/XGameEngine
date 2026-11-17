//! 真字体文本：`ab_glyph` 光栅化 + shelf 字形图集 + 逐字形四边形。
//!
//! 旧端 UI 文本走 `xgame-ui` 的 `ab_glyph` + 系统字体（微软雅黑等候选）；这里
//! 用**同一份字体候选与同一个栅格化库**，字形形状 / 抗锯齿才能和旧图对上。
//! 排版只做短文本：换行、对齐、等宽推进，不做复杂文字整形（与旧端同样的已知限制）。
//!
//! 图集是 RGBA8：rgb 恒为 255、alpha 存覆盖率，绘制时乘文本颜色即可。

use std::collections::HashMap;
use std::path::Path;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};

/// 中文字体候选，与旧端 `xgame-ui::text` 同一套（找不到时不崩，只是不画字）。
const FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\Deng.ttf",
    r"C:\Windows\Fonts\simhei.ttf",
    r"C:\Windows\Fonts\simsun.ttc",
    "/System/Library/Fonts/PingFang.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];

/// 粗体候选（缺省时退回常规字重，不报错 —— 与旧端同一条约定）。
const BOLD_FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyhbd.ttc",
    r"C:\Windows\Fonts\Dengb.ttf",
    r"C:\Windows\Fonts\simhei.ttf",
    "/System/Library/Fonts/PingFang.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
];

const ATLAS_MAX: u32 = 2048;
const ATLAS_MIN: u32 = 1024;
const GLYPH_PAD: u32 = 1;

/// 一个字形在屏幕上的落位（像素坐标，左上原点）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphQuad {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// 图集 uv：`[u0, v0, u1, v1]`
    pub uv: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct GlyphSlot {
    uv: [f32; 4],
    width: f32,
    height: f32,
    left: f32,
    top: f32,
    advance: f32,
}

/// 字体 + 字形图集。
pub struct FontAtlas {
    font: FontVec,
    bold: Option<FontVec>,
    size: u32,
    pixels: Vec<u8>,
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
    cache: HashMap<(char, u32, bool), GlyphSlot>,
    version: u64,
}

impl FontAtlas {
    /// 按旧端同一套候选加载系统字体；都找不到时返回 `None`（调用方跳过文本）。
    pub fn load() -> Option<Self> {
        let font = load_face(FONT_CANDIDATES)?;
        let bold = load_face(BOLD_FONT_CANDIDATES);
        Some(Self::with_font(font, bold))
    }

    fn with_font(font: FontVec, bold: Option<FontVec>) -> Self {
        let size = ATLAS_MIN;
        Self {
            font,
            bold,
            size,
            pixels: vec![255; (size * size * 4) as usize],
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
            cache: HashMap::new(),
            version: 1,
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// 内容版本号：变了就要重新上传 GPU 贴图。
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 行高（像素）：与旧端同为 ascent - descent + line_gap。
    pub fn line_height(&self, size_px: f32) -> f32 {
        let scaled = self.face(false).as_scaled(PxScale::from(size_px));
        (scaled.ascent() - scaled.descent() + scaled.line_gap()).max(1.0)
    }

    /// 把 UTF-8 文本排进 `origin` 为左上角、`rect_size` 为内容框的区域。
    ///
    /// `align` / `valign` 取值与 `ui.element` 的字符串一致（left/center/right、
    /// top/middle/bottom）。`wrap_width = None` 表示不换行。
    #[allow(clippy::too_many_arguments)]
    pub fn layout(
        &mut self,
        text: &str,
        size_px: f32,
        origin: [f32; 2],
        rect_size: [f32; 2],
        wrap_width: Option<f32>,
        align: &str,
        valign: &str,
        bold: bool,
    ) -> Vec<GlyphQuad> {
        let mut lines: Vec<Vec<char>> = Vec::new();
        for hard in text.split('\n') {
            if let Some(width) = wrap_width {
                lines.extend(wrap_line(hard, width, &|ch| {
                    self.advance(ch, size_px, bold)
                }));
            } else {
                lines.push(hard.chars().collect());
            }
        }
        if lines.is_empty() {
            return Vec::new();
        }
        let line_height = self.line_height(size_px);
        let ascent = self.face(bold).as_scaled(PxScale::from(size_px)).ascent();
        let total_height = line_height * lines.len() as f32;
        let block_top = match valign {
            "bottom" => rect_size[1] - total_height,
            "top" => 0.0,
            _ => (rect_size[1] - total_height) * 0.5,
        }
        .max(0.0);

        let mut quads = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            let line_width: f32 = line
                .iter()
                .map(|ch| self.advance(*ch, size_px, bold))
                .sum::<f32>();
            let mut pen_x = match align {
                "center" => (rect_size[0] - line_width) * 0.5,
                "right" => rect_size[0] - line_width,
                _ => 0.0,
            }
            .max(0.0);
            let baseline = origin[1] + block_top + row as f32 * line_height + ascent;
            let mut previous: Option<ab_glyph::GlyphId> = None;
            for ch in line {
                if let Some(prev) = previous {
                    // 负字距也要应用（旧端直接用字体 kerning），否则拉丁字母偏宽。
                    pen_x += self.kern(prev, self.face(bold).glyph_id(*ch), size_px, bold);
                }
                let slot = match self.glyph(*ch, size_px, bold) {
                    Some(slot) => slot,
                    None => {
                        pen_x += self.advance(*ch, size_px, bold);
                        previous = None;
                        continue;
                    }
                };
                if slot.width > 0.0 && slot.height > 0.0 {
                    // 字形位图是 1:1 光栅化的：落点吸到整数像素，线性采样才不会
                    // 把每个字形糊成两份（旧端同样是像素对齐的文本）。
                    quads.push(GlyphQuad {
                        x: (origin[0] + pen_x + slot.left).round(),
                        y: (baseline + slot.top).round(),
                        width: slot.width,
                        height: slot.height,
                        uv: slot.uv,
                    });
                }
                pen_x += slot.advance;
                previous = Some(self.face(bold).glyph_id(*ch));
            }
        }
        quads
    }

    /// 粗体字面是否可用（不可用时静默退回常规字重，与旧端一致）。
    pub fn has_bold(&self) -> bool {
        self.bold.is_some()
    }

    fn face(&self, bold: bool) -> &FontVec {
        match (bold, self.bold.as_ref()) {
            (true, Some(face)) => face,
            _ => &self.font,
        }
    }

    fn kern(
        &self,
        previous: ab_glyph::GlyphId,
        next: ab_glyph::GlyphId,
        size_px: f32,
        bold: bool,
    ) -> f32 {
        use ab_glyph::ScaleFont;
        self.face(bold)
            .as_scaled(PxScale::from(size_px))
            .kern(previous, next)
    }

    fn advance(&self, ch: char, size_px: f32, bold: bool) -> f32 {
        let id = self.face(bold).glyph_id(ch);
        let scaled = self.face(bold).as_scaled(PxScale::from(size_px));
        use ab_glyph::ScaleFont;
        scaled.h_advance(id)
    }

    /// 取（必要时光栅化并打包）一个字形。
    fn glyph(&mut self, ch: char, size_px: f32, bold: bool) -> Option<GlyphSlot> {
        if ch.is_whitespace() {
            return Some(GlyphSlot {
                uv: [0.0, 0.0, 0.0, 0.0],
                width: 0.0,
                height: 0.0,
                left: 0.0,
                top: 0.0,
                advance: self.advance(ch, size_px, bold),
            });
        }
        let key = (ch, size_px.round().max(1.0) as u32, bold);
        if let Some(slot) = self.cache.get(&key) {
            return Some(*slot);
        }
        let advance = self.advance(ch, size_px, bold);
        let face = self.face(bold);
        let glyph = face
            .glyph_id(ch)
            .with_scale_and_position(PxScale::from(size_px), ab_glyph::point(0.0, 0.0));
        let outlined = face.outline_glyph(glyph)?;
        let bounds = outlined.px_bounds();
        let width = bounds.width().ceil().max(0.0) as u32;
        let height = bounds.height().ceil().max(0.0) as u32;
        let slot = if width == 0 || height == 0 {
            GlyphSlot {
                uv: [0.0; 4],
                width: 0.0,
                height: 0.0,
                left: bounds.min.x,
                top: bounds.min.y,
                advance,
            }
        } else {
            // 先把覆盖率画进本地缓冲（`outlined` 借用了 self.font），再分配图集。
            let mut coverage = vec![0u8; (width * height) as usize];
            outlined.draw(|px, py, value| {
                if px < width && py < height {
                    let index = (py * width + px) as usize;
                    coverage[index] = coverage[index].max((value * 255.0).round() as u8);
                }
            });
            let (x, y) = self.allocate(width, height);
            for row in 0..height {
                for col in 0..width {
                    let index = ((y + row) * self.size + x + col) as usize * 4;
                    self.pixels[index] = 255;
                    self.pixels[index + 1] = 255;
                    self.pixels[index + 2] = 255;
                    self.pixels[index + 3] = coverage[(row * width + col) as usize];
                }
            }
            let size = self.size as f32;
            GlyphSlot {
                uv: [
                    x as f32 / size,
                    y as f32 / size,
                    (x + width) as f32 / size,
                    (y + height) as f32 / size,
                ],
                width: width as f32,
                height: height as f32,
                left: bounds.min.x,
                top: bounds.min.y,
                advance,
            }
        };
        self.cache.insert(key, slot);
        self.version += 1;
        Some(slot)
    }

    /// shelf 打包：返回字形在图集里的左上角。
    fn allocate(&mut self, width: u32, height: u32) -> (u32, u32) {
        let width = width + GLYPH_PAD;
        let height = height + GLYPH_PAD;
        if self.cursor_x + width > self.size {
            self.cursor_x = 0;
            self.cursor_y += self.row_height;
            self.row_height = 0;
        }
        if self.cursor_y + height > self.size {
            // 溢出：整体重建（字号种类有限，重建一次后稳定），必要时图集翻倍。
            let new_size = if self.size >= ATLAS_MAX {
                ATLAS_MAX
            } else {
                (self.size * 2).min(ATLAS_MAX)
            };
            self.size = new_size;
            self.pixels = vec![255; (new_size * new_size * 4) as usize];
            self.cursor_x = 0;
            self.cursor_y = 0;
            self.row_height = 0;
            self.cache.clear();
            self.version += 1;
        }
        let x = self.cursor_x;
        let y = self.cursor_y;
        self.cursor_x += width;
        self.row_height = self.row_height.max(height);
        (x, y)
    }
}

fn load_face(candidates: &[&str]) -> Option<FontVec> {
    for path in candidates {
        let path = Path::new(path);
        if !path.is_file() {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        // `.ttc` 是字体集合，取第 0 面（微软雅黑 / 苹方都是集合）。
        if let Ok(font) = FontVec::try_from_vec_and_index(bytes, 0) {
            return Some(font);
        }
    }
    None
}

/// 贪心换行：CJK 按字断行，ASCII 尽量在空格处断（不追求旧端的全部细节）。
fn wrap_line(text: &str, width: f32, advance: &impl Fn(char) -> f32) -> Vec<Vec<char>> {
    let mut lines = Vec::new();
    let mut current: Vec<char> = Vec::new();
    let mut current_width = 0.0f32;
    let mut last_space: Option<usize> = None;
    for ch in text.chars() {
        let step = advance(ch);
        if current_width + step > width && !current.is_empty() {
            if let Some(space) = last_space {
                let mut rest: Vec<char> = current.split_off(space + 1);
                lines.push(current);
                current = std::mem::take(&mut rest);
                current_width = current.iter().map(|ch| advance(*ch)).sum();
            } else {
                lines.push(std::mem::take(&mut current));
                current_width = 0.0;
            }
            last_space = None;
        }
        if ch == ' ' {
            last_space = Some(current.len());
        }
        current.push(ch);
        current_width += step;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas() -> Option<FontAtlas> {
        FontAtlas::load()
    }

    #[test]
    fn layout_advances_without_overlap() {
        let Some(mut atlas) = atlas() else {
            eprintln!("skip: 没有系统字体");
            return;
        };
        let quads = atlas.layout(
            "ABC",
            20.0,
            [0.0, 0.0],
            [200.0, 40.0],
            None,
            "left",
            "middle",
            false,
        );
        assert_eq!(quads.len(), 3);
        assert!(quads[1].x > quads[0].x + quads[0].width - 1.0);
        assert!(quads[2].x > quads[1].x);
    }

    #[test]
    fn center_alignment_centers_the_line() {
        let Some(mut atlas) = atlas() else {
            return;
        };
        let left = atlas.layout(
            "Hi",
            20.0,
            [0.0, 0.0],
            [200.0, 40.0],
            None,
            "left",
            "top",
            false,
        );
        let center = atlas.layout(
            "Hi",
            20.0,
            [0.0, 0.0],
            [200.0, 40.0],
            None,
            "center",
            "top",
            false,
        );
        let center_end = center.last().unwrap().x + center.last().unwrap().width;
        assert!(center[0].x - left[0].x > 20.0, "居中对齐应把整行右移");
        assert!(center_end < 200.0, "居中后不能超出内容框");
        assert!((left[0].y - center[0].y).abs() < 0.01, "对齐不应改变基线");
    }

    #[test]
    fn cjk_glyphs_rasterize_into_the_atlas() {
        let Some(mut atlas) = atlas() else {
            return;
        };
        let quads = atlas.layout(
            "点击",
            15.0,
            [0.0, 0.0],
            [80.0, 30.0],
            None,
            "left",
            "top",
            false,
        );
        assert_eq!(quads.len(), 2);
        assert!(quads[0].width > 5.0 && quads[0].height > 5.0);
        // 图集里确实有非全白像素（覆盖率写进了 alpha）。
        assert!(atlas
            .pixels()
            .chunks_exact(4)
            .any(|pixel| pixel[3] > 0 && pixel[3] < 255));
    }

    #[test]
    fn bold_face_uses_the_bold_font_when_available() {
        let Some(mut atlas) = atlas() else {
            return;
        };
        if !atlas.has_bold() {
            eprintln!("skip: 没有系统粗体字面");
            return;
        }
        let regular = atlas.layout(
            "Bold",
            20.0,
            [0.0, 0.0],
            [200.0, 40.0],
            None,
            "left",
            "top",
            false,
        );
        let bold = atlas.layout(
            "Bold",
            20.0,
            [0.0, 0.0],
            [200.0, 40.0],
            None,
            "left",
            "top",
            true,
        );
        assert_eq!(regular.len(), bold.len());
        // 粗体字面通常更宽（advance 更大）或至少不同位置，不能与常规完全一致。
        assert!(
            regular != bold,
            "粗体字面必须与常规字面产生不同的字形四边形"
        );
    }

    #[test]
    fn wrap_splits_long_lines() {
        let Some(mut atlas) = atlas() else {
            return;
        };
        let quads = atlas.layout(
            "ABCABCABCABC",
            20.0,
            [0.0, 0.0],
            [300.0, 80.0],
            Some(40.0),
            "left",
            "top",
            false,
        );
        let rows: std::collections::BTreeSet<u32> = quads.iter().map(|q| q.y as u32).collect();
        assert!(rows.len() > 1, "换行后应出现多行");
    }
}
