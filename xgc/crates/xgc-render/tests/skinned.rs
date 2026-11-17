//! GPU 蒙皮管线：关节矩阵平移应把左半 quad 推到右半。

use std::sync::{Mutex, MutexGuard};

use xgc_render::{
    identity, translation, GpuContext, GpuResource, GpuSkinMeshCache, Mesh, MeshCache, Renderer,
    ResourceDesc, ResourceUsage, SkinData, SkinnedRequest,
};

static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

#[test]
fn gpu_skinning_applies_joint_matrix() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "skin-target",
        "rgba8unorm",
        8,
        8,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("skin target 不是纹理");
    };
    let mut mesh = Mesh::new(
        vec![
            [-0.5, -0.5, 0.0],
            [0.5, -0.5, 0.0],
            [0.5, 0.5, 0.0],
            [-0.5, 0.5, 0.0],
        ],
        vec![0, 1, 2, 0, 2, 3],
    );
    mesh.skin = Some(SkinData {
        joints: vec![[0; 4]; 4],
        weights: vec![[1.0, 0.0, 0.0, 0.0]; 4],
        joint_nodes: vec![0],
        inverse_bind: vec![identity()],
    });
    let mut cache = GpuSkinMeshCache::new();
    let gpu_mesh = cache.upload(&context, "quad", &mesh);
    let pipeline = context.create_skinned_pipeline(texture.format(), false);
    pipeline
        .draw(
            &context,
            &SkinnedRequest {
                texture: &texture,
                depth: None,
                clear: true,
                mesh: gpu_mesh,
                pose: &[translation(0.5, 0.0, 0.0)],
                inverse_bind: &[identity()],
                model: identity(),
                color: [1.0, 0.0, 0.0, 1.0],
                view_proj: identity(),
            },
        )
        .unwrap();
    let pixels = context.read_texture_rgba8(&texture, 8, 8).unwrap();
    let left = 4 * (4 * 8 + 2);
    let right = 4 * (4 * 8 + 6);
    assert_eq!(&pixels[left..left + 4], &[0, 0, 0, 255]);
    assert_eq!(&pixels[right..right + 4], &[255, 0, 0, 255]);
}

#[test]
fn renderer_uses_gpu_skinning_for_pose_items() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "skin-frame",
        "rgba8unorm",
        8,
        8,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("skin frame 不是纹理");
    };
    let mut world = xgc_world::World::new();
    let skeleton = world.spawn((
        xgc_world::Skeleton { joint_count: 1 },
        xgc_world::SkeletonPose {
            matrices: ron::ser::to_string(&vec![translation(0.5, 0.0, 0.0)]).unwrap(),
        },
    ));
    let skeleton_id = world.entity(skeleton).unwrap();
    world.spawn((
        xgc_world::Parent {
            entity: skeleton_id,
        },
        xgc_world::GlobalTransform::default(),
        xgc_world::MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        xgc_world::Material {
            base_color: [1.0, 0.0, 0.0, 1.0],
            roughness: 0.5,
            ..xgc_world::Material::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    let stats = renderer
        .render_selected(&context, &texture, &world, None)
        .unwrap();
    assert_eq!(stats.draws, 1);
    let pixels = context.read_texture_rgba8(&texture, 8, 8).unwrap();
    let left = 4 * (4 * 8 + 2);
    let right = 4 * (4 * 8 + 6);
    assert_eq!(&pixels[left..left + 4], &[0, 0, 0, 255]);
    assert_eq!(&pixels[right..right + 4], &[255, 0, 0, 255]);
}
