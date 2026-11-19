//! 方向光阴影的 CPU 侧拟合：单级基线 + 4 级 CSM（与旧端逐行同式）。
//!
//! 从 `shadow.rs` 拆出来只为守住 600 行预算：这里是纯函数（可单测），
//! `shadow.rs` 只剩 GPU 深度图 / 管线。

use super::camera::{look_at_rh, mul, orthographic_rh, Mat4};
use super::shot::Bounds;

/// 阴影图边长：与旧引擎 `DEFAULT_SHADOW_RESOLUTION` 一致。
pub const SHADOW_RESOLUTION: u32 = 2048;
/// 接收端深度偏置（NDC 深度单位），旧引擎同名常量。
pub const SHADOW_DEPTH_BIAS: f32 = 0.0022;
/// 拟合半径上限：防止"有物体飞出视野"把深度精度吃光。
const MAX_SHADOW_DISTANCE: f32 = 120.0;
/// 级联层数（与旧端 `SHADOW_CASCADES` 一致）。
pub const SHADOW_CASCADES: usize = 4;
/// 级联切分的"对数 / 均匀"混合系数（practical split scheme），旧端同值。
const SHADOW_SPLIT_LAMBDA: f32 = 0.7;

/// 由光线传播方向 + 场景包围盒拟合正交光视图投影；空场景返回 `None`。
///
/// 与旧引擎逐行同式：光源在场景中心**逆着光**退开 `2r+1`，正交半宽 `r × 1.02`，
/// near / far 罩住整球。
pub fn fit_directional_shadow(direction: [f32; 3], bounds: &Bounds) -> Option<Mat4> {
    if bounds.is_empty() {
        return None;
    }
    let length =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    let dir = if length > 1e-6 {
        [
            direction[0] / length,
            direction[1] / length,
            direction[2] / length,
        ]
    } else {
        [0.0, -1.0, 0.0]
    };
    let center = bounds.center();
    let radius = bounds.radius().clamp(0.5, MAX_SHADOW_DISTANCE);
    let up = if dir[1].abs() > 0.99 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let distance = radius * 2.0 + 1.0;
    let eye = [
        center[0] - dir[0] * distance,
        center[1] - dir[1] * distance,
        center[2] - dir[2] * distance,
    ];
    let view = look_at_rh(eye, center, up);
    let extent = radius * 1.02;
    let near = (distance - radius - 1.0).max(0.01);
    let far = distance + radius + 1.0;
    let projection = orthographic_rh(-extent, extent, -extent, extent, near, far);
    Some(mul(projection, view))
}

/// 4 级方向光级联：每级的正交光视图投影 + 切分距离（相机视线方向的世界单位）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowCascades {
    pub count: usize,
    pub view_proj: [Mat4; SHADOW_CASCADES],
    /// 每级的远切分距离（`view_depth = dot(world - camera, forward)`）。
    pub splits: [f32; SHADOW_CASCADES],
    /// 相机视线方向（单位向量）：片元用它算视线深度来选层。
    pub forward: [f32; 3],
    /// 每级拟合框沿视线方向的深度区间（世界单位，相机为原点）。
    ///
    /// 它是**级间混合的回归判据**：`overlap > 0` 时非第 0 级的近端必须往前铺
    /// （区间下界变小），否则切分前的混合带在下一级里落在图外、混成一片全亮。
    /// 也是将来做逐级剔除时的剪枝依据。
    pub view_range: [(f32, f32); SHADOW_CASCADES],
}

