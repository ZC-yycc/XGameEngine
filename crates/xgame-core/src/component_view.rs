//! 旧场景格式的统一组件视图。
//!
//! 这里读写的仍是 `Entity` 原字段：不引入第二份可变配置，也不把运行时
//! `ComponentMap` 误当成可保存组件。Rust/Lua 的下标地址只在当前列表结构不变时有效；
//! 稳定实例 ID 需要下一版场景格式，不能拿这里的槽位冒充。

use std::fmt;

use crate::inspect::FieldValues;
use crate::scene::Entity;

/// 内建组件的旧格式类型地址与编辑器显示名都在这一处定义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinComponent {
    Transform,
    Render,
    Ui,
    Animator,
    Physics,
    Character,
    Audio,
    Camera,
    Particle,
    MaterialOverride,
}

impl BuiltinComponent {
    pub const ALL: [Self; 10] = [
        Self::Transform,
        Self::Render,
        Self::Ui,
        Self::Animator,
        Self::Physics,
        Self::Character,
        Self::Audio,
        Self::Camera,
        Self::Particle,
        Self::MaterialOverride,
    ];

    /// Transform 是对象固有属性；Render 目前由多个现有字段共同表达。
    pub const ADDABLE: [Self; 8] = [
        Self::Ui,
        Self::Animator,
        Self::Physics,
        Self::Character,
        Self::Audio,
        Self::Camera,
        Self::Particle,
        Self::MaterialOverride,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Transform => "transform",
            Self::Render => "render",
            Self::Ui => "ui",
            Self::Animator => "animator",
            Self::Physics => "physics",
            Self::Character => "character",
            Self::Audio => "audio",
            Self::Camera => "camera",
            Self::Particle => "particle",
            Self::MaterialOverride => "material_override",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Transform => "Transform",
            Self::Render => "Render",
            Self::Ui => "UI Element",
            Self::Animator => "Animator",
            Self::Physics => "Physics",
            Self::Character => "Character",
            Self::Audio => "Audio Source",
            Self::Camera => "Camera",
            Self::Particle => "Particle Emitter",
            Self::MaterialOverride => "Material Override",
        }
    }

    pub const fn addable(self) -> bool {
        !matches!(self, Self::Transform | Self::Render)
    }

    pub fn present(self, entity: &Entity) -> bool {
        match self {
            Self::Transform => true,
            Self::Render => {
                !entity.model.is_empty()
                    || !entity.lod.is_empty()
                    || entity.material_asset.is_some()
                    || !entity.morph_weights.is_empty()
            }
            Self::Ui => entity.ui.is_some(),
            Self::Animator => entity.animator.is_some(),
            Self::Physics => entity.physics.is_some(),
            Self::Character => entity.controller.is_some(),
            Self::Audio => entity.audio.is_some(),
            Self::Camera => entity.camera.is_some(),
            Self::Particle => entity.particle.is_some(),
            Self::MaterialOverride => entity.material.is_some(),
        }
    }
}

/// 旧格式中 Rust / Lua 地址使用槽位，重排、删除后须重新查询。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentAddress {
    Builtin(BuiltinComponent),
    Rust(usize),
    Lua(usize),
}

impl fmt::Display for ComponentAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin(kind) => write!(f, "builtin:{}", kind.code()),
            Self::Rust(slot) => write!(f, "rust:{slot}"),
            Self::Lua(slot) => write!(f, "lua:{slot}"),
        }
    }
}

/// 对已有字段的借用视图；不会克隆运行时状态或分配额外的组件值。
#[derive(Debug)]
pub struct AttachedComponent<'a> {
    pub address: ComponentAddress,
    pub name: &'a str,
    pub params: Option<&'a FieldValues>,
}

