//! Gizmo 拖拽变换计算——根据 GizmoDrag 状态和当前鼠标位置计算实体的新变换值。
//!
//! 通过 `compute_transform` 入口执行，该函数根据工具模式和被拖拽的操作柄类型
//! 选择不同的计算策略：轴拖拽、平面拖拽、旋转拖拽、均匀缩放。

use xgame_math::glam::{Quat, Vec3};
use crate::state::{GizmoDrag, GizmoPart, ToolMode};
use super::GizmoCtx;
use super::pick::hit_position_on_plane;

/// 旋转拖拽的灵敏度倍率。
const ROTATE_SENSITIVITY: f32 = 1.0;

/// 均匀缩放灵敏度（屏幕像素到缩放因子的映射）。
const UNIFORM_SCALE_SENSITIVITY: f32 = 0.005;

// ---------------------------------------------------------------------------
// 公共入口
// ---------------------------------------------------------------------------

/// 根据拖拽状态和当前鼠标位置计算实体的新 Transform 值（translation / scale / rotation）。
#[derive(Debug, Clone)]
pub enum TransformDelta {
    /// 位置变化（Translate 工具）。
    Position(Vec3),
    /// 旋转变化（Rotate 工具），四元数表示的是**相对起始姿态的增量旋转**。
    Rotation(Quat),
    /// 缩放变化（Scale 工具）。
    Scale(Vec3),
}

/// 计算拖拽产生的变换变化量。
///
/// 返回 `Some(delta)` 表示变换需要更新，`None` 表示无效状态。
pub fn compute_transform(
    drag:                       &GizmoDrag,
    current_mouse_pos:          egui::Pos2,
    ctx:                        &GizmoCtx,
    tool_mode:                  ToolMode,
) -> Option<TransformDelta> {
    match tool_mode {
        ToolMode::Translate => compute_translate(drag, current_mouse_pos, ctx),
        ToolMode::Rotate => compute_rotate(drag, current_mouse_pos, ctx),
        ToolMode::Scale => compute_scale(drag, current_mouse_pos, ctx),
        ToolMode::Rect => None,
    }
}

/// 以**拖拽起点的固定锚点**为参考线原点，求鼠标射线在轴上的坐标。
///
/// 这是修掉"拖拽抽搐"的关键：`ctx.entity_pos` 每帧都在变（就是物体被拖到的位置），
/// 拿它当参考线原点会让 `s_now` 依赖自己上一帧的输出 —— 详情见
/// [`GizmoDrag::axis_anchor`] 的推导。锚点固定之后，`s_now` 只由鼠标射线决定。
fn axis_param_anchored(
    mouse_pos: egui::Pos2,
    ctx: &GizmoCtx,
    axis: Vec3,
    anchor: Vec3,
) -> Option<f32> {
    // `axis_param_under_mouse` 只读 ctx 的 view_proj / 视口尺寸 / 偏移 / entity_pos，
    // 所以换掉 entity_pos 就是换参考线原点，不必改公共 API 签名。
    let mut anchored = ctx.clone();
    anchored.entity_pos = anchor;
    super::axis_param_under_mouse(mouse_pos, &anchored, axis)
}

// ---------------------------------------------------------------------------
// Translate 计算
// ---------------------------------------------------------------------------

