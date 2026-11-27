//! 内建组件的类型清单同时生成反射目录与世界工厂注册，避免 schema 宣告了世界无法构造的组件。

use xgc_reflect::{Component, Registry};

use super::{
    AnimationLayers, AnimationState, Animator, AudioGain, AudioListener, AudioMixer, AudioSource,
    Bone, Camera, CharacterController, CollisionLayer, GlobalTransform, LodGroup, Material,
    MeshRenderer, Name, Parent, ParticleEmitter, PhysicsBody, PluginComponent, PrefabInstance,
    Script, Skeleton, SkeletonPose, Slope, Transform, UiElement, UiInput, UiInteraction, UiLayout,
    Velocity, World,
};

macro_rules! builtin_components {
    ($($component:ty),+ $(,)?) => {
        pub fn builtin_registry() -> Registry {
            let mut registry = Registry::new();
            $(
                registry.register::<$component>().unwrap_or_else(|error| {
                    panic!("内建组件 {} 注册失败：{error}", <$component as Component>::NAME)
                });
            )+
            registry
        }

        impl World {
            pub(crate) fn register_builtin_components(&mut self) {
                $(self.register_component::<$component>();)+
            }
        }
    };
}

builtin_components![
    Transform,
    Velocity,
    Name,
    Parent,
    GlobalTransform,
    CollisionLayer,
    MeshRenderer,
    Material,
    Camera,
    LodGroup,
    Script,
    AudioSource,
    AudioListener,
    AudioGain,
    AudioMixer,
    PhysicsBody,
    CharacterController,
    Slope,
    Animator,
    Skeleton,
    Bone,
    SkeletonPose,
    AnimationState,
    AnimationLayers,
    UiElement,
    UiLayout,
    UiInteraction,
    UiInput,
    ParticleEmitter,
    PluginComponent,
    PrefabInstance,
];
