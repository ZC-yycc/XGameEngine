//! 文本排版与字形图集（CPU 侧）。
//!
//! # 为什么自己排版而不引 cosmic-text
//!
//! UI 文案是**短文本 + 极简排版**：换行、对齐、等宽推进，没有双向文本、
//! 没有复杂连字、不需要字体回退链。cosmic-text / swash 会带来一整套字形缓存
//! 与 shaping 引擎（编译时间 + 二进制体积 + 运行时状态），而这里要的只是
//! "把一串字符摆到像素上"。
//!
//! 用 `ab_glyph`（epaint 用的也是它，已在依赖树里）做两件事：
//! 取度量（advance / ascent / descent / kerning）与把轮廓光栅化成覆盖率。
//! 代价是**不做复杂文字整形** —— 阿拉伯文 / 天城文的连写不会正确成形，
//! 中文与拉丁文完全正常（已知限制写进 `Docs/差距分析.md`）。
//!
//! # 图集
//!
//! 单张 R8 图集（初始 1024²，按需翻倍到 2048²），shelf 打包（逐行填充）。
//! R8 而不是 RGBA：字形只需要覆盖率，内存是 1/4、上传带宽也是 1/4。
//! 图集里每个字形留 1px 透明边框，避免线性采样在相邻字形之间渗色。
//!
//! 溢出时的策略是**整体重建**：清空缓存 + 清空图集。UI 字号种类有限，
//! 重建一次之后就稳定了；比做 LRU 淘汰简单一个数量级，也不会出现
//! "某个字形被淘汰后同一帧又立刻请求"的抖动。

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont};
use xgame_core::{Color, UiAlign, UiTextStyle, UiVAlign};
use xgame_math::glam::Vec2;

use crate::draw::{SpriteMode, UiSprite, UiTexture};
use crate::layout::UiRect;

// ---------------------------------------------------------------------------
// 字体
// ---------------------------------------------------------------------------

/// 一个已加载的字面（regular / bold）。
struct Face {
    font: FontVec,
    /// 来源路径（日志用）。
    path: String,
}

/// 中文字体候选，按优先级排列。
///
/// 与编辑器（`xgame-editor::panels::theme::CJK_FONT_CANDIDATES`）同一套候选：
/// 游戏画面与编辑器界面用同一份字体，中文才不会"编辑器里正常、游戏里方块"。
/// 不内嵌字体文件：中文字体动辄 10–20 MB，且授权条款各不相同；系统字体
/// 是 Windows / macOS 上最稳的来源。找不到时**不崩**，只是不画字（告警一次）。
const CJK_FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc",   // 微软雅黑
    r"C:\Windows\Fonts\Deng.ttf",   // 等线
    r"C:\Windows\Fonts\simhei.ttf", // 黑体
    r"C:\Windows\Fonts\simsun.ttc", // 宋体
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];

/// 粗体候选（缺省时退回常规字重，不报错）。
const CJK_BOLD_FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyhbd.ttc",
    r"C:\Windows\Fonts\Dengb.ttf",
    r"C:\Windows\Fonts\simhei.ttf",
    "/System/Library/Fonts/PingFang.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
];

