//! 点光立方体阴影：6 面深度图 + 主轴参考深度（与旧端 `scene_pipeline.rs` 同口径）。
//!
//! 旧端只认「第一盏 `casts_shadow` 且 `range > 0` 的点光」，六面各跑一次 depth-only
//! pass，主着色器再用 `texture_depth_cube` + `textureSampleCompareLevel` 做 3×3 PCF。
//! 这里把 CPU 侧（矩阵 / 参考深度 / uniform 打包）与 GPU 资源（立方体深度图 + 面视图）
//! 放在一处：主着色器与 XGC 的 `shadow.rs` 都只消费这里的产物。
//!
//! 关键口径：参考深度用**主轴分量** `max(|dx|,|dy|,|dz|)` 而不是欧氏距离。
//! 立方体投影的 z/w 恰好是 `axis` 的函数，用 `length()` 会让斜面/角落整片变黑
//! （旧端踩过、注释里写明）；另外六面的 u 轴都要镜像（`scale(-1,1,1)`），否则
//! 影子会翻到光源的另一侧。

use super::camera::{self, Mat4};
use super::gpu::GpuContext;
use super::lights::MAX_LIGHTS;
use xgc_world::{LightKind, SceneEnvironment, SceneLight};

/// 立方体面数：+X / −X / +Y / −Y / +Z / −Z。
pub(crate) const POINT_SHADOW_FACES: usize = 6;

/// 采样比较偏置：旧端 `POINT_SHADOW_DEPTH_BIAS`。
pub(crate) const POINT_SHADOW_DEPTH_BIAS: f32 = 0.0035;

/// 帧 uniform 里的点光阴影段：光位置（16 B）+ 参数（16 B）。
///
/// 六面视图投影**不进**主帧 uniform：接收端只按方向选面采样，直接吃矩阵的只有阴影 pass，
/// 那里每个面一张 64 B 的帧缓冲。旧端把矩阵塞进光照 uniform 是因为阴影 pass 复用同一个
/// buffer（按 `face * 步长` 取段），XGC 拆出独立阴影帧后再带一份就是每帧白传 384 B。
pub(crate) const POINT_SHADOW_UNIFORM_BYTES: u64 = 32;

/// 本帧要投影的那盏点光：六面矩阵 + 光位置 + 近远平面。
///
/// `Copy` 是刻意的：它要同时进帧 uniform（只取光位置与参数）与 `ShadowTarget::render`
/// （只取六面矩阵），按值传省掉生命周期纠缠（416 B 也就一个结构体拷贝的量级）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PointShadow {
    matrices: [Mat4; POINT_SHADOW_FACES],
    light_position: [f32; 3],
    near: f32,
    far: f32,
    resolution: u32,
}

