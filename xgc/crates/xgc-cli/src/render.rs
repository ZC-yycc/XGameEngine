use std::path::Path;
use std::process::ExitCode;

use serde::Deserialize;
use xgc_asset::scan as scan_assets;
use xgc_render::{
    build_draw_list, load_glb, GpuContext, MeshCache, Pass, RenderGraph, Renderer, ResourceDesc,
    ResourceUsage, SkySettings,
};
use xgc_sim::{HierarchySystem, System, UiLayoutSystem};
use xgc_store::load_world;
use xgc_world::World;

use crate::runtime_cmd::advance_world;

fn parse_frame_count(value: Option<&str>) -> Result<u64, &'static str> {
    value
        .map(str::parse::<u64>)
        .transpose()
        .map(|frames| frames.unwrap_or(0))
        .map_err(|_| "帧数不是整数")
}

/// `project.ron` 的 `render` 节里 XGC 目前真正消费的部分（天空）。
///
/// 旧引擎把画面设置放在项目级而不是场景级；XGC 也按项目级读，缺省（没有
/// `project.ron` / 没有 `sky` 节）= 天空关，画面与这一刀之前逐位相同。
#[derive(Debug, Default, Deserialize)]
struct OldProjectFile {
    #[serde(default)]
    render: OldProjectRender,
}

#[derive(Debug, Default, Deserialize)]
struct OldProjectRender {
    #[serde(default)]
    sky: OldSkyConfig,
}

/// 缺省值必须逐项等于旧 `SkyConfig::default`（天空默认关）。
#[derive(Debug, Deserialize)]
struct OldSkyConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "sky_default_intensity")]
    intensity: f32,
    #[serde(default = "sky_default_zenith")]
    zenith: [f32; 3],
    #[serde(default = "sky_default_horizon")]
    horizon: [f32; 3],
    #[serde(default = "sky_default_ground")]
    ground: [f32; 3],
    #[serde(default = "sky_default_sun_size")]
    sun_size: f32,
    #[serde(default = "sky_default_intensity")]
    sun_intensity: f32,
}

fn sky_default_intensity() -> f32 {
    1.0
}

fn sky_default_zenith() -> [f32; 3] {
    [0.12, 0.28, 0.58]
}

fn sky_default_horizon() -> [f32; 3] {
    [0.58, 0.68, 0.82]
}

fn sky_default_ground() -> [f32; 3] {
    [0.16, 0.15, 0.14]
}

fn sky_default_sun_size() -> f32 {
    1.5
}

impl Default for OldSkyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: sky_default_intensity(),
            zenith: sky_default_zenith(),
            horizon: sky_default_horizon(),
            ground: sky_default_ground(),
            sun_size: sky_default_sun_size(),
            sun_intensity: sky_default_intensity(),
        }
    }
}

/// 读 `project.ron` 的天空节；缺文件 / 缺节返回 `None`。
fn load_sky(project_root: &Path) -> Option<SkySettings> {
    let text = std::fs::read_to_string(project_root.join("project.ron")).ok()?;
    let project: OldProjectFile = ron::from_str(&text).ok()?;
    let sky = project.render.sky;
    Some(
        SkySettings {
            enabled: sky.enabled,
            intensity: sky.intensity,
            zenith: sky.zenith,
            horizon: sky.horizon,
            ground: sky.ground,
            sun_size: sky.sun_size,
            sun_intensity: sky.sun_intensity,
        }
        .sanitized(),
    )
}

fn prepare_frame_world(
    world: World,
    assets_root: &Path,
    frames: u64,
    width: u32,
    height: u32,
) -> Result<World, String> {
    let mut world = advance_world(world, assets_root, frames)?;
    let mut hierarchy = HierarchySystem::default();
    hierarchy.run(&mut world, 0.0);
    UiLayoutSystem {
        viewport: [width as f32, height as f32],
    }
    .run(&mut world, 0.0);
    Ok(world)
}

