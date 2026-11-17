//! GPU 蒙皮必须与静态主场景共享 PBR / ACES / 基色贴图；超限姿态的 CPU 回退也不能丢 UV。

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

mod skinned_support;

use skinned_support::{write_textured_skin_gltf, write_textured_skin_gltf_with_joint};
use xgc_render::{
    identity, scene_bounds, translation, GpuContext, MeshCache, Renderer, ResourceDesc,
    ResourceUsage,
};
use xgc_world::{
    Camera, GlobalTransform, LightKind, Material, MeshRenderer, Parent, SceneEnvironment,
    SceneLight, Skeleton, SkeletonPose, World,
};

static GPU_LOCK: Mutex<()> = Mutex::new(());

/// 双角色差分用的画布边长。
const TWO_CHARACTER_SIZE: u32 = 96;
const SIZE: u32 = 64;

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Debug)]
struct Frame {
    pixels: Vec<u8>,
    gpu_skin_meshes: usize,
}

#[test]
fn identity_gpu_skin_matches_static_pbr_texture_pixel_for_pixel() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_textured_skin_gltf();
    let static_frame = render(&context, &root, None);
    let skinned = render(&context, &root, Some(vec![identity()]));
    std::fs::remove_dir_all(&root).unwrap();

    assert_eq!(static_frame.gpu_skin_meshes, 0);
    assert_eq!(skinned.gpu_skin_meshes, 1, "带贴图蒙皮必须走 GPU 快路径");
    assert_eq!(skinned.pixels, static_frame.pixels);
    let colors: std::collections::HashSet<_> = skinned
        .pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != [0, 0, 0])
        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect();
    assert!(colors.len() >= 3, "非白测试贴图必须真的被采样：{colors:?}");
}

#[test]
fn gpu_skin_moves_the_textured_surface_with_the_joint() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_textured_skin_gltf();
    let base = render(&context, &root, Some(vec![identity()]));
    let moved = render(&context, &root, Some(vec![translation(0.5, 0.0, 0.0)]));
    std::fs::remove_dir_all(&root).unwrap();

    assert_eq!(base.gpu_skin_meshes, 1);
    assert_eq!(moved.gpu_skin_meshes, 1);
    let base_x = colored_centroid_x(&base.pixels);
    let moved_x = colored_centroid_x(&moved.pixels);
    assert!(
        moved_x > base_x + 5.0,
        "关节平移后贴图表面必须一起右移：{base_x:.2} -> {moved_x:.2}"
    );
}

#[test]
fn oversized_pose_cpu_fallback_preserves_uv_and_texture() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_textured_skin_gltf();
    let static_frame = render(&context, &root, None);
    // 257 > 256 槽（旧引擎 BONES_PER_SLOT 同容量）才该掉回 CPU。
    let fallback = render(&context, &root, Some(vec![identity(); 257]));
    std::fs::remove_dir_all(&root).unwrap();

    assert_eq!(fallback.gpu_skin_meshes, 0, "257 关节姿态必须走 CPU 回退");
    assert_eq!(fallback.pixels, static_frame.pixels);
}

#[test]
fn palette_slots_past_64_stay_on_the_gpu_path() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    // 权重全落在 90 号槽：只有 palette 真的索引到 >64 才会跟着动。
    let root = write_textured_skin_gltf_with_joint(90, 96);
    let base = render(&context, &root, Some(vec![identity(); 96]));
    let mut pose = vec![identity(); 96];
    pose[90] = translation(0.6, 0.0, 0.0);
    let moved = render(&context, &root, Some(pose));
    std::fs::remove_dir_all(&root).unwrap();

    assert_eq!(
        moved.gpu_skin_meshes, 1,
        "96 关节必须留在 GPU 快路径（旧上限 64 会掉到 CPU）"
    );
    assert_eq!(base.gpu_skin_meshes, 1);
    let base_x = colored_centroid_x(&base.pixels);
    let moved_x = colored_centroid_x(&moved.pixels);
    assert!(
        moved_x > base_x + 5.0,
        "90 号槽的平移必须带着表面一起右移：{base_x:.2} -> {moved_x:.2}"
    );
}

#[test]
fn gpu_skinned_surface_receives_a_static_caster_shadow() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_textured_skin_gltf();
    let shadowed = render_shadow_receiver(&context, &root, true);
    let unshadowed = render_shadow_receiver(&context, &root, false);
    std::fs::remove_dir_all(&root).unwrap();

    let darkened = shadowed
        .pixels
        .chunks_exact(4)
        .zip(unshadowed.pixels.chunks_exact(4))
        .filter(|(on, off)| {
            let on = i32::from(on[0]) + i32::from(on[1]) + i32::from(on[2]);
            let off = i32::from(off[0]) + i32::from(off[1]) + i32::from(off[2]);
            on < off - 20
        })
        .count();
    assert_eq!(shadowed.gpu_skin_meshes, 1);
    assert!(
        darkened > 30,
        "GPU 蒙皮表面必须接收静态投射体阴影，实际仅 {darkened} 个像素变暗"
    );
}

