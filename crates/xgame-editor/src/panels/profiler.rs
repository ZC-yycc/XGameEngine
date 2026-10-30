//! 帧分析（Profiler）面板——帧间隔 / 主循环耗时的滚动窗口 + 绘制读数。
//!
//! 排版纪律：**数字在上、图在下、读数压底**。面板默认停在 Output Log 那一格
//! （1280 窗口下只有 ~90px 高的作画区），所以三块都必须能在矮高度下活下来 ——
//! 图按"剩余高度"分配，读数永远单行。
//!
//! 与 `frame_probe` 的分工见 [`crate::profiler`] 模块文档：那是离线探针（落盘），
//! 这是在线读数（界面）。两者共用同一个时间口径（`stable_dt`），但互不依赖。
//!
//! # 为什么这里不 request_repaint
//!
//! 面板只是**读者**：出不出帧由 `frame_policy` 决定（Play / 交互 / 动画时本来
//! 就在连续出帧）。为了"图能动"而在这里强推重绘，会让静止的编辑器永远满速 ——
//! 那正是 `frame_policy` 这一整套要避免的事。

use crate::panels::{icons, theme};
use crate::profiler::{Profiler, Summary};
use crate::state::EditorState;

/// 60 fps 预算（微秒）。
const BUDGET_60_US: u32 = 16_667;
/// 30 fps 预算（微秒）。纵轴下限就画到这里。
const BUDGET_30_US: u32 = 33_333;
/// 纵轴上限的硬顶（微秒）。一次停帧唤醒的长间隔可能上百毫秒，
/// 不设顶的话其余样本会被压成一条贴着底的线，等于什么都看不见。
const CEILING_CAP_US: f32 = 100_000.0;
/// 图的最小高度 —— 再矮就没有"图"的意义了（不如让脚注被挤掉）。
const MIN_GRAPH_HEIGHT: f32 = 28.0;

/// 在 `Ui` 内渲染 Profiler 面板内容。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    header(ui, &mut state.profiler);

    // 统计每帧都要算一次（面板画的时候才调，不是模拟热路径）：600 个 u32 排序
    // 加上一遍遍历，微秒级，换来"读数和图永远同一帧"的一致性。
    let summary = state.profiler.summary();
    metric_row(ui, &summary);

    // 两行脚注的高度**必须在分配图之前**扣掉，而且要按真实行高算。
    //
    // 踩过两次：先按"剩余高度 − 一行"给图，图例被切掉半行；改成写死 14px 又
    // 少了行距，图例占掉 24px，下面那行绘制读数整行落到了状态栏底下（1280×800
    // 静态截图可见）。行高问字体、再补一个 `item_spacing.y` —— 与 egui 自己的
    // 排版同源，改字号也不会再错位。
    let row =
        ui.fonts(|f| f.row_height(&egui::FontId::proportional(10.5))) + ui.spacing().item_spacing.y;
    let reserved = row * 2.0 + theme::SECTION_GAP;
    let graph_height = (ui.available_height() - reserved).max(MIN_GRAPH_HEIGHT);
    graph(ui, &state.profiler, &summary, graph_height);
    legend(ui, &summary);
    readings(ui, state);
}

/// 顶部操作行：暂停 / 清空 + 右侧累计帧数。
fn header(ui: &mut egui::Ui, profiler: &mut Profiler) {
    ui.horizontal(|ui| {
        let (label, tip) = if profiler.paused {
            ("Resume", "继续采样（当前窗口保留）")
        } else {
            ("Pause", "冻结当前窗口 —— 边看边滚的图没法读具体某一帧")
        };
        if icons::text_button(ui, label, tip).clicked() {
            profiler.paused = !profiler.paused;
        }
        if icons::text_button(ui, "Clear", "清空窗口并重新开始计数").clicked() {
            profiler.clear();
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} frames · window {}",
                    profiler.recorded(),
                    profiler.capacity()
                ))
                .monospace()
                .size(11.0)
                .color(theme::colors::TEXT_SECONDARY),
            )
            .on_hover_text(
                "累计记录帧数 · 窗口容量。窗口满了之后只保留最近 600 帧（60fps 下 10 秒）",
            );
            if profiler.paused {
                icons::small_icon(ui, icons::Icon::Pause, 9.0, theme::colors::WARN);
            }
        });
    });
}