/// practical split scheme：对数与均匀插值混合（λ = 0.7），单调、末级 = far。
pub fn cascade_splits(near: f32, far: f32, count: usize) -> Vec<f32> {
    if count == 0 {
        return Vec::new();
    }
    let near = near.max(1e-3);
    let far = far.max(near + 1e-3);
    let span = far - near;
    let min_gap = span / (count as f32 * 4.0);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let p = (i + 1) as f32 / count as f32;
        let log = near * (far / near).powf(p);
        let uniform = near + span * p;
        let mut d = SHADOW_SPLIT_LAMBDA * log + (1.0 - SHADOW_SPLIT_LAMBDA) * uniform;
        if let Some(prev) = out.last() {
            d = d.max(*prev + min_gap);
        }
        out.push(d.min(far));
    }
    if let Some(last) = out.last_mut() {
        *last = far;
    }
    out
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let len = length(a);
    if !len.is_finite() || len < 1e-9 {
        return None;
    }
    Some([a[0] / len, a[1] / len, a[2] / len])
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// 近 / 远平面各四个角。
type FrustumQuads = ([[f32; 3]; 4], [[f32; 3]; 4]);

/// 透视矩阵求逆后反推近 / 远平面的四个角（NDC z = 0 / 1，与 `perspective_rh_zo` 一致）。
fn frustum_corners(inv_view_proj: Mat4) -> Option<FrustumQuads> {
    let mut near = [[0.0; 3]; 4];
    let mut far = [[0.0; 3]; 4];
    for (index, (x, y)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        for (plane, z, target) in [(0, 0.0f32, &mut near), (1, 1.0f32, &mut far)] {
            let _ = plane;
            let clip = super::camera::transform_point(inv_view_proj, [x, y, z, 1.0]);
            if !clip[3].is_finite() || clip[3].abs() < 1e-6 {
                return None;
            }
            target[index] = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
        }
    }
    Some((near, far))
}

