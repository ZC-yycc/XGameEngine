//! 按实际项目清单和 cooked 包对照同一物理场景。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use xgc_base::layer::LayerIndex;
use xgc_store::{load_world, save_world};
use xgc_world::{CollisionLayer, PhysicsBody, Transform, Velocity, World};

#[test]
fn project_and_cooked_filter_a_pair_when_either_side_blocks_it() {
    let temp = Temp(std::env::temp_dir().join(format!("xgc-layer-project-{}", std::process::id())));
    let source = temp.0.join("project");
    let scene = source.join("scenes/entry.json");
    fs::create_dir_all(scene.parent().unwrap()).unwrap();
    let mut world = World::new();
    world.spawn((
        Transform {
            position: [0.0, 2.0, 0.0],
            ..Transform::default()
        },
        PhysicsBody {
            kind: "static".into(),
            shape: "cuboid".into(),
            half_extents: [3.0, 0.5, 3.0],
            ..PhysicsBody::default()
        },
        CollisionLayer {
            index: LayerIndex::new(31).unwrap(),
        },
    ));
    let ball = world.spawn((
        Transform {
            position: [0.0, 3.05, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        PhysicsBody {
            shape: "ball".into(),
            ..PhysicsBody::default()
        },
        CollisionLayer {
            index: LayerIndex::new(2).unwrap(),
        },
    ));
    let ball = world.entity(ball).unwrap();
    save_world(&scene, &world.world_data()).unwrap();

    let mut expected_hashes = Vec::new();
    for (name, masks) in [("solid", "{}"), ("blocked", "{31: 0}")] {
        fs::write(
            source.join("project.xgc"),
            format!(
                r#"(format_version: 1, name: "Groups", entry_scene: "scenes/entry.json",
                    scenes: ["scenes/entry.json"],
                    content: (models: "Resources", textures: "Resources", audio: "Resources",
                              prefabs: "Prefabs", scripts: "Scripts"),
                    layers: (names: {{31: "Platform"}}, masks: {masks}))"#
            ),
        )
        .unwrap();
        let snapshot = temp.0.join(format!("{name}.json"));
        let output = success(
            xgc()
                .arg("run-scripts")
                .args([&scene, &source])
                .arg("60")
                .arg(&snapshot),
        );
        let expected_hash = hash(&output);
        let mut result = World::new();
        result
            .restore_world_data(&load_world(&snapshot).unwrap())
            .unwrap();
        let y = result
            .get::<Transform>(result.key(ball).unwrap())
            .unwrap()
            .position[1];
        if name == "solid" {
            assert!((y - 3.0).abs() < 0.02, "球应停在平台上，实际 {y}");
        } else {
            assert!(y < 0.0, "第 31 层单向关闭接触后，球应落过平台，实际 {y}");
        }
        let mut agent = xgc_agent::Agent::new();
        let loaded = agent_call(&mut agent, "world.load", serde_json::json!({"path": scene}));
        assert_eq!(loaded["entities"], 2);
        let preview = agent_call(
            &mut agent,
            "sim.run",
            serde_json::json!({"frames": 60, "project": source}),
        );
        assert_eq!(preview["state_hash"], expected_hash);

        let cooked = temp.0.join(name);
        success(xgc().arg("pack").args([&scene, &source, &cooked]));
        let cooked_output = success(xgc().arg("run-cooked").arg(&cooked).arg("60"));
        assert_eq!(hash(&cooked_output), expected_hash);
        expected_hashes.push(expected_hash.to_string());

        if name == "blocked" {
            // 只改矩阵，世界和资产文件仍完全有效，签名也必须拒绝这个包。
            let manifest = cooked.join("cook.json");
            let mut unsigned: serde_json::Value =
                serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
            unsigned["runtime"]["layers"]["masks"]["31"] = serde_json::json!(u32::MAX);
            fs::write(manifest, serde_json::to_vec(&unsigned).unwrap()).unwrap();
            let rejected = xgc()
                .arg("run-cooked")
                .arg(&cooked)
                .arg("60")
                .output()
                .unwrap();
            assert_eq!(rejected.status.code(), Some(1));
            assert!(String::from_utf8_lossy(&rejected.stderr).contains("manifest 签名不匹配"));
        }
    }
    assert_ne!(expected_hashes[0], expected_hashes[1]);
}

fn agent_call(
    agent: &mut xgc_agent::Agent,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let request =
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let output = agent.handle_line(&request.to_string()).unwrap();
    let mut reply: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert!(reply.get("error").is_none(), "{output}");
    reply["result"].take()
}

fn xgc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xgc"))
}

fn hash(output: &str) -> &str {
    output
        .split_whitespace()
        .find_map(|word| word.strip_prefix("hash="))
        .unwrap()
}

fn success(command: &mut Command) -> String {
    let Output {
        status,
        stdout,
        stderr,
    } = command.output().unwrap();
    assert!(status.success(), "{}", String::from_utf8_lossy(&stderr));
    String::from_utf8(stdout).unwrap()
}

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
