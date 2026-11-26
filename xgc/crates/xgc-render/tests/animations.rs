//! Fox.gltf 自带动画的解析与采样回归：迁移世界的狐狸没有骨骼实体，
//! 动画只能从模型里读；这里钉住剪辑清单与采样行为，防止解析悄悄退化。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use xgc_render::{load_gltf, Interpolation, MeshCache, MeshError};
use xgc_world::{root_motion_delta, MotionSpan, RootMotion};

const RESOURCES: &str = "../../../projects/XGame/Resources";

#[test]
fn fox_gltf_exposes_survey_walk_and_run_clips() {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    let animations = mesh.animations.as_ref().expect("Fox.gltf 自带动画剪辑");
    assert!(!animations.nodes.is_empty());
    for name in ["Survey", "Walk", "Run"] {
        let clip = animations
            .clip(name)
            .unwrap_or_else(|| panic!("缺剪辑 {name}"));
        assert!(clip.duration.is_finite() && clip.duration > 0.0);
        assert_eq!(clip.channels.len(), 21, "旧端 Fox 的每个剪辑 21 条通道");
        assert!(clip
            .channels
            .iter()
            .all(|channel| channel.interpolation == Interpolation::Linear));
    }
}

#[test]
fn walk_clip_samples_distinct_joint_poses() {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().expect("Fox.gltf 带蒙皮").joint_nodes;
    assert!(!joints.is_empty());

    let start = animations
        .joint_pose("Walk", 0.0, true, joints)
        .expect("Walk 在 t=0 可采样");
    assert_eq!(start.len(), joints.len());
    let middle = animations
        .joint_pose("Walk", 0.5, true, joints)
        .expect("Walk 在 t=0.5 可采样");
    assert_eq!(middle.len(), joints.len());
    assert!(
        start
            .iter()
            .zip(&middle)
            .any(|(a, b)| a.iter().zip(b).any(|(x, y)| (x - y).abs() > 1e-6)),
        "t=0 与 t=0.5 的关节矩阵应不同"
    );
}

#[test]
fn missing_clip_or_bad_time_returns_none() {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().unwrap().joint_nodes;
    assert!(animations.joint_pose("Nope", 0.0, true, joints).is_none());
    assert!(animations
        .joint_pose("Walk", f32::NAN, true, joints)
        .is_none());
    assert!(animations.joint_pose("Walk", 0.0, true, &[]).is_none());
}

#[test]
fn additive_layer_adds_delta_on_top_of_the_base_pose() {
    use xgc_render::AdditiveLayer;

    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().unwrap().joint_nodes;

    let base = animations
        .joint_pose("Walk", 0.4, true, joints)
        .expect("Walk 可采样");
    // 权重 0：增量被完全乘掉，必须与纯基础姿势逐位一致（旧端 blend_additive 同式）。
    let zero = animations
        .joint_pose_with_additive(
            "Walk",
            0.4,
            true,
            Some(AdditiveLayer {
                clip: "Run",
                time: 0.5,
                looping: true,
                weight: 0.0,
            }),
            joints,
        )
        .expect("叠加层可采样");
    assert_eq!(zero, base, "weight=0 的叠加层不能改动基础姿势");

    // 权重 1：Run 相对它第 0 帧的增量要真的加上去。
    let full = animations
        .joint_pose_with_additive(
            "Walk",
            0.4,
            true,
            Some(AdditiveLayer {
                clip: "Run",
                time: 0.5,
                looping: true,
                weight: 1.0,
            }),
            joints,
        )
        .expect("叠加层可采样");
    assert!(
        full.iter()
            .zip(&base)
            .any(|(a, b)| a.iter().zip(b).any(|(x, y)| (x - y).abs() > 1e-4)),
        "weight=1 的叠加层应改变关节矩阵"
    );

    // 叠加剪辑不存在：退回基础姿势，不报错、不把角色画没。
    let missing = animations
        .joint_pose_with_additive(
            "Walk",
            0.4,
            true,
            Some(AdditiveLayer {
                clip: "NoSuchClip",
                time: 0.5,
                looping: true,
                weight: 1.0,
            }),
            joints,
        )
        .expect("叠加剪辑缺失时仍应返回基础姿势");
    assert_eq!(missing, base);
}