fn load_face(candidates: &[&str]) -> Option<Face> {
    for path in candidates {
        let path_ref = Path::new(path);
        if !path_ref.is_file() {
            continue;
        }
        let Ok(bytes) = std::fs::read(path_ref) else {
            continue;
        };
        // `.ttc` 是字体集合，取第 0 面（微软雅黑 / 苹方都是集合）。
        // `try_from_vec_and_index` 对单体 `.ttf` 同样成立（index 0）。
        match FontVec::try_from_vec_and_index(bytes, 0) {
            Ok(font) => {
                return Some(Face {
                    font,
                    path: (*path).to_owned(),
                })
            }
            Err(_) => continue,
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 字形图集
// ---------------------------------------------------------------------------

/// 单张 R8 字形图集（shelf 打包）。
pub struct GlyphAtlas {
    size: u32,
    pixels: Vec<u8>,
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
    version: u64,
}

/// 图集最大边长。2048² = 4 MB，足够放几千个不同字号的中文字形。
const ATLAS_MAX: u32 = 2048;
/// 图集初始边长。
const ATLAS_MIN: u32 = 1024;
/// 字形之间的透明间隔（像素）。
const GLYPH_PAD: u32 = 1;

impl Default for GlyphAtlas {
    fn default() -> Self {
        Self::new()
    }
}

impl GlyphAtlas {
    /// 空图集。
    pub fn new() -> Self {
        Self {
            size: ATLAS_MIN,
            pixels: vec![0; (ATLAS_MIN * ATLAS_MIN) as usize],
            cursor_x: 0,
            cursor_y: 0,
            row_height: 0,
            version: 0,
        }
    }

    /// 边长（像素，正方形）。
    pub fn size(&self) -> u32 {
        self.size
    }

    /// R8 像素数据（长度 = size²）。
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// 内容版本号：变了就说明有新字形写入，渲染器需要重新上传纹理。
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 写入一个像素的覆盖率。
    fn set(&mut self, x: u32, y: u32, value: u8) {
        if x >= self.size || y >= self.size {
            return;
        }
        let index = (y * self.size + x) as usize;
        if let Some(slot) = self.pixels.get_mut(index) {
            *slot = value;
        }
    }

    /// 从 (0,0) 重新开始打包（保留尺寸，清空内容）。
    fn reset(&mut self) {
        self.pixels.fill(0);
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.row_height = 0;
        self.version += 1;
    }

    /// 申请一个 `w × h` 的槽位，返回左上角。
    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w == 0 || h == 0 || w > self.size || h > self.size {
            return None;
        }
        loop {
            // 换行
            if self.cursor_x + w > self.size {
                self.cursor_x = 0;
                self.cursor_y += self.row_height + GLYPH_PAD;
                self.row_height = 0;
            }
            // 到底了：先尝试翻倍，翻不动就报告失败（调用方会清空重建）。
            if self.cursor_y + h > self.size {
                if self.size >= ATLAS_MAX {
                    return None;
                }
                self.grow()?;
                continue;
            }
            let pos = (self.cursor_x, self.cursor_y);
            self.cursor_x += w + GLYPH_PAD;
            self.row_height = self.row_height.max(h);
            return Some(pos);
        }
    }

    /// 边长翻倍（旧内容留在左上角；UV 用 texel 坐标存，不受影响）。
    fn grow(&mut self) -> Option<()> {
        let new_size = (self.size * 2).min(ATLAS_MAX);
        if new_size <= self.size {
            return None;
        }
        let mut pixels = vec![0u8; (new_size * new_size) as usize];
        for y in 0..self.size {
            let src = (y * self.size) as usize;
            let dst = (y * new_size) as usize;
            pixels[dst..dst + self.size as usize]
                .copy_from_slice(&self.pixels[src..src + self.size as usize]);
        }
        self.pixels = pixels;
        self.size = new_size;
        self.version += 1;
        Some(())
    }
}

// ---------------------------------------------------------------------------
// 文本引擎
// ---------------------------------------------------------------------------

/// 字形缓存键。
///
/// 字号量化到 1/4 像素：`20.0` 与 `20.000001` 不该各自光栅化一遍，
/// 但 0.25px 的差别对人眼已经无意义。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    face: u8,
    size_q: u32,
    ch: char,
}

/// 一个已光栅化字形在图集里的位置（texel 坐标）。
#[derive(Clone, Copy)]
struct GlyphSprite {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    /// 相对"笔位置（基线原点）"的左上角偏移。
    offset: Vec2,
}

/// 一段文本的排版缓存键里与内容无关的部分。
///
/// 对齐方式（`align` / `valign`）**不进键**：它们只在最后把行摆进内容矩形时
/// 用一次，同一段文字换对齐方式复用同一份折行结果。内容矩形的**宽度**进键
/// （换行结果由它决定），位置不进。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct LayoutShape {
    face: u8,
    size_q: u32,
    /// 换行宽度（`None` = 不换行）。f32 不能 `Hash`，用位模式。
    wrap_width_bits: Option<u32>,
}

/// 一条排版缓存记录。
struct LayoutEntry {
    key: LayoutKey,
    layout: CachedLayout,
}

/// 缓存键（内容原文 + 排版形状）。
struct LayoutKey {
    content: String,
    shape: LayoutShape,
}

impl LayoutKey {
    /// 命中判定：指纹只用来定位桶，**内容必须逐字节相等**才算命中
    /// （64 位哈希撞车时宁可重排一次，也不能把两段文字画成同一段）。
    fn matches(&self, content: &str, shape: LayoutShape) -> bool {
        self.shape == shape && self.content == content
    }
}

/// 排版结果：一段文本在给定字号 / 换行宽度下的**全部**形状信息。
///
/// 不含位置与颜色 —— 那些属于元素，逐帧组装即可。字段全部是排版阶段的
/// 中间量（字符、推进量、字距、行区间与行宽），命中缓存后逐帧只剩
/// “查字形 + 摆位置”。
struct CachedLayout {
    line_height: f32,
    ascent: f32,
    chars: Vec<char>,
    /// 每个字符的推进量（不含字距）。
    advances: Vec<f32>,
    /// 与前一个字符的字距；行首不使用（与旧实现 `prev = None` 一致）。
    kerns: Vec<f32>,
    lines: Vec<Line>,
    /// 淘汰用的逻辑时钟（每次命中刷新）。
    last_used: u64,
}