#[test]
fn scene_bounds_follow_the_current_pose() {
    let root = write_textured_skin_gltf();
    let world = world_with_skinned_quad(Some(vec![translation(4.0, 0.0, 0.0)]));
    let mut meshes = MeshCache::new(&root);
    let bounds = scene_bounds(&world, &mut meshes)
        .unwrap()
        .expect("场景里有网格");
    std::fs::remove_dir_all(&root).unwrap();

    // 绑定姿态盒子的中心在 0；姿态把四边形推到 +4，包围盒必须跟着走。
    assert!(
        bounds.center()[0] > 3.5,
        "场景包围盒必须用当前姿态，实际 {bounds:?}"
    );
}

#[test]
fn auto_framing_keeps_a_posed_mesh_in_frame() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_textured_skin_gltf();
    let world = world_with_skinned_quad(Some(vec![translation(4.0, 0.0, 0.0)]));
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "posed-framing",
            "rgba8unorm",
            96,
            96,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new(&root));
    renderer.render_shot(&context, texture, &world).unwrap();
    let pixels = context
        .read_texture_rgba8(texture, TWO_CHARACTER_SIZE, TWO_CHARACTER_SIZE)
        .unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    let lit = pixels
        .chunks_exact(4)
        .filter(|pixel| u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]) > 30)
        .count();
    assert!(
        lit > 50,
        "被姿态移开的网格必须仍在自动取景画面内，实际只有 {lit} 个可见像素"
    );
}

/// 只有一块蒙皮四边形 + 一盏强方向光的场景；**没有相机**，取景只能靠自动包围盒。
fn world_with_skinned_quad(pose: Option<Vec<[f32; 16]>>) -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![SceneLight {
            name: "Sun".to_string(),
            kind: LightKind::Directional,
            direction: [0.2, -0.3, -1.0],
            position: [0.0; 3],
            color: [1.0; 3],
            intensity: 3.0,
            range: 10.0,
            casts_shadow: false,
            shadow_resolution: 0,
        }],
        ambient: [0.4; 3],
    });
    let quad = (
        GlobalTransform::default(),
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
            world.spawn((quad.0, quad.1, quad.2, Parent { entity: parent }));
        }
        None => {
            world.spawn(quad);
        }
    }
    world
}

fn render(context: &GpuContext, root: &Path, pose: Option<Vec<[f32; 16]>>) -> Frame {
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "skinned-pbr-target",
            "rgba8unorm-srgb",
            SIZE,
            SIZE,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![SceneLight {
            name: "Sun".to_string(),
            kind: LightKind::Directional,
            direction: [0.2, -0.3, -1.0],
            position: [0.0; 3],
            color: [1.0, 0.85, 0.7],
            intensity: 0.35,
            range: 10.0,
            casts_shadow: false,
            shadow_resolution: 0,
        }],
        ambient: [0.22, 0.18, 0.15],
    });
    let transform = GlobalTransform::default();
    let mesh = MeshRenderer {
        model: "skin.gltf".to_string(),
    };
    let material = Material {
        base_color: [0.8, 0.7, 0.9, 1.0],
        roughness: 0.62,
        ..Material::default()
    };
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
            let skeleton_id = world.entity(skeleton).unwrap();
            world.spawn((
                transform,
                mesh,
                material,
                Parent {
                    entity: skeleton_id,
                },
            ));
        }
        None => {
            world.spawn((transform, mesh, material));
        }
    }
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 3.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));

    let mut renderer = Renderer::new(MeshCache::new(root));
    renderer.render(context, texture, &world).unwrap();
    Frame {
        pixels: context.read_texture_rgba8(texture, SIZE, SIZE).unwrap(),
        gpu_skin_meshes: renderer.gpu_skin_mesh_count(),
    }
}

