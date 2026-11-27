//! 生成可经 XGC 导入的确定性角色资产，并搭建本基准的世界。

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use xgc_render::translation;
use xgc_world::{
    AnimLayer, AnimationLayers, Animator, Camera, Command, ComponentData, GlobalTransform,
    LayerMode, MeshRenderer, SceneEnvironment, Transform, World, WorldData,
};

const COLUMNS: usize = 25;
const ROWS: usize = 20;
pub(super) const CHARACTERS: usize = COLUMNS * ROWS;
const BRANCHES: usize = 5;
const JOINTS_PER_BRANCH: usize = 40;
pub(super) const JOINTS: usize = BRANCHES * JOINTS_PER_BRANCH;
pub(super) const VERTICES: usize = JOINTS * 4;
pub(super) const TRIANGLES: usize = BRANCHES * ((JOINTS_PER_BRANCH - 1) * 8 + 4);
pub(super) const CLIPS: [&str; 4] = ["stride", "lift", "lean", "balance"];
pub(super) const LAYER_WEIGHTS: [f32; 3] = [0.35, 0.45, 0.40];
pub(super) const DURATION: f32 = 2.0;
pub(super) const ANIMATION_INSTANCES: usize = CHARACTERS * CLIPS.len();
pub(super) const MODEL: &str = "crowd.gltf";
const BUFFER: &str = "crowd.bin";
pub(super) const WIDTH: u32 = 1920;
pub(super) const HEIGHT: u32 = 1080;
pub(super) const CELL_SIZE: f32 = 2.0;
pub(super) const CAMERA_Z: f32 = 40.0;
pub(super) const FOV_DEG: f32 = 60.0;

pub(super) struct FixtureDir(pub(super) PathBuf);

