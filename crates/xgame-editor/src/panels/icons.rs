//! 手绘矢量图标。
//!
//! egui 自带的是**字形**图标（emoji 字体里的 📦 / 💾 / ⚙）。它们的字形宽度、
//! 基线、粗细都不一致，混在同一条工具栏里会显得东倒西歪 —— 这正是"有些地方
//! 不整齐"的根源之一。这里改成用 `Painter` 画几何图形：每个图标都落在给定的
//! 正方框里，笔画粗细统一（1.4px），颜色由调用方给，缩放不失真。
//!
//! 约定：所有函数都在 `rect` 的**内接区域**作画，不越界；调用方只负责给一个
//! 尺寸合适的方框（工具栏 16×16、列表行 12×12）。

use egui::{Color32, Pos2, Rect, Shape, Stroke, Vec2};

/// 可画的图标种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Translate,
    Rotate,
    Scale,
    Rect,
    Cube,
    Dot,
    Diamond,
    ChevronDown,
    ChevronRight,
    Save,
    Gear,
    /// 三根带滑块的横线（编辑器偏好设置）。
    Sliders,
    Folder,
    Build,
    Refresh,
    Grid,
    /// 叉号（列表行的"摘掉"按钮）。
    Close,
    /// 警告三角（未注册的组件）。
    Warning,
    /// 带折角的纸页（脚本文件）。
    Script,
    /// 拼块方块（Rust 组件）。
    Component,
    /// 加号（"＋ Add" 行）。
    Plus,
    /// 对勾（日志成功 / 校验通过）。
    Check,
    /// 图片（纹理资产）：带山与太阳的相框。
    Image,
    /// 声波（音频资产）：三条高低不一的竖线。
    Wave,
    /// 材质（`.mat` 资产）：一滴颜料 / 液滴。
    Material,
}

