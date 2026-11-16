//! 旧 XGameEngine `SceneV2` RON → XGC `WorldData` 的第一层迁移。
//!
//! 这一层只搬实体身份、名称、父子关系、`Transform`，并补默认
//! `GlobalTransform` 让层级系统能工作。Mesh / Material / Camera 等旧组件先不搬，
//! 迁移报告里给出数量，后续按组件逐步补齐，不造"看起来完整"的假数据。

#![deny(unsafe_code)]

use std::collections::HashSet;

use serde::Deserialize;

use old_scene::OldScene;
use xgc_base::EntityId;
use xgc_world::{
    AudioSource, Camera, CharacterController as XgcCharacter, Component, ComponentData, EntityData,
    GlobalTransform, Material, MeshRenderer, Name, Parent, PhysicsBody, Script, Transform,
    UiInteraction, UiLayout, Value, WorldData,
};
pub const OLD_SCENE_FORMAT_VERSION: u32 = 2;
mod old_animator;
mod old_lod;
mod old_particle;
mod old_rust;
mod old_scene;
mod old_ui;
mod report;
pub use report::{component_id_name, MigrationBatch, MigrationError, MigrationReport};
#[derive(Debug, Default, Deserialize)]
struct OldComponents {
    #[serde(default)]
    entries: Vec<OldEntry>,
}

