//! 视锥（frustum）与包围体测试 —— 每帧剔除的纯几何内核。
//!
//! # 为什么自己写
//!
//! 剔除的判据只有一句话："这个物体的世界包围体与视锥相交吗？" 把它写成
//! **不依赖 GPU、不依赖场景**的纯函数，才能对边界情形做穷举单测（点在
//! 相机背后、盒子恰好跨过近平面、矩阵退化……）。这些用例在渲染测试里
//! 几乎不可能构造，一旦错判的表现却只是"物体在屏幕边缘忽然消失"。
//!
//! # 平面约定
//!
//! 从 `view_proj` 按 Gribb–Hartmann 提六个平面，**法线朝视锥内侧**，
//! 于是"在内侧" ⇔ `dot(n, p) + d >= 0`。六个平面都归一化了：这样点积
//! 就是**世界单位的有符号距离**，球体测试可以直接拿半径比。
//!
//! wgpu 的 NDC 深度是 `[0, 1]`（`glam::Mat4::perspective_rh` 的约定），
//! 所以近平面取第 3 行、远平面取 `row3 - row2`；换成 OpenGL 的 `[-1, 1]`
//! 约定时近平面要改成 `row3 + row2`。这条差异如果搞反，表现是**近处物体
//! 整片消失**（相机贴脸时最明显），因此单测里专门钉了"相机原点附近的
//! 点必须可见"。
//!
//! # 保守性
//!
//! [`Frustum::from_view_proj`] 返回 `Option`：矩阵退化（NaN / 零行）时给
//! `None`，调用方必须**当作"不剔除"**。宁可多画，也不能因为一个坏相机
//! 把整场景判成不可见 —— 后者是"黑屏"，前者只是浪费。

use xgame_math::glam::{Mat4, Vec3, Vec4};

use crate::gltf_loader::Bounds;

/// 由视图投影矩阵定义的六个半空间。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    /// left / right / bottom / top / near / far，法线朝内且已归一化。
    planes: [Vec4; 6],
}

impl Frustum {
    /// 从 `view_proj`（世界 → 裁剪空间）提取视锥平面。
    ///
    /// 返回 `None` 表示矩阵退化（含 NaN / Inf，或某个平面法线长度为零）——
    /// 调用方应关闭剔除，而不是把整个场景判掉。
    pub fn from_view_proj(view_proj: &Mat4) -> Option<Self> {
        let row0 = view_proj.row(0);
        let row1 = view_proj.row(1);
        let row2 = view_proj.row(2);
        let row3 = view_proj.row(3);

        // 裁剪空间条件：-w <= x <= w、-w <= y <= w、0 <= z <= w。
        // 每条不等式移到左侧后就是"平面 · 齐次点 >= 0"。
        let raw = [
            row3 + row0, // left:   x >= -w
            row3 - row0, // right:  x <=  w
            row3 + row1, // bottom: y >= -w
            row3 - row1, // top:    y <=  w
            row2,        // near:   z >=  0（wgpu 的深度范围从 0 开始）
            row3 - row2, // far:    z <=  w
        ];

        let mut planes = [Vec4::ZERO; 6];
        for (slot, plane) in planes.iter_mut().zip(raw) {
            if !plane.is_finite() {
                return None;
            }
            let len = plane.truncate().length();
            // 退化矩阵（比如全零）会给出零法线：任何点积都是 0，
            // 语义上"全在内侧"，但那不是一幅可信的视图，直接判失败。
            if !len.is_finite() || len < 1e-6 {
                return None;
            }
            *slot = plane / len;
        }
        Some(Self { planes })
    }

    /// 六个平面的原始数据（`xyz` = 归一化内法线，`w` = 偏移）。
    ///
    /// 给测试与调试用：剔除错判时，能逐平面报出"被哪一个裁掉了"。
    pub fn planes(&self) -> [[f32; 4]; 6] {
        self.planes.map(|p| p.to_array())
    }