/// 在 `rect` 内画一个图标。
pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let t = (s / 10.0).clamp(1.0, 1.6); // 笔画粗细随尺寸缩放
    let stroke = Stroke::new(t, color);

    match icon {
        Icon::Play => {
            let pts = vec![
                Pos2::new(c.x - s * 0.22, c.y - s * 0.30),
                Pos2::new(c.x - s * 0.22, c.y + s * 0.30),
                Pos2::new(c.x + s * 0.30, c.y),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Pause => {
            let w = s * 0.16;
            let h = s * 0.56;
            painter.rect_filled(
                Rect::from_center_size(Pos2::new(c.x - s * 0.14, c.y), Vec2::new(w, h)),
                0.0,
                color,
            );
            painter.rect_filled(
                Rect::from_center_size(Pos2::new(c.x + s * 0.14, c.y), Vec2::new(w, h)),
                0.0,
                color,
            );
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(s * 0.52)), 1.0, color);
        }
        Icon::Translate => {
            // 四向箭头：一条横线 + 一条竖线，四个箭头尖。
            //
            // 箭头必须**够大**（占臂长约 1/4）：原先的 2.1×1.6px 小三角在 16px
            // 下几乎不可见，整个图标读起来就是一个"加号"，和"添加实体"撞语义。
            let arm = s * 0.42;
            painter.line_segment([c - Vec2::X * arm, c + Vec2::X * arm], stroke);
            painter.line_segment([c - Vec2::Y * arm, c + Vec2::Y * arm], stroke);
            let head = s * 0.17;
            for dir in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
                let tip = c + dir * arm;
                let base = tip - dir * head;
                let side = Vec2::new(-dir.y, dir.x) * head * 0.66;
                painter.add(Shape::convex_polygon(
                    vec![tip, base + side, base - side],
                    color,
                    Stroke::NONE,
                ));
            }
        }
        Icon::Rotate => {
            // 四分之三圆弧 + **实心箭头**（屏幕坐标 y 向下，角度增大 = 顺时针）。
            //
            // 箭头必须画在弧的**末端**并且够大：先前那版把 3px 的小三角摆在弧的
            // 起点、方向还朝着弧外，16px 下整个图标读起来就是个缺口的 "C"
            // （和"加载中"的转圈没区别）。
            let r = s * 0.29;
            let start = 200.0_f32;
            let sweep = 265.0_f32;
            let steps = 28;
            let mut pts = Vec::new();
            for i in 0..=steps {
                let a = (start + sweep * i as f32 / steps as f32).to_radians();
                pts.push(Pos2::new(c.x + r * a.cos(), c.y + r * a.sin()));
            }
            painter.add(Shape::line(pts, stroke));

            let a = (start + sweep).to_radians();
            let base = Pos2::new(c.x + r * a.cos(), c.y + r * a.sin());
            let dir = Vec2::new(-a.sin(), a.cos()); // 角度增大方向的切线
            let head = s * 0.30;
            let half = Vec2::new(-dir.y, dir.x) * s * 0.155;
            painter.add(Shape::convex_polygon(
                vec![base + dir * head, base + half, base - half],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Scale => {
            // 方框 + 从右下角伸出的**对角箭头**：箭头长度占满整个图标，16px 下
            // 仍能看出"把东西拉大"。
            //
            // 试过"一大一小两个嵌套方框"：重叠处两条边线在 1px 下搅在一起，
            // 看着像"复制"图标；补的那条对角连线又被两个框的间距压缩到 2px，
            // 根本看不见。
            let box_rect = Rect::from_min_max(
                Pos2::new(c.x - s * 0.42, c.y - s * 0.42),
                Pos2::new(c.x + s * 0.14, c.y + s * 0.14),
            );
            painter.rect_stroke(box_rect, 0.0, stroke);

            let from = Pos2::new(c.x + s * 0.02, c.y + s * 0.02);
            let dir = Vec2::new(1.0, 1.0).normalized();
            let tip = Pos2::new(c.x + s * 0.44, c.y + s * 0.44);
            painter.line_segment([from, tip - dir * s * 0.12], stroke);
            let half = Vec2::new(-dir.y, dir.x) * s * 0.15;
            painter.add(Shape::convex_polygon(
                vec![tip, tip - dir * s * 0.28 + half, tip - dir * s * 0.28 - half],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Rect => {
            // 方框 + 四角的小方块：读作"框选/矩形工具"。光画一个方框会和
            // Scale 的方框撞脸，四个角点把"可拖拽的控制框"这个语义点出来。
            let r = Rect::from_center_size(c, Vec2::splat(s * 0.62));
            painter.rect_stroke(r, 0.0, Stroke::new(t * 0.85, color));
            let dot = s * 0.13;
            for p in [
                r.left_top(),
                r.right_top(),
                r.left_bottom(),
                r.right_bottom(),
            ] {
                painter.rect_filled(Rect::from_center_size(p, Vec2::splat(dot)), 0.5, color);
            }
        }
        Icon::Cube => {
            // **实心**等轴测方块：三面三档明度拼出体积。
            //
            // 线框版本（六边形 + 三根辐条）在 12px 行高下笔画挤在一起，
            // 读起来是"圆圈里一个加号"（准星）。填色后只剩三个色块，
            // 小尺寸也一眼是方块。
            let w = s * 0.30;
            let h = s * 0.17;
            let v = s * 0.21;
            let top = Pos2::new(c.x, c.y - h - v);
            let right = Pos2::new(c.x + w, c.y - h);
            let left = Pos2::new(c.x - w, c.y - h);
            let bottom = Pos2::new(c.x, c.y + h + v);
            let right_low = Pos2::new(c.x + w, c.y + h);
            let left_low = Pos2::new(c.x - w, c.y + h);
            // 顶面（最亮）
            painter.add(Shape::convex_polygon(
                vec![top, right, c, left],
                color.gamma_multiply(0.95),
                Stroke::NONE,
            ));
            // 左下侧面（最暗）/ 右下侧面（居中）
            painter.add(Shape::convex_polygon(
                vec![left, c, bottom, left_low],
                color.gamma_multiply(0.45),
                Stroke::NONE,
            ));
            painter.add(Shape::convex_polygon(
                vec![c, right, right_low, bottom],
                color.gamma_multiply(0.68),
                Stroke::NONE,
            ));
        }
        Icon::Dot => {
            painter.circle_filled(c, s * 0.16, color);
        }
        Icon::Material => {
            // 水滴：上方尖角 + 下方圆。用两段凸多边形拼，12px 下仍能读出
            // "不是方块、不是圆点"的材质语义（材质 = 一层漆 / 颜料）。
            let r = s * 0.24;
            let tip = Pos2::new(c.x, c.y - s * 0.38);
            painter.add(Shape::convex_polygon(
                vec![
                    tip,
                    Pos2::new(c.x + r * 0.92, c.y - s * 0.02),
                    Pos2::new(c.x - r * 0.92, c.y - s * 0.02),
                ],
                color,
                Stroke::NONE,
            ));
            painter.circle_filled(Pos2::new(c.x, c.y + s * 0.10), r, color);
            // 高光：同色系更亮的一小块（不引入新主题色）。
            painter.circle_filled(
                Pos2::new(c.x - r * 0.34, c.y + s * 0.03),
                r * 0.26,
                color.gamma_multiply(1.6),
            );
        }
        Icon::Diamond => {
            let r = s * 0.30;
            painter.add(Shape::convex_polygon(
                vec![
                    Pos2::new(c.x, c.y - r),
                    Pos2::new(c.x + r, c.y),
                    Pos2::new(c.x, c.y + r),
                    Pos2::new(c.x - r, c.y),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::ChevronDown | Icon::ChevronRight => {
            let r = s * 0.18;
            let pts = if icon == Icon::ChevronDown {
                vec![
                    Pos2::new(c.x - r, c.y - r * 0.55),
                    Pos2::new(c.x, c.y + r * 0.55),
                    Pos2::new(c.x + r, c.y - r * 0.55),
                ]
            } else {
                vec![
                    Pos2::new(c.x - r * 0.55, c.y - r),
                    Pos2::new(c.x + r * 0.55, c.y),
                    Pos2::new(c.x - r * 0.55, c.y + r),
                ]
            };
            painter.add(Shape::line(pts, Stroke::new(t * 1.2, color)));
        }
        Icon::Save => {
            // 软盘：外框 + 上半的写入口（实心）。
            //
            // 原先还画了下方标签，16px 下三个框叠在一起糊成"回"字 —— 关掉缩放的
            // 显示器上根本看不出是软盘。上半实心 + 下半留空是最小可辨识的形状。
            let r = Rect::from_center_size(c, Vec2::new(s * 0.60, s * 0.62));
            painter.rect_stroke(r, 1.0, stroke);
            let w = r.width();
            painter.rect_filled(
                Rect::from_min_size(
                    Pos2::new(r.left() + w * 0.22, r.top()),
                    Vec2::new(w * 0.42, r.height() * 0.42),
                ),
                0.0,
                color,
            );
        }
        Icon::Gear => {
            // 齿圈 + 内孔：外圈画实体圆环，外面 8 颗短齿，中心一个小圆。
            painter.circle_stroke(c, s * 0.26, Stroke::new(t * 1.1, color));
            painter.circle_stroke(c, s * 0.10, Stroke::new(t * 0.9, color));
            for i in 0..8 {
                let a = (i as f32) * std::f32::consts::TAU / 8.0;
                let dir = Vec2::new(a.cos(), a.sin());
                painter.line_segment([c + dir * s * 0.28, c + dir * s * 0.38], stroke);
            }
        }
        Icon::Sliders => {
            // 三根横线 + 错开的滑块：与 Gear 区分开（齿轮是"项目设置"，
            // 滑块是"编辑器偏好"），不会在工具栏上出现两个一样的按钮。
            for (i, x) in [0.30_f32, 0.50, 0.70].iter().enumerate() {
                let y = c.y + (i as f32 - 1.0) * s * 0.22;
                painter.line_segment(
                    [
                        Pos2::new(c.x - s * 0.30, y),
                        Pos2::new(c.x + s * 0.30, y),
                    ],
                    Stroke::new(t * 0.8, color),
                );
                let knob_x = c.x + (x - 0.5) * s * 1.1;
                painter.circle_filled(Pos2::new(knob_x, y), s * 0.09, color);
            }
        }
        Icon::Folder => {
            // 文件夹：主体 + 左上的小页签。
            //
            // 页签必须**占宽度的近一半、高 2px 以上**：原先页签只有 0.30s 宽、
            // 0.12s 高，14px 图标里就是 4×1.7px 的一条，肉眼完全看不见 ——
            // 整个图标读作"空心方块"。主体的填充也压到 0.22 倍，让它退到描边之后，
            // 页签（全亮实心）才是第一个被看到的东西。
            let r = Rect::from_center_size(c, Vec2::new(s * 0.68, s * 0.44));
            let tab = Rect::from_min_size(
                Pos2::new(r.left(), r.top() - s * 0.16),
                Vec2::new(r.width() * 0.50, s * 0.17),
            );
            painter.rect_filled(r, 1.0, color.gamma_multiply(0.22));
            painter.rect_stroke(r, 1.0, Stroke::new(t * 0.9, color));
            painter.rect_filled(tab, egui::Rounding::same(0.5), color);
            // 页签与主体的接缝：一条底色线，否则两块实心糊成一块高方块。
            painter.line_segment(
                [
                    Pos2::new(tab.left(), r.top()),
                    Pos2::new(tab.right(), r.top()),
                ],
                Stroke::new(1.0_f32, color.gamma_multiply(0.22)),
            );
        }
        Icon::Build => {
            // 包裹箱：外框 + 横向封条 + 竖向提手带。
            //
            // 原本画的是锤子，但 16px 下"柄 + 头"读起来就是一个 T 字；
            // 打包动作的语义用"箱子"表达反而更直观，也和「Build → Build/ 产物」对得上。
            // 箱体：外框 + 靠上的**箱盖线** + 盖下的小提手。
            //
            // 盖子线不能画在中线、提手也不能贯穿箱体 —— 那样就是四格窗（和
            // 左边的「网格」工具图标撞脸）。三分之一处的横线 + 中间一小段
            // 竖提手才是"包裹箱"。
            let r = Rect::from_center_size(c, Vec2::new(s * 0.66, s * 0.58));
            painter.rect_stroke(r, 1.0, Stroke::new(t * 0.95, color));
            let lid_y = r.top() + r.height() * 0.34;
            painter.line_segment(
                [Pos2::new(r.left(), lid_y), Pos2::new(r.right(), lid_y)],
                Stroke::new(t * 0.95, color),
            );
            painter.line_segment(
                [
                    Pos2::new(c.x, lid_y),
                    Pos2::new(c.x, lid_y + r.height() * 0.26),
                ],
                Stroke::new(t * 1.1, color),
            );
        }
        Icon::Grid => {
            // 3×3 细网格（中心留空，看起来更像"网格导轨"而不是"九宫格"）。
            let r = Rect::from_center_size(c, Vec2::splat(s * 0.62));
            let thin = Stroke::new(t * 0.75, color);
            for i in 0..=3 {
                let t01 = i as f32 / 3.0;
                painter.line_segment(
                    [
                        Pos2::new(r.left() + r.width() * t01, r.top()),
                        Pos2::new(r.left() + r.width() * t01, r.bottom()),
                    ],
                    thin,
                );
                painter.line_segment(
                    [
                        Pos2::new(r.left(), r.top() + r.height() * t01),
                        Pos2::new(r.right(), r.top() + r.height() * t01),
                    ],
                    thin,
                );
            }
        }
        Icon::Refresh => {
            let r = s * 0.30;
            let mut pts = Vec::new();
            let steps = 20;
            for i in 0..=steps {
                let a = (300.0 + 280.0 * i as f32 / steps as f32).to_radians();
                pts.push(Pos2::new(c.x + r * a.cos(), c.y + r * a.sin()));
            }
            painter.add(Shape::line(pts, stroke));
            let a = 300f32.to_radians();
            let tip = Pos2::new(c.x + r * a.cos(), c.y + r * a.sin());
            let tangent = Vec2::new(-a.sin(), a.cos());
            let side = Vec2::new(-tangent.y, tangent.x) * s * 0.10;
            painter.add(Shape::convex_polygon(
                vec![tip - tangent * s * 0.14, tip + side, tip - side],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Close => {
            let r = s * 0.24;
            painter.line_segment(
                [Pos2::new(c.x - r, c.y - r), Pos2::new(c.x + r, c.y + r)],
                Stroke::new(t * 1.2, color),
            );
            painter.line_segment(
                [Pos2::new(c.x + r, c.y - r), Pos2::new(c.x - r, c.y + r)],
                Stroke::new(t * 1.2, color),
            );
        }
        Icon::Warning => {
            // 空心三角 + 感叹号。
            let h = s * 0.34;
            let w = s * 0.38;
            painter.add(Shape::closed_line(
                vec![
                    Pos2::new(c.x, c.y - h),
                    Pos2::new(c.x + w, c.y + h * 0.78),
                    Pos2::new(c.x - w, c.y + h * 0.78),
                ],
                Stroke::new(t, color),
            ));
            painter.line_segment(
                [Pos2::new(c.x, c.y - h * 0.28), Pos2::new(c.x, c.y + h * 0.22)],
                Stroke::new(t, color),
            );
            painter.circle_filled(Pos2::new(c.x, c.y + h * 0.48), t * 0.5, color);
        }
        Icon::Script => {
            // 纸页（右上角折角）+ 三条文字线。
            let w = s * 0.44;
            let h = s * 0.56;
            let page = Rect::from_center_size(c, Vec2::new(w, h));
            let fold = s * 0.20;
            painter.add(Shape::closed_line(
                vec![
                    page.left_top(),
                    Pos2::new(page.right() - fold, page.top()),
                    Pos2::new(page.right(), page.top() + fold),
                    page.right_bottom(),
                    page.left_bottom(),
                ],
                Stroke::new(t * 0.9, color),
            ));
            painter.line_segment(
                [
                    Pos2::new(page.right() - fold, page.top()),
                    Pos2::new(page.right() - fold, page.top() + fold),
                ],
                Stroke::new(t * 0.8, color.gamma_multiply(0.8)),
            );
            for i in 0..3 {
                let y = page.top() + h * (0.46 + 0.18 * i as f32);
                painter.line_segment(
                    [
                        Pos2::new(page.left() + w * 0.22, y),
                        Pos2::new(page.right() - w * 0.20, y),
                    ],
                    Stroke::new(t * 0.8, color.gamma_multiply(0.85)),
                );
            }
        }
        Icon::Component => {
            // 拼块：一个方块 + 右侧的凸起。
            let r = Rect::from_center_size(c, Vec2::splat(s * 0.48));
            painter.rect_stroke(r, 1.5, Stroke::new(t * 0.9, color));
            painter.rect_filled(
                Rect::from_center_size(
                    Pos2::new(r.right(), r.center().y),
                    Vec2::new(s * 0.12, s * 0.22),
                ),
                0.5,
                color,
            );
            painter.rect_filled(
                Rect::from_center_size(
                    Pos2::new(r.center().x, r.top()),
                    Vec2::new(s * 0.22, s * 0.12),
                ),
                0.5,
                color.gamma_multiply(0.7),
            );
        }
        Icon::Plus => {
            let r = s * 0.26;
            painter.line_segment([c - Vec2::X * r, c + Vec2::X * r], Stroke::new(t * 1.1, color));
            painter.line_segment([c - Vec2::Y * r, c + Vec2::Y * r], Stroke::new(t * 1.1, color));
        }
        Icon::Image => {
            // 相框 + 内部一座山 + 一颗太阳：小尺寸下仍能读出"这是图片"。
            let r = Rect::from_center_size(c, Vec2::new(s * 0.62, s * 0.50));
            painter.rect_stroke(r, 1.0, Stroke::new(t * 0.9, color));
            painter.add(Shape::closed_line(
                vec![
                    Pos2::new(r.left() + s * 0.08, r.bottom() - s * 0.06),
                    Pos2::new(r.center().x - s * 0.04, r.center().y - s * 0.06),
                    Pos2::new(r.right() - s * 0.06, r.bottom() - s * 0.06),
                ],
                Stroke::new(t * 0.8, color.gamma_multiply(0.85)),
            ));
            painter.circle_filled(
                Pos2::new(r.right() - s * 0.14, r.top() + s * 0.14),
                s * 0.05,
                color.gamma_multiply(0.75),
            );
        }
        Icon::Wave => {
            // 三条竖线（中间最高）—— 音频的通用符号，比音符在 11px 下更清楚。
            for (i, h) in [0.20_f32, 0.34, 0.24].iter().enumerate() {
                let x = c.x + (i as f32 - 1.0) * s * 0.18;
                painter.line_segment(
                    [
                        Pos2::new(x, c.y - s * h * 0.5),
                        Pos2::new(x, c.y + s * h * 0.5),
                    ],
                    Stroke::new(t * 1.0, color),
                );
            }
        }
        Icon::Check => {
            let pts = vec![
                Pos2::new(c.x - s * 0.28, c.y + s * 0.02),
                Pos2::new(c.x - s * 0.08, c.y + s * 0.24),
                Pos2::new(c.x + s * 0.30, c.y - s * 0.24),
            ];
            painter.add(Shape::line(pts, Stroke::new(t * 1.2, color)));
        }
    }
}

/// 画一个 16×16 的图标按钮（自绘、无文字），返回点击响应。
///
/// 按钮底色跟随 egui 的交互状态（hover / pressed），与普通按钮一致；
/// 图标本身不参与布局，因此不会因为字形宽度把整行推歪。
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    tooltip: &str,
    active: bool,
) -> egui::Response {
    use super::theme::colors;

    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::click());
    // 按下态用"压暗的选中蓝"而不是 `visuals.bg_fill`：默认的 `BUTTON_ACTIVE`
    // 是中性灰，四个工具按钮里"当前是哪个"就看不出来了。
    if active {
        ui.painter()
            .rect_filled(rect, 2.0, colors::SELECTION_BG.gamma_multiply(0.62));
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 2.0, colors::BUTTON_HOVER);
    }
    if response.is_pointer_button_down_on() {
        ui.painter()
            .rect_stroke(rect, 2.0, Stroke::new(1.0_f32, colors::ACCENT_BLUE));
    }
    let color = if active {
        colors::TEXT_BRIGHT
    } else if response.hovered() {
        colors::TEXT_PRIMARY
    } else {
        colors::TEXT_SECONDARY
    };
    // 内缩 16%：26px 的按钮里给出 ~18px 的图形，与 UE 工具栏的图标比例接近；
    // 早先缩 22% 只有 14px，四个变换工具在高分屏上辨识度太差。
    paint(ui.painter(), rect.shrink(size * 0.16), icon, color);
    response.on_hover_text(tooltip)
}

/// 「图标 + 文字」的扁平工具栏按钮。
///
/// UE 工具栏的按钮常态是**平的**（无边框、无底色），悬停才浮出一块浅灰。
/// egui 默认按钮自带底色和边框，整条工具栏会像一排贴纸 —— 所以这里自绘。
pub fn icon_text_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    tooltip: &str,
) -> egui::Response {
    icon_text_button_colored(ui, icon, label, tooltip, None)
}

/// 同上，但可指定图标颜色（播放键用绿色）。
pub fn icon_text_button_colored(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    tooltip: &str,
    icon_color: Option<Color32>,
) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), Color32::PLACEHOLDER);
    let pad_x = 7.0;
    let icon_size = 13.0;
    let gap = 5.0;
    let size = Vec2::new(
        pad_x * 2.0 + icon_size + gap + galley.size().x,
        super::theme::FIELD_HEIGHT + 2.0,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact(&response);

    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, 2.0, visuals.bg_fill);
    }

    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.left() + pad_x + icon_size * 0.5, rect.center().y),
        Vec2::splat(icon_size),
    );
    paint(
        ui.painter(),
        icon_rect,
        icon,
        icon_color.unwrap_or(visuals.fg_stroke.color),
    );
    ui.painter().galley(
        Pos2::new(icon_rect.right() + gap, rect.center().y - galley.size().y * 0.5),
        galley,
        visuals.text_color(),
    );

    response.on_hover_text(tooltip)
}

/// 扁平的文字按钮（工具栏里"Save As…"这类次要动作）。
pub fn text_button(ui: &mut egui::Ui, label: &str, tooltip: &str) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let size = Vec2::new(
        galley.size().x + 14.0,
        super::theme::FIELD_HEIGHT + 2.0,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact(&response);
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, 2.0, visuals.bg_fill);
    }
    ui.painter().galley(
        Pos2::new(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        visuals.text_color(),
    );
    response.on_hover_text(tooltip)
}

