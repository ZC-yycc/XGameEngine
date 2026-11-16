//! 内建注册表回归：CLI / Agent / xtask 共用同一份清单。

use xgc_world::builtin_registry;

#[test]
fn builtin_registry_has_stable_types() {
    let registry = builtin_registry();
    let schema = registry.schema();
    let names: Vec<_> = schema.types.iter().map(|ty| ty.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "core.transform",
            "core.velocity",
            "core.name",
            "core.parent",
            "core.global_transform",
            "render.mesh",
            "render.material",
            "render.camera",
            "script.lua",
            "audio.source",
        ]
    );
    assert!(registry.validate().is_empty());

    let second = builtin_registry().schema();
    assert_eq!(schema.types.len(), second.types.len());
    for (first, second) in schema.types.iter().zip(second.types.iter()) {
        assert_eq!(first.id, second.id);
        assert_eq!(first.name, second.name);
        assert_eq!(first.fields.len(), second.fields.len());
    }
}
