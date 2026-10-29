//! 绘制数据：把「布局 + 样式 + 文本」压成「矩形 + 精灵 + 批」。
//!
//! 这一层是 CPU 与 GPU 的唯一接口。它刻意做成**扁平数组 + 批区间**：
//!
//! ```text
//! UiDrawData {
//!     quads:    [UiQuad]        // 圆角 / 描边矩形（实例化）
//!     sprites:  [UiSprite]      // 图片 / 字形（实例化，带 UV）
//!     commands: [Rects(n) | Sprites(n, 纹理)]   // 顺序即绘制顺序
//!     images:   [路径]           // 本帧用到的图片（索引 = UiTexture::Image）
//! }
//! ```
//!
//! 为什么不存"元素树"让渲染器自己走：渲染器只该关心"画什么"，不该重新
//! 实现一遍锚点、换行与对齐 —— 那是两套几何、迟早错位。这里算完，GPU 侧
//! 就只剩"上传 + 两次 draw"。
//!
//! 批的合并规则与渲染顺序一致：**相邻且同纹理的精灵合并成一次 draw**。
//! 文本天然连续（一段文字的精灵是挨着生成的），所以一段 HUD 文字通常
//! 只有一个文本批。

use std::collections::HashMap;

use xgame_core::{Color, Scene, UiElement, UiKind};
use xgame_math::glam::Vec2;

use crate::layout::{self, LayoutItem, UiRect, UiTree};
use crate::text::TextEngine;

// ---------------------------------------------------------------------------
// 绘制图元
// ---------------------------------------------------------------------------

/// 一个圆角矩形（背景 / 边框）。
///
/// 圆角与描边走 SDF：着色器按"到圆角矩形的有符号距离"做抗锯齿，
/// 因此任意圆角半径都不糊，也不需要预生成九宫格贴图。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiQuad {
    /// 矩形（像素）。
    pub rect: UiRect,
    /// 填充色（线性；`a = 0` 表示只画边框）。
    pub color: Color,
    /// 圆角半径（像素）。
    pub corner_radius: f32,
    /// 边框宽度（像素，内侧）。
    pub border_width: f32,
    /// 边框色。
    pub border_color: Color,
}

/// 精灵的采样方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpriteMode {
    /// 单通道覆盖率（字形图集）：用 `r` 当 alpha，颜色取顶点色。
    AlphaMask,
    /// RGBA 图片：与顶点色相乘。
    Rgba,
}

/// 精灵引用哪张纹理。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiTexture {
    /// 字形图集。
    Atlas,
    /// 本帧 `UiDrawData::images` 里的第 n 张。
    Image(u32),
}

/// 一个纹理四边形（图片 / 字形）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiSprite {
    /// 目标矩形（像素）。
    pub rect: UiRect,
    /// `[u0, v0, u1, v1]`（归一化）。
    pub uv: [f32; 4],
    /// 乘子色。
    pub color: Color,
    /// 采样方式。
    pub mode: SpriteMode,
    /// 纹理。
    pub texture: UiTexture,
}

/// 裁剪区（整数像素，左上原点，右下开区间）。
///
/// 在 CPU 侧就量化成整数：GPU scissor 只接受整数，而且相邻元素只要量化后
/// 相同就能继续合批 —— 浮点上的微小差异不该切断批次。`width/height = 0`
/// 表示空裁剪区（这条批什么也画不出来）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiClip {
    /// 左边界（含）。
    pub x: u32,
    /// 上边界（含）。
    pub y: u32,
    /// 宽度（像素）。
    pub width: u32,
    /// 高度（像素）。
    pub height: u32,
}

/// 一条绘制指令（同一类图元的一段连续区间）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiCommand {
    /// 画 `quads[start..start+count]`。
    Rects {
        /// 起始下标。
        start: u32,
        /// 数量。
        count: u32,
        /// 裁剪区（`None` = 不裁剪）。
        clip: Option<UiClip>,
    },
    /// 画 `sprites[start..start+count]`（同一张纹理）。
    Sprites {
        /// 起始下标。
        start: u32,
        /// 数量。
        count: u32,
        /// 纹理。
        texture: UiTexture,
        /// 裁剪区（`None` = 不裁剪）。
        clip: Option<UiClip>,
    },
}