/// 排版缓存容量上限。
///
/// 每条记录 = 一段文字的字符与行区间（几十到几百字节）。1024 条足以装下
/// 一屏 HUD + 常见动态文本；超出后按最近使用淘汰一半，避免逐帧变化的
/// 数字把内存拖成无界增长。
const LAYOUT_CACHE_CAPACITY: usize = 1024;

/// 文本引擎：字体 + 字形缓存 + 图集。
pub struct TextEngine {
    faces: Vec<Face>,
    glyphs: HashMap<GlyphKey, GlyphSprite>,
    atlas: GlyphAtlas,
    /// 排版缓存：指纹 → 记录。
    layouts: HashMap<u64, LayoutEntry>,
    /// 逻辑时钟：只用于淘汰，不表示真实时间。
    layout_tick: u64,
    layout_cache_enabled: bool,
    layout_hits: u64,
    layout_misses: u64,
    warned: bool,
}

/// 排版结果的一行（`chars` 的下标区间）。
#[derive(Clone, Copy)]
struct Line {
    start: usize,
    end: usize,
    width: f32,
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEngine {
    /// 加载系统字体。找不到中文字体时仍可工作（只是不画字）。
    pub fn new() -> Self {
        let mut faces = Vec::new();
        let mut paths = Vec::new();
        if let Some(face) = load_face(CJK_FONT_CANDIDATES) {
            paths.push(face.path.clone());
            faces.push(face);
        }
        if let Some(face) = load_face(CJK_BOLD_FONT_CANDIDATES) {
            paths.push(face.path.clone());
            faces.push(face);
        }
        if paths.is_empty() {
            log::warn!("UI 文本：没有找到可用的系统字体，界面文字将不会绘制");
        } else {
            log::info!("UI 文本字体: {}", paths.join(", "));
        }
        Self {
            faces,
            glyphs: HashMap::new(),
            atlas: GlyphAtlas::new(),
            layouts: HashMap::new(),
            layout_tick: 0,
            layout_cache_enabled: true,
            layout_hits: 0,
            layout_misses: 0,
            warned: false,
        }
    }

    /// 有没有可用字体（无字体时布局与交互照常工作，只是不画字）。
    pub fn has_font(&self) -> bool {
        !self.faces.is_empty()
    }

    /// 字形图集。
    pub fn atlas(&self) -> &GlyphAtlas {
        &self.atlas
    }

    /// 当前缓存了多少个字形（测试 / 诊断用）。
    pub fn cached_glyphs(&self) -> usize {
        self.glyphs.len()
    }

    /// 排版缓存统计：`(命中, 未命中, 条目数)`。
    ///
    /// 命中/未命中是**累计**的（跨帧累加），条目数是当前值。A/B 基准与
    /// 回归测试靠它证明缓存真的在工作，而不是"看起来没变慢"。
    pub fn layout_cache_stats(&self) -> (u64, u64, usize) {
        (self.layout_hits, self.layout_misses, self.layouts.len())
    }

    /// 开关排版缓存（基准 A/B 用；关掉时同时清空已有条目）。
    pub fn set_layout_cache_enabled(&mut self, enabled: bool) {
        self.layout_cache_enabled = enabled;
        if !enabled {
            self.layouts.clear();
        }
    }

    /// 排版缓存是否开着。
    pub fn layout_cache_enabled(&self) -> bool {
        self.layout_cache_enabled
    }

    /// 清空排版缓存（图集重建与内容热重载之外的诊断入口）。
    pub fn clear_layout_cache(&mut self) {
        self.layouts.clear();
    }

    /// 把一段文本排进 `out`，返回新增的精灵数量。
    ///
    /// `content` 是**内容矩形**（已经扣掉内边距与边框），文本在其中按
    /// `align` / `valign` 摆放；`alpha` 是整体透明度乘子（禁用态用）。
    ///
    /// 折行与逐字推进量按「内容 + 字号 + 换行宽度」缓存：HUD 文案绝大多数
    /// 帧不变，第二帧起只剩查字形与摆位置。对齐方式不进键（见 [`LayoutShape`]）。
    pub fn emit(
        &mut self,
        text: &UiTextStyle,
        content: UiRect,
        alpha: f32,
        out: &mut Vec<UiSprite>,
    ) -> usize {
        if text.content.is_empty() || content.is_empty() {
            return 0;
        }

        // 字段逐个借出：缓存 / 字体 / 字形 / 图集各自可变或只读，
        // 拆开之后 `emit_layout` 能同时拿到它们。
        let TextEngine {
            faces,
            glyphs,
            atlas,
            layouts,
            layout_tick,
            layout_cache_enabled,
            layout_hits,
            layout_misses,
            warned,
        } = self;

        if faces.is_empty() {
            if !*warned {
                *warned = true;
                log::warn!("UI 文本：缺少字体，文本不会显示");
            }
            return 0;
        }

        let face_index = if text.bold && faces.len() > 1 {
            1usize
        } else {
            0
        };
        let face = face_index as u8;
        let px = (text.size * 4.0).round().max(4.0) / 4.0;
        let size_q = (px * 4.0).round() as u32;
        let wrap_width_bits = text.wrap.then(|| content.width().to_bits());
        let shape = LayoutShape {
            face,
            size_q,
            wrap_width_bits,
        };
        let fingerprint = layout_fingerprint(&text.content, shape);
        *layout_tick = (*layout_tick).wrapping_add(1);
        let tick = *layout_tick;
        let font = &faces[face_index].font;

        // -- 1. 命中：直接出精灵 --
        if *layout_cache_enabled {
            if let Some(entry) = layouts.get_mut(&fingerprint) {
                if entry.key.matches(&text.content, shape) {
                    entry.layout.last_used = tick;
                    *layout_hits += 1;
                    // 重新取不可变借用：`emit_layout` 还要可变借 glyphs / atlas。
                    let entry = layouts.get(&fingerprint).expect("上一步刚查到");
                    return emit_layout(
                        &entry.layout,
                        text,
                        content,
                        alpha,
                        out,
                        font,
                        face,
                        size_q,
                        glyphs,
                        atlas,
                    );
                }
            }
        }

        // -- 2. 未命中：重排一次，再出精灵 --
        *layout_misses += 1;
        let max_width = if text.wrap {
            content.width()
        } else {
            f32::INFINITY
        };
        let layout = build_layout(font, &text.content, px, max_width, tick);
        let count = emit_layout(
            &layout,
            text,
            content,
            alpha,
            out,
            font,
            face,
            size_q,
            glyphs,
            atlas,
        );

        if *layout_cache_enabled {
            // 指纹碰撞（另一段文字占着同一指纹）时这次就不缓存：
            // 宁可多排一次，也不能覆盖别人的记录。
            let free = match layouts.get(&fingerprint) {
                Some(existing) => existing.key.matches(&text.content, shape),
                None => true,
            };
            if free {
                layouts.insert(
                    fingerprint,
                    LayoutEntry {
                        key: LayoutKey {
                            content: text.content.clone(),
                            shape,
                        },
                        layout,
                    },
                );
                evict_layouts(layouts);
            }
        }

        count
    }

    /// 一段文本的排版宽度（单行、不换行）—— 测试与自适应宽度用。
    pub fn measure_width(&self, text: &UiTextStyle) -> f32 {
        let Some(face) = self.faces.first() else {
            return 0.0;
        };
        let scaled = face.font.as_scaled(PxScale::from(text.size));
        let mut width = 0.0;
        let mut prev = None;
        for ch in text.content.chars() {
            let id = scaled.glyph_id(ch);
            let mut advance = scaled.h_advance(id);
            if let Some(prev_id) = prev {
                advance += scaled.kern(prev_id, id);
            }
            width += advance;
            prev = Some(id);
        }
        width
    }
}

// ---------------------------------------------------------------------------
// 排版缓存
// ---------------------------------------------------------------------------

/// 内容 + 形状 → 64 位指纹。指纹只负责定位缓存桶，命中还要逐字节比对内容。
fn layout_fingerprint(content: &str, shape: LayoutShape) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    shape.hash(&mut hasher);
    hasher.finish()
}

