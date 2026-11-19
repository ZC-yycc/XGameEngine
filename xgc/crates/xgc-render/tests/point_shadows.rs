//! 点光立方体阴影：开着的点光投下阴影、关着的点光一个 pass 都不跑。
//!
//! 判据用"同一个点光开 / 关 `casts_shadow` 的两帧逐像素比较"：不手算投影，
//! 也不依赖具体机位。`RenderStats::point_shadow_draw_calls` 是六面 × 投射体数，
//! 它让"六面深度图真的画了、且只画了应该画的数量"能被直接观测。

use std::path::Path;
use std::sync::Mutex;

mod skinned_support;

use skinned_support::write_skinned_cube_gltf;
use xgc_render::{identity, GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{
    GlobalTransform, LightKind, Material, MeshRenderer, Parent, SceneEnvironment, SceneLight,
    Skeleton, SkeletonPose, World,
};

static GPU_LOCK: Mutex<()> = Mutex::new(());

const WIDTH: u32 = 192;
const HEIGHT: u32 = 108;

/// 一盏点光 + 地面 + 悬空投射体 + 俯视相机。
///
/// 灯放在 `+X` 一边，影子就落到投射体的 `-X` 一侧（正下方会被投射体自己挡住）。
fn scene(casts_shadow: bool, range: f32) -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![SceneLight {
            name: "Lamp".to_string(),
            kind: LightKind::Point,
            direction: [0.0, -1.0, 0.0],
            position: [2.0, 3.0, 0.0],
            color: [1.0; 3],
            intensity: 6.0,
            range,
            casts_shadow,
            shadow_resolution: 0,
        }],
        ambient: [0.0; 3],
    });
    // 地面：薄板，顶面在 y = 0。
    world.spawn((
        GlobalTransform {
            position: [0.0, -0.1, 0.0],
            scale: [20.0, 0.2, 20.0],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://cube".to_string(),
        },
        Material::default(),
    ));
    // 投射体：悬在地面上方。
    world.spawn((
        GlobalTransform {
            position: [0.0, 1.5, 0.0],
            scale: [1.2, 1.2, 1.2],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://cube".to_string(),
        },
        Material::default(),
    ));
    // 俯视相机：绕 +X 转 -90°，forward 变成 (0,-1,0)。
    world.spawn((
        GlobalTransform {
            position: [0.0, 6.0, 0.0],
            rotation: [
                -std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            ..GlobalTransform::default()
        },
        xgc_world::Camera::default(),
    ));
    world
}

fn render(world: &World) -> (Vec<u8>, usize) {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "point-shadow",
            "rgba8unorm",
            WIDTH,
            HEIGHT,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new("."));
    let stats = renderer.render(&context, texture, world).unwrap();
    let pixels = context.read_texture_rgba8(texture, WIDTH, HEIGHT).unwrap();
    (pixels, stats.point_shadow_draw_calls)
}

/// 一个 GPU 蒙皮投射体 + 地面 + 同一盏点光：用来钉"蒙皮也进六面深度图"。
///
/// 只靠 `point_shadow_draw_calls` 抓不到漏画蒙皮（它统计的是投射体条数，蒙皮那条
/// 即使没进 pass 也仍被计数），所以判据必须是"画面上真的多了一块影子"。
fn skinned_scene(casts_shadow: bool) -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![SceneLight {
            name: "Lamp".to_string(),
            kind: LightKind::Point,
            direction: [0.0, -1.0, 0.0],
            position: [2.0, 3.0, 0.0],
            color: [1.0; 3],
            intensity: 6.0,
            range: 10.0,
            casts_shadow,
            shadow_resolution: 0,
        }],
        ambient: [0.0; 3],
    });
    world.spawn((
        GlobalTransform {
            position: [0.0, -0.1, 0.0],
            scale: [20.0, 0.2, 20.0],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://cube".to_string(),
        },
        Material::default(),
    ));
    let skeleton = world.spawn((
        Skeleton { joint_count: 1 },
        SkeletonPose {
            matrices: ron::to_string(&vec![identity()]).unwrap(),
        },
    ));
    let parent = world.entity(skeleton).unwrap();
    world.spawn((
        GlobalTransform {
            position: [0.0, 1.5, 0.0],
            scale: [1.5; 3],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "skin.gltf".to_string(),
        },
        Material::default(),
        Parent { entity: parent },
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 6.0, 0.0],
            rotation: [
                -std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            ..GlobalTransform::default()
        },
        xgc_world::Camera::default(),
    ));
    world
}

fn render_skinned(root: &Path, world: &World) -> (Vec<u8>, usize, usize) {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "point-shadow-skinned",
            "rgba8unorm",
            WIDTH,
            HEIGHT,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new(root));
    let stats = renderer.render(&context, texture, world).unwrap();
    let pixels = context.read_texture_rgba8(texture, WIDTH, HEIGHT).unwrap();
    (
        pixels,
        stats.point_shadow_draw_calls,
        renderer.gpu_skin_mesh_count(),
    )
}

fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner())
}

