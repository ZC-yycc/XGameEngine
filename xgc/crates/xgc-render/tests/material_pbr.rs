//! 金属 / 电介质在**预过滤环境 IBL** 下的口径：金属把基色当 F0（环境反射被染色、没有
//! 漫反射），电介质保留漫反射。
//!
//! 期望值不再由 CPU 镜像现算：环境镜面项现在是 GPU 上烘焙的预过滤立方体 + BRDF 积分表，
//! CPU 侧没有同口径镜像（旧的 Lazarov 解析近似已随切片删除，留着就是"镜像不再镜像"）。
//! 这里改成**相对判据**：同一片天空下只换 F0 / metallic，比较通道关系与亮度差——钉住的
//! 正是"金属反射被 F0 染色""电介质还有漫反射"这两条行为，而不是某次烘焙的数值。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use xgc_render::{GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage, SkySettings};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, SceneEnvironment, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

const SIZE: u32 = 32;

/// 项目 `project.ron` 里的那片天空（太阳盘关掉，只看天空本身的辐射）。
fn project_sky() -> SkySettings {
    SkySettings {
        enabled: true,
        intensity: 1.0,
        zenith: [0.09, 0.26, 0.60],
        horizon: [0.62, 0.72, 0.88],
        ground: [0.15, 0.14, 0.13],
        sun_size: 2.0,
        sun_intensity: 0.0,
    }
}

/// 临时项目目录 + 若干 `.mat`：`MeshCache` 解析资产用相对路径，所以资产必须落在
/// 项目根下（与迁移出来的项目同一路径规则）。
fn material_project(tag: &str, materials: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "xgc_material_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    for (name, body) in materials {
        std::fs::write(root.join(name), body).unwrap();
    }
    root
}

fn cleanup(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
}

/// 渲染一个正对 +Z 的 quad（无光源、环境光 0），返回中心像素。
fn render_quad(root: &Path, asset: &str) -> [u8; 4] {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "material-pbr",
            "rgba8unorm",
            SIZE,
            SIZE,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: Vec::new(),
        ambient: [0.0; 3],
    });
    world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material {
            asset: asset.to_string(),
            ..Material::default()
        },
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 1.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    let mut renderer = Renderer::new(MeshCache::new(root));
    renderer.set_sky(project_sky());
    renderer.render(&context, texture, &world).unwrap();
    let pixels = context.read_texture_rgba8(texture, SIZE, SIZE).unwrap();
    let center = 4 * ((SIZE / 2) * SIZE + SIZE / 2) as usize;
    [
        pixels[center],
        pixels[center + 1],
        pixels[center + 2],
        pixels[center + 3],
    ]
}

fn skip() -> bool {
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return true;
    }
    false
}

#[test]
fn metallic_reflection_takes_its_color_from_f0() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if skip() {
        return;
    }
    // 两片金属只差基色：F0 就是基色，所以反射应当被染色。
    let root = material_project(
        "tint",
        &[
            (
                "white.mat",
                "(base_color: (1.0, 1.0, 1.0, 1.0), metallic: 1.0, roughness: 0.15)",
            ),
            (
                "red.mat",
                "(base_color: (0.9, 0.05, 0.05, 1.0), metallic: 1.0, roughness: 0.15)",
            ),
        ],
    );
    let white = render_quad(&root, "white.mat");
    let red = render_quad(&root, "red.mat");
    // 天空偏蓝：中性 F0 的白金属反射出来就是蓝的（B 明显大于 R）。
    assert!(
        i16::from(white[2]) - i16::from(white[0]) > 8,
        "白金属反射的是蓝天：{white:?}"
    );
    // 红金属的 F0 把反射压成红的（R 明显大于 B）。
    assert!(
        i16::from(red[0]) - i16::from(red[2]) > 20,
        "金属反射必须被基色（F0）染色：红 {red:?} / 白 {white:?}"
    );
    assert_eq!(white[3], 255);
    assert_eq!(red[3], 255);
    cleanup(&root);
}

#[test]
fn dielectric_diffuse_scales_with_albedo() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if skip() {
        return;
    }
    // 两片电介质只差基色：漫反射项 ∝ 基色，环境镜面项（F0 = 0.04）与基色无关，
    // 所以白片必须明显亮于黑片——这正是"电介质保留漫反射"的判据。
    let root = material_project(
        "albedo",
        &[
            (
                "chalk.mat",
                "(base_color: (1.0, 1.0, 1.0, 1.0), metallic: 0.0, roughness: 0.6)",
            ),
            (
                "soot.mat",
                "(base_color: (0.0, 0.0, 0.0, 1.0), metallic: 0.0, roughness: 0.6)",
            ),
        ],
    );
    let chalk = render_quad(&root, "chalk.mat");
    let soot = render_quad(&root, "soot.mat");
    for axis in 0..3 {
        assert!(
            i16::from(chalk[axis]) - i16::from(soot[axis]) >= 20,
            "电介质的漫反射必须随基色变亮：白 {chalk:?} / 黑 {soot:?}（通道 {axis}）"
        );
    }
    cleanup(&root);
}