/// 重排一段文本（缓存未命中时走这里）。
///
/// 与旧实现逐行同构：推进量 / 字距先算一遍（旧实现每帧要算两遍），
/// 再折行、最后统一重算行宽。`max_width` 只影响折行，不影响字宽。
fn build_layout(
    font: &FontVec,
    content: &str,
    px: f32,
    max_width: f32,
    tick: u64,
) -> CachedLayout {
    let scaled = font.as_scaled(PxScale::from(px));
    let ascent = scaled.ascent();
    let descent = scaled.descent();
    let line_gap = scaled.line_gap();
    let line_height = (ascent - descent + line_gap).max(px);

    let chars: Vec<char> = content.chars().collect();
    let mut advances = Vec::with_capacity(chars.len());
    let mut kerns = Vec::with_capacity(chars.len());
    let mut prev: Option<GlyphId> = None;
    for &ch in &chars {
        let id = scaled.glyph_id(ch);
        advances.push(scaled.h_advance(id));
        kerns.push(prev.map_or(0.0, |prev_id| scaled.kern(prev_id, id)));
        prev = Some(id);
    }

    // -- 拆行 --
    let mut lines = Vec::new();
    let mut line_start = 0usize;
    let mut width = 0.0f32;
    let mut last_break: Option<usize> = None;
    for i in 0..chars.len() {
        let ch = chars[i];
        if ch == '\n' {
            lines.push(Line {
                start: line_start,
                end: i,
                width: 0.0,
            });
            line_start = i + 1;
            width = 0.0;
            last_break = None;
            continue;
        }
        let advance = advances[i] + if i > line_start { kerns[i] } else { 0.0 };
        if width + advance > max_width && i > line_start {
            // 优先在最近的断点处折行；找不到断点（超长单词 / 连续数字）
            // 就硬断 —— 溢出到别的元素上比断在单词中间更难看。
            let brk = match last_break {
                Some(b) if b > line_start => b,
                _ => i,
            };
            lines.push(Line {
                start: line_start,
                end: brk,
                width: 0.0,
            });
            line_start = brk;
            width = 0.0;
            last_break = None;
            // 断点之后的字符要重新累加（最多一行，代价可忽略）。
            for k in brk..i {
                width += advances[k] + if k > brk { kerns[k] } else { 0.0 };
                if can_break_after(chars[k]) {
                    last_break = Some(k + 1);
                }
            }
        }
        width += advance;
        if can_break_after(ch) {
            last_break = Some(i + 1);
        }
    }
    lines.push(Line {
        start: line_start,
        end: chars.len(),
        width: 0.0,
    });

    // -- 行宽（换行分支里记的是中间值，这里统一重算）--
    for line in lines.iter_mut() {
        let mut width = 0.0;
        for i in line.start..line.end {
            width += advances[i] + if i > line.start { kerns[i] } else { 0.0 };
        }
        line.width = width;
    }

    CachedLayout {
        line_height,
        ascent,
        chars,
        advances,
        kerns,
        lines,
        last_used: tick,
    }
}

