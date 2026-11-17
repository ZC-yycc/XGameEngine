//! 蒙皮网格必须像旧引擎那样投射方向光阴影：GPU palette 路径与 CPU 回退路径都要投。
//!
//! 判据全部用"开 / 关阴影两帧逐像素比较"，不手算投影：旧实现（蒙皮不进阴影批次）
//! 会让每一条都红。

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

mod skinned_support;

use skinned_support::write_skinned_cube_gltf;
use xgc_render::{
    identity, translation, GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage,
};
use xgc_world::{
    Camera, GlobalTransform, LightKind, Material, MeshRenderer, Parent, SceneEnvironment,
    SceneLight, Skeleton, SkeletonPose, World,
};

static GPU_LOCK: Mutex<()> = Mutex::new(());

const WIDTH: u32 = 128;
const HEIGHT: u32 = 72;

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

struct Frame {
    pixels: Vec<u8>,
    gpu_skin_meshes: usize,
}

#[test]
fn gpu_skinned_mesh_casts_a_shadow_onto_the_floor() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_skinned_cube_gltf();
    let pose = vec![identity()];
    let shadowed = render(&context, &root, Some(pose.clone()), true);
    let unshadowed = render(&context, &root, Some(pose), false);
    std::fs::remove_dir_all(&root).unwrap();

    assert_eq!(
        shadowed.gpu_skin_meshes, 1,
        "带 JOINTS_0 的网格必须走 GPU 蒙皮"
    );
    let darkened = darkened_pixels(&shadowed.pixels, &unshadowed.pixels);
    assert!(
        darkened > 30,
        "GPU 蒙皮投射体必须在落地板上留下阴影，实际只有 {darkened} 个像素变暗"
    );
}

#[test]
fn cpu_fallback_skinned_mesh_also_casts_a_shadow() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_skinned_cube_gltf();
    // 257 关节超出 palette 容量 → 主 pass 回退 CPU 蒙皮；阴影也必须跟着投。
    let shadowed = render(&context, &root, Some(vec![identity(); 257]), true);
    let unshadowed = render(&context, &root, Some(vec![identity(); 257]), false);
    std::fs::remove_dir_all(&root).unwrap();

    assert_eq!(shadowed.gpu_skin_meshes, 0, "257 关节必须走 CPU 回退");
    let darkened = darkened_pixels(&shadowed.pixels, &unshadowed.pixels);
    assert!(
        darkened > 30,
        "CPU 回退的蒙皮投射体也必须投影，实际只有 {darkened} 个像素变暗"
    );
}

#[test]
fn skinned_shadow_follows_the_current_pose() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_skinned_cube_gltf();
    let bind = render(&context, &root, Some(vec![identity()]), true);
    let bind_off = render(&context, &root, Some(vec![identity()]), false);
    let moved = render(
        &context,
        &root,
        Some(vec![translation(0.6, 0.0, 0.0)]),
        true,
    );
    let moved_off = render(
        &context,
        &root,
        Some(vec![translation(0.6, 0.0, 0.0)]),
        false,
    );
    std::fs::remove_dir_all(&root).unwrap();

    let base_x = shadow_centroid_x(&bind.pixels, &bind_off.pixels);
    let moved_x = shadow_centroid_x(&moved.pixels, &moved_off.pixels);
    // 这里只钉"阴影会跟着姿态走"：屏幕方向由相机手性决定（俯视相机的 +X 落在屏幕
    // 左侧），所以比位移绝对值；用绑定姿态的实现在这一步会得到 0 位移。
    assert!(
        (moved_x - base_x).abs() > 1.0,
        "阴影必须按当前姿态变形（用绑定姿态会停住）：{base_x:.2} -> {moved_x:.2}"
    );
}

#[test]
fn identity_palette_casts_the_same_shadow_as_the_static_mesh() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_skinned_cube_gltf();
    let static_frame = render(&context, &root, None, true);
    let skinned = render(&context, &root, Some(vec![identity()]), true);
    std::fs::remove_dir_all(&root).unwrap();

    assert_eq!(static_frame.gpu_skin_meshes, 0);
    assert_eq!(skinned.gpu_skin_meshes, 1);
    assert_eq!(
        skinned.pixels, static_frame.pixels,
        "单位 palette 的蒙皮阴影必须与静态网格阴影逐像素相同（两支入口不许有偏移）"
    );
}