    /// 点是否在视锥内（含边界）。测试 / 调试用；剔除走体积版本。
    pub fn contains_point(&self, point: Vec3) -> bool {
        self.planes
            .iter()
            .all(|p| p.truncate().dot(point) + p.w >= 0.0)
    }

    /// 球体是否与视锥相交（保守：相切算相交）。
    pub fn intersects_sphere(&self, center: Vec3, radius: f32) -> bool {
        let r = radius.max(0.0);
        self.planes
            .iter()
            .all(|p| p.truncate().dot(center) + p.w >= -r)
    }

    /// 轴对齐盒是否与视锥相交（保守：完全在外侧才返回 `false`）。
    ///
    /// 每个平面只测**最靠内侧的那个角点**（p-vertex）：若它都在平面外侧，
    /// 整个盒子必然在外侧。相比"8 个角点都测一遍"省 7/8 的点积，且与
    /// 角点法结果等价。
    pub fn intersects_aabb(&self, min: Vec3, max: Vec3) -> bool {
        for p in &self.planes {
            let n = p.truncate();
            let vertex = Vec3::new(
                if n.x >= 0.0 { max.x } else { min.x },
                if n.y >= 0.0 { max.y } else { min.y },
                if n.z >= 0.0 { max.z } else { min.z },
            );
            if n.dot(vertex) + p.w < 0.0 {
                return false;
            }
        }
        true
    }