/// 按旧端同式算 4 级级联：相机视锥沿视线切 4 段，逐段拟合紧致正交光视锥。
///
/// `overlap`（0.0~1.0）是**级间混合**的重叠量：非零时每一级（第 0 级除外）的
/// 近端往前多铺 `overlap × 本级跨度`，于是切分前那条混合带里的片元在两级里都
/// 采得到（不铺的话下一级的图在切分处根本没覆盖，混进去等于混一团"全亮"）。
/// `0.0` 时拟合结果与引入混合前逐位一致。
#[allow(clippy::too_many_arguments)]
pub fn cascade_shadow_matrices(
    inv_view_proj: Mat4,
    camera_pos: [f32; 3],
    direction: [f32; 3],
    scene: &Bounds,
    resolution: u32,
    count: usize,
    overlap: f32,
) -> Option<ShadowCascades> {
    if count == 0 || count > SHADOW_CASCADES || scene.is_empty() {
        return None;
    }
    let (near_q, far_q) = frustum_corners(inv_view_proj)?;
    let quad_center = |q: &[[f32; 3]; 4]| {
        [
            (q[0][0] + q[1][0] + q[2][0] + q[3][0]) / 4.0,
            (q[0][1] + q[1][1] + q[2][1] + q[3][1]) / 4.0,
            (q[0][2] + q[1][2] + q[2][2] + q[3][2]) / 4.0,
        ]
    };
    let near_center = quad_center(&near_q);
    let far_center = quad_center(&far_q);
    let forward = normalize(sub(far_center, near_center))?;
    let near_d = dot(sub(near_center, camera_pos), forward).max(0.0);
    let far_plane_d = dot(sub(far_center, camera_pos), forward);
    if !far_plane_d.is_finite() || far_plane_d <= near_d + 1e-3 {
        return None;
    }
    // 覆盖距离：场景在视线方向上用到的最远处（旧端同式）。
    let mut scene_far = f32::NEG_INFINITY;
    for corner in scene.corners() {
        scene_far = scene_far.max(dot(sub(corner, camera_pos), forward));
    }
    let mut far_d = scene_far.min(MAX_SHADOW_DISTANCE).min(far_plane_d);
    if !far_d.is_finite() || far_d < near_d + 0.5 {
        far_d = far_plane_d.min(MAX_SHADOW_DISTANCE);
    }
    if far_d <= near_d + 1e-3 {
        return None;
    }
    let splits = cascade_splits(near_d, far_d, count);
    let dir = normalize(direction).unwrap_or([0.0, -1.0, 0.0]);
    let up = if dir[1].abs() > 0.99 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let span = far_plane_d - near_d;
    let at = |d: f32| ((d - near_d) / span).clamp(0.0, 1.0);
    let res = resolution.max(1) as f32;
    let mut out = ShadowCascades {
        count,
        view_proj: [super::camera::IDENTITY; SHADOW_CASCADES],
        splits: [0.0; SHADOW_CASCADES],
        forward,
        view_range: [(0.0, 0.0); SHADOW_CASCADES],
    };
    for c in 0..count {
        let d0 = if c == 0 { near_d } else { splits[c - 1] };
        let d1 = splits[c];
        // 级间混合的重叠：本级（非第 0 级）的近端往前多铺一点，让切分前的
        // 混合带在两级里都采得到（`overlap = 0` 时也是 0，逐位一致）。
        let overlap_d = if c > 0 && overlap.is_finite() && overlap > 0.0 {
            (d1 - d0) * overlap
        } else {
            0.0
        };
        let (t0, t1) = (at(d0 - overlap_d), at(d1));
        let mut corners = [[0.0f32; 3]; 8];
        for k in 0..4 {
            corners[k] = lerp(near_q[k], far_q[k], t0);
            corners[k + 4] = lerp(near_q[k], far_q[k], t1);
        }
        let center = [
            corners.iter().map(|c| c[0]).sum::<f32>() / 8.0,
            corners.iter().map(|c| c[1]).sum::<f32>() / 8.0,
            corners.iter().map(|c| c[2]).sum::<f32>() / 8.0,
        ];
        let radius = corners
            .iter()
            .fold(0.0f32, |m, c| m.max(length(sub(*c, center))))
            .clamp(0.5, MAX_SHADOW_DISTANCE);
        let eye = [
            center[0] - dir[0] * (radius * 2.0 + 1.0),
            center[1] - dir[1] * (radius * 2.0 + 1.0),
            center[2] - dir[2] * (radius * 2.0 + 1.0),
        ];
        let view = look_at_rh(eye, center, up);
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for corner in corners {
            let p = super::camera::transform_point(view, [corner[0], corner[1], corner[2], 1.0]);
            if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
                return None;
            }
            for axis in 0..3 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
        // 正方形范围 + 对齐到整数纹素（相位稳定 ⇒ 相机移动时影子边缘不抖）。
        let half_raw = ((max[0] - min[0]).max(max[1] - min[1]) * 0.5).max(0.05);
        let texel_raw = 2.0 * half_raw / res;
        let half = (half_raw / texel_raw).ceil() * texel_raw;
        let texel = 2.0 * half / res;
        let cx = ((min[0] + max[0]) * 0.5 / texel).floor() * texel;
        let cy = ((min[1] + max[1]) * 0.5 / texel).floor() * texel;
        // 视空间前方是 -z；glam 的正交投影要正的距离，所以 near / far 取 -max.z / -min.z。
        let pad = radius * 0.05 + 1.0;
        let near_z = (-max[2] - pad).max(0.01);
        let far_z = -min[2] + pad;
        if far_z <= near_z + 1e-3 {
            return None;
        }
        let projection = orthographic_rh(cx - half, cx + half, cy - half, cy + half, near_z, far_z);
        out.view_proj[c] = mul(projection, view);
        out.splits[c] = d1;
        // 拟合框沿视线方向的跨度：反推 8 个角点再投影到 `forward`。斜射光下框沿
        // 光线方向被拉长，能装下视线深度之外的投影物，所以这个区间只用来做
        // 剪枝与回归，不当作"框内"的判据。
        let inverse = super::camera::invert(out.view_proj[c])?;
        let mut dmin = f32::INFINITY;
        let mut dmax = f32::NEG_INFINITY;
        for i in 0..8 {
            let ndc = [
                if i & 1 == 0 { -1.0 } else { 1.0 },
                if i & 2 == 0 { -1.0 } else { 1.0 },
                if i & 4 == 0 { 0.0 } else { 1.0 },
                1.0,
            ];
            let point = super::camera::transform_point(inverse, ndc);
            if !point[0].is_finite() || !point[1].is_finite() || !point[2].is_finite() {
                return None;
            }
            let d = dot(sub([point[0], point[1], point[2]], camera_pos), forward);
            dmin = dmin.min(d);
            dmax = dmax.max(d);
        }
        if !dmin.is_finite() || !dmax.is_finite() {
            return None;
        }
        out.view_range[c] = (dmin, dmax);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_are_monotonic_and_end_at_far() {
        let splits = cascade_splits(0.1, 120.0, SHADOW_CASCADES);
        assert_eq!(splits.len(), SHADOW_CASCADES);
        for pair in splits.windows(2) {
            assert!(pair[1] > pair[0], "切分必须单调递增：{splits:?}");
        }
        assert!((splits[SHADOW_CASCADES - 1] - 120.0).abs() < 1e-3);
    }

    #[test]
    fn single_level_fit_covers_all_scene_corners() {
        let bounds = Bounds {
            min: [-3.0, -0.5, -2.0],
            max: [4.0, 6.0, 3.0],
        };
        let matrix = fit_directional_shadow([0.45, -1.0, 0.35], &bounds).expect("拟合成功");
        let inverse = super::super::camera::invert(matrix).expect("可逆");
        for corner in bounds.corners() {
            let clip = super::super::camera::transform_point(
                matrix,
                [corner[0], corner[1], corner[2], 1.0],
            );
            let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
            assert!(
                ndc[0].abs() <= 1.02 && ndc[1].abs() <= 1.02,
                "角点应在正交框内"
            );
            let back =
                super::super::camera::transform_point(inverse, [ndc[0], ndc[1], ndc[2], 1.0]);
            assert!((back[0] - corner[0]).abs() < 0.1);
        }
    }

    /// **级间混合的重叠拟合**：`overlap > 0` 时每个非第 0 级的拟合框必须往
    /// 相机方向多铺 `overlap × 本级跨度`，让切分前的混合带在下一级的图里也采得到。
    /// `overlap = 0`（老路径）时近端不动 —— 这条严格不等式在恒 0 的实现下是红的。
    #[test]
    fn cascade_overlap_extends_the_fit_toward_the_camera() {
        // 相机在场景外俯视（与旧端同名用例同一组探针）：视线深度全为正，
        // 拟合框近端的变化才不会被"框伸到相机背后"的角点盖住。
        let eye = [0.0, 6.0, 24.0];
        let view = super::super::camera::look_at_rh(eye, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let projection =
            super::super::camera::perspective_rh_zo(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 200.0);
        let inverse = super::super::camera::invert(mul(projection, view)).expect("可逆");
        let bounds = Bounds {
            min: [-10.0, -0.5, -10.0],
            max: [10.0, 6.0, 10.0],
        };
        let fits = |overlap: f32| {
            cascade_shadow_matrices(
                inverse,
                eye,
                [0.4, -1.0, 0.3],
                &bounds,
                SHADOW_RESOLUTION,
                SHADOW_CASCADES,
                overlap,
            )
            .expect("级联拟合成功")
        };
        let plain = fits(0.0);
        let blended = fits(0.25);
        // 切分点不能动：重叠改的是拟合框，不是层级划分。
        assert_eq!(plain.splits, blended.splits);
        // 第 0 级没有"上一级"，重叠对它无效 —— 矩阵与深度区间都逐位一致。
        assert_eq!(plain.view_proj[0], blended.view_proj[0]);
        assert_eq!(plain.view_range[0], blended.view_range[0]);
        for c in 1..SHADOW_CASCADES {
            // 拟合框的近端往相机方向铺了：沿视线的最小覆盖深度**严格变小**。
            // 只比"严格变小"而不比具体幅度的原因：拟合框还被纹素吸附与半径余量
            // 撑开，框角点往相机背后的投影会让近端变化远小于 `overlap × 跨度`。
            assert!(
                blended.view_range[c].0 < plain.view_range[c].0,
                "第 {c} 级的近端必须更靠近相机：{:?} vs {:?}",
                blended.view_range[c],
                plain.view_range[c]
            );
        }
    }

    /// **重叠量的垃圾值当 0**：负数 / NaN / 无穷与 `0.0` 逐位一致（旧端
    /// `cascade_overlap_garbage_is_treated_as_zero` 同口径）。少了 `is_finite` 这半个守卫，
    /// `+inf` 会把近端推到 0（NaN 要整层守卫都缺了才会漏进来），画面出现整条假阴影。
    #[test]
    fn cascade_overlap_garbage_is_treated_as_zero() {
        let eye = [0.0, 6.0, 24.0];
        let view = super::super::camera::look_at_rh(eye, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let projection =
            super::super::camera::perspective_rh_zo(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 200.0);
        let inverse = super::super::camera::invert(mul(projection, view)).expect("相机求逆");
        let bounds = Bounds {
            min: [-10.0, -0.5, -10.0],
            max: [10.0, 6.0, 10.0],
        };
        let fits = |overlap: f32| {
            cascade_shadow_matrices(
                inverse,
                eye,
                [0.4, -1.0, 0.3],
                &bounds,
                SHADOW_RESOLUTION,
                SHADOW_CASCADES,
                overlap,
            )
            .expect("级联拟合成功")
        };
        let plain = fits(0.0);
        for overlap in [-0.5f32, f32::NAN, f32::INFINITY] {
            let garbage = fits(overlap);
            assert!(
                plain.splits == garbage.splits,
                "垃圾重叠量不该改分界点：{overlap}"
            );
            for c in 0..SHADOW_CASCADES {
                assert!(
                    plain.view_proj[c] == garbage.view_proj[c],
                    "垃圾重叠量不该改第 {c} 级投影：{overlap}"
                );
                assert!(
                    plain.view_range[c] == garbage.view_range[c],
                    "垃圾重叠量不该改第 {c} 级深度范围：{overlap}"
                );
            }
        }
    }

    #[test]
    fn cascades_cover_the_camera_frustum() {
        let view =
            super::super::camera::look_at_rh([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
        let projection =
            super::super::camera::perspective_rh_zo(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 1000.0);
        let inverse = super::super::camera::invert(mul(projection, view)).expect("可逆");
        let bounds = Bounds {
            min: [-6.0, -1.0, -30.0],
            max: [6.0, 4.0, -1.0],
        };
        let cascades = cascade_shadow_matrices(
            inverse,
            [0.0, 0.0, 0.0],
            [0.45, -1.0, 0.35],
            &bounds,
            SHADOW_RESOLUTION,
            SHADOW_CASCADES,
            0.0,
        )
        .expect("级联拟合成功");
        assert_eq!(cascades.count, SHADOW_CASCADES);
        assert!((length(cascades.forward) - 1.0).abs() < 1e-3);
        // 级联必须**一级比一级远**：切分单调、矩阵各不相同。
        for c in 1..SHADOW_CASCADES {
            assert!(cascades.splits[c] > cascades.splits[c - 1]);
            assert_ne!(cascades.view_proj[c], cascades.view_proj[c - 1]);
        }
        // 每级深度区间中点（相机轴线上）必须落在该级正交框的 [0,1] 深度范围内。
        let (near_q, _) = frustum_corners(inverse).expect("角点");
        let _ = near_q;
        for c in 0..SHADOW_CASCADES {
            let d0 = if c == 0 { 0.1 } else { cascades.splits[c - 1] };
            let d1 = cascades.splits[c];
            let mid = (d0 + d1) * 0.5;
            let point = [
                cascades.forward[0] * mid,
                cascades.forward[1] * mid,
                cascades.forward[2] * mid,
            ];
            let clip = super::super::camera::transform_point(
                cascades.view_proj[c],
                [point[0], point[1], point[2], 1.0],
            );
            let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
            assert!(
                ndc[0].abs() <= 1.02 && ndc[1].abs() <= 1.02 && (0.0..=1.0).contains(&ndc[2]),
                "级联 {c} 没装下自己深度中点的轴线点：{ndc:?}"
            );
        }
    }
}
