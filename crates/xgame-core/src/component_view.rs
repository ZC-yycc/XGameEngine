//! 运行时值模型的统一组件视图。
//!
//! 这里读写的仍是 `Entity` 原字段：不引入第二份可变配置，也不把运行时
//! `ComponentMap` 误当成可保存组件。Rust/Lua 的下标地址只服务编辑器当次操作；
//! 持久身份由文件模型的 `ComponentId` 独立维护，不能拿这里的槽位冒充。

use std::fmt;

use crate::inspect::FieldValues;
use crate::scene::Entity;

/// 内建组件的值地址与编辑器显示名都在这一处定义。
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
    /// Mesh / Material 在运行时值模型中横跨 `Entity` 字段，须单独筛选字段。
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

    /// 横跨多个值字段的组件只返回自己拥有的字段。
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

/// Rust / Lua 的编辑器地址使用值列表槽位，重排、删除后须重新查询。
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

    /// 只创建缺失的内建组件；用运行时值字段作唯一数据源。
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

    /// 脚本资产是否存在由项目边界判断；挂载规则是一条路径一份。
    pub fn attach_lua_script(&mut self, path: &str) -> bool {
        if path.is_empty() || self.scripts.iter().any(|existing| existing == path) {
            return false;
        }
        self.scripts.push(path.to_owned());
        true
    }

    /// 槽位地址只能作用于当前列表；移除 Rust 的最后一个同名项时清孤儿参数。
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
