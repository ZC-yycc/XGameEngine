use std::path::Path;
use std::process::ExitCode;

use xgc_asset::{scan as scan_assets, AssetPaths, SearchPaths};
use xgc_render::{
    build_draw_list, load_glb, GpuContext, MeshCache, Pass, RenderGraph, Renderer, ResourceDesc,
    ResourceUsage,
};
use xgc_sim::{HierarchySystem, System, UiLayoutSystem};
use xgc_store::load_world;
use xgc_world::World;

use crate::project_render::load_render;
use crate::runtime_cmd::{advance_world_with_particles, load_search_paths};

fn parse_frame_count(value: Option<&str>) -> Result<u64, &'static str> {
    value
        .map(str::parse::<u64>)
        .transpose()
        .map(|frames| frames.unwrap_or(0))
        .map_err(|_| "帧数不是整数")
}

#[cfg(test)]
fn prepare_frame_world(
    world: World,
    assets_root: &Path,
    frames: u64,
    width: u32,
    height: u32,
) -> Result<World, String> {
    prepare_frame_world_with_particles(world, assets_root, frames, width, height)
        .map(|(world, _)| world)
}

fn prepare_frame_world_with_particles(
    world: World,
    assets_root: &Path,
    frames: u64,
    width: u32,
    height: u32,
) -> Result<(World, Vec<xgc_world::ParticleQuad>), String> {
    let (mut world, particles) = advance_world_with_particles(world, assets_root, frames)?;
    let mut hierarchy = HierarchySystem::default();
    hierarchy.run(&mut world, 0.0);
    UiLayoutSystem {
        viewport: [width as f32, height as f32],
    }
    .run(&mut world, 0.0);
    Ok((world, particles))
}

/// 渲染一帧并回读 RGBA8；`render-frame` 与 `gate` 共用这一份渲染配置。
///
/// 天空 / 曝光 / 后处理的开关都从项目的 render 配置读：两个命令如果各写一遍，
/// 割接门禁就会在一条「和交付不一样」的渲染路径上判定通过。相机优先级由
/// `Renderer::render_shot` 决定（显式机位 > 场景相机 > 顺光取景）。
pub(crate) fn render_world_rgba(
    context: &GpuContext,
    assets_root: Option<&str>,
    world: &World,
    particles: Vec<xgc_world::ParticleQuad>,
    width: u32,
    height: u32,
) -> Result<(Vec<u8>, xgc_render::RenderStats), String> {
    let desc = ResourceDesc::texture2d(
        "frame",
        "rgba8unorm-srgb",
        width,
        height,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context
        .create_resource(&desc)
        .map_err(|err| format!("创建渲染目标失败：{err}"))?;
    let texture = match resource.as_texture() {
        Some(texture) => texture,
        None => return Err("渲染目标不是纹理".to_string()),
    };
    let root = Path::new(assets_root.unwrap_or("."));
    let search = SearchPaths::new(&load_search_paths(root)?);
    let catalog = scan_assets(root).map_err(|err| format!("扫描资产失败：{err}"))?;
    let meshes = MeshCache::with_assets(AssetPaths::new(root, search, Some(catalog)));
    let mut renderer = Renderer::new(meshes);
    renderer.set_particles(particles);
    renderer.set_clear_color([0.62, 0.72, 0.84, 1.0]);
    // 天空开着的项目：背景取地平线色（旧端天空盒在地平线附近就是它），
    // 环境照明由渲染器从同一份解析天空投影出来。
    if let Some(settings) = load_render(root) {
        if settings.sky.enabled {
            let [r, g, b] = settings
                .sky
                .horizon
                .map(|channel| channel * settings.sky.intensity);
            renderer.set_clear_color([f64::from(r), f64::from(g), f64::from(b), 1.0]);
        }
        renderer.set_sky(settings.sky);
        renderer.set_lod_bias(settings.lod_bias);
        // 曝光缺省 1.0；光晕默认关（旧端 `render.bloom` 省略时 intensity = 0）。
        renderer.set_exposure(settings.exposure);
        renderer.set_bloom(settings.bloom);
        renderer.set_fxaa(settings.fxaa);
        // 软阴影 / 级间混合默认关：关着时着色器走老路径，帧输出逐字节不变。
        renderer.set_shadow_settings(settings.shadow);
        // TAA 默认关：关着时不建历史、相机不抖动，帧输出逐字节不变。
        renderer.set_taa(settings.taa);
        // SSAO 默认关：关着时不建 AO 纹理、不跑 pass，帧输出逐字节不变。
        renderer.set_ssao(settings.ssao);
    }
    if std::env::var_os("XGC_DEBUG_DRAWS").is_some() {
        for item in build_draw_list(world).items {
            eprintln!(
                "draw entity={:?} model={} color={:?} t={:?}",
                item.entity, item.model, item.color, item.transform
            );
        }
    }
    let stats = renderer
        .render_shot(context, texture, world)
        .map_err(|err| format!("渲染失败：{err}"))?;
    let pixels = context
        .read_texture_rgba8(texture, width, height)
        .map_err(|err| format!("回读失败：{err}"))?;
    Ok((pixels, stats))
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
    let (world, particles) = match prepare_frame_world_with_particles(
        world,
        Path::new(assets_root.unwrap_or(".")),
        frames,
        width,
        height,
    ) {
        Ok(prepared) => prepared,
        Err(err) => {
            eprintln!("推进世界失败：{err}");
            return ExitCode::from(1);
        }
    };
    let rendered = GpuContext::headless()
        .map_err(|err| format!("GPU 不可用：{err}"))
        .and_then(|context| {
            render_world_rgba(&context, assets_root, &world, particles, width, height)
        });
    let (pixels, stats) = match rendered {
        Ok(rendered) => rendered,
        Err(err) => {
            eprintln!("{err}");
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