/// 逐行逐字生成四边形（缓存命中与未命中共用）。
#[allow(clippy::too_many_arguments)]
fn emit_layout(
    layout: &CachedLayout,
    text: &UiTextStyle,
    content: UiRect,
    alpha: f32,
    out: &mut Vec<UiSprite>,
    font: &FontVec,
    face: u8,
    size_q: u32,
    glyphs: &mut HashMap<GlyphKey, GlyphSprite>,
    atlas: &mut GlyphAtlas,
) -> usize {
    let px = size_q as f32 / 4.0;

    // -- 垂直起点 --
    let total_height = layout.line_height * layout.lines.len() as f32;
    let mut baseline = match text.valign {
        UiVAlign::Top => content.min.y,
        UiVAlign::Middle => content.center().y - total_height * 0.5,
        UiVAlign::Bottom => content.max.y - total_height,
    } + layout.ascent;

    // -- 逐行逐字生成四边形 --
    let start = out.len();
    let alpha = alpha.clamp(0.0, 1.0);
    let base = Color::from(text.color);
    let color = Color {
        a: base.a * alpha,
        ..base
    };
    let atlas_size = atlas.size() as f32;
    for line in layout.lines.iter() {
        let start_x = match text.align {
            UiAlign::Left => content.min.x,
            UiAlign::Center => content.center().x - line.width * 0.5,
            UiAlign::Right => content.max.x - line.width,
        };
        let mut pen = start_x;
        for i in line.start..line.end {
            let ch = layout.chars[i];
            let advance = layout.advances[i] + if i > line.start { layout.kerns[i] } else { 0.0 };

            if ch == ' ' || ch == '\t' {
                pen += advance;
                continue;
            }
            let Some(sprite) = glyph_sprite(font, face, px, ch, glyphs, atlas) else {
                pen += advance;
                continue;
            };
            // 落在整数像素上：字形按整像素光栅化，小数坐标只会让双线性
            // 采样把边缘糊掉。
            let min = Vec2::new(
                (pen + sprite.offset.x).round(),
                (baseline + sprite.offset.y).round(),
            );
            let max = min + Vec2::new(sprite.w as f32, sprite.h as f32);
            out.push(UiSprite {
                rect: UiRect { min, max },
                uv: [
                    sprite.x as f32 / atlas_size,
                    sprite.y as f32 / atlas_size,
                    (sprite.x + sprite.w) as f32 / atlas_size,
                    (sprite.y + sprite.h) as f32 / atlas_size,
                ],
                color,
                mode: SpriteMode::AlphaMask,
                texture: UiTexture::Atlas,
            });
            pen += advance;
        }
        baseline += layout.line_height;
    }
    out.len() - start
}

