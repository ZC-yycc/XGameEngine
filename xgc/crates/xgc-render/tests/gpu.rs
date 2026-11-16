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