/// 工具栏里的竖向分组线（比 `ui.separator()` 短，垂直居中）。
pub fn v_separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(9.0, super::theme::FIELD_HEIGHT),
        egui::Sense::hover(),
    );
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(1.0_f32, super::theme::colors::SEPARATOR),
    );
}

/// 行内小图标：只占位、不接点击，颜色由调用方给。
///
/// 列表行里的"这是什么"标记一律走它 —— 用字形图标（📜 / 🧩）时，emoji
/// 的字形宽度各不相同，同一列会左右错位，那正是"不整齐"的直接来源。
pub fn small_icon(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    paint(ui.painter(), rect, icon, color);
    response
}

/// 列表行尾的「✕」：常态是暗灰，悬停变亮并浮出底色。
///
/// 比 `small_button("✕")` 好在两点：宽度固定（不会因为字形把行推歪），
/// 且常态不抢注意力 —— UE 的行内删除按钮就是这个观感。
pub fn close_button(ui: &mut egui::Ui, tooltip: &str) -> bool {
    use super::theme::colors;
    let size = 16.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 2.0, colors::BUTTON_HOVER);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if response.hovered() {
        colors::TEXT_BRIGHT
    } else {
        colors::TEXT_DIM
    };
    paint(ui.painter(), rect.shrink(4.0), Icon::Close, color);
    response.on_hover_text(tooltip).clicked()
}