fn compute_translate(
    drag:                       &GizmoDrag,
    mouse_pos:                  egui::Pos2,
    ctx:                        &GizmoCtx,
) -> Option<TransformDelta> {
    match drag.part {
        // ── 轴拖拽 ──
        GizmoPart::AxisX | GizmoPart::AxisY | GizmoPart::AxisZ => {
            let axis = part_to_axis(drag.part);

            // 沿轴的位移 = 轴坐标的变化量。
            //
            // 必须用 `axis_param_under_mouse`（射线在轴对应的**直线**上的投影）。
            // 旧实现拿 `hit_position_on_axis` 的平面交点再投影到轴上，那个投影
            // **恒为 0**（交点在过实体、垂直于轴的平面上），于是位移恒等于
            // `-entity_pos·axis` 这个与鼠标无关的常量 —— 表现就是"拖红箭头不动"。
            //
            // 参考线锚在 `drag.axis_anchor`（拖拽起点）而**不是** `ctx.entity_pos`：
            // 后者每帧跟着物体走，会让位移变成 `Δ_k = A_k − Δ_{k-1}` 的正反馈 ——
            // 物体在两点之间以帧率来回跳、且只走鼠标位移的一半。详见
            // [`GizmoDrag::axis_anchor`]。
            let s_now = axis_param_anchored(mouse_pos, ctx, axis, drag.axis_anchor)?;
            let new_pos = drag.start_value + axis * (s_now - drag.start_axis);

            Some(TransformDelta::Position(new_pos))
        }

        // ── 平面拖拽 ──
        GizmoPart::PlaneXY | GizmoPart::PlaneXZ | GizmoPart::PlaneYZ => {
            let normal = plane_normal(drag.part);
            let hit = hit_position_on_plane(mouse_pos, ctx, normal)?;

            let delta = hit - drag.start_hit;
            let new_pos = drag.start_value + delta;

            Some(TransformDelta::Position(new_pos))
        }

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rotate 计算
// ---------------------------------------------------------------------------

fn compute_rotate(
    drag: &GizmoDrag,
    mouse_pos: egui::Pos2,
    ctx: &GizmoCtx,
) -> Option<TransformDelta> {
    let axis = match drag.part {
        GizmoPart::RingX => Vec3::X,
        GizmoPart::RingY => Vec3::Y,
        GizmoPart::RingZ => Vec3::Z,
        _ => return None,
    };

    // 获取垂直于轴的两个正交向量（定义旋转平面）
    let (u, v) = super::math::perpendicular_axes(axis);

    // 将鼠标射线投影到旋转平面（经过 entity_pos）
    let hit = hit_position_on_plane(mouse_pos, ctx, axis)?;

    // 计算从 entity_pos 到 hit 的方向向量
    let to_hit = (hit - ctx.entity_pos).normalize();

    // 在旋转平面上分解方向
    let u_comp = to_hit.dot(u);
    let v_comp = to_hit.dot(v);
    let angle = v_comp.atan2(u_comp); // 在旋转平面上的角度

    // 从起始命中点获取起始角度
    let start_dir = (drag.start_hit - ctx.entity_pos).normalize();
    let start_u = start_dir.dot(u);
    let start_v = start_dir.dot(v);
    let start_angle = start_v.atan2(start_u);

    // 计算角度变化量
    let mut delta_angle = angle - start_angle;

    // 处理角度环绕：确保 delta 在 [-PI, PI] 范围内
    if delta_angle > std::f32::consts::PI {
        delta_angle -= std::f32::consts::TAU;
    } else if delta_angle < -std::f32::consts::PI {
        delta_angle += std::f32::consts::TAU;
    }

    delta_angle *= ROTATE_SENSITIVITY;

    // 构建旋转四元数
    let rot = Quat::from_axis_angle(axis, delta_angle);

    // 返回旋转变化量（相对旋转）
    Some(TransformDelta::Rotation(rot))
}

// ---------------------------------------------------------------------------
// Scale 计算
// ---------------------------------------------------------------------------

fn compute_scale(
    drag: &GizmoDrag,
    mouse_pos: egui::Pos2,
    ctx: &GizmoCtx,
) -> Option<TransformDelta> {
    match drag.part {
        // ── 均匀缩放 ──
        GizmoPart::Uniform => {
            // 使用屏幕空间距离变化来驱动均匀缩放
            let start_screen = drag.start_screen_pos;
            let delta_screen = mouse_pos - start_screen;

            // 从屏幕 Y 方向变化计算缩放因子
            let scale_factor = 1.0 + delta_screen.y * UNIFORM_SCALE_SENSITIVITY;
            let scale_factor = scale_factor.max(0.01); // 不能缩放到负值

            let start_scale = drag.start_value;
            let new_scale = start_scale * scale_factor;

            Some(TransformDelta::Scale(new_scale))
        }

        // ── 轴缩放 ──
        GizmoPart::AxisX | GizmoPart::AxisY | GizmoPart::AxisZ => {
            let axis = part_to_axis(drag.part);

            // 与平移同理：轴上的位置只能用直线投影求。
            // 旧实现用平面交点，`t` 恒为 0，`ratio` 于是恒为 `0 / 常量`，
            // 被 `.max(0.01)` 夹住 → 一拖就塌成 1%（起点在原点时 start_t 也是 0，
            // 于是完全不动）。两种表现都不对。
            //
            // 同样锚在拖拽起点：缩放本身不改位置，用 `ctx.entity_pos` 恰好也是稳定的，
            // 但锚点固定后语义才与 `start_axis` 严格一致（后者正是按起点算的）。
            let s_now = axis_param_anchored(mouse_pos, ctx, axis, drag.axis_anchor)?;

            let start_scale = drag.start_value;

            // 抓住操作柄的位置就是起始轴坐标，比值即缩放比。
            let ratio = if drag.start_axis.abs() > 1e-6 {
                (s_now / drag.start_axis).max(0.01)
            } else {
                1.0
            };

            // 构建新的缩放值：对应轴分量乘以 ratio，其他分量保持起始值
            let mut new_scale = start_scale;
            if axis.x.abs() > 0.5 {
                new_scale.x = start_scale.x * ratio;
            } else if axis.y.abs() > 0.5 {
                new_scale.y = start_scale.y * ratio;
            } else {
                new_scale.z = start_scale.z * ratio;
            }

            Some(TransformDelta::Scale(new_scale))
        }

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// 将 GizmoPart 映射到对应的轴方向。
fn part_to_axis(part: GizmoPart) -> Vec3 {
    match part {
        GizmoPart::AxisX | GizmoPart::RingX => Vec3::X,
        GizmoPart::AxisY | GizmoPart::RingY => Vec3::Y,
        GizmoPart::AxisZ | GizmoPart::RingZ => Vec3::Z,
        _ => Vec3::Y,
    }
}

/// 获取平面操作柄的法线方向。
fn plane_normal(part: GizmoPart) -> Vec3 {
    match part {
        GizmoPart::PlaneXY => Vec3::Z,  // XY 平面法线为 Z
        GizmoPart::PlaneXZ => Vec3::Y,  // XZ 平面法线为 Y
        GizmoPart::PlaneYZ => Vec3::X,  // YZ 平面法线为 X
        _ => Vec3::Y,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gizmo::{axis_param_under_mouse, gizmo_world_size, hit_position_on_axis, project_to_screen};

    /// 和编辑器默认视角一致：相机在 (5,5,5) 看向原点。
    const EYE: Vec3 = Vec3::new(5.0, 5.0, 5.0);

    fn test_ctx(entity_pos: Vec3) -> GizmoCtx {
        let view_proj = xgame_math::projection::perspective(60f32.to_radians(), 4.0 / 3.0, 0.1, 100.0)
            * xgame_math::projection::look_at(EYE, Vec3::ZERO, Vec3::Y);
        GizmoCtx {
            view_proj,
            screen_size: egui::vec2(1200.0, 800.0),
            screen_offset: egui::vec2(0.0, 24.0), // 顶栏占掉的高度
            entity_pos,
            gizmo_size: gizmo_world_size(EYE, entity_pos),
            hovered: None,
            active: None,
        }
    }

    /// 世界点 → 屏幕点（用来"把鼠标放到某个世界位置"）。
    fn at(ctx: &GizmoCtx, world: Vec3) -> egui::Pos2 {
        project_to_screen(world, &ctx.view_proj, ctx.screen_size, ctx.screen_offset)
    }

    /// 轴上 `k` 倍 gizmo 长度处的世界点。
    fn on_axis(ctx: &GizmoCtx, axis: Vec3, k: f32) -> Vec3 {
        ctx.entity_pos + axis * ctx.gizmo_size * k
    }

    /// 造一个"已经抓住轴上某点"的拖拽状态（`scene_view` 里那套起始量的最小复刻）。
    fn drag_on_axis(ctx: &GizmoCtx, axis: Vec3, at_k: f32, part: GizmoPart, start_value: Vec3) -> GizmoDrag {
        let grab = at(ctx, on_axis(ctx, axis, at_k));
        GizmoDrag {
            part,
            start_value,
            start_hit: Vec3::ZERO,
            start_axis: axis_param_under_mouse(grab, ctx, axis).expect("抓取点应当落在轴上"),
            axis_anchor: ctx.entity_pos,
            start_screen_pos: grab,
            start_rotation: Quat::IDENTITY,
        }
    }

    /// **根因记录**：`hit_position_on_axis` 给的是「过实体、以轴为法线」的平面交点，
    /// 它到实体的位移**永远垂直于轴**，投影恒为 0。
    ///
    /// 所以拿它算轴坐标必然得到与鼠标无关的常量 —— 这正是平移"拖不动"、
    /// 缩放"瞬间塌成 1%"的来源。轴坐标必须用 [`axis_param_under_mouse`]。
    #[test]
    fn plane_hit_projects_to_zero_on_the_axis() {
        let ctx = test_ctx(Vec3::new(2.0, 0.0, 0.0));
        for k in [0.3_f32, 0.7, 1.4] {
            let screen = at(&ctx, on_axis(&ctx, Vec3::X, k));
            let hit = hit_position_on_axis(screen, &ctx, Vec3::X).expect("应当有交点");
            let along_axis = (hit - ctx.entity_pos).dot(Vec3::X);
            assert!(
                along_axis.abs() < 1e-3,
                "平面交点在轴上的投影应当恒为 0，实得 {along_axis}"
            );
        }

        // 而"直线投影"这个量是跟着 k 走的，不是常量。
        let s1 = axis_param_under_mouse(at(&ctx, on_axis(&ctx, Vec3::X, 0.5)), &ctx, Vec3::X).unwrap();
        let s2 = axis_param_under_mouse(at(&ctx, on_axis(&ctx, Vec3::X, 1.5)), &ctx, Vec3::X).unwrap();
        assert!((s2 - s1).abs() > 1e-3, "轴坐标必须随鼠标位置变化: {s1} vs {s2}");
    }

    /// **回归**：沿轴平移必须跟着鼠标走。
    ///
    /// 旧实现里沿 X 拖红箭头，位置纹丝不动（位移恒等于 `-entity_pos·axis` 这个常量）。
    #[test]
    fn axis_translate_follows_the_mouse() {
        let ctx = test_ctx(Vec3::ZERO);
        let drag = drag_on_axis(&ctx, Vec3::X, 0.7, GizmoPart::AxisX, ctx.entity_pos);

        // 把鼠标挪到轴上更远处（1.4 倍），位移应当约 0.7 倍 gizmo 长度
        let moved = at(&ctx, on_axis(&ctx, Vec3::X, 1.4));
        let delta = compute_transform(&drag, moved, &ctx, ToolMode::Translate).expect("应当算出位移");

        match delta {
            TransformDelta::Position(p) => {
                let want = ctx.gizmo_size * 0.7;
                assert!(
                    (p.x - want).abs() < want * 0.05,
                    "沿 X 的位移应当约 {want}，实得 {}",
                    p.x
                );
                assert!(
                    p.y.abs() < 1e-3 && p.z.abs() < 1e-3,
                    "拖 X 轴只该改 x，实得 {p}"
                );
            }
            other => panic!("应当是 Position，实得 {other:?}"),
        }
    }

    /// **回归**：沿轴缩放要跟着抓取距离走，绝不能一拖就塌成 1%。
    ///
    /// 旧实现里 `t` 恒为 0，比值是 `0 / 常量`，被 `.max(0.01)` 夹住 → 物体瞬间缩到 1%。
    #[test]
    fn axis_scale_tracks_the_grab_distance() {
        let ctx = test_ctx(Vec3::ZERO);
        let drag = drag_on_axis(&ctx, Vec3::X, 0.8, GizmoPart::AxisX, Vec3::ONE);

        // 鼠标退回一半距离 → 缩放约减半
        let nearer = at(&ctx, on_axis(&ctx, Vec3::X, 0.4));
        let delta = compute_transform(&drag, nearer, &ctx, ToolMode::Scale).expect("应当算出缩放");

        match delta {
            TransformDelta::Scale(s) => {
                assert!((s.x - 0.5).abs() < 0.05, "缩放应当约减半，实得 {}", s.x);
                assert!(s.x > 0.2, "绝不能塌成旧实现的 1%（0.01），实得 {}", s.x);
                assert_eq!((s.y, s.z), (1.0, 1.0), "只该改被拖的那个轴");
            }
            other => panic!("应当是 Scale，实得 {other:?}"),
        }
    }

    /// **回归**：旋转环拖拽必须给出有限的四元数。
    ///
    /// 旧实现在 `scene_view` 里给旋转环的 `start_hit` 是 `entity_pos` 本身，于是
    /// `start_hit - entity_pos` 是零向量，`.normalize()` 得到 NaN，起始角度、增量
    /// 四元数、实体旋转一路全变 NaN —— 表现为"拖一下环，物体从画面上消失"。
    #[test]
    fn ring_rotate_stays_finite() {
        let ctx = test_ctx(Vec3::ZERO);
        let r = ctx.gizmo_size * 0.8; // 与 RING_RADIUS 一致

        // 起始：Z 环所在平面（XY 平面）上的 +X 方向
        let grab_screen = at(&ctx, ctx.entity_pos + Vec3::X * r);
        let start_hit =
            hit_position_on_plane(grab_screen, &ctx, Vec3::Z).expect("射线应当与旋转平面相交");
        let drag = GizmoDrag {
            part: GizmoPart::RingZ,
            start_value: Vec3::ZERO,
            start_hit,
            start_axis: 0.0,
            axis_anchor: ctx.entity_pos,
            start_screen_pos: grab_screen,
            start_rotation: Quat::IDENTITY,
        };

        // 当前：转到 45° 方向
        let cur = at(&ctx, ctx.entity_pos + (Vec3::X + Vec3::Y) * (r / 2f32.sqrt()));
        let delta = compute_transform(&drag, cur, &ctx, ToolMode::Rotate).expect("应当算出旋转");

        match delta {
            TransformDelta::Rotation(q) => {
                assert!(q.is_finite(), "旋转四元数不能是 NaN/Inf：{q:?}");
                let (axis, angle) = q.to_axis_angle();
                assert!(angle.abs() > 0.1, "转了 45° 应当有明显角度，实得 {angle}");
                assert!(
                    (axis.z.abs() - 1.0).abs() < 1e-3,
                    "绕 Z 环应当绕 Z 轴转，实得轴 {axis}"
                );
            }
            other => panic!("应当是 Rotation，实得 {other:?}"),
        }
    }

    /// **回归（跨帧）**：沿轴拖拽必须 1:1 跟手，且不能自己来回跳。
    ///
    /// 上面那几条测试都用**固定的** ctx —— 那正是放过这个 bug 的盲区。真实交互里
    /// `scene_view::handle_gizmo_interaction` 每帧都按**已被改过**的实体位置重建
    /// ctx，再把 delta 应用回去。参考线若锚在 `ctx.entity_pos`，参考线就跟着物体
    /// 一起跑，形成 `Δ_k = A_k − Δ_{k-1}` 的正反馈：极点落在 z = −1（奈奎斯特），
    /// 物体在相邻帧之间来回跳、稳态只走鼠标位移的一半。这里逐帧复刻那个循环。
    #[test]
    fn axis_translate_follows_the_mouse_across_frames() {
        let start = Vec3::new(1.0, 0.0, 2.0);
        let base = test_ctx(start); // 相机固定，屏幕投影只由 view_proj 决定
        let axis = Vec3::X;

        let grab = at(&base, on_axis(&base, axis, 0.7));
        let drag = GizmoDrag {
            part: GizmoPart::AxisX,
            start_value: start,
            start_hit: Vec3::ZERO,
            start_axis: axis_param_under_mouse(grab, &base, axis).expect("抓取点应当在轴上"),
            axis_anchor: start,
            start_screen_pos: grab,
            start_rotation: Quat::IDENTITY,
        };

        // 鼠标沿轴正向逐帧前进 0.5；每帧：重建 ctx(当前位置) → 算 delta → 写回位置。
        let mut pos = start;
        let mut offsets = Vec::new();
        for step in 1..=6 {
            let want = step as f32 * 0.5;
            // 目标世界点仍落在轴上，`at` 只依赖 view_proj，所以屏幕坐标是固定的。
            let mouse = at(&base, on_axis(&base, axis, 0.7) + axis * want);

            let mut ctx = base.clone();
            ctx.entity_pos = pos; // ← 真实交互就是这么干的（这就是自反馈的来源）

            match compute_transform(&drag, mouse, &ctx, ToolMode::Translate).expect("应当算出位移") {
                TransformDelta::Position(p) => {
                    // 世界位移只沿轴，且其余分量不动
                    assert!(
                        (p.y - start.y).abs() < 1e-3 && (p.z - start.z).abs() < 1e-3,
                        "拖 X 轴不该动 y/z，实得 {p}"
                    );
                    pos = p;
                }
                other => panic!("应当是 Position，实得 {other:?}"),
            }
            offsets.push(pos.x - start.x);
        }

        // 1) 单调不移：正反馈的自激振荡会让它在两点之间来回跳。
        for w in offsets.windows(2) {
            assert!(w[1] >= w[0] - 1e-3, "位移必须单调前进，实得 {offsets:?}");
        }
        // 2) 1:1 跟手：正反馈的稳态增益只有 1/2，会明显落后。
        for (i, got) in offsets.iter().enumerate() {
            let want = (i + 1) as f32 * 0.5;
            assert!(
                (got - want).abs() < 0.02,
                "第 {} 帧应当沿轴走 {want}，实得 {got}（整段 {offsets:?}）",
                i + 1
            );
        }
    }

    /// **回归（跨帧）**：轴缩放的参考线同样必须锚定 —— 缩放不改位置，所以旧写法
    /// 恰好也稳定，但一旦实体被别的机制挪动（父节点移动）就会重演平移的正反馈。
    /// 这里显式每帧"挪一下实体"，确保缩放结果不依赖 `ctx.entity_pos`。
    #[test]
    fn axis_scale_ignores_where_the_entity_currently_is() {
        let start = Vec3::new(0.0, 0.0, 0.0);
        let base = test_ctx(start);
        let axis = Vec3::X;
        let grab = at(&base, on_axis(&base, axis, 0.8));

        let drag = GizmoDrag {
            part: GizmoPart::AxisX,
            start_value: Vec3::ONE,
            start_hit: Vec3::ZERO,
            start_axis: axis_param_under_mouse(grab, &base, axis).expect("抓取点应当在轴上"),
            axis_anchor: start,
            start_screen_pos: grab,
            start_rotation: Quat::IDENTITY,
        };

        // 目标：鼠标退回一半距离 → 缩放 0.5。故意把 ctx.entity_pos 放到别处，
        // 模拟"实体被父节点挪走"。结果必须一模一样。
        let mouse = at(&base, on_axis(&base, axis, 0.4));
        let mut ctx = base.clone();
        ctx.entity_pos = Vec3::new(37.0, -12.0, 5.0);

        match compute_transform(&drag, mouse, &ctx, ToolMode::Scale).expect("应当算出缩放") {
            TransformDelta::Scale(s) => {
                assert!((s.x - 0.5).abs() < 0.05, "缩放应当约减半，实得 {}", s.x);
            }
            other => panic!("应当是 Scale，实得 {other:?}"),
        }
    }
}