/// 每个像素相对"没有点光阴影"基线的变暗幅度（0 = 完全受光）。
fn darkening(baseline: &[u8], shaded: &[u8]) -> Vec<i32> {
    baseline
        .chunks_exact(4)
        .zip(shaded.chunks_exact(4))
        .map(|(lit, shadow)| i32::from(lit[0]) - i32::from(shadow[0]))
        .collect()
}

#[test]
fn a_point_light_casts_a_visible_shadow() {
    let _gpu = gpu_lock();
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let (lit, lit_draws) = render(&scene(false, 10.0));
    let (shadowed, shadow_draws) = render(&scene(true, 10.0));
    // 六面 × 两个投射体（地面 + 立方体）。
    assert_eq!(lit_draws, 0, "关掉 casts_shadow 不该跑立方体阴影");
    assert_eq!(shadow_draws, 12, "六面各画一次地面与立方体：{shadow_draws}");
    let deltas = darkening(&lit, &shadowed);
    let darkened = deltas.iter().filter(|&&delta| delta > 5).count();
    assert!(
        darkened > 100,
        "点光必须在地面上投出可见阴影：{darkened} 个像素变暗"
    );
    // 影子必须是**局部**的：灯在 +X 一边，影子只盖住投射体斜后方一小块。
    // 若参考深度算错（例如整帧都判成遮挡），整张地面都会变暗——这条把它拦下来。
    let total = WIDTH as usize * HEIGHT as usize;
    assert!(
        darkened < total / 2,
        "点光阴影必须是局部的，不该把整帧压黑：{darkened} / {total}"
    );
}

#[test]
fn the_same_scene_renders_deterministically() {
    let _gpu = gpu_lock();
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let world = scene(true, 10.0);
    let (first, first_draws) = render(&world);
    let (second, second_draws) = render(&world);
    assert_eq!(first_draws, second_draws);
    assert!(
        first == second,
        "点光阴影必须逐字节确定（立方体面顺序固定）"
    );
}

#[test]
fn a_point_light_without_a_usable_range_does_not_cast() {
    let _gpu = gpu_lock();
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    // 半径非有限：打包成 0，着色器整盏跳过；也不该为它跑六面 pass。
    let (_, nan_draws) = render(&scene(true, f32::NAN));
    let (_, inf_draws) = render(&scene(true, f32::INFINITY));
    let (_, zero_draws) = render(&scene(true, 0.0));
    assert_eq!(nan_draws, 0, "半径 NaN 的点光不该投影");
    assert_eq!(inf_draws, 0, "半径无穷的点光不该投影");
    assert_eq!(zero_draws, 0, "半径 0 的点光不该投影");
}

#[test]
fn a_point_light_that_is_out_of_budget_does_not_cast() {
    let _gpu = gpu_lock();
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    // 前 8 盏都不投影、第 9 盏投影：打包表里根本没有第 9 盏，立方体阴影也不该跑。
    let mut world = scene(false, 10.0);
    let mut lights = Vec::new();
    for index in 0..8 {
        lights.push(SceneLight {
            name: format!("Filler{index}"),
            kind: LightKind::Point,
            direction: [0.0, -1.0, 0.0],
            position: [index as f32, 3.0, 0.0],
            color: [1.0; 3],
            intensity: 0.5,
            range: 10.0,
            casts_shadow: false,
            shadow_resolution: 0,
        });
    }
    lights.push(SceneLight {
        name: "Ninth".to_string(),
        kind: LightKind::Point,
        direction: [0.0, -1.0, 0.0],
        position: [2.0, 3.0, 0.0],
        color: [1.0; 3],
        intensity: 6.0,
        range: 10.0,
        casts_shadow: true,
        shadow_resolution: 0,
    });
    world.set_environment(SceneEnvironment {
        lights,
        ambient: [0.0; 3],
    });
    let (_, draws) = render(&world);
    assert_eq!(draws, 0, "第 9 盏灯不在 8 槽光源表里，不该为它跑立方体阴影");
}

#[test]
fn a_skinned_caster_is_drawn_into_all_six_cube_faces() {
    let _gpu = gpu_lock();
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let root = write_skinned_cube_gltf();
    let (lit, lit_draws, lit_skinned) = render_skinned(&root, &skinned_scene(false));
    let (shadowed, shadow_draws, shadow_skinned) = render_skinned(&root, &skinned_scene(true));
    std::fs::remove_dir_all(&root).unwrap();

    assert_eq!(lit_draws, 0, "关掉 casts_shadow 不该跑立方体阴影");
    assert_eq!(lit_skinned, 1, "带 JOINTS_0 的网格必须走 GPU 蒙皮");
    assert_eq!(shadow_skinned, 1, "阴影开关不该改变蒙皮路由");
    // 六面 × 两个投射体（地面 + 蒙皮角色）。
    assert_eq!(
        shadow_draws, 12,
        "六面各画一次地面与蒙皮投射体：{shadow_draws}"
    );
    let deltas = darkening(&lit, &shadowed);
    let darkened = deltas.iter().filter(|&&delta| delta > 5).count();
    // 蒙皮那条没进六面深度图时，地面自己挡不出影子，整帧几乎不变——这条会红。
    assert!(
        darkened > 20,
        "蒙皮投射体必须投下立方体阴影：{darkened} 个像素变暗"
    );
}