/// 一行 KPI：帧率 / 帧时间 / 1% low / 主循环 / 物理 / 界面。
fn metric_row(ui: &mut egui::Ui, s: &Summary) {
    use theme::colors;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if s.frames == 0 {
            ui.label(
                egui::RichText::new("等待帧数据…")
                    .size(11.0)
                    .color(colors::TEXT_DIM),
            );
            return;
        }

        // 帧率只在真有连续帧时才有意义：全是空闲帧时 summary 报 0，
        // 直接写成 idle —— "0 fps" 看起来像卡死，其实是编辑器闲下来了。
        let fps_text = if s.live_frames == 0 {
            "idle".to_owned()
        } else {
            format!("{:.0}", s.fps)
        };
        let fps_color = if s.live_frames == 0 {
            colors::TEXT_DIM
        } else if s.fps >= 50.0 {
            colors::TEXT_BRIGHT
        } else if s.fps >= 28.0 {
            colors::WARN
        } else {
            colors::ERROR
        };
        metric(ui, "fps", fps_text, fps_color);

        metric(
            ui,
            "frame",
            format!("{:.1} ms", s.avg_dt_us / 1000.0),
            budget_color(s.avg_dt_us),
        );
        metric(
            ui,
            "1% low",
            format!("{:.1} ms", s.p99_dt_us as f32 / 1000.0),
            budget_color(s.p99_dt_us as f32),
        );
        metric(
            ui,
            "work",
            format!("{:.2} ms", s.avg_work_us / 1000.0),
            colors::TEXT_PRIMARY,
        );
        metric(
            ui,
            "runtime",
            format!("{:.2} ms", s.avg_runtime_us / 1000.0),
            colors::TEXT_SECONDARY,
        );
        metric(
            ui,
            "viewport",
            format!("{:.2} ms", s.avg_ui_us / 1000.0),
            colors::TEXT_SECONDARY,
        );
    });
}

/// 「标签 + 等宽数值 + 竖线」——与状态栏同一套读法（标签暗、数值亮）。
fn metric(ui: &mut egui::Ui, label: &str, value: String, color: egui::Color32) {
    ui.label(
        egui::RichText::new(label)
            .size(11.0)
            .color(theme::colors::TEXT_DIM),
    );
    ui.label(
        egui::RichText::new(value)
            .monospace()
            .size(11.0)
            .color(color),
    );
    icons::v_separator(ui);
}

/// 帧时间是否越过预算线。
fn budget_color(dt_us: f32) -> egui::Color32 {
    use theme::colors;
    if dt_us <= BUDGET_60_US as f32 {
        colors::PLAY_GREEN
    } else if dt_us <= BUDGET_30_US as f32 {
        colors::WARN
    } else {
        colors::ERROR
    }
}

/// 纵轴上限（微秒）：按**中位帧**定标，给 3 倍余量。
///
/// - 用中位数而不是最大值：一次 80ms 的唤醒间隔会把其余 599 帧压成贴着底的一条线；
/// - 给 3 倍余量：好帧占到图高的 1/3，抖动与尖刺才看得出形状；
/// - 下限 6ms / 上限 100ms：避免"全是一帧"时纵轴塌成 0，也避免停帧几百毫秒
///   之后整张图全是空白（超出的柱子会被裁到顶）。
fn ceiling_us(summary: &Summary) -> f32 {
    (summary.p50_dt_us as f32 * 3.0).clamp(6_000.0, CEILING_CAP_US)
}

