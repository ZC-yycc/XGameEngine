//! 内建组件注册表的唯一入口：CLI / Agent / schema 门禁都从这里取，避免各处
//! 再抄一份组件清单。

use xgc_reflect::Registry;

use super::{
    AudioSource, Camera, GlobalTransform, Material, MeshRenderer, Name, Parent, PhysicsBody,
    Script, Transform, Velocity,
};

pub fn builtin_registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .register::<Transform>()
        .expect("内置组件注册失败：core.transform");
    registry
        .register::<Velocity>()
        .expect("内置组件注册失败：core.velocity");
    registry
        .register::<Name>()
        .expect("内置组件注册失败：core.name");
    registry
        .register::<Parent>()
        .expect("内置组件注册失败：core.parent");
    registry
        .register::<GlobalTransform>()
        .expect("内置组件注册失败：core.global_transform");
    registry
        .register::<MeshRenderer>()
        .expect("内置组件注册失败：render.mesh");
    registry
        .register::<Material>()
        .expect("内置组件注册失败：render.material");
    registry
        .register::<Camera>()
        .expect("内置组件注册失败：render.camera");
    registry
        .register::<Script>()
        .expect("内置组件注册失败：script.lua");
    registry
        .register::<AudioSource>()
        .expect("内置组件注册失败：audio.source");
    registry
        .register::<PhysicsBody>()
        .expect("内置组件注册失败：physics.body");
    registry
}