/// 一帧的 UI 绘制数据。
#[derive(Default, Debug)]
pub struct UiDrawData {
    /// 本帧视口尺寸（像素）。
    pub viewport: Vec2,
    /// 矩形图元。
    pub quads: Vec<UiQuad>,
    /// 精灵图元。
    pub sprites: Vec<UiSprite>,
    /// 绘制顺序。
    pub commands: Vec<UiCommand>,
    /// 本帧用到的图片路径（去重）。
    pub images: Vec<String>,
    /// 生成这份数据时的字形图集版本。
    pub atlas_version: u64,
    /// 本帧参与绘制的元素数（统计 / 测试用）。
    pub element_count: usize,
    /// 当前元素的裁剪区（`Painter` 在合成每个元素前设置；内部状态）。
    pub(crate) clip: Option<UiClip>,
}

impl UiDrawData {
    /// 清空（保留容量）。
    pub fn reset(&mut self, viewport: Vec2) {
        self.viewport = viewport;
        self.quads.clear();
        self.sprites.clear();
        self.commands.clear();
        self.images.clear();
        self.atlas_version = 0;
        self.element_count = 0;
        self.clip = None;
    }

    /// 设置后续图元的裁剪区（`None` = 不裁剪）。
    ///
    /// 浮点裁剪矩形在这里量化成整数 scissor：GPU 只认整数，且量化后相同的
    /// 相邻元素还能继续合批。超出视口的部分先夹掉。
    pub fn set_clip(&mut self, clip: Option<UiRect>) {
        self.clip = clip.map(|rect| quantize_clip(rect, self.viewport));
    }

    /// 追加一个矩形，尽量并入上一条**同裁剪区**的矩形批。
    pub fn push_quad(&mut self, quad: UiQuad) {
        let start = self.quads.len() as u32;
        let clip = self.clip;
        self.quads.push(quad);
        match self.commands.last_mut() {
            Some(UiCommand::Rects {
                start: s,
                count,
                clip: c,
            }) if *s + *count == start && *c == clip => *count += 1,
            _ => self.commands.push(UiCommand::Rects {
                start,
                count: 1,
                clip,
            }),
        }
    }

    /// 追加一个精灵，尽量并入上一条同纹理、同裁剪区的精灵批。
    pub fn push_sprite(&mut self, sprite: UiSprite) {
        let start = self.sprites.len() as u32;
        let texture = sprite.texture;
        let clip = self.clip;
        self.sprites.push(sprite);
        match self.commands.last_mut() {
            Some(UiCommand::Sprites {
                start: s,
                count,
                texture: t,
                clip: c,
            }) if *s + *count == start && *t == texture && *c == clip => *count += 1,
            _ => self.commands.push(UiCommand::Sprites {
                start,
                count: 1,
                texture,
                clip,
            }),
        }
    }

    /// 把一段连续精灵（例如一段文本的字形）登记成批。
    ///
    /// 调用方直接往 `sprites` 里追加（文本引擎为了省一次拷贝会整段写），
    /// 再由这个方法补上批记录。
    pub fn push_sprite_range(&mut self, start: usize, end: usize, texture: UiTexture) {
        if end <= start {
            return;
        }
        let start = start as u32;
        let count = (end - start as usize) as u32;
        let clip = self.clip;
        match self.commands.last_mut() {
            Some(UiCommand::Sprites {
                start: s,
                count: c,
                texture: t,
                clip: prev,
            }) if *s + *c == start && *t == texture && *prev == clip => *c += count,
            _ => self.commands.push(UiCommand::Sprites {
                start,
                count,
                texture,
                clip,
            }),
        }
    }

    /// 登记一张图片，返回它在 `images` 里的索引（同一帧内去重）。
    pub fn image_index(&mut self, path: &str) -> u32 {
        if let Some(index) = self.images.iter().position(|p| p == path) {
            return index as u32;
        }
        self.images.push(path.to_owned());
        (self.images.len() - 1) as u32
    }
}