#[test]
fn box_glb_has_no_animations() {
    let root = Path::new(RESOURCES);
    if !root.join("Box.glb").exists() {
        eprintln!("skip: 没有 Box.glb");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Box.glb").unwrap().unwrap();
    assert!(
        mesh.animations.is_none(),
        "Box.glb 没有 animations，别凭空造剪辑"
    );
}

#[test]
fn cubic_gltf_samples_trs_from_the_output_accessors() {
    let fixture = CubicFixture::new("pose", |_| {});
    let mesh = load_gltf(&fixture.path()).unwrap();
    let animations = mesh.animations.unwrap();
    let clip = animations.clip("Spline").unwrap();
    assert_eq!(clip.duration, 2.25);
    assert_eq!(clip.channels.len(), 4);
    assert!(clip
        .channels
        .iter()
        .all(|channel| channel.interpolation.name() == "CUBICSPLINE"));
    // 首帧的进切线和末帧的出切线不参与段内采样，仍须保留在模型里。
    assert_eq!(clip.channels[0].values.len(), 18);
    assert_eq!(&clip.channels[0].values[..3], &[7.0, 8.0, 9.0]);
    assert_eq!(&clip.channels[0].values[15..], &[9.0, 8.0, 7.0]);

    let start = animations.local_pose("Spline", -1.0, false).unwrap();
    assert_eq!(start[0].translation, [2.0, -1.0, 4.0]);
    assert_eq!(start[0].scale, [1.0, 2.0, 1.0]);
    assert_eq!(start[0].rotation, [0.0, 0.0, 0.0, 1.0]);
    let end = animations.local_pose("Spline", 4.0, false).unwrap();
    assert_eq!(end[0].translation, [4.0, 3.0, 8.0]);
    assert_eq!(end[0].scale, [3.0, 1.0, 2.0]);
    assert_eq!(end[0].rotation, [0.0, 0.0, 1.0, 0.0]);

    let middle = animations.local_pose("Spline", 1.25, false).unwrap();
    // 2 秒段的中点权重是 (1/2, 1/8, 1/2, -1/8)，切线要先乘 2。
    assert_eq!(middle[0].translation, [3.5, 1.5, 6.5]);
    assert_eq!(middle[0].scale, [1.5, 2.0, 1.5]);
    for (actual, expected) in
        middle[0]
            .rotation
            .into_iter()
            .zip([0.0, 2.0 / 3.0, 2.0 / 3.0, 1.0 / 3.0])
    {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }
    // 相反号的同一旋转会在中点抵消，短弧 slerp 应保住单位四元数。
    assert_eq!(middle[1].rotation, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(animations.local_pose("Spline", 3.5, true).unwrap(), middle);
    assert_eq!(animations.local_pose("Spline", 2.25, true).unwrap(), start);
}

#[test]
fn cubic_root_translation_matches_the_pose_and_unfolds_loops() {
    let fixture = CubicFixture::new("motion", |_| {});
    let animations = load_gltf(&fixture.path()).unwrap().animations.unwrap();
    let root_motion = animations.root_motion_set();
    let track = root_motion.clip("Spline").unwrap().track("hip").unwrap();
    for time in [-0.5, 0.25, 0.625, 1.25, 2.125, 2.25, 3.0] {
        let pose = animations.local_pose("Spline", time, false).unwrap();
        assert_eq!(track.sample(time), pose[0].translation, "{time}");
    }
    let motion = RootMotion {
        node: "hip".to_string(),
        axes: [true, false, true],
    };
    let span = MotionSpan {
        clip: "Spline",
        from: 1.25,
        to: 2.5,
        looping: true,
    };
    assert_eq!(
        root_motion_delta(&root_motion, &motion, span, None),
        Some([0.5, 0.0, 1.5])
    );
    assert_eq!(
        root_motion_delta(&root_motion, &motion, MotionSpan { to: 3.5, ..span }, None),
        Some([2.0, 0.0, 4.0])
    );
}

#[test]
fn linear_and_step_gltf_keep_existing_translation_sampling() {
    for (interpolation, middle) in [("LINEAR", [4.5, 3.5, 6.5]), ("STEP", [7.0, 8.0, 9.0])] {
        let fixture = CubicFixture::new(interpolation, |document| {
            // 两组三元直接作为两帧关键值，沿用同一二进制 fixture。
            document["animations"][0]["samplers"][0]["interpolation"] = json!(interpolation);
            document["accessors"][2]["count"] = json!(2);
        });
        let animations = load_gltf(&fixture.path()).unwrap().animations.unwrap();
        let root_motion = animations.root_motion_set();
        let track = root_motion.clip("Spline").unwrap().track("hip").unwrap();
        for (time, expected) in [
            (0.25, [7.0, 8.0, 9.0]),
            (1.25, middle),
            (2.25, [2.0, -1.0, 4.0]),
        ] {
            let pose = animations.local_pose("Spline", time, false).unwrap();
            assert_eq!(pose[0].translation, expected, "{interpolation}, {time}");
            assert_eq!(track.sample(time), expected, "{interpolation}, {time}");
        }
    }
}

#[test]
fn cubic_gltf_rejects_bad_output_accessor_counts() {
    for count in [5, usize::MAX] {
        let fixture = CubicFixture::new(&format!("count-{count}"), |document| {
            document["accessors"][2]["count"] = json!(count);
        });
        let error = load_gltf(&fixture.path()).unwrap_err();
        assert!(matches!(error, MeshError::Glb(_)), "{error}");
    }
}

#[test]
fn cubic_gltf_rejects_out_of_bounds_animation_accessors() {
    let short_output = CubicFixture::new("short-output", |document| {
        // BIN 里还有下一条通道；当前 bufferView 仍然必须覆盖自己的全部关键帧。
        document["bufferViews"][2]["byteLength"] = json!(60);
    });
    let huge_input = CubicFixture::new("huge-input", |document| {
        document["accessors"][1]["count"] = json!(usize::MAX);
    });
    for fixture in [short_output, huge_input] {
        let error = load_gltf(&fixture.path()).unwrap_err();
        assert!(matches!(error, MeshError::Glb(_)), "{error}");
    }
}

struct CubicFixture(PathBuf);

impl CubicFixture {
    fn new(label: &str, edit: impl FnOnce(&mut Value)) -> Self {
        let dir = std::env::temp_dir().join(format!("xgc-cubic-{}-{label}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let fixture = Self(dir);

        let mut bytes = Vec::new();
        let mut views = Vec::new();
        let mut accessors = Vec::new();
        let mut push = |values: &[f32], count: usize, ty: &str| {
            views.push(json!({
                "buffer": 0, "byteOffset": bytes.len(), "byteLength": values.len() * 4
            }));
            bytes.extend(values.iter().flat_map(|value| value.to_le_bytes()));
            accessors.push(json!({
                "bufferView": views.len() - 1, "componentType": 5126, "count": count, "type": ty
            }));
        };
        push(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0], 3, "VEC3");
        push(&[0.25, 2.25], 2, "SCALAR");
        // 每组三元或四元依次是入切线、关键值、出切线。两帧分别写六组。
        push(
            [
                [7.0, 8.0, 9.0],
                [2.0, -1.0, 4.0],
                [4.0, 0.0, 2.0],
                [2.0, -2.0, 0.0],
                [4.0, 3.0, 8.0],
                [9.0, 8.0, 7.0],
            ]
            .as_flattened(),
            6,
            "VEC3",
        );
        push(
            [
                [0.0; 3],
                [1.0, 2.0, 1.0],
                [0.0, 2.0, -0.5],
                [2.0, 0.0, -0.5],
                [3.0, 1.0, 2.0],
                [0.0; 3],
            ]
            .as_flattened(),
            6,
            "VEC3",
        );
        push(
            [
                [0.0; 4],
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 2.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0; 4],
            ]
            .as_flattened(),
            6,
            "VEC4",
        );
        push(
            [
                [0.0; 4],
                [0.0, 0.0, 0.0, 1.0],
                [0.0; 4],
                [0.0; 4],
                [0.0, 0.0, 0.0, -1.0],
                [0.0; 4],
            ]
            .as_flattened(),
            6,
            "VEC4",
        );
        accessors[0]["min"] = json!([0, 0, 0]);
        accessors[0]["max"] = json!([1, 1, 0]);
        accessors[1]["min"] = json!([0.25]);
        accessors[1]["max"] = json!([2.25]);

        let samplers: Vec<_> = (2..6)
            .map(|output| json!({"input": 1, "output": output, "interpolation": "CUBICSPLINE"}))
            .collect();
        let mut document = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"uri": "spline.bin", "byteLength": bytes.len()}],
            "bufferViews": views,
            "accessors": accessors,
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
            "nodes": [{"name": "hip", "mesh": 0, "children": [1]}, {"name": "bone"}],
            "scenes": [{"nodes": [0]}],
            "scene": 0,
            "animations": [{
                "name": "Spline",
                "samplers": samplers,
                "channels": [
                    {"sampler": 0, "target": {"node": 0, "path": "translation"}},
                    {"sampler": 1, "target": {"node": 0, "path": "scale"}},
                    {"sampler": 2, "target": {"node": 0, "path": "rotation"}},
                    {"sampler": 3, "target": {"node": 1, "path": "rotation"}}
                ]
            }]
        });
        edit(&mut document);
        fs::write(fixture.path(), document.to_string()).unwrap();
        fs::write(fixture.0.join("spline.bin"), bytes).unwrap();
        fixture
    }

    fn path(&self) -> PathBuf {
        self.0.join("spline.gltf")
    }
}

impl Drop for CubicFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