impl PointShadow {
    /// 按 `scene_shading.wgsl` 的 `PointShadow` 结构逐字段打包（只有光位置与参数）。
    pub(crate) fn uniform_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(POINT_SHADOW_UNIFORM_BYTES as usize);
        for value in [
            self.light_position[0],
            self.light_position[1],
            self.light_position[2],
            self.far,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        // x = near，y = 纹理步长（与旧端 `2.0 / resolution` 同义），z = 启用标记，
        // w = 比较偏置。
        for value in [
            self.near,
            2.0 / self.resolution as f32,
            1.0,
            POINT_SHADOW_DEPTH_BIAS,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        debug_assert_eq!(bytes.len(), POINT_SHADOW_UNIFORM_BYTES as usize);
        bytes
    }

    /// 进阴影 pass 的六面视图投影（顺序固定为 +X/−X/+Y/−Y/+Z/−Z）。
    pub(crate) fn matrices(&self) -> &[Mat4; POINT_SHADOW_FACES] {
        &self.matrices
    }
}

/// 从环境里挑出本帧投射立方体阴影的点光。
///
/// 判据与旧端 `pick_point_shadow_slot` 一致，且**只在前 8 盏有效灯里找**：打包表之外
/// 的灯根本不参与着色，为它跑 6 个 pass 是纯浪费（与 `primary_shadow_light` 同理）。
pub(crate) fn pick_light(environment: &SceneEnvironment) -> Option<&SceneLight> {
    environment
        .lights
        .iter()
        .filter(|light| light.is_active())
        .take(MAX_LIGHTS)
        .find(|light| {
            light.kind == LightKind::Point
                && light.casts_shadow
                && light.range.is_finite()
                && light.range > 0.0
        })
}

/// 由灯参数推出六面矩阵与近远平面（旧端 `render` 里的每帧准备同式）。
pub(crate) fn plan(light: &SceneLight, resolution: u32) -> PointShadow {
    let far = if light.range.is_finite() {
        light.range.max(0.05)
    } else {
        0.05
    };
    let near = (far * 0.005).max(0.02);
    PointShadow {
        matrices: point_shadow_matrices(light.position, near, far),
        light_position: light.position,
        near,
        far,
        resolution,
    }
}

/// 六面视图投影：`flip_u * perspective * look_at`。
///
/// 方向 / up 向量照 WebGPU 的立方体贴图映射表挑（旧端 `DIRS` / `UPS` 同表）；
/// up 不能与视线共线，否则 `look_at` 退化，所以 ±Y 面用 ∓Z 当 up。
pub(crate) fn point_shadow_matrices(
    center: [f32; 3],
    near: f32,
    far: f32,
) -> [Mat4; POINT_SHADOW_FACES] {
    let near = if near.is_finite() {
        near.max(1e-3)
    } else {
        1e-3
    };
    let far = if far.is_finite() {
        far.max(near + 1e-3)
    } else {
        near + 1e-3
    };
    let projection = camera::perspective_rh_zo(std::f32::consts::FRAC_PI_2, 1.0, near, far);
    let directions = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ];
    let ups = [
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, -1.0],
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    // 六面都镜像 u：不翻的话 ±X / ±Z 面上的影子会左右颠倒。
    let flip_u: Mat4 = [
        -1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0, //
    ];
    std::array::from_fn(|face| {
        let direction = directions[face];
        let target = [
            center[0] + direction[0],
            center[1] + direction[1],
            center[2] + direction[2],
        ];
        let view = camera::look_at_rh(center, target, ups[face]);
        camera::mul(flip_u, camera::mul(projection, view))
    })
}

/// 立方体面的参考深度：`(far / (far - near)) * (1 - near / axis)`。
///
/// 与 `perspective_rh_zo` 的 z/w 同一尺度。`axis <= near`（贴脸或背面）返回 0 是
/// **单测哨兵**：生产着色器里同条件返回 1.0（全亮），这样"贴脸"不会被误判成深度 0
/// 的遮挡物。这份 CPU 镜像只给单测钉住数值口径。
#[cfg(test)]
pub(crate) fn point_shadow_reference_depth(axis_distance: f32, near: f32, far: f32) -> f32 {
    let near = if near.is_finite() {
        near.max(1e-3)
    } else {
        1e-3
    };
    let far = if far.is_finite() {
        far.max(near + 1e-3)
    } else {
        near + 1e-3
    };
    if !axis_distance.is_finite() || axis_distance <= near {
        return 0.0;
    }
    (far / (far - near)) * (1.0 - near / axis_distance)
}

/// 立方体深度图 + 六面视图（设备生命周期内复用，与方向光阴影并列）。
pub(crate) struct PointShadowTarget {
    /// 主着色器采样用：`texture_depth_cube`。
    view: wgpu::TextureView,
    /// 每个面开一次 depth pass 用。
    faces: [wgpu::TextureView; POINT_SHADOW_FACES],
    /// 没有点光阴影时绑的 1×1 立方体：绑定组布局固定，不能"这次不绑"。
    fallback: wgpu::TextureView,
}