    /// [`Bounds`] 版本的 AABB 测试。空盒视为"不相交"（它没有可画的东西）。
    pub fn intersects_bounds(&self, bounds: &Bounds) -> bool {
        if bounds.is_empty() {
            return false;
        }
        self.intersects_aabb(bounds.min, bounds.max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_math::glam::Quat;

    /// 站在原点、朝 -Z 看、90° 视场、近 0.1 远 100 的相机。
    fn camera() -> Mat4 {
        let view = Mat4::look_at_rh(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), Vec3::Y);
        let proj = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        proj * view
    }

    #[test]
    fn point_in_front_is_visible_and_point_behind_is_not() {
        let f = Frustum::from_view_proj(&camera()).expect("标准透视矩阵必须能提平面");
        assert!(
            f.contains_point(Vec3::new(0.0, 0.0, -5.0)),
            "正前方的点可见"
        );
        assert!(
            !f.contains_point(Vec3::new(0.0, 0.0, 5.0)),
            "相机背后的点不可见"
        );
    }

    #[test]
    fn near_plane_follows_wgpu_depth_convention() {
        // 相机原点与近平面之间的点必须被裁掉；差一点跨进近平面的点必须留下。
        let f = Frustum::from_view_proj(&camera()).unwrap();
        assert!(
            !f.contains_point(Vec3::new(0.0, 0.0, -0.05)),
            "近平面以内的点不可见"
        );
        assert!(
            f.contains_point(Vec3::new(0.0, 0.0, -0.2)),
            "近平面之外可见"
        );
        assert!(
            !f.contains_point(Vec3::new(0.0, 0.0, 5.0)),
            "相机背后的点不可见"
        );
    }

    #[test]
    fn far_plane_culls_runaway_points() {
        let f = Frustum::from_view_proj(&camera()).unwrap();
        assert!(f.contains_point(Vec3::new(0.0, 0.0, -50.0)));
        assert!(
            !f.contains_point(Vec3::new(0.0, 0.0, -150.0)),
            "远平面之外不可见"
        );
    }

    #[test]
    fn aabb_crossing_the_near_plane_is_kept() {
        let f = Frustum::from_view_proj(&camera()).unwrap();
        // 盒子从相机背后（z = +1）一直伸到前方 z = -2：与视锥相交。
        assert!(f.intersects_aabb(Vec3::new(-0.5, -0.5, -2.0), Vec3::new(0.5, 0.5, 1.0)));
    }

    #[test]
    fn aabb_fully_behind_camera_is_culled() {
        let f = Frustum::from_view_proj(&camera()).unwrap();
        assert!(!f.intersects_aabb(Vec3::new(-0.5, -0.5, 1.0), Vec3::new(0.5, 0.5, 2.0)));
    }

    #[test]
    fn aabb_fully_outside_the_side_is_culled() {
        let f = Frustum::from_view_proj(&camera()).unwrap();
        // 90° 视场：z=-10 处半宽为 10；盒子在 x=20 处完全在外侧。
        assert!(!f.intersects_aabb(Vec3::new(19.0, -0.5, -11.0), Vec3::new(21.0, 0.5, -9.0)));
        // 贴着边缘的盒子必须留下（保守性）。
        assert!(f.intersects_aabb(Vec3::new(8.9, -0.5, -11.0), Vec3::new(9.1, 0.5, -9.0)));
    }

    #[test]
    fn sphere_test_matches_points_and_radius() {
        let f = Frustum::from_view_proj(&camera()).unwrap();
        assert!(f.intersects_sphere(Vec3::new(0.0, 0.0, -5.0), 0.1));
        // 圆心在右边界外一点点，但半径把它拉回来了。
        assert!(f.intersects_sphere(Vec3::new(5.5, 0.0, -5.0), 1.0));
        assert!(!f.intersects_sphere(Vec3::new(8.0, 0.0, -5.0), 1.0));
    }

    #[test]
    fn orthographic_frustum_contains_whole_scene_box() {
        // 阴影拟合用的正交矩阵：盒内任意点都必须在视锥内。
        let proj = Mat4::orthographic_rh(-10.0, 10.0, -10.0, 10.0, 0.1, 50.0);
        let view = Mat4::look_at_rh(Vec3::new(0.0, 20.0, 0.0), Vec3::ZERO, Vec3::Z);
        let f = Frustum::from_view_proj(&(proj * view)).unwrap();
        let bounds = Bounds {
            min: Vec3::new(-5.0, -5.0, -5.0),
            max: Vec3::new(5.0, 5.0, 5.0),
        };
        assert!(f.intersects_bounds(&bounds));
        for c in bounds.corners() {
            assert!(f.contains_point(c), "正交视锥必须罩住整个盒：{c:?}");
        }
    }

    #[test]
    fn rotated_aabb_and_reference_corner_transform_agree() {
        // 解析式与 8 角点参考实现必须一致（这是把 per-frame 路径切到快速版的依据）。
        let m = Mat4::from_rotation_translation(
            Quat::from_euler(xgame_math::glam::EulerRot::XYZ, 0.4, -1.1, 0.7),
            Vec3::new(3.0, -2.0, 5.0),
        ) * Mat4::from_scale(Vec3::new(1.5, 0.5, 2.0));
        let b = Bounds {
            min: Vec3::new(-1.0, -2.0, -0.3),
            max: Vec3::new(2.0, 0.5, 0.8),
        };
        let fast = b.transformed_affine(&m);
        let slow = b.transformed(&m);
        for (a, e) in [(fast.min, slow.min), (fast.max, slow.max)] {
            assert!(
                (a - e).abs().max_element() < 1e-5,
                "解析式 {a:?} 与角点法 {e:?} 必须一致"
            );
        }
    }

    #[test]
    fn degenerate_matrix_disables_culling() {
        assert!(Frustum::from_view_proj(&Mat4::ZERO).is_none());
        let nan = Mat4::from_cols(
            Vec4::new(f32::NAN, 0.0, 0.0, 0.0),
            Vec4::W,
            Vec4::Z,
            Vec4::W,
        );
        assert!(Frustum::from_view_proj(&nan).is_none());
        // 退化矩阵里的"空盒"也不能被误判成可见（调用方本来就不会用，
        // 但 `intersects_bounds` 的语义要自洽）。
        let f = Frustum::from_view_proj(&camera()).unwrap();
        assert!(!f.intersects_bounds(&Bounds::EMPTY));
    }
}