#[derive(Debug, Deserialize)]
struct OldEntry {
    data: OldComponentData,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
enum OldComponentData {
    Mesh(OldMesh),
    Lod(ron::Value),
    Material(OldMaterial),
    Ui(ron::Value),
    Animator(ron::Value),
    Character(ron::Value),
    Audio(OldAudio),
    Camera(OldCamera),
    Particle(ron::Value),
    Physics(OldPhysics),
    Rust {
        #[allow(dead_code)]
        registered_name: String,
        #[allow(dead_code)]
        fields: Option<ron::Value>,
    },
    Lua {
        path: String,
    },
}

#[derive(Debug, Deserialize)]
struct OldAnimator {
    clip: String,
    speed: f32,
    looping: bool,
    autoplay: bool,
    fade_time: f32,
}

#[derive(Debug, Deserialize)]
struct OldCharacter {
    height: f32,
    radius: f32,
    step_offset: f32,
    slope_limit: f32,
    skin_width: f32,
    snap_to_ground: f32,
}

#[derive(Debug, Deserialize)]
struct OldAudio {
    #[serde(default)]
    clip: String,
    #[serde(default = "one")]
    volume: f32,
    #[serde(default = "one")]
    pitch: f32,
    #[serde(default)]
    looping: bool,
    #[serde(default)]
    autoplay: bool,
    #[serde(default = "default_true")]
    spatial: bool,
}

fn one() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
enum OldBodyKind {
    Static,
    Dynamic,
    Kinematic,
}

#[derive(Debug, Deserialize)]
enum OldShape {
    Cuboid { half_extents: glam::Vec3 },
    Ball { radius: f32 },
    Capsule { half_height: f32, radius: f32 },
    Cylinder { half_height: f32, radius: f32 },
    Auto,
}

#[derive(Debug, Deserialize)]
struct OldPhysics {
    #[serde(default = "default_body_kind")]
    kind: OldBodyKind,
    #[serde(default = "default_shape")]
    shape: OldShape,
    #[serde(default = "default_friction")]
    friction: f32,
    #[serde(default)]
    restitution: f32,
    #[serde(default)]
    mass: f32,
    #[serde(default = "one")]
    gravity_scale: f32,
    #[serde(default)]
    is_sensor: bool,
}

fn default_body_kind() -> OldBodyKind {
    OldBodyKind::Dynamic
}

fn default_shape() -> OldShape {
    OldShape::Auto
}

fn default_friction() -> f32 {
    0.5
}

#[derive(Debug, Deserialize)]
struct OldMesh {
    model: String,
}

#[derive(Debug, Deserialize)]
struct OldMaterial {
    #[allow(dead_code)]
    asset: Option<String>,
    overrides: Option<OldMaterialOverrides>,
}

#[derive(Debug, Default, Deserialize)]
struct OldMaterialOverrides {
    base_color: Option<glam::Vec4>,
    roughness: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct OldCamera {
    fov: f32,
    near: f32,
    far: f32,
    active: bool,
}

pub fn migrate_str(text: &str) -> Result<(WorldData, MigrationReport), MigrationError> {
    let scene: OldScene =
        ron::from_str(text).map_err(|err| MigrationError::Parse(err.to_string()))?;
    if scene.format_version != OLD_SCENE_FORMAT_VERSION {
        return Err(MigrationError::UnsupportedVersion(scene.format_version));
    }

    let mut seen = HashSet::new();
    for entity in &scene.entities {
        if !seen.insert(entity.id.0) {
            return Err(MigrationError::DuplicateId(entity.id.0));
        }
    }

    let mut report = MigrationReport {
        entities: scene.entities.len(),
        ..MigrationReport::default()
    };
    let mut entities = Vec::with_capacity(scene.entities.len());
    let mut max_id = 0;

    for old in &scene.entities {
        max_id = max_id.max(old.id.0);
        if !old.components.entries.is_empty() {
            report.with_old_components += 1;
        }

        let mut mesh_model = None;
        let mut material = None;
        let mut camera = None;
        let mut script_path = None;
        let mut audio = None;
        let mut physics = None;
        let mut animator = None;
        let mut character = None;
        let mut ui = None;
        let mut particle = None;
        let mut rust = None;
        let mut lod = None;
        for entry in &old.components.entries {
            match &entry.data {
                OldComponentData::Mesh(mesh) => {
                    mesh_model = Some(mesh.model.clone());
                    report.mapped_components += 1;
                }
                OldComponentData::Lod(value) => {
                    if let Some(component) =
                        old_lod::convert(value.clone()).map_err(MigrationError::Parse)?
                    {
                        lod = Some(component);
                        report.mapped_components += 1;
                    }
                }
                OldComponentData::Material(old_material) => {
                    let overrides = old_material.overrides.as_ref();
                    material = Some((
                        overrides
                            .and_then(|overrides| overrides.base_color)
                            .map(|color| color.to_array())
                            .unwrap_or([1.0; 4]),
                        overrides
                            .and_then(|overrides| overrides.roughness)
                            .unwrap_or(0.5),
                    ));
                    report.mapped_components += 1;
                }
                OldComponentData::Camera(old_camera) => {
                    camera = Some(old_camera);
                    report.mapped_components += 1;
                }
                OldComponentData::Lua { path } => {
                    script_path = Some(path.clone());
                    report.mapped_components += 1;
                }
                OldComponentData::Audio(old_audio) => {
                    audio = Some(AudioSource {
                        clip: old_audio.clip.clone(),
                        volume: old_audio.volume,
                        pitch: old_audio.pitch,
                        looping: old_audio.looping,
                        autoplay: old_audio.autoplay,
                        spatial: old_audio.spatial,
                    });
                    report.mapped_components += 1;
                }
                OldComponentData::Physics(old_physics) => {
                    physics = Some(old_physics);
                    report.mapped_components += 1;
                }
                OldComponentData::Character(old_character) => {
                    let old: OldCharacter = old_character
                        .clone()
                        .into_rust()
                        .map_err(|err| MigrationError::Parse(err.to_string()))?;
                    character = Some(old);
                    report.mapped_components += 1;
                }
                OldComponentData::Ui(value) => {
                    ui = Some(old_ui::convert(value.clone()).map_err(MigrationError::Parse)?);
                    report.mapped_components += 1;
                }
                OldComponentData::Particle(value) => {
                    particle =
                        Some(old_particle::convert(value.clone()).map_err(MigrationError::Parse)?);
                    report.mapped_components += 1;
                }
                OldComponentData::Rust {
                    registered_name,
                    fields,
                } => {
                    rust = Some(
                        old_rust::convert(registered_name, fields.as_ref())
                            .map_err(MigrationError::Parse)?,
                    );
                    report.mapped_components += 1;
                }
                OldComponentData::Animator(old_animator) => {
                    let old: OldAnimator = old_animator
                        .clone()
                        .into_rust()
                        .map_err(|err| MigrationError::Parse(err.to_string()))?;
                    animator = Some((
                        old.clip,
                        old.speed,
                        old.looping,
                        old.autoplay,
                        old.fade_time,
                    ));
                    report.mapped_components += 1;
                }
            }
        }

        let position = old.transform.position.to_array();
        let rotation = old.transform.rotation.to_array();
        let scale = old.transform.scale.to_array();
        let mut components = vec![
            ComponentData {
                component: Transform::ID,
                fields: vec![
                    (
                        "position".to_string(),
                        Value::Vec3([
                            f64::from(position[0]),
                            f64::from(position[1]),
                            f64::from(position[2]),
                        ]),
                    ),
                    (
                        "rotation".to_string(),
                        Value::Quat([
                            f64::from(rotation[0]),
                            f64::from(rotation[1]),
                            f64::from(rotation[2]),
                            f64::from(rotation[3]),
                        ]),
                    ),
                    (
                        "scale".to_string(),
                        Value::Vec3([
                            f64::from(scale[0]),
                            f64::from(scale[1]),
                            f64::from(scale[2]),
                        ]),
                    ),
                ],
            },
            ComponentData {
                component: Name::ID,
                fields: vec![("value".to_string(), Value::String(old.name.clone()))],
            },
            ComponentData {
                component: GlobalTransform::ID,
                fields: Vec::new(),
            },
        ];

        if let Some(parent) = old.parent {
            if !seen.contains(&parent.0) {
                report.missing_parents += 1;
            } else {
                components.push(ComponentData {
                    component: Parent::ID,
                    fields: vec![("entity".to_string(), Value::Entity(parent.0))],
                });
            }
        }

        if let Some(model) = mesh_model {
            components.push(ComponentData {
                component: MeshRenderer::ID,
                fields: vec![("model".to_string(), Value::String(model))],
            });
        }
        if let Some((base_color, roughness)) = material {
            components.push(ComponentData {
                component: Material::ID,
                fields: vec![
                    (
                        "base_color".to_string(),
                        Value::Vec4([
                            f64::from(base_color[0]),
                            f64::from(base_color[1]),
                            f64::from(base_color[2]),
                            f64::from(base_color[3]),
                        ]),
                    ),
                    ("roughness".to_string(), Value::F64(f64::from(roughness))),
                ],
            });
        }
        if let Some(camera) = camera {
            components.push(ComponentData {
                component: Camera::ID,
                fields: vec![
                    ("fov_y_deg".to_string(), Value::F64(f64::from(camera.fov))),
                    ("near".to_string(), Value::F64(f64::from(camera.near))),
                    ("far".to_string(), Value::F64(f64::from(camera.far))),
                    ("active".to_string(), Value::Bool(camera.active)),
                ],
            });
        }
        if let Some((clip, speed, looping, autoplay, fade_time)) = animator {
            let position = old.transform.position.to_array();
            let rotation = old.transform.rotation.to_array();
            let scale = old.transform.scale.to_array();
            components.push(old_animator::animator_component(
                old_animator::AnimatorInput {
                    clip,
                    speed,
                    looping,
                    autoplay,
                    fade_time,
                    position: [
                        f64::from(position[0]),
                        f64::from(position[1]),
                        f64::from(position[2]),
                    ],
                    rotation: [
                        f64::from(rotation[0]),
                        f64::from(rotation[1]),
                        f64::from(rotation[2]),
                        f64::from(rotation[3]),
                    ],
                    scale: [
                        f64::from(scale[0]),
                        f64::from(scale[1]),
                        f64::from(scale[2]),
                    ],
                },
            ));
        }
        if let Some(character) = character {
            components.push(ComponentData {
                component: XgcCharacter::ID,
                fields: vec![
                    (
                        "height".to_string(),
                        Value::F64(f64::from(character.height)),
                    ),
                    (
                        "radius".to_string(),
                        Value::F64(f64::from(character.radius)),
                    ),
                    (
                        "step_offset".to_string(),
                        Value::F64(f64::from(character.step_offset)),
                    ),
                    (
                        "slope_limit".to_string(),
                        Value::F64(f64::from(character.slope_limit)),
                    ),
                    (
                        "skin_width".to_string(),
                        Value::F64(f64::from(character.skin_width)),
                    ),
                    (
                        "snap_to_ground".to_string(),
                        Value::F64(f64::from(character.snap_to_ground)),
                    ),
                ],
            });
        }
        if let Some(ui) = ui {
            components.push(ui);
            components.push(ComponentData {
                component: UiLayout::ID,
                fields: Vec::new(),
            });
            components.push(ComponentData {
                component: UiInteraction::ID,
                fields: Vec::new(),
            });
        }
        if let Some(particle) = particle {
            components.push(particle);
        }
        if let Some(rust) = rust {
            components.push(rust);
        }
        if let Some(lod) = lod {
            components.push(lod);
        }
        if let Some(path) = script_path {
            components.push(ComponentData {
                component: Script::ID,
                fields: vec![("path".to_string(), Value::String(path))],
            });
        }
        if let Some(audio) = audio {
            components.push(ComponentData {
                component: AudioSource::ID,
                fields: vec![
                    ("clip".to_string(), Value::String(audio.clip)),
                    ("volume".to_string(), Value::F64(f64::from(audio.volume))),
                    ("pitch".to_string(), Value::F64(f64::from(audio.pitch))),
                    ("looping".to_string(), Value::Bool(audio.looping)),
                    ("autoplay".to_string(), Value::Bool(audio.autoplay)),
                    ("spatial".to_string(), Value::Bool(audio.spatial)),
                ],
            });
        }
        if let Some(physics) = physics {
            let kind = match physics.kind {
                OldBodyKind::Static => "static",
                OldBodyKind::Dynamic => "dynamic",
                OldBodyKind::Kinematic => "kinematic",
            };
            let (shape, half_extents, radius, half_height) = match physics.shape {
                OldShape::Cuboid { half_extents } => ("cuboid", half_extents.to_array(), 0.5, 0.5),
                OldShape::Ball { radius } => ("ball", [0.5; 3], radius, 0.5),
                OldShape::Capsule {
                    half_height,
                    radius,
                } => ("capsule", [0.5; 3], radius, half_height),
                OldShape::Cylinder {
                    half_height,
                    radius,
                } => ("cylinder", [0.5; 3], radius, half_height),
                OldShape::Auto => ("auto", [0.5; 3], 0.5, 0.5),
            };
            components.push(ComponentData {
                component: PhysicsBody::ID,
                fields: vec![
                    ("kind".to_string(), Value::String(kind.to_string())),
                    ("shape".to_string(), Value::String(shape.to_string())),
                    (
                        "half_extents".to_string(),
                        Value::Vec3([
                            f64::from(half_extents[0]),
                            f64::from(half_extents[1]),
                            f64::from(half_extents[2]),
                        ]),
                    ),
                    ("radius".to_string(), Value::F64(f64::from(radius))),
                    (
                        "half_height".to_string(),
                        Value::F64(f64::from(half_height)),
                    ),
                    (
                        "friction".to_string(),
                        Value::F64(f64::from(physics.friction)),
                    ),
                    (
                        "restitution".to_string(),
                        Value::F64(f64::from(physics.restitution)),
                    ),
                    ("mass".to_string(), Value::F64(f64::from(physics.mass))),
                    (
                        "gravity_scale".to_string(),
                        Value::F64(f64::from(physics.gravity_scale)),
                    ),
                    ("is_sensor".to_string(), Value::Bool(physics.is_sensor)),
                ],
            });
        }

        entities.push(EntityData {
            id: EntityId::from_raw(old.id.0),
            revision: 0,
            components,
        });
    }

    Ok((
        WorldData {
            revision: 0,
            next_id: max_id.saturating_add(1),
            entities,
        },
        report,
    ))
}

/// 批量迁移目录里的 `.ron`；单场景失败只记入 `failures`，不中断其它场景。
mod batch;
pub use batch::migrate_dir;