#[test]
fn repeated_frames_reuse_the_shadow_skin_arenas() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_skinned_cube_gltf();
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "skinned-caster-shadow-arena",
            "rgba8unorm",
            WIDTH,
            HEIGHT,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    // 同一个渲染器连渲两帧：阴影 pass 的 palette / model 只该在第一帧（含两槽容量）建缓冲。
    let world = shadow_world_of(&[(-1.6, vec![identity()]), (1.6, vec![identity()])], true);
    let mut renderer = Renderer::new(MeshCache::new(&root));
    let first = renderer.render(&context, texture, &world).unwrap();
    let second = renderer.render(&context, texture, &world).unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    assert_eq!(
        first.shadow_skin_uniform_buffers, 2,
        "首批必须为阴影 pass 的 palette / model 各建一张竞技场缓冲"
    );
    assert_eq!(
        second.shadow_skin_uniform_buffers, 0,
        "同尺寸第二帧必须复用阴影蒙皮竞技场、不再建缓冲"
    );
}

/// 两个 GPU 蒙皮投射体各占一个 palette / model 槽：任一边绑错槽（或永远绑槽 0），
/// 落在自己位置上的阴影就会按对方的姿态变形、跑出这扇窗口。
#[test]
fn two_skinned_casters_keep_their_own_shadow_palettes() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_skinned_cube_gltf();
    // 沿 Z 分开放（俯视相机下 Z 是屏幕纵轴，正负两侧都在画面内），姿态再各自沿 Z 反向
    // 平移：绑错槽时阴影会按对方的姿态落在别的行上。
    let left_pose = vec![translation(0.0, 0.0, 0.5)];
    let right_pose = vec![translation(0.0, 0.0, -0.5)];
    let left_on = render_world(
        &context,
        &root,
        &shadow_world_of(&[(-2.4, left_pose.clone())], true),
    );
    let left_off = render_world(
        &context,
        &root,
        &shadow_world_of(&[(-2.4, left_pose.clone())], false),
    );
    let right_on = render_world(
        &context,
        &root,
        &shadow_world_of(&[(2.4, right_pose.clone())], true),
    );
    let right_off = render_world(
        &context,
        &root,
        &shadow_world_of(&[(2.4, right_pose.clone())], false),
    );
    let both_on = render_world(
        &context,
        &root,
        &shadow_world_of(&[(-2.4, left_pose), (2.4, right_pose)], true),
    );
    let both_off = render_world(
        &context,
        &root,
        &shadow_world_of(
            &[
                (-2.4, vec![translation(0.0, 0.0, 0.5)]),
                (2.4, vec![translation(0.0, 0.0, -0.5)]),
            ],
            false,
        ),
    );
    std::fs::remove_dir_all(&root).unwrap();

    // 两个投射体共用同一份模型缓存，所以"GPU 蒙皮网格"仍是 1 张；两条 draw 都在画由下面
    // 两侧窗口里的阴影像素保证（任一边没进阴影 pass 就会失败）。
    assert_eq!(both_on.gpu_skin_meshes, 1, "两个投射体共用一个模型缓存");
    let left_y = shadow_centroid_y(&left_on.pixels, &left_off.pixels);
    let right_y = shadow_centroid_y(&right_on.pixels, &right_off.pixels);
    assert!(
        (left_y - right_y).abs() > 24.0,
        "两侧阴影必须落在不同行上才能分辨串槽（至少两个窗口宽）：{left_y:.2} / {right_y:.2}"
    );
    let both_left = shadow_centroid_y_near(&both_on.pixels, &both_off.pixels, left_y);
    let both_right = shadow_centroid_y_near(&both_on.pixels, &both_off.pixels, right_y);
    assert!(
        (both_left - left_y).abs() < 6.0,
        "两个投射体同时在画时左侧阴影必须留在自己的位置上：{left_y:.2} -> {both_left:.2}"
    );
    assert!(
        (both_right - right_y).abs() < 6.0,
        "两个投射体同时在画时右侧阴影必须留在自己的位置上：{right_y:.2} -> {both_right:.2}"
    );
}

fn darkened_pixels(shadowed: &[u8], unshadowed: &[u8]) -> usize {
    shadowed
        .chunks_exact(4)
        .zip(unshadowed.chunks_exact(4))
        .filter(|(on, off)| {
            let on = i32::from(on[0]) + i32::from(on[1]) + i32::from(on[2]);
            let off = i32::from(off[0]) + i32::from(off[1]) + i32::from(off[2]);
            on < off - 20
        })
        .count()
}

/// 变暗像素的列重心：用来钉"阴影跟着姿态左右走"。
fn shadow_centroid_x(shadowed: &[u8], unshadowed: &[u8]) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for (index, (on, off)) in shadowed
        .chunks_exact(4)
        .zip(unshadowed.chunks_exact(4))
        .enumerate()
    {
        if !is_darkened(on, off) {
            continue;
        }
        total += (index as u32 % WIDTH) as f32;
        count += 1.0;
    }
    assert!(count > 20.0, "阴影像素太少，无法定位：{count}");
    total / count
}