impl FixtureDir {
    pub(super) fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tmp/xgc-s3");
        fs::create_dir_all(&root).unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = root.join(format!("fixture-{}-{nonce}", process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct GltfBuffer {
    bytes: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl GltfBuffer {
    fn add(
        &mut self,
        bytes: impl IntoIterator<Item = u8>,
        count: usize,
        component: u32,
        kind: &str,
        bounds: &str,
    ) -> usize {
        let offset = self.bytes.len().next_multiple_of(4);
        self.bytes.resize(offset, 0);
        self.bytes.extend(bytes);
        let length = self.bytes.len() - offset;
        let id = self.views.len();
        self.views.push(format!(
            r#"{{"buffer":0,"byteOffset":{offset},"byteLength":{length}}}"#
        ));
        self.accessors.push(format!(
            r#"{{"bufferView":{id},"componentType":{component},"count":{count},"type":"{kind}"{bounds}}}"#
        ));
        id
    }
}

pub(super) fn fixture_assets(root: &Path) -> usize {
    // 躯干、双臂和双腿各 40 节。每节都有截面、权重、逆绑定与逐剪辑平移轨道。
    // 相邻截面由索引连接，子关节截面由自身和父关节共同影响。
    let branches: [([f32; 2], [f32; 2], f32); BRANCHES] = [
        ([0.0, 0.0], [0.0, 0.018], 0.19),
        ([-0.14, 0.36], [-0.013, -0.005], 0.065),
        ([0.14, 0.36], [0.013, -0.005], 0.065),
        ([-0.10, -0.04], [-0.003, -0.016], 0.083),
        ([0.10, -0.04], [0.003, -0.016], 0.083),
    ];
    let mut positions = Vec::with_capacity(VERTICES);
    let mut influences = Vec::with_capacity(VERTICES);
    let mut weights = Vec::with_capacity(VERTICES);
    let mut indices = Vec::with_capacity(TRIANGLES * 3);
    let mut inverse_bind = Vec::with_capacity(JOINTS);
    let mut nodes = Vec::with_capacity(JOINTS + 1);
    let mut local_bind = Vec::with_capacity(JOINTS);
    for (branch, (origin, step, width)) in branches.into_iter().enumerate() {
        let length = step[0].hypot(step[1]);
        let side = [-step[1] / length, step[0] / length];
        let branch_root = branch * JOINTS_PER_BRANCH;
        for segment in 0..JOINTS_PER_BRANCH {
            let joint = branch_root + segment;
            let first_vertex = (joint * 4) as u16;
            let tip = segment + 1 == JOINTS_PER_BRANCH;
            let [x, y] = std::array::from_fn(|axis| origin[axis] + step[axis] * segment as f32);
            let local = if segment == 0 { origin } else { step };
            let radius = width * (1.0 - 0.35 * segment as f32 / (JOINTS_PER_BRANCH - 1) as f32);
            let parent = if segment == 0 { 0 } else { joint - 1 };
            let mut children = Vec::new();
            if !tip {
                children.push(joint + 1);
            }
            if joint == 0 {
                children.extend((1..BRANCHES).map(|child| child * JOINTS_PER_BRANCH));
            }
            nodes.push(format!(
                r#"{{"name":"joint_{joint}","translation":[{},{},0],"children":{children:?}}}"#,
                local[0], local[1]
            ));
            local_bind.push([local[0], local[1], 0.0]);
            inverse_bind.push(translation(-x, -y, 0.0));
            for (sign, z) in [(-1.0, 0.045), (1.0, 0.045), (1.0, -0.045), (-1.0, -0.045)] {
                positions.push([x + sign * radius * side[0], y + sign * radius * side[1], z]);
                influences.push([joint as u16, parent as u16, 0, 0]);
                weights.push([0.75_f32, 0.25, 0.0, 0.0]);
            }
            if segment == 0 {
                let v = first_vertex;
                indices.extend([v, v + 2, v + 1, v, v + 3, v + 2]);
            } else {
                for edge in 0..4 {
                    let a = first_vertex - 4 + edge;
                    let b = first_vertex - 4 + (edge + 1) % 4;
                    let c = a + 4;
                    let d = b + 4;
                    indices.extend([a, b, d, a, d, c]);
                }
            }
            if tip {
                let v = first_vertex;
                indices.extend([v, v + 1, v + 2, v, v + 2, v + 3]);
            }
        }
    }
    nodes.push(r#"{"mesh":0,"skin":0}"#.into());
    let mut gltf = GltfBuffer::default();
    let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for pos in &positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(pos[axis]);
            max[axis] = max[axis].max(pos[axis]);
        }
    }
    let position = gltf.add(
        positions.into_iter().flatten().flat_map(f32::to_le_bytes),
        VERTICES,
        5126,
        "VEC3",
        &format!(r#", "min":{min:?}, "max":{max:?}"#),
    );
    let joints = gltf.add(
        influences.into_iter().flatten().flat_map(u16::to_le_bytes),
        VERTICES,
        5123,
        "VEC4",
        "",
    );
    let weight = gltf.add(
        weights.into_iter().flatten().flat_map(f32::to_le_bytes),
        VERTICES,
        5126,
        "VEC4",
        "",
    );
    let index = gltf.add(
        indices.into_iter().flat_map(u16::to_le_bytes),
        TRIANGLES * 3,
        5123,
        "SCALAR",
        "",
    );
    let inverse = gltf.add(
        inverse_bind
            .into_iter()
            .flatten()
            .flat_map(f32::to_le_bytes),
        JOINTS,
        5126,
        "MAT4",
        "",
    );
    let input = gltf.add(
        [0.0_f32, DURATION * 0.5, DURATION]
            .into_iter()
            .flat_map(f32::to_le_bytes),
        3,
        5126,
        "SCALAR",
        r#", "min":[0],"max":[2]"#,
    );
    // 四条剪辑各有独立且非零的运动，后续从画面验证去掉任意一条会改变每个角色。
    let motions = [
        [0.008, 0.0],
        [0.001, 0.008],
        [-0.009, 0.003],
        [0.005, -0.009],
    ];
    let channels: Vec<_> = (0..JOINTS)
        .map(|joint| {
            format!(r#"{{"sampler":{joint},"target":{{"node":{joint},"path":"translation"}}}}"#)
        })
        .collect();
    let channels = channels.join(",");
    let mut animations = Vec::new();
    for (clip, [dx, dy]) in CLIPS.into_iter().zip(motions) {
        let mut samplers = Vec::with_capacity(JOINTS);
        for (joint, &bind) in local_bind.iter().enumerate() {
            let amplitude = 0.85 + (joint % 5) as f32 * 0.075;
            let key = [bind[0] + dx * amplitude, bind[1] + dy * amplitude, bind[2]];
            let output = gltf.add(
                [bind, key, bind]
                    .into_iter()
                    .flatten()
                    .flat_map(f32::to_le_bytes),
                3,
                5126,
                "VEC3",
                "",
            );
            samplers.push(format!(
                r#"{{"input":{input},"output":{output},"interpolation":"LINEAR"}}"#
            ));
        }
        animations.push(format!(
            r#"{{"name":"{clip}","samplers":[{}],"channels":[{channels}]}}"#,
            samplers.join(",")
        ));
    }
    let joint_nodes: Vec<_> = (0..JOINTS).collect();
    let document = format!(
        r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0,{JOINTS}]}}],
        "nodes":[{}],"skins":[{{"joints":{joint_nodes:?},"skeleton":0,"inverseBindMatrices":{inverse}}}],
        "meshes":[{{"primitives":[{{"attributes":{{"POSITION":{position},"JOINTS_0":{joints},"WEIGHTS_0":{weight}}},"indices":{index},"material":0}}]}}],
        "materials":[{{"pbrMetallicRoughness":{{"baseColorFactor":[0.65,0.40,0.24,1],"metallicFactor":0,"roughnessFactor":0.65}}}}],
        "buffers":[{{"uri":"{BUFFER}","byteLength":{}}}],"bufferViews":[{}],"accessors":[{}],"animations":[{}]}}"#,
        nodes.join(","),
        gltf.bytes.len(),
        gltf.views.join(","),
        gltf.accessors.join(","),
        animations.join(","),
    );
    fs::write(root.join(MODEL), &document).unwrap();
    fs::write(root.join(BUFFER), &gltf.bytes).unwrap();
    document.len() + gltf.bytes.len()
}

