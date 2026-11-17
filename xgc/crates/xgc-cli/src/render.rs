use std::path::Path;
use std::process::ExitCode;

use xgc_asset::scan as scan_assets;
use xgc_render::{
    build_draw_list, load_glb, GpuContext, MeshCache, Pass, RenderGraph, Renderer, ResourceDesc,
    ResourceUsage,
};
use xgc_sim::{HierarchySystem, System, UiLayoutSystem};
use xgc_store::load_world;
use xgc_world::World;

fn prepare_frame_world(world: &mut World, width: u32, height: u32) {
    let mut hierarchy = HierarchySystem::default();
    hierarchy.run(world, 0.0);
    UiLayoutSystem {
        viewport: [width as f32, height as f32],
    }
    .run(world, 0.0);
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
) -> ExitCode {
    let (Some(input), Some(output)) = (input, output) else {
        eprintln!("用法：xgc render-frame <世界.json> <out.ppm> [资产根] [宽] [高]");
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
    let context = match GpuContext::headless() {
        Ok(context) => context,
        Err(err) => {
            eprintln!("GPU 不可用：{err}");
            return ExitCode::from(1);
        }
    };
    let width = width
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(320);
    let height = height
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(180);
    prepare_frame_world(&mut world, width, height);
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
    if std::env::var_os("XGC_DEBUG_DRAWS").is_some() {
        for item in build_draw_list(&world).items {
            eprintln!(
                "draw entity={:?} model={} color={:?} t={:?}",
                item.entity, item.model, item.color, item.transform
            );
        }
    }
    let stats = match renderer.render(&context, texture, &world) {
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
        "render-frame: {} 个场景 draw / {} 次 draw call / {} 个 UI draw，{}x{} -> {}",
        stats.draws, stats.draw_calls, stats.ui_draws, width, height, output
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::prepare_frame_world;
    use xgc_world::{GlobalTransform, Parent, Transform, World};

    #[test]
    fn frame_preparation_composes_parent_chain() {
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

        prepare_frame_world(&mut world, 1280, 720);

        assert_eq!(
            world.get::<GlobalTransform>(child).unwrap().position,
            [-5.0, 3.0, 0.0]
        );
    }
}
