//! 真实 GPU 资源创建；没有适配器时测试直接跳过（不把 CI 绑在 GPU 上）。

use xgc_render::{GpuContext, GpuResource, ResourceDesc, ResourceTable, ResourceUsage};

#[test]
fn gpu_resources_are_created_when_adapter_available() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let texture = ResourceDesc::texture2d(
        "color",
        "rgba8unorm",
        4,
        4,
        vec![ResourceUsage::ColorAttachment, ResourceUsage::Sampled],
    );
    assert!(matches!(
        context.create_resource(&texture).unwrap(),
        GpuResource::Texture(_)
    ));

    let buffer = ResourceDesc::buffer("uniforms", "struct", 256, vec![ResourceUsage::Uniform]);
    assert!(matches!(
        context.create_resource(&buffer).unwrap(),
        GpuResource::Buffer(_)
    ));

    let mut table = ResourceTable::new();
    table.add(texture).unwrap();
    table.add(buffer).unwrap();
    let created = context.create_resources(&table).unwrap();
    assert_eq!(created.len(), 2);

    let readback = ResourceDesc::texture2d(
        "readback",
        "rgba8unorm",
        2,
        2,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&readback).unwrap();
    let texture = resource.as_texture().unwrap();
    context
        .clear_texture_rgba(texture, [1.0, 0.0, 0.0, 1.0])
        .unwrap();
    let pixels = context.read_texture_rgba8(texture, 2, 2).unwrap();
    assert_eq!(pixels.len(), 16);
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    assert_eq!(&pixels[12..16], &[255, 0, 0, 255]);

    context.draw_triangle(texture).unwrap();
    let pixels = context.read_texture_rgba8(texture, 2, 2).unwrap();
    assert!((pixels[0] as i32 - 26).abs() <= 1, "实际 {pixels:?}");
    assert!((pixels[1] as i32 - 204).abs() <= 1, "实际 {pixels:?}");
    assert!((pixels[2] as i32 - 51).abs() <= 1, "实际 {pixels:?}");
    assert_eq!(pixels[3], 255);

    let pipeline = context.create_triangle_pipeline(texture.format());
    pipeline
        .draw(&context, texture, [0.2, 0.4, 0.6, 1.0])
        .unwrap();
    let pixels = context.read_texture_rgba8(texture, 2, 2).unwrap();
    assert!((pixels[0] as i32 - 51).abs() <= 1, "实际 {pixels:?}");
    assert!((pixels[1] as i32 - 102).abs() <= 1, "实际 {pixels:?}");
    assert!((pixels[2] as i32 - 153).abs() <= 1, "实际 {pixels:?}");
    assert_eq!(pixels[3], 255);
}

#[test]
fn unsupported_format_is_reported() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let texture = ResourceDesc::texture2d(
        "bad",
        "not-a-format",
        1,
        1,
        vec![ResourceUsage::ColorAttachment],
    );
    assert!(context.create_resource(&texture).is_err());
}