fn render_shadow_receiver(context: &GpuContext, root: &Path, casts_shadow: bool) -> Frame {
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "skinned-shadow-receiver",
            "rgba8unorm",
            128,
            72,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![SceneLight {
            name: "Sun".to_string(),
            kind: LightKind::Directional,
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
    let skeleton = world.spawn((
        Skeleton { joint_count: 1 },
        SkeletonPose {
            matrices: ron::to_string(&vec![identity()]).unwrap(),
        },
    ));
    let skeleton_id = world.entity(skeleton).unwrap();
    world.spawn((
        GlobalTransform {
            position: [0.0, -0.1, 0.0],
            rotation: [
                -std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            scale: [7.0, 7.0, 7.0],
        },
        MeshRenderer {
            model: "skin.gltf".to_string(),
        },
        Material::default(),
        Parent {
            entity: skeleton_id,
        },
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 2.0, 0.0],
            scale: [1.5; 3],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://cube".to_string(),
        },
        Material::default(),
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
        Camera::default(),
    ));
    let mut renderer = Renderer::new(MeshCache::new(root));
    renderer.render(context, texture, &world).unwrap();
    Frame {
        pixels: context.read_texture_rgba8(texture, 128, 72).unwrap(),
        gpu_skin_meshes: renderer.gpu_skin_mesh_count(),
    }
}

#[test]
fn batched_skinned_draws_keep_their_own_palettes() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = write_textured_skin_gltf();
    // 左侧角色固定，右侧角色换姿态：一批提交里两条 draw 必须各用各的 palette。
    let still = render_two_characters(&context, &root, identity());
    let moved = render_two_characters(&context, &root, translation(1.0, 0.0, 0.0));
    std::fs::remove_dir_all(&root).unwrap();

    let (still_left, still_right) = column_halves(&still.pixels);
    let (moved_left, moved_right) = column_halves(&moved.pixels);
    // 没有这两条断言时，路由退回 CPU 蒙皮也能让像素全绿——那就测不到 draw_batch。
    // 两个角色共用同一个模型，所以蒙皮网格缓存只有 1 条（palette 才是逐角色的）。
    assert_eq!(still.gpu_skin_meshes, 1, "两个角色都必须走 GPU 蒙皮");
    assert_eq!(moved.gpu_skin_meshes, 1);
    assert!(
        lit(&still_left) > 50,
        "左侧角色必须可见：{}",
        lit(&still_left)
    );
    assert!(
        lit(&moved_right) > 50,
        "右侧角色必须可见：{}",
        lit(&moved_right)
    );
    assert!(
        still_left == moved_left,
        "右侧角色换姿态不能影响左侧角色（说明 palette 串了）"
    );
    assert!(
        still_right != moved_right,
        "右侧角色换了姿态，它的像素必须跟着变"
    );
}

/// 按**列**切成左右两半（每行各取前 / 后一半像素），不是把字节流对半砍。
fn column_halves(pixels: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let width = TWO_CHARACTER_SIZE as usize;
    let half = width / 2;
    let mut left = Vec::with_capacity(pixels.len() / 2);
    let mut right = Vec::with_capacity(pixels.len() / 2);
    for row in pixels.chunks_exact(width * 4) {
        left.extend_from_slice(&row[..half * 4]);
        right.extend_from_slice(&row[half * 4..]);
    }
    (left, right)
}

fn lit(pixels: &[u8]) -> usize {
    pixels
        .chunks_exact(4)
        .filter(|pixel| u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]) > 30)
        .count()
}

/// 两台骨架 / 两个蒙皮四边形（左 x=-2 固定，右 x=+2 用 `right_pose`）+ 固定相机。
fn render_two_characters(context: &GpuContext, root: &Path, right_pose: [f32; 16]) -> Frame {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![SceneLight {
            name: "Sun".to_string(),
            kind: LightKind::Directional,
            direction: [0.2, -0.3, -1.0],
            position: [0.0; 3],
            color: [1.0; 3],
            intensity: 3.0,
            range: 10.0,
            casts_shadow: false,
            shadow_resolution: 0,
        }],
        ambient: [0.4; 3],
    });
    for (x, pose) in [(-2.0_f32, identity()), (2.0, right_pose)] {
        let skeleton = world.spawn((
            Skeleton { joint_count: 1 },
            SkeletonPose {
                matrices: ron::to_string(&vec![pose]).unwrap(),
            },
        ));
        let parent = world.entity(skeleton).unwrap();
        world.spawn((
            GlobalTransform {
                position: [x, 0.0, 0.0],
                ..GlobalTransform::default()
            },
            MeshRenderer {
                model: "skin.gltf".to_string(),
            },
            Material::default(),
            Parent { entity: parent },
        ));
    }
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 6.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "two-skinned-characters",
            "rgba8unorm",
            TWO_CHARACTER_SIZE,
            TWO_CHARACTER_SIZE,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new(root));
    renderer.render(context, texture, &world).unwrap();
    Frame {
        pixels: context
            .read_texture_rgba8(texture, TWO_CHARACTER_SIZE, TWO_CHARACTER_SIZE)
            .unwrap(),
        gpu_skin_meshes: renderer.gpu_skin_mesh_count(),
    }
}

fn colored_centroid_x(pixels: &[u8]) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for (index, pixel) in pixels.chunks_exact(4).enumerate() {
        if pixel[..3] != [0, 0, 0] {
            total += (index as u32 % SIZE) as f32;
            count += 1.0;
        }
    }
    assert!(count > 100.0, "测试网格必须覆盖足够像素，实际 {count}");
    total / count
}