/// 缓存满了就按最近使用淘汰一半（见 [`LAYOUT_CACHE_CAPACITY`]）。
fn evict_layouts(layouts: &mut HashMap<u64, LayoutEntry>) {
    if layouts.len() <= LAYOUT_CACHE_CAPACITY {
        return;
    }
    let mut ticks: Vec<u64> = layouts.values().map(|entry| entry.layout.last_used).collect();
    let mid = ticks.len() / 2;
    ticks.select_nth_unstable(mid);
    let cutoff = ticks[mid];
    // `>=`：与中位数同刻度的记录留不留都不影响有界性，这里留新的那一半。
    layouts.retain(|_, entry| entry.layout.last_used >= cutoff);
    // 极端情况（所有记录同一刻度）保留过多：整体丢掉重来。
    if layouts.len() > LAYOUT_CACHE_CAPACITY {
        layouts.clear();
    }
}

// ---------------------------------------------------------------------------
// 字形光栅化（自由函数：字体只读、缓存与图集各自可变）
// ---------------------------------------------------------------------------

/// 取字形（必要时光栅化并写进图集）。
fn glyph_sprite(
    font: &FontVec,
    face: u8,
    px: f32,
    ch: char,
    glyphs: &mut HashMap<GlyphKey, GlyphSprite>,
    atlas: &mut GlyphAtlas,
) -> Option<GlyphSprite> {
    let key = GlyphKey {
        face,
        size_q: (px * 4.0).round() as u32,
        ch,
    };
    if let Some(sprite) = glyphs.get(&key) {
        return Some(*sprite);
    }
    let id = font.glyph_id(ch);
    // 空格 / 控制字符没有轮廓，只有推进量。
    let glyph = ab_glyph::Glyph {
        id,
        scale: PxScale::from(px),
        position: ab_glyph::point(0.0, 0.0),
    };
    let outlined = font.outline_glyph(glyph)?;
    let bounds = outlined.px_bounds();
    let w = bounds.width().ceil() as u32;
    let h = bounds.height().ceil() as u32;
    if w == 0 || h == 0 {
        return None;
    }
    // +2：四周各留 1px 透明边，避免线性采样吃到邻格。
    let (x, y) = alloc_glyph(glyphs, atlas, w + 2, h + 2)?;
    // 覆盖率直接写进 R8：`draw` 每像素回调一次。闭包独占 `atlas`
    // （`outlined` 只借 font），所以这里不需要任何 unsafe。
    outlined.draw(|gx, gy, coverage| {
        let value = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
        atlas.set(x + 1 + gx, y + 1 + gy, value);
    });
    let sprite = GlyphSprite {
        x: x + 1,
        y: y + 1,
        w,
        h,
        offset: Vec2::new(bounds.min.x, bounds.min.y),
    };
    glyphs.insert(key, sprite);
    Some(sprite)
}

/// 申请图集槽位；满了就清空重建一次。
fn alloc_glyph(
    glyphs: &mut HashMap<GlyphKey, GlyphSprite>,
    atlas: &mut GlyphAtlas,
    w: u32,
    h: u32,
) -> Option<(u32, u32)> {
    if let Some(pos) = atlas.alloc(w, h) {
        return Some(pos);
    }
    // 图集到顶 / 单个字形太大：清空重建。清空必须连缓存一起清 ——
    // 否则缓存里的 UV 会指向已经被清零的区域（画出来是空白）。
    glyphs.clear();
    atlas.reset();
    let pos = atlas.alloc(w, h);
    if pos.is_none() {
        log::warn!("UI 文本：字形 {w}×{h} 超出图集上限，已跳过");
    }
    pos
}

/// 这个字符之后允许折行吗？
///
/// 拉丁文按空格断词；中日韩可以在任意字之间断行（没有空格可分）。
/// 这是"够用"的近似：真正的 UAX #14 断行规则有一百多条，对游戏 HUD 没有收益。
fn can_break_after(ch: char) -> bool {
    ch == ' ' || ch == '\t' || is_cjk(ch)
}