/// 量化裁剪矩形到整数像素并夹进视口。
///
/// 左 / 上取 floor、右 / 下取 ceil：部分覆盖的像素必须保留，否则边缘会
/// 出现"裁剪框比元素小一格"的缝隙。空矩形得到 `width = height = 0`。
fn quantize_clip(rect: UiRect, viewport: Vec2) -> UiClip {
    if !rect.min.is_finite() || !rect.max.is_finite() || !viewport.is_finite() {
        return UiClip::default();
    }
    let vw = viewport.x.max(0.0);
    let vh = viewport.y.max(0.0);
    let x0 = rect.min.x.max(0.0).min(vw).floor();
    let y0 = rect.min.y.max(0.0).min(vh).floor();
    let x1 = rect.max.x.max(0.0).min(vw).ceil();
    let y1 = rect.max.y.max(0.0).min(vh).ceil();
    UiClip {
        x: x0 as u32,
        y: y0 as u32,
        width: (x1 - x0).max(0.0) as u32,
        height: (y1 - y0).max(0.0) as u32,
    }
}

// ---------------------------------------------------------------------------
// 画笔
// ---------------------------------------------------------------------------

/// 把场景里的 UI 元素合成为 [`UiDrawData`]。
pub struct Painter {
    text: TextEngine,
    tree: UiTree,
    items: Vec<LayoutItem>,
    draw: UiDrawData,
    /// 图片原始尺寸（路径 → 宽高）。渲染器加载纹理后回填，`keep_aspect` 用。
    image_sizes: HashMap<String, Vec2>,
}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

impl Painter {
    /// 新建（会尝试加载系统字体）。
    pub fn new() -> Self {
        Self {
            text: TextEngine::new(),
            tree: UiTree::new(),
            items: Vec::new(),
            draw: UiDrawData::default(),
            image_sizes: HashMap::new(),
        }
    }

    /// 文本引擎（图集在这里）。
    pub fn text(&self) -> &TextEngine {
        &self.text
    }

    /// 回填一张图片的原始尺寸（渲染器加载完纹理后调用）。
    pub fn set_image_size(&mut self, path: &str, size: Vec2) {
        self.image_sizes.insert(path.to_owned(), size);
    }

    /// 当前绘制数据。
    pub fn draw(&self) -> &UiDrawData {
        &self.draw
    }

    /// 本帧参与绘制的元素数。
    pub fn element_count(&self) -> usize {
        self.draw.element_count
    }

    /// 重建整帧绘制数据。
    pub fn build(&mut self, scene: &Scene, viewport: Vec2) -> &UiDrawData {
        self.tree.build(scene);
        layout::visit(scene, &self.tree, viewport, &mut self.items);

        // 图集可能在本次合成中翻倍（新字形按需光栅化）。已写下的精灵 UV 是
        // 按**当时**的边长归一化的，边长一变就全错（采样到别的字形或图集外）。
        // 字形此时已全部进缓存，重来一遍只是再摆一次位置；翻倍最多一次，
        // 所以这里不需要循环。
        let atlas_size = self.text.atlas().size();
        self.paint(scene, viewport);
        if self.text.atlas().size() != atlas_size {
            self.paint(scene, viewport);
        }
        &self.draw
    }

    /// 按已算好的布局条目合成绘制数据（`build` 可能调用两次）。
    fn paint(&mut self, scene: &Scene, viewport: Vec2) {
        let Painter {
            text,
            items,
            draw,
            image_sizes,
            ..
        } = self;
        draw.reset(viewport);
        draw.element_count = items.len();

        for item in items.iter() {
            let Some(entity) = scene.entities.get(item.index) else {
                continue;
            };
            let Some(ui) = entity.ui.as_ref() else {
                continue;
            };
            draw.set_clip(item.clip);
            emit_element(ui, item.rect, text, image_sizes, draw);
        }

        draw.atlas_version = text.atlas().version();
    }
}

