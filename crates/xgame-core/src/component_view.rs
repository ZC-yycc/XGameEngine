//! 旧场景格式的统一组件视图。
//!
//! 这里读写的仍是 `Entity` 原字段：不引入第二份可变配置，也不把运行时
//! `ComponentMap` 误当成可保存组件。Rust/Lua 的下标地址只在当前列表结构不变时有效；
//! 稳定实例 ID 需要下一版场景格式，不能拿这里的槽位冒充。

use std::fmt;

use crate::inspect::FieldValues;
use crate::scene::Entity;

/// 内建组件的旧格式类型地址与编辑器显示名都在这一处定义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BuiltinComponent {
    Transform,
    Mesh,
    Lod,
    Material,
    Ui,
    Animator,
    Physics,
    Character,
    Audio,
    Camera,
    Particle,
}

impl BuiltinComponent {
    pub const ALL: [Self; 11] = [
        Self::Transform,
        Self::Mesh,
        Self::Lod,
        Self::Material,
        Self::Ui,
        Self::Animator,
        Self::Physics,
        Self::Character,
        Self::Audio,
        Self::Camera,
        Self::Particle,
    ];

    /// Transform 是对象固有属性，其余内建数据都可独立增删。
    pub const ADDABLE: [Self; 10] = [
        Self::Mesh,
        Self::Lod,
        Self::Material,
        Self::Ui,
        Self::Animator,
        Self::Physics,
        Self::Character,
        Self::Audio,
        Self::Camera,
        Self::Particle,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Self::Transform => "transform",
            Self::Mesh => "mesh",
            Self::Lod => "lod",
            Self::Material => "material",
            Self::Ui => "ui",
            Self::Animator => "animator",
            Self::Physics => "physics",
            Self::Character => "character",
            Self::Audio => "audio",
            Self::Camera => "camera",
            Self::Particle => "particle",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Transform => "Transform",
            Self::Mesh => "Mesh",
            Self::Lod => "LOD",
            Self::Material => "Material",
            Self::Ui => "UI Element",
            Self::Animator => "Animator",
            Self::Physics => "Physics",
            Self::Character => "Character",
            Self::Audio => "Audio Source",
            Self::Camera => "Camera",
            Self::Particle => "Particle Emitter",
        }
    }

    pub const fn addable(self) -> bool {
        !matches!(self, Self::Transform)
    }

    /// 字段说明由场景 schema 负责；这里只给出该组件对应的落盘类型。
    /// Mesh / Material 在 v1 中横跨 `Entity` 字段，须单独筛选字段。
    pub const fn schema_type(self) -> &'static str {
        match self {
            Self::Transform => "Transform",
            Self::Mesh | Self::Material => "Entity",
            Self::Lod => "LodGroup",
            Self::Ui => "UiElement",
            Self::Animator => "Animator",
            Self::Physics => "PhysicsBody",
            Self::Character => "CharacterController",
            Self::Audio => "AudioSource",
            Self::Camera => "Camera",
            Self::Particle => "ParticleEmitter",
        }
    }

    /// v1 中横跨多个字段的组件只返回自己拥有的字段。
    pub const fn schema_field_names(self) -> &'static [&'static str] {
        match self {
            Self::Mesh => &["model", "morph_weights"],
            Self::Material => &["material_asset", "material"],
            _ => &[],
        }
    }

    pub fn present(self, entity: &Entity) -> bool {
        match self {
            Self::Transform => true,
            Self::Mesh => !entity.model.is_empty() || !entity.morph_weights.is_empty(),
            Self::Lod => !entity.lod.is_empty(),
            Self::Material => entity.material_asset.is_some() || entity.material.is_some(),
            Self::Ui => entity.ui.is_some(),
            Self::Animator => entity.animator.is_some(),
            Self::Physics => entity.physics.is_some(),
            Self::Character => entity.controller.is_some(),
            Self::Audio => entity.audio.is_some(),
            Self::Camera => entity.camera.is_some(),
            Self::Particle => entity.particle.is_some(),
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
            BuiltinComponent::Mesh => {
                self.model = self
                    .lod
                    .drawable_levels()
                    .next()
                    .map_or_else(|| "builtin://cube".to_owned(), |(_, level)| level.model.clone());
            }
            BuiltinComponent::Lod => {
                let seed = if self.model.is_empty() {
                    "builtin://cube".to_owned()
                } else {
                    self.model.clone()
                };
                self.lod = crate::LodGroup {
                    levels: vec![
                        crate::LodLevel::new(seed.clone(), 20.0),
                        crate::LodLevel::new(seed, 0.0),
                    ],
                };
            }
            BuiltinComponent::Material => {
                self.material = Some(crate::MaterialOverride::default());
            }
            BuiltinComponent::Ui => self.ui = Some(crate::UiElement::panel(320.0, 180.0)),
            BuiltinComponent::Animator => self.animator = Some(crate::Animator::default()),
            BuiltinComponent::Physics => self.physics = Some(crate::PhysicsBody::default()),
            BuiltinComponent::Character => {
                self.controller = Some(crate::CharacterController::default());
            }
            BuiltinComponent::Audio => self.audio = Some(crate::AudioSource::default()),
            BuiltinComponent::Camera => self.camera = Some(crate::Camera::default()),
            BuiltinComponent::Particle => self.particle = Some(crate::ParticleEmitter::default()),
            BuiltinComponent::Transform => return false,
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
                BuiltinComponent::Mesh => {
                    let present = kind.present(self);
                    self.model.clear();
                    self.morph_weights.clear();
                    present
                }
                BuiltinComponent::Lod => {
                    let present = kind.present(self);
                    self.lod = crate::LodGroup::default();
                    present
                }
                BuiltinComponent::Material => {
                    let present = kind.present(self);
                    self.material_asset = None;
                    self.material = None;
                    present
                }
                BuiltinComponent::Ui => self.ui.take().is_some(),
                BuiltinComponent::Animator => self.animator.take().is_some(),
                BuiltinComponent::Physics => self.physics.take().is_some(),
                BuiltinComponent::Character => self.controller.take().is_some(),
                BuiltinComponent::Audio => self.audio.take().is_some(),
                BuiltinComponent::Camera => self.camera.take().is_some(),
                BuiltinComponent::Particle => self.particle.take().is_some(),
                BuiltinComponent::Transform => false,
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
        assert!(entity.add_builtin_component(BuiltinComponent::Camera));
        assert!(
            entity.remove_attached_component(ComponentAddress::Builtin(BuiltinComponent::Camera))
        );
        assert!(
            !entity.remove_attached_component(ComponentAddress::Builtin(BuiltinComponent::Camera))
        );
    }

    #[test]
    fn builtin_catalog_points_to_real_scene_schema_fields() {
        let doc = crate::schema::scene_schema();
        for kind in BuiltinComponent::ALL {
            let schema = doc
                .type_schema(kind.schema_type())
                .expect("目录里的类型应存在");
            for name in kind.schema_field_names() {
                assert!(schema.fields.iter().any(|field| field.name == *name));
            }
        }
    }

    #[test]
    fn visual_components_are_independently_addable_and_removable() {
        let by_code = |code: &str| {
            BuiltinComponent::ALL
                .into_iter()
                .find(|kind| kind.code() == code)
                .unwrap_or_else(|| panic!("内建组件目录缺少 {code}"))
        };
        let mesh = by_code("mesh");
        let lod = by_code("lod");
        let material = by_code("material");
        assert!(BuiltinComponent::ALL
            .into_iter()
            .all(|kind| kind.code() != "render"));

        let mut entity = Entity::new("Visual");
        assert!(entity.add_builtin_component(mesh));
        assert!(mesh.present(&entity));
        entity.morph_weights = vec![0.25, 0.75];

        assert!(entity.add_builtin_component(lod));
        assert!(lod.present(&entity));

        assert!(entity.add_builtin_component(material));
        assert!(material.present(&entity));
        entity.material_asset = Some("Resources/Materials/Wood.mat".into());

        assert!(entity.remove_attached_component(ComponentAddress::Builtin(mesh)));
        assert!(!mesh.present(&entity));
        assert!(entity.model.is_empty());
        assert!(entity.morph_weights.is_empty());
        assert!(lod.present(&entity), "移除 Mesh 不能顺手删除 LOD");
        assert!(material.present(&entity), "移除 Mesh 不能顺手删除 Material");

        assert!(entity.remove_attached_component(ComponentAddress::Builtin(lod)));
        assert!(!lod.present(&entity));
        assert!(entity.lod.is_empty());
        assert!(material.present(&entity), "移除 LOD 不能顺手删除 Material");

        assert!(entity.remove_attached_component(ComponentAddress::Builtin(material)));
        assert!(!material.present(&entity));
        assert!(entity.material_asset.is_none());
        assert!(entity.material.is_none());
    }
}
