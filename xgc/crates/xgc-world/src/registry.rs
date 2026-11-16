//! 内建组件注册表的唯一入口：CLI / Agent / schema 门禁都从这里取，避免各处
//! 再抄一份组件清单。

use xgc_reflect::Registry;

use super::{
    Animator, AudioSource, Camera, CharacterController, GlobalTransform, LodGroup, Material,
    MeshRenderer, Name, Parent, ParticleEmitter, PhysicsBody, PluginComponent, Script, Transform,
    UiElement, UiInput, UiInteraction, UiLayout, Velocity,
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
        .register::<LodGroup>()
        .expect("内置组件注册失败：render.lod");
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
        .register::<Animator>()
        .expect("内置组件注册失败：animation.animator");
    registry
        .register::<CharacterController>()
        .expect("内置组件注册失败：physics.character");
    registry
        .register::<UiElement>()
        .expect("内置组件注册失败：ui.element");
    registry
        .register::<UiLayout>()
        .expect("内置组件注册失败：ui.layout");
    registry
        .register::<UiInteraction>()
        .expect("内置组件注册失败：ui.interaction");
    registry
        .register::<UiInput>()
        .expect("内置组件注册失败：ui.input");
    registry
        .register::<ParticleEmitter>()
        .expect("内置组件注册失败：particle.emitter");
    registry
        .register::<PluginComponent>()
        .expect("内置组件注册失败：plugin.rust");
    registry
}