/// 单个元素的合成（背景 → 图片 → 文本）。
fn emit_element(
    ui: &UiElement,
    rect: UiRect,
    text: &mut TextEngine,
    image_sizes: &HashMap<String, Vec2>,
    draw: &mut UiDrawData,
) {
    if rect.is_empty() {
        return;
    }

    // 禁用态：只对按钮生效。
    //
    // 不能拿 `!interactable` 当禁用：装饰面板本来就是不可交互的，
    // 给它们统一降透明度会让整个 HUD 发灰。按钮的"不可交互"才是禁用。
    let disabled = ui.kind == UiKind::Button && !ui.interactable;
    let alpha = if disabled { 0.5 } else { 1.0 };

    // -- 1. 背景 / 边框 --
    let style = &ui.style;
    if style.background[3] > 0.0 || style.border_width > 0.0 {
        let mut color = Color::from(style.background);
        color.a *= alpha;
        if ui.kind == UiKind::Button && !disabled {
            // 悬停 / 按下：直接在背景色上做明度乘子。
            // 不做补间动画（UI 动画属 P2），静态明度差已经能给出明确的反馈。
            if ui.state.pressed {
                color = scale_rgb(color, 0.78);
            } else if ui.state.hovered {
                color = scale_rgb(color, 1.22);
            }
        }
        let mut border = Color::from(style.border_color);
        border.a *= alpha;
        draw.push_quad(UiQuad {
            rect,
            color,
            corner_radius: style.corner_radius,
            border_width: style.border_width,
            border_color: border,
        });
    }

    // 内容矩形：扣掉内边距与边框（边框画在内侧，文字要让开）。
    let inset = style.padding + style.border_width;
    let content = if inset > 0.0 { rect.inset(inset) } else { rect };

    // -- 2. 图片 --
    if let Some(image) = &ui.image {
        if !image.path.is_empty() && image.fill > 0.0 && !content.is_empty() {
            let target = fit_image(
                content,
                image_sizes.get(&image.path).copied(),
                image.keep_aspect,
            );
            if !target.is_empty() {
                let index = draw.image_index(&image.path);
                let mut tint = Color::from(image.tint);
                tint.a *= alpha;
                let mut rect = target;
                let mut uv = [0.0, 0.0, 1.0, 1.0];
                if image.fill < 1.0 {
                    // 从左往右裁剪：矩形与 UV 同步收窄，才能让图案保持比例
                    // （只裁 UV 会拉伸图案，只裁矩形会露出边缘像素）。
                    let width = rect.width() * image.fill;
                    rect.max.x = rect.min.x + width;
                    uv[2] = image.fill;
                }
                draw.push_sprite(UiSprite {
                    rect,
                    uv,
                    color: tint,
                    mode: SpriteMode::Rgba,
                    texture: UiTexture::Image(index),
                });
            }
        }
    }

    // -- 3. 文本 --
    if !ui.text.content.is_empty() && !content.is_empty() {
        let start = draw.sprites.len();
        text.emit(&ui.text, content, alpha, &mut draw.sprites);
        draw.push_sprite_range(start, draw.sprites.len(), UiTexture::Atlas);
    }
}

/// 图片在内容矩形里的落位（`keep_aspect` 时按原始比例做最大内接）。
fn fit_image(rect: UiRect, natural: Option<Vec2>, keep_aspect: bool) -> UiRect {
    if !keep_aspect {
        return rect;
    }
    let Some(natural) = natural else {
        // 尺寸未知（第一帧还没加载完）：先铺满，下一帧就正了。
        return rect;
    };
    if natural.x <= 0.0 || natural.y <= 0.0 {
        return rect;
    }
    let scale = (rect.width() / natural.x).min(rect.height() / natural.y);
    let size = natural * scale;
    UiRect::from_min_size(rect.center() - size * 0.5, size)
}

