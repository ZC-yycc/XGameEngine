//! 旧 XGameEngine `SceneV2` RON → XGC `WorldData` 的第一层迁移。
//!
//! 单文件投影保留场景 ID，供逐帧验收使用。项目级迁移另分配项目内 ID 并恢复预制体来源。

#![deny(unsafe_code)]

use std::collections::HashSet;

use serde::Deserialize;

use environment::scene_environment;
use old_character::OldCharacter;
use old_scene::OldScene;
use transform_migrate::transform_components;
use xgc_base::EntityId;
use xgc_world::{
    AudioGain, AudioSource, Camera, CharacterController as XgcCharacter, Component, ComponentData,
    EntityData, Material, MeshRenderer, Name, Parent, PhysicsBody, Script, UiInteraction, UiLayout,
    Value, Velocity, WorldData,
};

pub const OLD_SCENE_FORMAT_VERSION: u32 = 2;
mod compare;
mod environment;
mod old_animator;
mod old_character;
mod old_lod;
mod old_particle;
mod old_rust;
mod old_scene;
mod old_ui;
mod project_check;
mod project_migrate;
mod report;
mod transform_migrate;
mod verify;
pub use compare::{compare_headless, CompareReport, Stats};
pub use project_check::{
    check_old_project, ProjectCheckError, ProjectCheckReport, OLD_PROJECT_FORMAT_VERSION,
};
pub use project_migrate::{migrate_old_project, ProjectMigrationError, ProjectMigrationReport};
pub use report::{component_id_name, MigrationBatch, MigrationError, MigrationReport};
pub use verify::verify_migration;
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
    Animator(old_animator::OldAnimator),
    Character(ron::Value),
    Audio(OldAudio),
    Camera(OldCamera),
    Particle(old_particle::OldParticle),
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
    /// 材质资产引用（迁移后进 `render.material.asset`）。
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
    let scene = ron::from_str(text).map_err(|err| MigrationError::Parse(err.to_string()))?;
    migrate_parsed(&scene)
}

fn migrate_parsed(scene: &OldScene) -> Result<(WorldData, MigrationReport), MigrationError> {
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
        let mut animator_additive = None;
        let mut animator_root_motion = None;
        let (mut animator_layers, mut animator_events) = (String::new(), String::new());
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
                        old_material.asset.clone().unwrap_or_default(),
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
                        bus: "sfx".to_string(),
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
                    particle = Some(old_particle::convert(value));
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
                OldComponentData::Animator(old) => {
                    let machine = match &old.machine {
                        Some(machine) => Some(
                            old_animator::MigratedMachine::new(machine)
                                .map_err(MigrationError::Parse)?,
                        ),
                        None => None,
                    };
                    animator_additive = old.additive.clone();
                    animator_events = old_animator::events_to_component_field(&old.events)
                        .map_err(MigrationError::Parse)?;
                    animator_layers = old_animator::layers_to_component_field(&old.layers)
                        .map_err(MigrationError::Parse)?;
                    if let Some(root) = &old.root_motion {
                        animator_root_motion =
                            Some(root.to_component_field().map_err(MigrationError::Parse)?);
                    }
                    animator = Some((
                        old.clip.clone(),
                        old.speed,
                        old.looping,
                        old.autoplay,
                        old.fade_time,
                        machine,
                    ));
                    report.mapped_components += 1;
                }
            }
        }

        let position = old.transform.position.to_array();
        let mut rotation = old.transform.rotation.to_array();
        if camera.is_some() {
            // 旧相机以 +Z 为前方；XGC 相机以 -Z 为前方。
            rotation = [-rotation[2], rotation[3], rotation[0], -rotation[1]];
        }
        let scale = old.transform.scale.to_array();
        let (transform_data, global_data) = transform_components(position, rotation, scale);
        let mut components = vec![
            transform_data,
            ComponentData {
                component: Name::ID,
                fields: vec![("value".to_string(), Value::String(old.name.clone()))],
            },
            global_data,
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
        if let Some((base_color, roughness, asset)) = material {
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
                    ("asset".to_string(), Value::String(asset)),
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
        if let Some((clip, speed, looping, autoplay, fade_time, machine)) = animator {
            let additive = animator_additive.unwrap_or_default();
            let root_motion = animator_root_motion.unwrap_or_default();
            let (layers, events) = (animator_layers.clone(), animator_events);
            let position = old.transform.position.to_array();
            let rotation = old.transform.rotation.to_array();
            let scale = old.transform.scale.to_array();
            components.extend(old_animator::animator_components(
                old_animator::AnimatorInput {
                    clip,
                    speed,
                    looping,
                    autoplay,
                    fade_time,
                    machine,
                    additive_clip: additive.clip,
                    additive_weight: additive.weight,
                    additive_speed: additive.speed,
                    additive_looping: additive.looping,
                    root_motion,
                    layers,
                    events,
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
                    ("bus".to_string(), Value::String("sfx".to_string())),
                ],
            });
            components.push(ComponentData {
                component: AudioGain::ID,
                fields: Vec::new(),
            });
        }
        let mut dynamic_body = false;
        if let Some(physics) = physics {
            let kind = match physics.kind {
                OldBodyKind::Static => "static",
                OldBodyKind::Dynamic => {
                    dynamic_body = true;
                    "dynamic"
                }
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

        // 旧引擎的动态刚体速度由物理后端持有、场景文件不存（加载即静止）；
        // XGC 的物理系统按 `(Transform, Velocity, PhysicsBody)` 查询，缺速度列的
        // 动态刚体**永远不动**。迁移时补一个零速度，让"落体 / 堆积"两边一致。
        if dynamic_body
            && !components
                .iter()
                .any(|component| component.component == Velocity::ID)
        {
            components.push(ComponentData {
                component: Velocity::ID,
                fields: vec![("linear".to_string(), Value::Vec3([0.0; 3]))],
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
            environment: scene_environment(scene),
        },
        report,
    ))
}

/// 批量迁移目录里的 `.ron`；单场景失败只记入 `failures`，不中断其它场景。
mod batch;
pub use batch::migrate_dir;
