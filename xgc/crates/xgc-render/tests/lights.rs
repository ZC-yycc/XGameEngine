//! 多光源照明：方向光 / 点光混排、点光衰减、8 盏上限、逐盏能量叠加。
//!
//! 判据尽量用"两帧比较"而不是手算绝对亮度：着色里的高光项与 ACES 曲线都不好
//! 手抄，但"开 / 关"、"远 / 近"、"两盏 vs 一盏更亮的"关系是稳定的。

use std::sync::Mutex;

use xgc_render::{GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{
    Camera, GlobalTransform, LightKind, Material, MeshRenderer, SceneEnvironment, SceneLight, World,
};

static GPU_LOCK: Mutex<()> = Mutex::new(());
const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;

fn point(position: [f32; 3], intensity: f32, range: f32) -> SceneLight {
    SceneLight {
        name: "lamp".to_string(),
        kind: LightKind::Point,
        position,
        intensity,
        range,
        ..SceneLight::default()
    }
}

fn directional(intensity: f32) -> SceneLight {
    SceneLight::directional("sun", [0.0, 0.0, -1.0], [1.0; 3], intensity)
}

/// 一块正对相机的 quad + 一盏相机；灯光与环境由调用方给。
fn scene(lights: Vec<SceneLight>, ambient: [f32; 3]) -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment { lights, ambient });
    world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material::default(),
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 6.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    world
}

fn render(world: &World) -> Vec<u8> {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "lights",
            "rgba8unorm",
            WIDTH,
            HEIGHT,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.render(&context, texture, world).unwrap();
    context.read_texture_rgba8(texture, WIDTH, HEIGHT).unwrap()
}

/// 画面中心像素（quad 正中央）。
fn center(pixels: &[u8]) -> [u8; 4] {
    let offset = (((HEIGHT / 2) * WIDTH + WIDTH / 2) * 4) as usize;
    [
        pixels[offset],
        pixels[offset + 1],
        pixels[offset + 2],
        pixels[offset + 3],
    ]
}

#[test]
fn point_light_illuminates_a_surface() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let lit = render(&scene(vec![point([0.0, 0.0, 3.0], 3.0, 10.0)], [0.0; 3]));
    let dark = render(&scene(vec![], [0.0; 3]));
    assert!(
        center(&lit)[0] > 20 && center(&dark)[0] == 0,
        "点光必须照亮正对它的表面：亮 {:?} / 黑 {:?}",
        center(&lit),
        center(&dark),
    );
}

#[test]
fn point_light_outside_its_range_is_ignored() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let dark = render(&scene(vec![], [0.02; 3]));
    // 灯在 3 米外、半径只有 1 米：范围外的片元整盏跳过，画面应与没有灯逐字节相同。
    let out_of_range = render(&scene(vec![point([0.0, 0.0, 3.0], 3.0, 1.0)], [0.02; 3]));
    let illegal = render(&scene(
        vec![point([0.0, 0.0, 3.0], 3.0, f32::NAN)],
        [0.02; 3],
    ));
    assert!(out_of_range == dark, "范围外的点光必须整盏跳过，画面不能变",);
    assert!(illegal == dark, "半径非有限的点光按关灯处理，画面不能变",);
}

#[test]
fn point_light_fades_with_distance() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let near = render(&scene(vec![point([0.0, 0.0, 1.5], 3.0, 10.0)], [0.0; 3]));
    let far = render(&scene(vec![point([0.0, 0.0, 4.5], 3.0, 10.0)], [0.0; 3]));
    assert!(
        center(&near)[0] > center(&far)[0] + 10,
        "点光必须随距离衰减：近 {:?} / 远 {:?}",
        center(&near),
        center(&far),
    );
}

#[test]
fn stacking_two_lights_equals_one_brighter_light() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    // 同位置、同半径：两盏强度 1 的光与一盏强度 2 的光在着色里是同一份能量和。
    let stacked = render(&scene(
        vec![
            point([0.0, 0.0, 3.0], 1.0, 10.0),
            point([0.0, 0.0, 3.0], 1.0, 10.0),
        ],
        [0.0; 3],
    ));
    let brighter = render(&scene(vec![point([0.0, 0.0, 3.0], 2.0, 10.0)], [0.0; 3]));
    assert!(
        stacked == brighter,
        "逐盏累加必须等于同能量的单盏：两盏 {:?} / 单盏 {:?}",
        center(&stacked),
        center(&brighter),
    );
}

#[test]
fn only_the_first_eight_lights_take_part() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    // 8 盏灯就已经填满表。强度逐盏不同、第 9 盏又是最强的一盏：只要它被塞进
    // 某个槽位（哪怕是覆盖第 0 盏），中心亮度就会变。
    let mut lights: Vec<SceneLight> = (0..8)
        .map(|index| point([0.0, 0.0, 3.0], 0.3 + index as f32 * 0.05, 10.0))
        .collect();
    let eight = render(&scene(lights.clone(), [0.0; 3]));
    lights.push(point([0.0, 0.0, 3.0], 2.0, 10.0));
    let nine = render(&scene(lights, [0.0; 3]));
    assert!(
        eight == nine,
        "第 9 盏灯必须被丢掉（uniform 只有 8 槽）：8 盏 {:?} / 9 盏 {:?}",
        center(&eight),
        center(&nine),
    );
}

#[test]
fn directional_and_point_lights_add_up() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let sun = render(&scene(vec![directional(1.0)], [0.0; 3]));
    let lamp = render(&scene(vec![point([0.0, 0.0, 3.0], 1.0, 10.0)], [0.0; 3]));
    let both = render(&scene(
        vec![directional(1.0), point([0.0, 0.0, 3.0], 1.0, 10.0)],
        [0.0; 3],
    ));
    let (sun_value, lamp_value, both_value) = (center(&sun)[0], center(&lamp)[0], center(&both)[0]);
    assert!(
        both_value > sun_value.max(lamp_value) + 5,
        "方向光与点光必须叠加：方向 {:?} / 点光 {:?} / 一起 {:?}",
        center(&sun),
        center(&lamp),
        center(&both),
    );
}