impl Entity {
    /// 面向编辑器和工具的统一发现入口；低频调用，绝不放入每帧系统热路径。
    pub fn attached_components(&self) -> Vec<AttachedComponent<'_>> {
        let mut out = Vec::with_capacity(
            BuiltinComponent::ALL.len() + self.behaviors.len() + self.scripts.len(),
        );
        for kind in BuiltinComponent::ALL {
            if kind.present(self) {
                out.push(AttachedComponent {
                    address: ComponentAddress::Builtin(kind),
                    name: kind.name(),
                    params: None,
                });
            }
        }
        for (slot, name) in self.behaviors.iter().enumerate() {
            out.push(AttachedComponent {
                address: ComponentAddress::Rust(slot),
                name,
                params: self.behavior_params.get(name),
            });
        }
        for (slot, path) in self.scripts.iter().enumerate() {
            out.push(AttachedComponent {
                address: ComponentAddress::Lua(slot),
                name: path,
                params: None,
            });
        }
        out
    }

    /// 只创建缺失的内建组件；用旧字段作唯一数据源。
    pub fn add_builtin_component(&mut self, kind: BuiltinComponent) -> bool {
        if !kind.addable() || kind.present(self) {
            return false;
        }
        match kind {
            BuiltinComponent::Ui => self.ui = Some(crate::UiElement::panel(320.0, 180.0)),
            BuiltinComponent::Animator => self.animator = Some(crate::Animator::default()),
            BuiltinComponent::Physics => self.physics = Some(crate::PhysicsBody::default()),
            BuiltinComponent::Character => {
                self.controller = Some(crate::CharacterController::default());
            }
            BuiltinComponent::Audio => self.audio = Some(crate::AudioSource::default()),
            BuiltinComponent::Camera => self.camera = Some(crate::Camera::default()),
            BuiltinComponent::Particle => self.particle = Some(crate::ParticleEmitter::default()),
            BuiltinComponent::MaterialOverride => {
                self.material = Some(crate::MaterialOverride::default());
            }
            BuiltinComponent::Transform | BuiltinComponent::Render => return false,
        }
        true
    }

    /// 注册名是否可用由宿主注册表判断；这里负责防重复与写入原来的列表。
    pub fn add_registered_component(&mut self, name: &str) -> bool {
        if name.is_empty() || self.behaviors.iter().any(|existing| existing == name) {
            return false;
        }
        self.behaviors.push(name.to_owned());
        true
    }

    /// 脚本资产是否存在由项目边界判断；旧格式的挂载规则仍是一条路径一份。
    pub fn attach_lua_script(&mut self, path: &str) -> bool {
        if path.is_empty() || self.scripts.iter().any(|existing| existing == path) {
            return false;
        }
        self.scripts.push(path.to_owned());
        true
    }

    /// 旧地址只能作用于当前列表；移除 Rust 的最后一个同名项时清孤儿参数。
    pub fn remove_attached_component(&mut self, address: ComponentAddress) -> bool {
        match address {
            ComponentAddress::Builtin(kind) => match kind {
                BuiltinComponent::Ui => self.ui.take().is_some(),
                BuiltinComponent::Animator => self.animator.take().is_some(),
                BuiltinComponent::Physics => self.physics.take().is_some(),
                BuiltinComponent::Character => self.controller.take().is_some(),
                BuiltinComponent::Audio => self.audio.take().is_some(),
                BuiltinComponent::Camera => self.camera.take().is_some(),
                BuiltinComponent::Particle => self.particle.take().is_some(),
                BuiltinComponent::MaterialOverride => self.material.take().is_some(),
                BuiltinComponent::Transform | BuiltinComponent::Render => false,
            },
            ComponentAddress::Rust(slot) => {
                if slot >= self.behaviors.len() {
                    return false;
                }
                let name = self.behaviors.remove(slot);
                if !self.behaviors.contains(&name) {
                    self.behavior_params.remove(&name);
                }
                true
            }
            ComponentAddress::Lua(slot) => {
                if slot >= self.scripts.len() {
                    return false;
                }
                self.scripts.remove(slot);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Camera, FieldValue, Scene};

    #[test]
    fn legacy_view_uses_only_existing_serialized_fields() {
        let mut entity = Entity::new("Player");
        assert_eq!(
            entity
                .attached_components()
                .iter()
                .map(|item| item.address.to_string())
                .collect::<Vec<_>>(),
            ["builtin:transform"],
        );
        assert!(entity.add_builtin_component(BuiltinComponent::Camera));
        assert!(!entity.add_builtin_component(BuiltinComponent::Camera));
        assert_eq!(entity.camera, Some(Camera::default()));
        assert!(entity.add_registered_component("Health"));
        assert!(!entity.add_registered_component("Health"));
        assert!(entity.attach_lua_script("Scripts/player.lua"));
        assert!(!entity.attach_lua_script("Scripts/player.lua"));
        entity.behavior_params.insert(
            "Health".into(),
            [("value".into(), FieldValue::F32(75.0))].into(),
        );
        let addresses: Vec<_> = entity
            .attached_components()
            .iter()
            .map(|item| item.address.to_string())
            .collect();
        assert_eq!(
            addresses,
            ["builtin:transform", "builtin:camera", "rust:0", "lua:0"]
        );

        let mut scene = Scene::new("legacy");
        scene.add_entity(entity);
        let ron = ron::ser::to_string(&scene).unwrap();
        assert!(ron.contains("behavior_params"));
        assert!(!ron.contains("attached_components"));
        assert!(!ron.contains("ComponentAddress"));
    }

    #[test]
    fn removal_keeps_shared_legacy_params_until_last_duplicate_is_gone() {
        let mut entity = Entity::new("legacy duplicates");
        entity.behaviors = vec!["Health".into(), "Health".into()];
        entity.behavior_params.insert(
            "Health".into(),
            [("value".into(), FieldValue::F32(10.0))].into(),
        );
        assert!(entity.remove_attached_component(ComponentAddress::Rust(0)));
        assert!(entity.behavior_params.contains_key("Health"));
        assert!(entity.remove_attached_component(ComponentAddress::Rust(0)));
        assert!(entity.behavior_params.is_empty());
        assert!(!entity.remove_attached_component(ComponentAddress::Rust(0)));
        assert!(!entity
            .remove_attached_component(ComponentAddress::Builtin(BuiltinComponent::Transform)));
        assert!(
            !entity.remove_attached_component(ComponentAddress::Builtin(BuiltinComponent::Render))
        );
        assert!(entity.add_builtin_component(BuiltinComponent::Camera));
        assert!(
            entity.remove_attached_component(ComponentAddress::Builtin(BuiltinComponent::Camera))
        );
        assert!(
            !entity.remove_attached_component(ComponentAddress::Builtin(BuiltinComponent::Camera))
        );
    }
}