impl PointShadowTarget {
    pub(crate) fn new(context: &GpuContext, resolution: u32) -> Self {
        let device = context.device();
        let map = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xgc-point-shadow-map"),
            size: wgpu::Extent3d {
                width: resolution,
                height: resolution,
                depth_or_array_layers: POINT_SHADOW_FACES as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = map.create_view(&wgpu::TextureViewDescriptor {
            label: Some("xgc-point-shadow-cube"),
            format: Some(wgpu::TextureFormat::Depth32Float),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            aspect: wgpu::TextureAspect::DepthOnly,
            base_mip_level: 0,
            mip_level_count: Some(1),
            base_array_layer: 0,
            array_layer_count: Some(POINT_SHADOW_FACES as u32),
        });
        let faces = std::array::from_fn(|face| {
            map.create_view(&wgpu::TextureViewDescriptor {
                label: Some("xgc-point-shadow-face"),
                format: Some(wgpu::TextureFormat::Depth32Float),
                dimension: Some(wgpu::TextureViewDimension::D2),
                aspect: wgpu::TextureAspect::DepthOnly,
                base_mip_level: 0,
                mip_level_count: Some(1),
                base_array_layer: face as u32,
                array_layer_count: Some(1),
            })
        });
        let fallback = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xgc-point-shadow-fallback"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: POINT_SHADOW_FACES as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let fallback = fallback.create_view(&wgpu::TextureViewDescriptor {
            label: Some("xgc-point-shadow-fallback-cube"),
            format: Some(wgpu::TextureFormat::Depth32Float),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            aspect: wgpu::TextureAspect::DepthOnly,
            base_mip_level: 0,
            mip_level_count: Some(1),
            base_array_layer: 0,
            array_layer_count: Some(POINT_SHADOW_FACES as u32),
        });
        Self {
            view,
            faces,
            fallback,
        }
    }

    /// 主 pass 采样用：关着时返回 1×1 假图（着色器按 `params.z` 整段跳过）。
    pub(crate) fn source(&self, enabled: bool) -> &wgpu::TextureView {
        if enabled {
            &self.view
        } else {
            &self.fallback
        }
    }

    /// 阴影 pass 用：第 `face` 面的 2D 视图。
    pub(crate) fn face(&self, face: usize) -> &wgpu::TextureView {
        &self.faces[face]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一条立方体查表用例：方向、应当命中的面、sc / tc 的线性轴。
    type LookupCase = ([f32; 3], usize, [f32; 3], [f32; 3]);

    fn project(matrix: Mat4, point: [f32; 3]) -> [f32; 4] {
        camera::transform_point(matrix, [point[0], point[1], point[2], 1.0])
    }

    #[test]
    fn each_face_looks_down_its_own_axis() {
        let center = [1.0, 2.0, 3.0];
        let matrices = point_shadow_matrices(center, 0.02, 10.0);
        let directions = [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ];
        for (face, direction) in directions.iter().enumerate() {
            let point = [
                center[0] + direction[0],
                center[1] + direction[1],
                center[2] + direction[2],
            ];
            let clip = project(matrices[face], point);
            assert!(
                clip[3] > 0.0,
                "第 {face} 面的正前方必须在相机前：{}",
                clip[3]
            );
            let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
            assert!(
                ndc[0].abs() <= 1.0 && ndc[1].abs() <= 1.0 && (0.0..=1.0).contains(&ndc[2]),
                "第 {face} 面的轴上点必须落在该面视锥内：{ndc:?}"
            );
        }
    }

    #[test]
    fn the_u_axis_is_mirrored_so_shadows_do_not_swap_sides() {
        let center = [0.0, 0.0, 0.0];
        let matrices = point_shadow_matrices(center, 0.02, 10.0);
        // +X 面：look_at(+X, up=+Y) 的右手系里世界 +Z 落在 NDC 右侧，`flip_u` 镜像后
        // 必须落到左侧；-Z 则在右侧。两侧恰好对调，就是"影子没有翻到另一边"。
        let front = project(matrices[0], [1.0, 0.0, 1.0]);
        let back = project(matrices[0], [1.0, 0.0, -1.0]);
        assert!(front[3] > 0.0 && back[3] > 0.0, "+X 面正前方");
        let front_u = front[0] / front[3] * 0.5 + 0.5;
        let back_u = back[0] / back[3] * 0.5 + 0.5;
        assert!(
            front_u < 0.5 && back_u > 0.5,
            "u 轴必须镜像，否则影子会翻到另一侧：+Z u = {front_u}、-Z u = {back_u}"
        );
    }

    #[test]
    fn reference_depth_follows_the_perspective_scale() {
        let near = 0.02;
        let far = 10.0;
        let mid = point_shadow_reference_depth(5.0, near, far);
        let deep = point_shadow_reference_depth(9.0, near, far);
        assert!(mid > 0.0 && mid < 1.0, "中间深度在 (0,1)：{mid}");
        assert!(deep > mid, "离光更远必须更深：{mid} vs {deep}");
        assert_eq!(
            point_shadow_reference_depth(near, near, far),
            0.0,
            "贴脸返回 0"
        );
        assert_eq!(point_shadow_reference_depth(f32::NAN, near, far), 0.0);
        assert_eq!(point_shadow_reference_depth(f32::INFINITY, near, far), 0.0);
    }

    #[test]
    fn illegal_near_and_far_are_sanitized() {
        // 近平面垃圾落 1e-3、远平面垃圾落 near + 1e-3：退化出来的框很小但仍是
        // 合法的透视投影，落在框内的点必须给出 (0,1) 的深度。
        let matrices = point_shadow_matrices([0.0; 3], f32::NAN, f32::INFINITY);
        let clip = project(matrices[0], [0.0015, 0.0, 0.0]);
        let depth = clip[2] / clip[3];
        assert!(
            clip[3] > 0.0 && (0.0..=1.0).contains(&depth),
            "垃圾近远平面必须退化成可用投影：z/w = {depth}"
        );
    }

    #[test]
    fn cube_lookup_orientation_matches_the_transform() {
        let center = [1.0, 2.0, 3.0];
        let matrices = point_shadow_matrices(center, 0.02, 30.0);
        // 方向 → (应当命中的面, sc / tc 的线性轴)。这正是 WebGPU / D3D 的立方体贴图
        // 查表：u = 0.5·(sc/ma + 1)、v = 0.5·(tc/ma + 1)，ma = 主轴分量。
        // 任意 up 表互换、或漏掉 `flip_u`，都会让某个方向落到别的面、或镜像到另一侧；
        // 只断言轴上点落在视锥内是抓不到的（旧端同样用这张表做回归）。
        let cases: [LookupCase; 8] = [
            // +X：sc = -z、tc = -y
            ([1.0, 0.4, -0.2], 0, [0.0, 0.0, -1.0], [0.0, -1.0, 0.0]),
            // -X：sc = +z、tc = -y
            ([-0.9, -0.3, 0.5], 1, [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]),
            // +Y：sc = +x、tc = +z
            ([0.3, 1.0, 0.7], 2, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            // -Y：sc = +x、tc = -z
            ([0.2, -1.0, -0.6], 3, [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
            // +Z：sc = +x、tc = -y
            ([0.4, -0.3, 1.0], 4, [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
            // -Z：sc = -x、tc = -y
            ([-0.5, 0.2, -1.0], 5, [-1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
            // 再各来一个贴近面对角线的方向：越靠近面中心，镜像错误越容易被放大成整数格。
            ([1.0, 0.9, 0.2], 0, [0.0, 0.0, -1.0], [0.0, -1.0, 0.0]),
            ([0.1, 0.2, 1.0], 4, [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        ];
        for (direction, face, sc_axis, tc_axis) in cases {
            let point = [
                center[0] + direction[0],
                center[1] + direction[1],
                center[2] + direction[2],
            ];
            let clip = project(matrices[face], point);
            assert!(
                clip[3] > 0.0,
                "方向 {direction:?} 必须落在第 {face} 面正前方"
            );
            let u = clip[0] / clip[3] * 0.5 + 0.5;
            let v = 0.5 - clip[1] / clip[3] * 0.5;
            let major = direction
                .iter()
                .fold(0.0f32, |acc, value| acc.max(value.abs()));
            let sc =
                direction[0] * sc_axis[0] + direction[1] * sc_axis[1] + direction[2] * sc_axis[2];
            let tc =
                direction[0] * tc_axis[0] + direction[1] * tc_axis[1] + direction[2] * tc_axis[2];
            let expected_u = 0.5 * (sc / major) + 0.5;
            let expected_v = 0.5 * (tc / major) + 0.5;
            assert!(
                (u - expected_u).abs() < 1e-5 && (v - expected_v).abs() < 1e-5,
                "方向 {direction:?} 的贴图坐标必须与立方体查表一致：\
                 u {u} / {expected_u}、v {v} / {expected_v}"
            );
        }
    }

    #[test]
    fn depth_matches_the_projection_and_uses_the_major_axis() {
        let center = [1.0, 2.0, 3.0];
        let near = 0.05;
        let far = 30.0;
        let matrices = point_shadow_matrices(center, near, far);
        let axes = [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ];
        // 轴上点：矩阵的 z/w 必须与参考深度公式逐点相等（公式写错、或与投影矩阵不同尺度
        // 时这里立刻红；只断言"越远越深"是抓不到的）。
        for (face, axis) in axes.iter().enumerate() {
            for distance in [1.0f32, 2.0, 5.0, 12.5, 22.0, 29.0] {
                let point = [
                    center[0] + axis[0] * distance,
                    center[1] + axis[1] * distance,
                    center[2] + axis[2] * distance,
                ];
                let clip = project(matrices[face], point);
                let projected = clip[2] / clip[3];
                let expected = point_shadow_reference_depth(distance, near, far);
                assert!(
                    (projected - expected).abs() < 1e-5,
                    "第 {face} 面 {distance} 单位的深度：{projected} / {expected}"
                );
            }
        }
        // 离轴点：主轴距离 6、欧氏距离 ≈ 6.403。参考深度必须走**主轴分量**——旧端注释
        // 记录过用欧氏距离会让斜面整片压黑的真实事故。
        let off_axis = [6.0f32, 2.0, -1.0];
        let point = [
            center[0] + off_axis[0],
            center[1] + off_axis[1],
            center[2] + off_axis[2],
        ];
        let clip = project(matrices[0], point);
        let projected = clip[2] / clip[3];
        let major = point_shadow_reference_depth(off_axis[0], near, far);
        let euclid = point_shadow_reference_depth(
            (off_axis[0] * off_axis[0] + off_axis[1] * off_axis[1] + off_axis[2] * off_axis[2])
                .sqrt(),
            near,
            far,
        );
        assert!(
            (projected - major).abs() < 1e-5,
            "离轴点深度必须按主轴分量算：{projected} / {major}"
        );
        assert!(
            (major - euclid).abs() > 2e-4,
            "主轴与欧氏距离在这条用例里必须可分辨：{major} / {euclid}"
        );
    }

    #[test]
    fn the_first_shadow_casting_point_light_is_picked() {
        let mut environment = SceneEnvironment::default();
        // 第一盏点光不投影、第二盏投影：应该选中第二盏。
        let off = SceneLight {
            kind: LightKind::Point,
            position: [0.0, 1.0, 0.0],
            casts_shadow: false,
            ..SceneLight::default()
        };
        let on = SceneLight {
            kind: LightKind::Point,
            position: [2.0, 1.0, 0.0],
            ..SceneLight::default()
        };
        environment.lights.push(off);
        environment.lights.push(on);
        let picked = pick_light(&environment).expect("第二盏点光投射阴影");
        assert_eq!(picked.position, [2.0, 1.0, 0.0]);
        let uniforms = plan(picked, 512).uniform_bytes();
        assert_eq!(uniforms.len() as u64, 32);
        assert_eq!(
            f32::from_le_bytes(uniforms[20..24].try_into().unwrap()),
            2.0 / 512.0
        );
    }
}