/// 只缩放 RGB，保留 alpha（悬停 / 按下的明度变化不该改变透明度）。
fn scale_rgb(color: Color, factor: f32) -> Color {
    Color {
        r: (color.r * factor).clamp(0.0, 1.0),
        g: (color.g * factor).clamp(0.0, 1.0),
        b: (color.b * factor).clamp(0.0, 1.0),
        a: color.a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_core::{Entity, Scene, UiAlign, UiElement};

    fn scene_with(ui: UiElement) -> Scene {
        let mut scene = Scene::new("draw");
        let mut entity = Entity::new("E");
        entity.ui = Some(ui);
        scene.entities.push(entity);
        scene
    }

    /// 按钮 = 1 个背景矩形 + 一段文本批，顺序固定（背景在前）。
    #[test]
    fn button_emits_background_then_text() {
        let mut painter = Painter::new();
        let scene = scene_with(UiElement::button("Hi").at([0.0, 0.0], 0.0, 0.0, 200.0, 60.0));
        let has_font = painter.text().has_font();
        let draw = painter.build(&scene, Vec2::new(800.0, 600.0));

        assert!(matches!(
            draw.commands[0],
            UiCommand::Rects { count: 1, .. }
        ));
        assert_eq!(draw.quads.len(), 1);
        if has_font {
            assert!(
                draw.commands
                    .iter()
                    .any(|c| matches!(c, UiCommand::Sprites { .. })),
                "按钮文案应当产生文本批"
            );
            assert!(!draw.sprites.is_empty());
        }
        assert_eq!(draw.element_count, 1);
    }

    /// 隐藏的元素不产生任何图元。
    #[test]
    fn hidden_element_emits_nothing() {
        let mut painter = Painter::new();
        let mut element = UiElement::panel(100.0, 100.0);
        element.visible = false;
        let draw = painter.build(&scene_with(element), Vec2::new(800.0, 600.0));
        assert!(draw.quads.is_empty());
        assert!(draw.sprites.is_empty());
        assert!(draw.commands.is_empty());
        assert_eq!(draw.element_count, 0);
    }

    /// 相邻的同类图元必须合并（否则 200 个 HUD 元素就是 200 次 draw）。
    #[test]
    fn adjacent_quads_merge_into_one_batch() {
        let mut scene = Scene::new("batch");
        for i in 0..50 {
            let mut entity = Entity::new(format!("Panel{i}"));
            entity.ui =
                Some(UiElement::panel(20.0, 20.0).at([0.0, 0.0], i as f32 * 22.0, 0.0, 20.0, 20.0));
            scene.entities.push(entity);
        }
        let mut painter = Painter::new();
        let draw = painter.build(&scene, Vec2::new(800.0, 600.0));
        assert_eq!(draw.quads.len(), 50);
        assert_eq!(
            draw.commands.len(),
            1,
            "50 个相邻矩形应当合成 1 个批: {:?}",
            draw.commands
        );
    }

    /// 画中画：子元素在父之后（后画 = 盖在上面）。
    #[test]
    fn children_are_drawn_after_parent() {
        let mut scene = Scene::new("nest");
        let mut parent = Entity::new("Parent");
        parent.ui = Some(UiElement::panel(200.0, 200.0));
        let parent_id = parent.id;
        scene.entities.push(parent);
        let mut child = Entity::new("Child");
        child.parent = Some(parent_id);
        child.ui = Some(UiElement::panel(50.0, 50.0).at([0.0, 0.0], 10.0, 10.0, 50.0, 50.0));
        scene.entities.push(child);

        let mut painter = Painter::new();
        let draw = painter.build(&scene, Vec2::new(800.0, 600.0));
        assert_eq!(draw.quads.len(), 2);
        assert_eq!(draw.quads[0].rect.width(), 200.0, "父先画");
        assert_eq!(draw.quads[1].rect.width(), 50.0, "子后画");
    }

    /// 文本居中：内容矩形的中心就是文本块的中心。
    #[test]
    fn centered_text_is_horizontally_centered() {
        let mut painter = Painter::new();
        if !painter.text().has_font() {
            return;
        }
        let mut element = UiElement::text("AAAA").at([0.0, 0.0], 0.0, 0.0, 400.0, 100.0);
        element.text.align = UiAlign::Center;
        let scene = scene_with(element);
        let draw = painter.build(&scene, Vec2::new(800.0, 600.0));
        let min = draw
            .sprites
            .iter()
            .map(|s| s.rect.min.x)
            .fold(f32::MAX, f32::min);
        let max = draw
            .sprites
            .iter()
            .map(|s| s.rect.max.x)
            .fold(f32::MIN, f32::max);
        let center = (min + max) * 0.5;
        assert!(
            (center - 200.0).abs() < 2.0,
            "文本中心应在 200 附近: {center}"
        );
    }

    /// 填充比例同时裁剪矩形与 UV（血条语义）。
    #[test]
    fn image_fill_crops_rect_and_uv() {
        let mut painter = Painter::new();
        let mut element =
            UiElement::image("Resources/hp.png").at([0.0, 0.0], 0.0, 0.0, 200.0, 20.0);
        element.image.as_mut().unwrap().fill = 0.25;
        element.image.as_mut().unwrap().keep_aspect = false;
        let draw = painter.build(&scene_with(element), Vec2::new(800.0, 600.0));
        assert_eq!(draw.sprites.len(), 1);
        let sprite = draw.sprites[0];
        assert!((sprite.rect.width() - 50.0).abs() < 1e-4);
        assert!((sprite.uv[2] - 0.25).abs() < 1e-4);
        assert_eq!(draw.images, vec!["Resources/hp.png".to_owned()]);
    }

    /// 保持比例：256×128 的图放进 200×200 的框，应当变成 200×100 居中。
    #[test]
    fn keep_aspect_letterboxes_image() {
        let mut painter = Painter::new();
        painter.set_image_size("Resources/wide.png", Vec2::new(256.0, 128.0));
        let mut element =
            UiElement::image("Resources/wide.png").at([0.0, 0.0], 0.0, 0.0, 200.0, 200.0);
        element.image.as_mut().unwrap().fill = 1.0;
        let draw = painter.build(&scene_with(element), Vec2::new(800.0, 600.0));
        let sprite = draw.sprites[0];
        assert!((sprite.rect.width() - 200.0).abs() < 1e-3);
        assert!((sprite.rect.height() - 100.0).abs() < 1e-3);
        assert!((sprite.rect.min.y - 50.0).abs() < 1e-3, "垂直居中");
    }

    /// 图集在一帧内翻倍时，**全部**精灵的 UV 都要按最终边长归一化。
    ///
    /// 旧实现只在合成开始时读一次边长：翻倍后写下的 texel 坐标除以旧边长，
    /// UV 会越界（采样到图集外）。`build` 必须在边长变化后重来一遍。
    #[test]
    fn atlas_growth_mid_frame_normalizes_every_sprite() {
        let mut painter = Painter::new();
        if !painter.text().has_font() {
            return;
        }
        let before = painter.text().atlas().size();
        // 900 个不同的 CJK 字形 × 48px：一帧内必然把 1024² 撑到 2048²。
        let content: String = (0..900u32)
            .filter_map(|i| char::from_u32(0x4E00 + i))
            .collect();
        let mut element =
            UiElement::text(content.as_str()).at([0.0, 0.0], 0.0, 0.0, 4000.0, 200.0);
        element.text.size = 48.0;
        element.text.wrap = false;
        let scene = scene_with(element);
        let viewport = Vec2::new(4096.0, 256.0);

        let draw = painter.build(&scene, viewport);
        let first = draw.sprites.clone();
        let atlas_size = painter.text().atlas().size();
        assert!(
            atlas_size > before,
            "测试前提：图集应当在本帧内翻倍"
        );
        assert!(!first.is_empty());
        for sprite in &first {
            assert!(
                sprite.uv.iter().all(|v| (0.0..=1.0).contains(v)),
                "UV 越过图集边界: {:?}（边长 {atlas_size}）",
                sprite.uv
            );
        }
        let draw = painter.build(&scene, viewport);
        assert_eq!(first, draw.sprites, "第二次合成不得再变化");
    }

    /// 裁剪区进入绘制批：父元素不裁，子元素的图元带上父矩形量化后的 scissor。
    #[test]
    fn clip_children_batches_carry_quantized_scissor() {
        let mut scene = Scene::new("clip-draw");
        let mut root = Entity::new("Clip");
        let mut ui = UiElement::panel(100.0, 100.0).at([0.0, 0.0], 10.0, 10.0, 100.0, 100.0);
        ui.clip_children = true;
        root.ui = Some(ui);
        let root_id = root.id;
        scene.entities.push(root);

        let mut child = Entity::new("Child");
        child.parent = Some(root_id);
        child.ui = Some(UiElement::button("OK").at([0.0, 0.0], 10.0, 10.0, 60.0, 30.0));
        scene.entities.push(child);

        let mut painter = Painter::new();
        let draw = painter.build(&scene, Vec2::new(800.0, 600.0));
        let expected = UiClip {
            x: 10,
            y: 10,
            width: 100,
            height: 100,
        };
        let clip_of = |command: &UiCommand| match *command {
            UiCommand::Rects { clip, .. } | UiCommand::Sprites { clip, .. } => clip,
        };
        assert!(draw.commands.len() >= 2, "父 + 子至少两条批");
        assert_eq!(clip_of(&draw.commands[0]), None, "裁剪元素自身不裁");
        assert!(
            draw.commands[1..]
                .iter()
                .all(|command| clip_of(command) == Some(expected)),
            "后代的所有批都要带裁剪区: {:?}",
            draw.commands
        );
    }

    /// 裁剪开关必须改变批记录：关掉之后父子矩形会合并成同一批。
    ///
    /// 这条同时盯住“别把 clip 写进合批判据之外”的回归 —— 合并后的批若还带着
    /// 旧 scissor，后代就会被错误裁剪（或相反，裁剪彻底失效）。
    #[test]
    fn clip_toggle_changes_command_clips() {
        let mut scene = Scene::new("clip-sig");
        let mut root = Entity::new("Clip");
        let mut ui = UiElement::panel(100.0, 100.0).at([0.0, 0.0], 10.0, 10.0, 100.0, 100.0);
        ui.clip_children = true;
        root.ui = Some(ui);
        let root_id = root.id;
        scene.entities.push(root);
        let mut child = Entity::new("Child");
        child.parent = Some(root_id);
        child.ui = Some(UiElement::panel(50.0, 50.0).at([0.0, 0.0], 0.0, 0.0, 50.0, 50.0));
        scene.entities.push(child);

        let mut painter = Painter::new();
        let clip_of = |command: &UiCommand| match *command {
            UiCommand::Rects { clip, .. } | UiCommand::Sprites { clip, .. } => clip,
        };
        let draw = painter.build(&scene, Vec2::new(800.0, 600.0));
        assert_eq!(draw.commands.len(), 2, "父子裁剪区不同，不能合并: {:?}", draw.commands);
        let with_clip = clip_of(&draw.commands[1]);
        assert!(with_clip.is_some(), "开了裁剪就该带 scissor");

        scene.entities[0].ui.as_mut().unwrap().clip_children = false;
        let draw = painter.build(&scene, Vec2::new(800.0, 600.0));
        assert_eq!(draw.commands.len(), 1, "关掉裁剪后两块面板应合并成一批");
        assert_eq!(clip_of(&draw.commands[0]), None, "关掉裁剪后不该再限制后代");
        assert_eq!(draw.quads.len(), 2, "合批不能吞掉图元");
    }

    /// 悬停变亮、按下变暗：静态明度差就是"这个按钮能点"的全部反馈。
    #[test]
    fn hover_brightens_and_press_darkens() {
        let mut painter = Painter::new();
        let plain = painter.build(&scene_with(UiElement::button("X")), Vec2::new(800.0, 600.0));
        let base = plain.quads[0].color;

        let mut hovered_ui = UiElement::button("X");
        hovered_ui.state.hovered = true;
        let hovered = painter.build(&scene_with(hovered_ui), Vec2::new(800.0, 600.0));
        assert!(
            hovered.quads[0].color.r > base.r && hovered.quads[0].color.g > base.g,
            "悬停应当更亮: {:?} vs {base:?}",
            hovered.quads[0].color
        );

        let mut pressed_ui = UiElement::button("X");
        pressed_ui.state.pressed = true;
        let pressed = painter.build(&scene_with(pressed_ui), Vec2::new(800.0, 600.0));
        assert!(
            pressed.quads[0].color.r < base.r,
            "按下应当更暗: {:?} vs {base:?}",
            pressed.quads[0].color
        );
        // 透明度不该被明度变化带偏（两者是独立的视觉通道）。
        assert!((pressed.quads[0].color.a - base.a).abs() < 1e-6);
    }

    /// 禁用按钮整体半透明，且不再显示悬停高亮。
    #[test]
    fn disabled_button_is_faded() {
        let mut painter = Painter::new();
        let mut element = UiElement::button("X");
        element.interactable = false;
        element.state.hovered = true;
        let draw = painter.build(&scene_with(element), Vec2::new(800.0, 600.0));
        let base = UiElement::button("X");
        assert!(draw.quads[0].color.a < base.style.background[3]);
    }
}
