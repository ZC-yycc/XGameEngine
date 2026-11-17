//! 天空盒 pass：背景由解析天空填满（渐变 + 太阳盘），几何照常遮挡。
//!
//! 像素期望值全部由 CPU 镜像现算：方向用同一条 `inv(view_proj)` 从 NDC 反推，
//! 与 GPU 天空盒片元的路径逐项一致。

use xgc_render::{
    display_u8, invert, look_at_rh, mul, perspective_rh_zo, sky_color, transform_point, GpuContext,
    MeshCache, Renderer, ResourceDesc, ResourceUsage, SkySettings,
};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, SceneEnvironment, World};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 36;

fn project_sky() -> SkySettings {
    SkySettings {
        enabled: true,
        intensity: 1.0,
        zenith: [0.09, 0.26, 0.60],
        horizon: [0.62, 0.72, 0.88],
        ground: [0.15, 0.14, 0.13],
        sun_size: 2.0,
        // 太阳关掉：像素期望值只取决于天空渐变，不受太阳盘位置影响。
        sun_intensity: 0.0,
    }
}

/// 与 `camera_view_projection` 同一条构造路径（相机在原点朝 -Z）。
fn view_projection() -> [f32; 16] {
    let camera = Camera::default();
    let view = look_at_rh([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
    let aspect = WIDTH as f32 / HEIGHT as f32;
    let projection = perspective_rh_zo(
        camera.fov_y_deg.to_radians(),
        aspect,
        camera.near.max(0.0001),
        camera.far,
    );
    mul(projection, view)
}

/// 屏幕像素（中心取 +0.5）→ 世界方向。
fn pixel_direction(view_proj: [f32; 16], x: u32, y: u32) -> [f32; 3] {
    let ndc_x = ((x as f32 + 0.5) / WIDTH as f32) * 2.0 - 1.0;
    let ndc_y = 1.0 - ((y as f32 + 0.5) / HEIGHT as f32) * 2.0;
    let inverse = invert(view_proj).expect("相机矩阵可逆");
    let far = transform_point(inverse, [ndc_x, ndc_y, 1.0, 1.0]);
    let dir = [far[0] / far[3], far[1] / far[3], far[2] / far[3]];
    let length = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    [dir[0] / length, dir[1] / length, dir[2] / length]
}

fn render(world: &World, sky: SkySettings) -> Vec<u8> {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "sky-box",
            "rgba8unorm-srgb",
            WIDTH,
            HEIGHT,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_sky(sky);
    renderer.render(&context, texture, world).unwrap();
    context.read_texture_rgba8(texture, WIDTH, HEIGHT).unwrap()
}

fn empty_world() -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: Vec::new(),
        ambient: [0.0; 3],
    });
    world.spawn((GlobalTransform::default(), Camera::default()));
    world
}

#[test]
fn sky_box_paints_the_analytic_sky() {
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let sky = project_sky();
    let pixels = render(&empty_world(), sky);
    let view_proj = view_projection();
    // 取三处：天顶附近 / 画面中心（地平线）/ 地面段。
    for (x, y) in [(32u32, 2u32), (32, 18), (32, 34)] {
        let direction = pixel_direction(view_proj, x, y);
        let expected = display_u8(sky_color(&sky, None, direction));
        let offset = 4 * ((y * WIDTH + x) as usize);
        let got = &pixels[offset..offset + 3];
        for axis in 0..3 {
            assert!(
                (i16::from(got[axis]) - i16::from(expected[axis])).abs() <= 2,
                "像素 ({x},{y}) 方向 {direction:?} 必须等于解析天空：实际 {got:?}，期望 {expected:?}"
            );
        }
    }
    // 渐变必须真的存在：天顶（zenith 更深）比地平线暗。
    let top = 4 * ((2 * WIDTH + 32) as usize);
    let middle = 4 * ((18 * WIDTH + 32) as usize);
    assert!(
        pixels[top] < pixels[middle] && pixels[top + 1] < pixels[middle + 1],
        "天空必须有天顶 → 地平线渐变：top={:?} middle={:?}",
        &pixels[top..top + 3],
        &pixels[middle..middle + 3]
    );
}

#[test]
fn geometry_still_occludes_the_sky() {
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let mut world = empty_world();
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, -2.0],
            scale: [1.5, 1.5, 1.5],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://cube".to_string(),
        },
        Material {
            base_color: [1.0, 0.0, 0.0, 1.0],
            ..Material::default()
        },
    ));
    let pixel_at = |pixels: &[u8], x: u32, y: u32| {
        let offset = 4 * ((y * WIDTH + x) as usize);
        [pixels[offset], pixels[offset + 1], pixels[offset + 2]]
    };
    let pixels = render(&world, project_sky());
    // 中心是立方体（红），角落是天空 —— 天空盒不得覆盖几何。
    let center = pixel_at(&pixels, WIDTH / 2, HEIGHT / 2);
    let sky = sky_color(
        &project_sky(),
        None,
        pixel_direction(view_projection(), 2, 2),
    );
    assert!(
        center[0] > center[1] * 2 && center[0] > center[2] * 2,
        "几何必须挡住天空：中心 {center:?}"
    );
    let corner = pixel_at(&pixels, 2, 2);
    let expected = display_u8(sky);
    for axis in 0..3 {
        assert!(
            (i16::from(corner[axis]) - i16::from(expected[axis])).abs() <= 2,
            "角落应显示天空：实际 {corner:?}，期望 {expected:?}"
        );
    }
}
