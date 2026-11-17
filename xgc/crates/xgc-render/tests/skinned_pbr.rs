//! GPU 蒙皮必须与静态主场景共享 PBR / ACES / 基色贴图；超限姿态的 CPU 回退也不能丢 UV。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use xgc_render::{
    identity, translation, GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage,
};
use xgc_world::{
    Camera, GlobalTransform, LightKind, Material, MeshRenderer, Parent, SceneEnvironment,
    SceneLight, Skeleton, SkeletonPose, World,
};

static GPU_LOCK: Mutex<()> = Mutex::new(());
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

fn write_textured_skin_gltf() -> PathBuf {
    write_textured_skin_gltf_with_joint(0, 1)
}

/// 四个顶点的权重等分到 `joint` 槽；骨架声明 `joint_count` 根骨头（各一份单位逆绑定）。
/// 槽位上限（旧端 256）就是靠"骨架比槽位多一根"来钉的。
fn write_textured_skin_gltf_with_joint(joint: u16, joint_count: u16) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "xgc_skinned_pbr_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();

    let mut bin = Vec::new();
    for position in [
        [-0.8f32, -0.8, 0.0],
        [0.8, -0.8, 0.0],
        [0.8, 0.8, 0.0],
        [-0.8, 0.8, 0.0],
    ] {
        for value in position {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for uv in [[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]] {
        for value in uv {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for _ in 0..4 {
        for value in [joint; 4] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    // 总权重刻意不是 1；静态 / GPU 逐像素同值会钉住 shader 的正权重归一。
    for _ in 0..4 {
        for value in [0.25f32, 0.25, 0.0, 0.0] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    // 每根骨头一份逆绑定（全单位阵）：palette 只认 `joint < inverse_bind.len()`。
    for _ in 0..joint_count {
        for value in identity() {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for index in [0u16, 1, 2, 0, 2, 3] {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    std::fs::write(root.join("skin.bin"), &bin).unwrap();

    let joints: Vec<String> = (1..=joint_count)
        .map(|index| format!("{{\"name\": \"joint-{index}\"}}"))
        .collect();
    let nodes = format!("{{\"mesh\": 0, \"skin\": 0}}, {}", joints.join(", "));
    let joint_slots: Vec<String> = (1..=joint_count).map(|index| index.to_string()).collect();
    let skins = format!(
        "{{\"joints\": [{}], \"inverseBindMatrices\": 4}}",
        joint_slots.join(", ")
    );
    let indices_offset = 240 + (joint_count as usize - 1) * 64;
    let json = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "buffers": [{{"uri": "skin.bin", "byteLength": {buffer_len}}}],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": 48}},
    {{"buffer": 0, "byteOffset": 48, "byteLength": 32}},
    {{"buffer": 0, "byteOffset": 80, "byteLength": 32}},
    {{"buffer": 0, "byteOffset": 112, "byteLength": 64}},
    {{"buffer": 0, "byteOffset": 176, "byteLength": {ibm_len}}},
    {{"buffer": 0, "byteOffset": {indices_offset}, "byteLength": 12}}
  ],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3"}},
    {{"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC2"}},
    {{"bufferView": 2, "componentType": 5123, "count": 4, "type": "VEC4"}},
    {{"bufferView": 3, "componentType": 5126, "count": 4, "type": "VEC4"}},
    {{"bufferView": 4, "componentType": 5126, "count": {joint_count}, "type": "MAT4"}},
    {{"bufferView": 5, "componentType": 5123, "count": 6, "type": "SCALAR"}}
  ],
  "images": [{{"uri": "albedo.png"}}],
  "textures": [{{"source": 0}}],
  "materials": [{{"pbrMetallicRoughness": {{
    "baseColorFactor": [0.9, 0.8, 0.7, 1.0],
    "baseColorTexture": {{"index": 0}}
  }}}}],
  "meshes": [{{"primitives": [{{
    "attributes": {{"POSITION": 0, "TEXCOORD_0": 1, "JOINTS_0": 2, "WEIGHTS_0": 3}},
    "indices": 5,
    "material": 0,
    "mode": 4
  }}]}}],
  "nodes": [{nodes}],
  "skins": [{skins}],
  "scenes": [{{"nodes": [0]}}],
  "scene": 0
}}"#,
        buffer_len = bin.len(),
        ibm_len = joint_count as usize * 64,
    );
    std::fs::write(root.join("skin.gltf"), json).unwrap();
    write_test_png(&root.join("albedo.png"));
    root
}

fn write_test_png(path: &Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(file, 2, 2);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&[
            240, 40, 20, 255, 20, 220, 40, 255, 30, 60, 240, 255, 230, 190, 20, 255,
        ])
        .unwrap();
}
