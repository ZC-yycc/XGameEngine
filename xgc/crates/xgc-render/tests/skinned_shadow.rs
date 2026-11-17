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

fn shadow_centroid_x(shadowed: &[u8], unshadowed: &[u8]) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for (index, (on, off)) in shadowed
        .chunks_exact(4)
        .zip(unshadowed.chunks_exact(4))
        .enumerate()
    {
        let on_sum = i32::from(on[0]) + i32::from(on[1]) + i32::from(on[2]);
        let off_sum = i32::from(off[0]) + i32::from(off[1]) + i32::from(off[2]);
        if on_sum < off_sum - 20 {
            total += (index as u32 % WIDTH) as f32;
            count += 1.0;
        }
    }
    assert!(count > 20.0, "阴影像素太少，无法定位：{count}");
    total / count
}

/// `pose = None` 时按静态网格出 draw（不挂 Skeleton）；`Some` 时挂到骨架走蒙皮。
fn render(
    context: &GpuContext,
    root: &Path,
    pose: Option<Vec<[f32; 16]>>,
    casts_shadow: bool,
) -> Frame {
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "skinned-caster-shadow",
            "rgba8unorm",
            WIDTH,
            HEIGHT,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
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
    let caster = (
        GlobalTransform {
            position: [0.0, 2.0, 0.0],
            scale: [1.5; 3],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "skin.gltf".to_string(),
        },
        Material::default(),
    );
    match pose {
        Some(pose) => {
            let skeleton = world.spawn((
                Skeleton {
                    joint_count: pose.len() as u32,
                },
                SkeletonPose {
                    matrices: ron::to_string(&pose).unwrap(),
                },
            ));
            let parent = world.entity(skeleton).unwrap();
            world.spawn((caster.0, caster.1, caster.2, Parent { entity: parent }));
        }
        None => {
            world.spawn(caster);
        }
    }
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
    let mut renderer = Renderer::new(MeshCache::new(root));
    renderer.render(context, texture, &world).unwrap();
    Frame {
        pixels: context.read_texture_rgba8(texture, WIDTH, HEIGHT).unwrap(),
        gpu_skin_meshes: renderer.gpu_skin_mesh_count(),
    }
}