pub(crate) fn render_graph() -> ExitCode {
    let mut graph = RenderGraph::new();
    let resources = [
        ResourceDesc::texture2d(
            "shadow_map",
            "depth32float",
            2048,
            2048,
            vec![ResourceUsage::DepthAttachment, ResourceUsage::Sampled],
        ),
        ResourceDesc::texture2d(
            "color",
            "rgba16float",
            1280,
            720,
            vec![ResourceUsage::ColorAttachment, ResourceUsage::Sampled],
        ),
        ResourceDesc::texture2d(
            "backbuffer",
            "bgra8unorm",
            1280,
            720,
            vec![ResourceUsage::ColorAttachment],
        ),
    ];
    for resource in resources {
        if let Err(err) = graph.add_resource(resource) {
            eprintln!("声明渲染资源失败：{err}");
            return ExitCode::from(1);
        }
    }
    let passes = [
        Pass::new("shadow", &[], &["shadow_map"]),
        Pass::new("opaque", &["shadow_map"], &["color"]),
        Pass::new("post", &["color"], &["backbuffer"]),
    ];
    for pass in passes {
        if let Err(err) = graph.add(pass) {
            eprintln!("构建渲染图失败：{err}");
            return ExitCode::from(1);
        }
    }
    match graph.order_names() {
        Ok(order) => {
            println!(
                "render-graph: {} 个 pass -> {}",
                order.len(),
                order.join(" -> ")
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("渲染图排序失败：{err}");
            ExitCode::from(1)
        }
    }
}

pub(crate) fn gpu_probe() -> ExitCode {
    match GpuContext::headless() {
        Ok(context) => {
            let info = context.adapter_info();
            println!("gpu: {} / {:?}", info.name, info.backend);
            let desc = ResourceDesc::texture2d(
                "probe",
                "rgba8unorm",
                4,
                4,
                vec![ResourceUsage::ColorAttachment, ResourceUsage::Sampled],
            );
            match context.create_resource(&desc) {
                Ok(resource) => {
                    let Some(texture) = resource.as_texture() else {
                        eprintln!("GPU 资源不是纹理");
                        return ExitCode::from(1);
                    };
                    if let Err(err) = context.clear_texture_rgba(texture, [1.0, 0.0, 0.0, 1.0]) {
                        eprintln!("清屏失败：{err}");
                        return ExitCode::from(1);
                    }
                    match context.read_texture_rgba8(texture, 4, 4) {
                        Ok(pixels) => {
                            println!("gpu: clear pass + readback {:?}", &pixels[..4]);
                            if let Err(err) = context.draw_triangle(texture) {
                                eprintln!("三角形绘制失败：{err}");
                                return ExitCode::from(1);
                            }
                            match context.read_texture_rgba8(texture, 4, 4) {
                                Ok(pixels) => {
                                    println!("gpu: triangle pixel {:?}", &pixels[..4]);
                                    let pipeline =
                                        context.create_triangle_pipeline(texture.format());
                                    if let Err(err) =
                                        pipeline.draw(&context, texture, [0.2, 0.4, 0.6, 1.0])
                                    {
                                        eprintln!("材质三角形绘制失败：{err}");
                                        return ExitCode::from(1);
                                    }
                                    match context.read_texture_rgba8(texture, 4, 4) {
                                        Ok(pixels) => {
                                            println!("gpu: material pixel {:?}", &pixels[..4]);
                                            ExitCode::SUCCESS
                                        }
                                        Err(err) => {
                                            eprintln!("材质回读失败：{err}");
                                            ExitCode::from(1)
                                        }
                                    }
                                }
                                Err(err) => {
                                    eprintln!("三角形回读失败：{err}");
                                    ExitCode::from(1)
                                }
                            }
                        }
                        Err(err) => {
                            eprintln!("回读失败：{err}");
                            ExitCode::from(1)
                        }
                    }
                }
                Err(err) => {
                    eprintln!("创建 GPU 资源失败：{err}");
                    ExitCode::from(1)
                }
            }
        }
        Err(err) => {
            eprintln!("GPU 不可用：{err}");
            ExitCode::from(1)
        }
    }
}

pub(crate) fn mesh_probe(path: Option<&str>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("用法：xgc mesh-probe <glb 路径>");
        return ExitCode::from(2);
    };
    match load_glb(Path::new(path)) {
        Ok(mesh) => {
            println!(
                "mesh: {} 个顶点，{} 个索引",
                mesh.positions.len(),
                mesh.indices.len()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("加载网格失败：{err}");
            ExitCode::from(1)
        }
    }
}

pub(crate) fn render_frame(
    input: Option<&str>,
    output: Option<&str>,
    assets_root: Option<&str>,
    width: Option<&str>,
    height: Option<&str>,
    frames: Option<&str>,
) -> ExitCode {
    let (Some(input), Some(output)) = (input, output) else {
        eprintln!("用法：xgc render-frame <世界.json> <out.ppm> [资产根] [宽] [高] [帧数]");
        return ExitCode::from(2);
    };
    let data = match load_world(Path::new(input)) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("读取世界失败：{err}");
            return ExitCode::from(1);
        }
    };
    let mut world = World::new();
    if let Err(message) = world.restore_world_data(&data) {
        eprintln!("恢复世界失败：{message}");
        return ExitCode::from(1);
    }
    let width = width
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(320);
    let height = height
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(180);
    let frames = match parse_frame_count(frames) {
        Ok(frames) => frames,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let world = match prepare_frame_world(
        world,
        Path::new(assets_root.unwrap_or(".")),
        frames,
        width,
        height,
    ) {
        Ok(world) => world,
        Err(err) => {
            eprintln!("推进世界失败：{err}");
            return ExitCode::from(1);
        }
    };
    let context = match GpuContext::headless() {
        Ok(context) => context,
        Err(err) => {
            eprintln!("GPU 不可用：{err}");
            return ExitCode::from(1);
        }
    };
    let desc = ResourceDesc::texture2d(
        "frame",
        "rgba8unorm-srgb",
        width,
        height,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = match context.create_resource(&desc) {
        Ok(resource) => resource,
        Err(err) => {
            eprintln!("创建渲染目标失败：{err}");
            return ExitCode::from(1);
        }
    };
    let Some(texture) = resource.as_texture() else {
        eprintln!("渲染目标不是纹理");
        return ExitCode::from(1);
    };
    let meshes = match assets_root {
        Some(root) => match scan_assets(Path::new(root)) {
            Ok(catalog) => MeshCache::with_catalog(root, catalog),
            Err(err) => {
                eprintln!("扫描资产失败：{err}");
                return ExitCode::from(1);
            }
        },
        None => MeshCache::new("."),
    };
    let mut renderer = Renderer::new(meshes);
    renderer.set_clear_color([0.62, 0.72, 0.84, 1.0]);
    // 天空开着的项目：背景取地平线色（旧端天空盒在地平线附近就是它），
    // 环境照明由渲染器从同一份解析天空投影出来。
    if let Some(sky) = load_sky(Path::new(assets_root.unwrap_or("."))) {
        if sky.enabled {
            let [r, g, b] = sky.horizon.map(|channel| channel * sky.intensity);
            renderer.set_clear_color([f64::from(r), f64::from(g), f64::from(b), 1.0]);
        }
        renderer.set_sky(sky);
    }
    if std::env::var_os("XGC_DEBUG_DRAWS").is_some() {
        for item in build_draw_list(&world).items {
            eprintln!(
                "draw entity={:?} model={} color={:?} t={:?}",
                item.entity, item.model, item.color, item.transform
            );
        }
    }
    // 截图走旧的"顺光自动取景"，不用场景相机：否则与旧端截图连构图都不同。
    let stats = match renderer.render_shot(&context, texture, &world) {
        Ok(stats) => stats,
        Err(err) => {
            eprintln!("渲染失败：{err}");
            return ExitCode::from(1);
        }
    };
    let pixels = match context.read_texture_rgba8(texture, width, height) {
        Ok(pixels) => pixels,
        Err(err) => {
            eprintln!("回读失败：{err}");
            return ExitCode::from(1);
        }
    };
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    ppm.reserve((width * height * 3) as usize);
    for pixel in pixels.chunks_exact(4) {
        ppm.extend_from_slice(&pixel[..3]);
    }
    if let Err(err) = std::fs::write(Path::new(output), &ppm) {
        eprintln!("写入 PPM 失败：{err}");
        return ExitCode::from(1);
    }
    println!(
        "render-frame: frame={frames} hash={:016x}，{} 个场景 draw / {} 次 draw call / {} 个 UI draw，{}x{} -> {}",
        world.state_hash(),
        stats.draws,
        stats.draw_calls,
        stats.ui_draws,
        width,
        height,
        output
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{parse_frame_count, prepare_frame_world};
    use xgc_world::{
        GlobalTransform, Name, Parent, Script, Transform, UiElement, UiLayout, Velocity, World,
    };

    /// `project.ron` 的天空节必须按旧 `SkyConfig` 的字段与缺省值读出来。
    #[test]
    fn reads_sky_section_from_project_ron() {
        let root = std::path::Path::new("../../../projects/XGame");
        if !root.join("project.ron").exists() {
            eprintln!("skip: 没有旧项目 project.ron");
            return;
        }
        let sky = super::load_sky(root).expect("showcase 项目应带 sky 节");
        assert!(sky.enabled, "项目里天空是开着的");
        assert_eq!(sky.intensity, 1.0);
        assert_eq!(sky.zenith, [0.09, 0.26, 0.60]);
        assert_eq!(sky.horizon, [0.62, 0.72, 0.88]);
        assert_eq!(sky.ground, [0.15, 0.14, 0.13]);
        assert_eq!(sky.sun_size, 2.0);
        assert_eq!(sky.sun_intensity, 1.0);
        // 没有 project.ron 的资产根：天空关（画面与接入前一致）。
        assert!(super::load_sky(std::path::Path::new("does-not-exist")).is_none());
    }

    #[test]
    fn frame_count_defaults_to_zero_and_rejects_invalid_values() {
        assert_eq!(parse_frame_count(None), Ok(0));
        assert_eq!(parse_frame_count(Some("150")), Ok(150));
        assert!(parse_frame_count(Some("-1")).is_err());
        assert!(parse_frame_count(Some("nope")).is_err());
    }

    #[test]
    fn zero_frames_skip_scripts_and_last_script_writes_refresh_derived_data() {
        let dir = std::env::temp_dir().join(format!(
            "xgc_render_frame_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("update.lua"),
            "function update(ctx)\n\
             set_pos(ctx, 4.0, 1.0, 0.0)\n\
             ui_set_size(ctx, entity_id(ctx, 'Panel'), 123.0, 45.0)\n\
             end\n",
        )
        .unwrap();
        let build_world = || {
            let mut world = World::new();
            let root = world.spawn((
                Transform {
                    position: [-5.0, 1.0, 0.0],
                    ..Transform::default()
                },
                GlobalTransform::default(),
                Script {
                    path: "update.lua".to_string(),
                },
            ));
            let root_id = world.entity(root).unwrap();
            let child = world.spawn((
                Transform {
                    position: [0.0, 2.0, 0.0],
                    ..Transform::default()
                },
                GlobalTransform::default(),
                Parent { entity: root_id },
            ));
            let panel = world.spawn((
                Name {
                    value: "Panel".to_string(),
                },
                UiElement {
                    offset_max: [80.0, 30.0],
                    ..UiElement::default()
                },
                UiLayout::default(),
            ));
            (world, root, child, panel)
        };

        let (world, root, child, panel) = build_world();
        let world = prepare_frame_world(world, &dir, 0, 1280, 720).unwrap();
        assert_eq!(world.get::<Transform>(root).unwrap().position[0], -5.0);
        assert_eq!(
            world.get::<GlobalTransform>(child).unwrap().position,
            [-5.0, 3.0, 0.0]
        );
        assert_eq!(
            world.get::<UiLayout>(panel).unwrap().rect[2..],
            [80.0, 30.0]
        );

        let (world, root, child, panel) = build_world();
        let world = prepare_frame_world(world, &dir, 1, 1280, 720).unwrap();
        assert_eq!(world.get::<Transform>(root).unwrap().position[0], 4.0);
        assert_eq!(
            world.get::<GlobalTransform>(child).unwrap().position,
            [4.0, 3.0, 0.0]
        );
        assert_eq!(
            world.get::<UiElement>(panel).unwrap().offset_max,
            [123.0, 45.0]
        );
        assert_eq!(
            world.get::<UiLayout>(panel).unwrap().rect[2..],
            [123.0, 45.0]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn frame_preparation_advances_tick_and_composes_parent_chain() {
        let mut world = World::new();
        let root = world.spawn((
            Transform {
                position: [-5.0, 1.0, 0.0],
                ..Transform::default()
            },
            GlobalTransform::default(),
        ));
        let root_id = world.entity(root).unwrap();
        let child = world.spawn((
            Transform {
                position: [0.0, 2.0, 0.0],
                ..Transform::default()
            },
            GlobalTransform::default(),
            Parent { entity: root_id },
        ));
        let mover = world.spawn((
            Transform::default(),
            GlobalTransform::default(),
            Velocity {
                linear: [1.0, 0.0, 0.0],
            },
        ));

        let world = prepare_frame_world(world, Path::new("missing"), 2, 1280, 720).unwrap();

        assert_eq!(
            world.get::<GlobalTransform>(child).unwrap().position,
            [-5.0, 3.0, 0.0]
        );
        let expected = 2.0 / 60.0;
        let actual = world.get::<Transform>(mover).unwrap().position[0];
        assert!((actual - expected).abs() < 1e-6, "actual={actual}");
    }
}