/// 小标签（骨骼遮罩这类"集合里的一个元素"）：圆角底 + 文本 + ✕，整块可点。
///
/// 取代 `small_button(format!("{m} ✕"))`：那个写法里的 ✕ 是 U+2715，
/// 系统中文粗体里没有这个字形，实机上直接显示成豆腐块 □。
pub fn chip(ui: &mut egui::Ui, label: &str, tooltip: &str) -> bool {
    use super::theme::colors;

    let font = egui::FontId::proportional(10.5);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let height = 16.0;
    let width = galley.size().x + 20.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::click());
    let bg = if response.hovered() {
        colors::BUTTON_HOVER
    } else {
        colors::BUTTON_BG.gamma_multiply(0.7)
    };
    ui.painter().rect_filled(rect, 3.0, bg);
    ui.painter().galley(
        Pos2::new(
            rect.left() + 6.0,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        colors::TEXT_PRIMARY,
    );
    let close_rect = Rect::from_center_size(
        Pos2::new(rect.right() - 7.0, rect.center().y),
        Vec2::splat(8.0),
    );
    paint(
        ui.painter(),
        close_rect,
        Icon::Close,
        if response.hovered() {
            colors::TEXT_BRIGHT
        } else {
            colors::TEXT_DIM
        },
    );
    response.on_hover_text(tooltip).clicked()
}

/// 「＋ Add」占位行：整宽、扁平，常态只有暗色 "+ 文字"，悬停浮出浅底。
///
/// Inspector / Hierarchy 底部的"添加"入口都用它，保证两处观感一致；
/// 禁用态（`add_enabled_ui` 里）会自己压暗并且不接受点击。
pub fn add_row(ui: &mut egui::Ui, label: &str, tooltip: &str) -> egui::Response {
    use super::theme::colors;
    let enabled = ui.is_enabled();
    let width = ui.available_width();
    let height = super::theme::ROW_HEIGHT;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 2.0, colors::ROW_HOVER_BG);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = match (enabled, response.hovered()) {
        (false, _) => colors::TEXT_DIM.gamma_multiply(0.7),
        (true, true) => colors::TEXT_PRIMARY,
        (true, false) => colors::TEXT_SECONDARY,
    };
    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.left() + 10.0, rect.center().y),
        Vec2::splat(11.0),
    );
    paint(ui.painter(), icon_rect, Icon::Plus, color);
    let font = egui::FontId::proportional(11.5);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    ui.painter().galley(
        Pos2::new(
            icon_rect.right() + 5.0,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        color,
    );
    response.on_hover_text(tooltip)
}