/// 是否中日韩字符（含全角标点与假名）。
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x11FF      // 谚文字母
        | 0x2E80..=0x2EFF    // 部首补充
        | 0x3000..=0x303F    // 标点
        | 0x3040..=0x30FF    // 假名
        | 0x3400..=0x4DBF    // 扩展 A
        | 0x4E00..=0x9FFF    // 基本区
        | 0xF900..=0xFAFF    // 兼容表意
        | 0xFF00..=0xFFEF    // 全角
        | 0x20000..=0x2FA1F  // 扩展 B–F
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::UiRect;
    use xgame_math::glam::Vec2;

    fn style(content: &str, size: f32) -> UiTextStyle {
        UiTextStyle {
            content: content.to_owned(),
            size,
            ..UiTextStyle::default()
        }
    }

    fn rect(w: f32, h: f32) -> UiRect {
        UiRect::from_min_size(Vec2::ZERO, Vec2::new(w, h))
    }

    /// 没有字体时不能 panic —— 这是"无字体环境"的兜底路径。
    #[test]
    fn missing_font_is_graceful() {
        let mut engine = TextEngine::new();
        if engine.has_font() {
            return; // 本机有字体，跳过这条兜底用例
        }
        let mut out = Vec::new();
        let n = engine.emit(&style("hello", 16.0), rect(200.0, 40.0), 1.0, &mut out);
        assert_eq!(n, 0);
        assert!(out.is_empty());
        assert_eq!(engine.measure_width(&style("hello", 16.0)), 0.0);
    }

    /// 有字体时：每个可见字符一个精灵，空格不产生精灵。
    #[test]
    fn visible_chars_produce_sprites() {
        let mut engine = TextEngine::new();
        if !engine.has_font() {
            eprintln!("跳过：本机没有可用字体");
            return;
        }
        let mut out = Vec::new();
        let n = engine.emit(&style("a b", 20.0), rect(400.0, 40.0), 1.0, &mut out);
        assert_eq!(n, 2, "空格不该产生精灵");
        assert!(out.iter().all(|s| s.texture == UiTexture::Atlas));
        assert!(out
            .iter()
            .all(|s| s.rect.width() > 0.0 && s.rect.height() > 0.0));
        // 相同内容第二次应该命中缓存，不改变图集版本。
        let version = engine.atlas().version();
        let mut out2 = Vec::new();
        engine.emit(&style("a b", 20.0), rect(400.0, 40.0), 1.0, &mut out2);
        assert_eq!(engine.atlas().version(), version, "字形应命中缓存");
    }

    /// 折行：窄矩形把同一段文本折成多行（纵向跨度明显变大）。
    #[test]
    fn wrapping_splits_lines() {
        let mut engine = TextEngine::new();
        if !engine.has_font() {
            return;
        }
        let text = style("hello world hello world", 20.0);
        let mut wide = Vec::new();
        engine.emit(&text, rect(2000.0, 200.0), 1.0, &mut wide);
        let mut narrow = Vec::new();
        engine.emit(&text, rect(120.0, 400.0), 1.0, &mut narrow);
        let span = |sprites: &[UiSprite]| {
            let min = sprites
                .iter()
                .map(|s| s.rect.min.y)
                .fold(f32::MAX, f32::min);
            let max = sprites
                .iter()
                .map(|s| s.rect.max.y)
                .fold(f32::MIN, f32::max);
            max - min
        };
        assert!(span(&narrow) > span(&wide) * 1.8, "窄宽度应当折成多行");
    }

    /// 对齐：右对齐的右边界贴着内容矩形右边。
    #[test]
    fn right_alignment_pins_to_right_edge() {
        let mut engine = TextEngine::new();
        if !engine.has_font() {
            return;
        }
        let mut out = Vec::new();
        let mut text = style("Hi", 24.0);
        text.align = UiAlign::Right;
        engine.emit(&text, rect(300.0, 60.0), 1.0, &mut out);
        let max = out.iter().map(|s| s.rect.max.x).fold(f32::MIN, f32::max);
        assert!(max <= 300.0 + 1.0, "右对齐不该越过内容矩形: {max}");
        assert!(max > 280.0, "右对齐应当贴住右边: {max}");
    }

    /// 透明度乘子会作用到每个精灵（禁用态整块变淡）。
    #[test]
    fn alpha_multiplier_applies_to_glyphs() {
        let mut engine = TextEngine::new();
        if !engine.has_font() {
            return;
        }
        let mut out = Vec::new();
        engine.emit(&style("A", 20.0), rect(100.0, 40.0), 0.5, &mut out);
        assert!(out.iter().all(|s| (s.color.a - 0.5).abs() < 1e-6));
    }

    #[test]
    fn cjk_is_breakable_but_latin_is_not() {
        assert!(can_break_after('中'));
        assert!(can_break_after('，'));
        assert!(can_break_after(' '));
        assert!(!can_break_after('a'));
        assert!(!can_break_after('.'));
    }

    /// 图集翻倍后旧内容保留在左上角（UV 是 texel 坐标，不受尺寸变化影响）。
    #[test]
    fn atlas_growth_preserves_existing_texels() {
        let mut atlas = GlyphAtlas::new();
        let (x, y) = atlas.alloc(8, 8).expect("首次分配");
        assert_eq!((x, y), (0, 0));
        for gy in 0..8 {
            for gx in 0..8 {
                atlas.set(gx, gy, (gx + gy) as u8);
            }
        }
        let old = atlas.size();
        // 两块 1000×900 必然把 1024² 撑爆 → 翻倍到 2048²。
        assert!(atlas.alloc(1000, 900).is_some());
        assert!(atlas.alloc(1000, 900).is_some());
        assert_eq!(atlas.size(), old * 2, "应当翻倍一次");

        let size = atlas.size();
        assert_eq!(atlas.pixels()[0], 0, "旧内容 (0,0) 应保留");
        assert_eq!(atlas.pixels()[1], 1, "旧内容 (1,0) 应保留");
        assert_eq!(
            atlas.pixels()[(7 * size + 7) as usize],
            14,
            "旧内容 (7,7) 在**新步长**下仍应保留"
        );
    }

    /// 同样的内容第二帧必须命中；对齐方式不进键，换对齐仍应命中。
    #[test]
    fn layout_cache_hits_on_repeat_and_ignores_alignment() {
        let mut engine = TextEngine::new();
        if !engine.has_font() {
            return;
        }
        let mut out = Vec::new();
        engine.emit(
            &style("cache me please", 20.0),
            rect(400.0, 80.0),
            1.0,
            &mut out,
        );
        assert_eq!(engine.layout_cache_stats(), (0, 1, 1));

        out.clear();
        engine.emit(
            &style("cache me please", 20.0),
            rect(400.0, 80.0),
            1.0,
            &mut out,
        );
        assert_eq!(engine.layout_cache_stats(), (1, 1, 1));

        out.clear();
        let mut centered = style("cache me please", 20.0);
        centered.align = UiAlign::Center;
        engine.emit(&centered, rect(400.0, 80.0), 1.0, &mut out);
        assert_eq!(
            engine.layout_cache_stats(),
            (2, 1, 1),
            "对齐只影响最终摆放，不该让折行缓存失效"
        );
    }

    /// 内容 / 字号 / 换行宽度任一变化都必须重新排版。
    #[test]
    fn layout_cache_key_covers_width_content_and_size() {
        let mut engine = TextEngine::new();
        if !engine.has_font() {
            return;
        }
        let mut out = Vec::new();
        engine.emit(&style("hello world", 20.0), rect(400.0, 80.0), 1.0, &mut out);
        engine.emit(&style("hello world", 20.0), rect(80.0, 80.0), 1.0, &mut out);
        engine.emit(&style("hello worlds", 20.0), rect(400.0, 80.0), 1.0, &mut out);
        engine.emit(&style("hello world", 21.0), rect(400.0, 80.0), 1.0, &mut out);
        assert_eq!(
            engine.layout_cache_stats(),
            (0, 4, 4),
            "四个维度各变一次，应四次都未命中"
        );
    }

    /// 缓存命中 / 未命中的输出必须逐字段一致，且与关缓存时一致。
    #[test]
    fn cached_output_is_identical_to_uncached() {
        use xgame_core::UiTextStyle;
        let mut cached = TextEngine::new();
        if !cached.has_font() {
            return;
        }
        let mut plain = TextEngine::new();
        plain.set_layout_cache_enabled(false);

        let cases = [
            ("hello world hello world", 18.0, 400.0, 80.0, UiAlign::Left),
            ("hello world hello world", 18.0, 90.0, 80.0, UiAlign::Left),
            ("中文折行测试，标点也要能断。", 24.0, 120.0, 120.0, UiAlign::Center),
            ("Right aligned", 32.0, 300.0, 60.0, UiAlign::Right),
        ];
        for (content, size, width, height, align) in cases {
            let text = UiTextStyle {
                content: content.to_owned(),
                size,
                align,
                ..UiTextStyle::default()
            };
            let area = rect(width, height);
            let mut first = Vec::new();
            let mut plain_out = Vec::new();
            cached.emit(&text, area, 0.75, &mut first);
            plain.emit(&text, area, 0.75, &mut plain_out);
            assert_eq!(first, plain_out, "缓存首次排版应与关缓存时一致: {content}");

            let mut second = Vec::new();
            cached.emit(&text, area, 0.75, &mut second);
            assert_eq!(first, second, "缓存命中的输出必须逐字段一致: {content}");
        }
    }

    /// 逐帧变化的文案不能把缓存撑成无界（满了按最近使用淘汰）。
    #[test]
    fn layout_cache_stays_bounded() {
        let mut engine = TextEngine::new();
        if !engine.has_font() {
            return;
        }
        let mut out = Vec::new();
        let total = LAYOUT_CACHE_CAPACITY + 200;
        for i in 0..total {
            let text = style(&format!("score {i}"), 16.0);
            out.clear();
            engine.emit(&text, rect(200.0, 24.0), 1.0, &mut out);
        }
        let (_, misses, entries) = engine.layout_cache_stats();
        assert_eq!(misses as usize, total, "每段新文案都该未命中一次");
        assert!(
            entries <= LAYOUT_CACHE_CAPACITY,
            "缓存条目必须有界: {entries} > {LAYOUT_CACHE_CAPACITY}"
        );
    }
}