/// 帧时间图：每帧一根竖条（高度 = 帧间隔），底部深一档的一段是**主循环工作**。
///
/// 两段叠在一起才有诊断价值：柱子和预算线齐平但几乎没有灰色 = 在等
/// （vsync / 上屏 / 唤醒）；灰色顶满整根柱子 = 真的在算。
fn graph(ui: &mut egui::Ui, profiler: &Profiler, summary: &Summary, height: f32) {
    use theme::colors;

    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, colors::INPUT_BG);

    if profiler.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "还没有帧数据",
            egui::FontId::proportional(11.0),
            colors::TEXT_DIM,
        );
        return;
    }

    let ceiling = ceiling_us(summary);
    let scale = rect.height() / ceiling;

    // 预算线（16.7 / 33.3 ms）：不画虚线网格，只画两条 —— 面板矮，网格反而糊。
    for budget in [BUDGET_60_US, BUDGET_30_US] {
        let y = rect.bottom() - budget as f32 * scale;
        if y > rect.top() + 1.0 {
            painter.hline(
                rect.x_range(),
                y,
                egui::Stroke::new(1.0_f32, colors::SEPARATOR_SOFT),
            );
        }
    }

    let bar_w = (rect.width() / profiler.len() as f32).max(1.0);
    for (i, sample) in profiler.iter().enumerate() {
        let x = rect.left() + (i as f32 + 0.5) * bar_w;
        let dt_h = (sample.dt_us as f32 * scale).clamp(1.0, rect.height());
        // 空闲帧压暗：它那根很长的柱子是"停帧间隔"，不是卡顿 ——
        // 与真长帧一个颜色会让每次停帧都像出事了。
        let mut color = budget_color(sample.dt_us as f32);
        if !sample.live {
            color = color.gamma_multiply(0.45);
        }
        painter.vline(
            x,
            rect.bottom() - dt_h..=rect.bottom(),
            egui::Stroke::new(bar_w.min(3.0), color),
        );

        let work_h = (sample.work_us as f32 * scale).clamp(0.0, dt_h);
        if work_h >= 1.0 {
            painter.vline(
                x,
                rect.bottom() - work_h..=rect.bottom(),
                egui::Stroke::new(bar_w.min(3.0), colors::TEXT_PRIMARY.gamma_multiply(0.55)),
            );
        }
    }
}

/// 图例 + 窗口跨度。压在图下方**一行**（矮面板装不下第二行，写长了会换行、
/// 把下面的绘制读数顶出可视区）。
fn legend(ui: &mut egui::Ui, summary: &Summary) {
    ui.add_space(2.0);
    if summary.frames == 0 {
        ui.label(theme::dim_note(
            "Play 或转动视角时开始采样；空闲停帧期间不产生样本。",
        ));
        return;
    }
    let text = format!(
        "灰 = 主循环工作 · 绿 ≤16.7 / 黄 ≤33.3 / 红 >33.3ms · 压暗 = 空闲帧 · {:.1}s / {} 帧 · 纵轴 {:.0}ms",
        summary.window_seconds(),
        summary.frames,
        ceiling_us(summary) / 1000.0,
    );
    // 先按无限宽排版再截断：交给 `Label` 自动换行的话，长一截就会折成两行
    // （面板固定高度下第二行直接被裁掉）。悬停给全文。
    let galley = ui.painter().layout(
        text.clone(),
        egui::FontId::proportional(10.5),
        theme::colors::TEXT_DIM,
        f32::INFINITY,
    );
    ui.add(egui::Label::new(galley).truncate())
        .on_hover_text(text);
}

/// 底部读数：绘制批次数 / 剔除比例 / 粒子。
///
/// 这些数字是"渲染侧有没有退化"的第一现场（实例化拆批、剔除失效、粒子撞顶），
/// 与帧时间放在同一块面板里，看"慢了"和"为什么慢"才不用来回切面板。
fn readings(ui: &mut egui::Ui, state: &EditorState) {
    use theme::colors;

    if state.scene_renderer.is_none() {
        ui.label(
            egui::RichText::new("无渲染器（wgpu 不可用）—— 只有帧时间")
                .size(10.5)
                .color(colors::TEXT_DIM),
        );
        return;
    }

    let stats = state.render_stats();
    let mut text = format!(
        "draw {} · opaque {} batch / {} inst · blended {} · culled {:.0}%",
        stats.draw_calls(),
        stats.opaque_batches,
        stats.opaque_instances,
        stats.blended_draws,
        stats.cull_ratio() * 100.0,
    );
    let particles = state.particle_stats();
    if particles.emitters > 0 {
        text.push_str(&format!(
            " · particles {} / {} ({} emitters)",
            particles.alive, particles.capacity, particles.emitters
        ));
    }
    // 点光立方体阴影只在有灯开 `casts_shadow` 时出现，平时不占这一行的宽度。
    if stats.point_shadow_instances > 0 {
        text.push_str(&format!(
            " · point shadow {} inst / {} draws",
            stats.point_shadow_instances, stats.point_shadow_draw_calls
        ));
    }

    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .monospace()
                .size(10.5)
                .color(colors::TEXT_SECONDARY),
        )
        .truncate(),
    )
    .on_hover_text(
        "draw = 主 pass 发出的 draw call 数（不透明批次 + 半透明逐项）\n\
         culled = 被视锥剔除的绘制项占比（相机一转它就动 = 剔除在干活）\n\
         particles = 活着 / 上限；撞上限时新粒子不再出生\n\
         point shadow = 点光立方体阴影六面提交的实例 / draw 数（六面全画，不做逐面剔除）",
    );
}