pub(super) fn phase(character: usize, clip: usize) -> f32 {
    (character % 9) as f32 / 32.0 + clip as f32 / 24.0
}

pub(super) fn speed(character: usize, clip: usize) -> f32 {
    [0.8, 1.0, 1.1, 1.2][clip] + (character % 5) as f32 * 0.025
}

pub(super) fn fixture_world() -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: Vec::new(),
        ambient: [0.30, 0.34, 0.39],
    });
    let mut layer_commands = Vec::with_capacity(CHARACTERS);
    for character in 0..CHARACTERS {
        let x = (character % COLUMNS) as f32 - (COLUMNS - 1) as f32 * 0.5;
        let y = (character / COLUMNS) as f32 - (ROWS - 1) as f32 * 0.5;
        let local = Transform {
            position: [x * CELL_SIZE, y * CELL_SIZE, 0.0],
            ..Transform::default()
        };
        let key = world.spawn((
            local,
            GlobalTransform::from_local(&local),
            MeshRenderer {
                model: MODEL.into(),
            },
            Animator {
                clip: CLIPS[0].into(),
                time: phase(character, 0),
                speed: speed(character, 0),
                duration: DURATION,
                ..Animator::default()
            },
        ));
        // Bundle 最多接四项。层栈按实际组件命令挂载；场景搭建另计。
        let layers = AnimationLayers {
            layers: CLIPS
                .iter()
                .enumerate()
                .skip(1)
                .map(|(clip, &name)| AnimLayer {
                    name: name.into(),
                    clip: name.into(),
                    mode: LayerMode::Override,
                    weight: LAYER_WEIGHTS[clip - 1],
                    time: phase(character, clip),
                    speed: speed(character, clip),
                    ..AnimLayer::default()
                })
                .collect(),
        };
        layer_commands.push(Command::AddComponent {
            entity: world.entity(key).unwrap(),
            component: ComponentData::from_component(&layers),
        });
    }
    let plan = world.plan(layer_commands);
    world.apply(&plan).unwrap();
    let position = [0.0, 0.0, CAMERA_Z];
    world.spawn((
        Transform {
            position,
            ..Transform::default()
        },
        GlobalTransform {
            position,
            ..GlobalTransform::default()
        },
        Camera {
            fov_y_deg: FOV_DEG,
            ..Camera::default()
        },
    ));
    world
}

pub(super) fn restore(snapshot: &WorldData) -> World {
    let mut world = World::new();
    world.restore_world_data(snapshot).unwrap();
    world
}