/// 变暗像素的行重心：俯视相机下 Z 是屏幕纵轴，用它区分"沿 Z 分开的两个投射体"。
fn shadow_centroid_y(shadowed: &[u8], unshadowed: &[u8]) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for (index, (on, off)) in shadowed
        .chunks_exact(4)
        .zip(unshadowed.chunks_exact(4))
        .enumerate()
    {
        if !is_darkened(on, off) {
            continue;
        }
        total += (index as u32 / WIDTH) as f32;
        count += 1.0;
    }
    assert!(count > 20.0, "阴影像素太少，无法定位：{count}");
    total / count
}

/// 只统计 `center ± 12` 行内的阴影像素：窗口外的阴影属于另一个投射体。
///
/// 容差 6 行：多一个投射体会改变场景包围盒，CSM 拟合随之微移（实测约 2.5 行）；
/// 而绑错槽会让阴影按对方姿态整体挪一个世界单位（约 10 行），仍然分得开。
fn shadow_centroid_y_near(shadowed: &[u8], unshadowed: &[u8], center: f32) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for (index, (on, off)) in shadowed
        .chunks_exact(4)
        .zip(unshadowed.chunks_exact(4))
        .enumerate()
    {
        let row = (index as u32 / WIDTH) as f32;
        if (row - center).abs() > 12.0 || !is_darkened(on, off) {
            continue;
        }
        total += row;
        count += 1.0;
    }
    assert!(count > 5.0, "窗口内阴影像素太少，无法定位：{count}");
    total / count
}

fn is_darkened(on: &[u8], off: &[u8]) -> bool {
    let on = i32::from(on[0]) + i32::from(on[1]) + i32::from(on[2]);
    let off = i32::from(off[0]) + i32::from(off[1]) + i32::from(off[2]);
    on < off - 20
}

/// `pose = None` 时按静态网格出 draw（不挂 Skeleton）；`Some` 时挂到骨架走蒙皮。
fn render(
    context: &GpuContext,
    root: &Path,
    pose: Option<Vec<[f32; 16]>>,
    casts_shadow: bool,
) -> Frame {
    let world = shadow_world(pose, casts_shadow);
    render_world(context, root, &world)
}

fn render_world(context: &GpuContext, root: &Path, world: &World) -> Frame {
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "skinned-caster-shadow-world",
            "rgba8unorm",
            WIDTH,
            HEIGHT,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new(root));
    renderer.render(context, texture, world).unwrap();
    Frame {
        pixels: context.read_texture_rgba8(texture, WIDTH, HEIGHT).unwrap(),
        gpu_skin_meshes: renderer.gpu_skin_mesh_count(),
    }
}

/// 场景：一块落地板 + 一个投射体（`pose = None` 时不挂 Skeleton，按静态网格出
/// draw）+ 俯视相机。阴影开关决定方向光 `casts_shadow`，其余逐字段相同。
fn shadow_world(pose: Option<Vec<[f32; 16]>>, casts_shadow: bool) -> World {
    let mut world = base_scene(casts_shadow);
    let caster = |position: [f32; 3]| {
        (
            GlobalTransform {
                position,
                scale: [1.5; 3],
                ..GlobalTransform::default()
            },
            MeshRenderer {
                model: "skin.gltf".to_string(),
            },
            Material::default(),
        )
    };
    match pose {
        Some(pose) => {
            add_skinned(&mut world, [0.0, 2.0, 0.0], pose);
        }
        None => {
            world.spawn(caster([0.0, 2.0, 0.0]));
        }
    }
    world
}

/// 多个 GPU 蒙皮投射体：每个 `(沿 Z 的落点, 姿态)` 各自一份骨架。
fn shadow_world_of(casters: &[(f32, Vec<[f32; 16]>)], casts_shadow: bool) -> World {
    let mut world = base_scene(casts_shadow);
    for (z, pose) in casters {
        add_skinned(&mut world, [0.0, 2.0, *z], pose.clone());
    }
    world
}

fn add_skinned(world: &mut World, position: [f32; 3], pose: Vec<[f32; 16]>) {
    let skeleton = world.spawn((
        Skeleton {
            joint_count: pose.len() as u32,
        },
        SkeletonPose {
            matrices: ron::to_string(&pose).unwrap(),
        },
    ));
    let parent = world.entity(skeleton).unwrap();
    world.spawn((
        GlobalTransform {
            position,
            scale: [1.5; 3],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "skin.gltf".to_string(),
        },
        Material::default(),
        Parent { entity: parent },
    ));
}

/// 方向光 + 落地板 + 俯视相机：所有用例共用，只有投射体不同。
fn base_scene(casts_shadow: bool) -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![SceneLight {
            name: "Sun".to_string(),
            kind: LightKind::Directional,
            // 斜着打：影子落在投射体旁边而不是正下方。
            direction: [0.6, -1.0, 0.0],
            position: [0.0; 3],
            color: [1.0; 3],
            intensity: 3.0,
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
        Camera::default(),
    ));
    world
}
