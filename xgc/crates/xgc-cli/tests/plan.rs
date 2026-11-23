//! `xgc plan`：默认调度执行计划的确定性输出。
//!
//! 钉住合同：两次运行逐字节相同；批次连续且摊平 == 注册顺序；宿主阶段恰好一次、
//! 默认调度里夹在固定步与动画之间；系统名与 `default_system_names` 同源、
//! 组件名都来自内建注册表。

use std::process::Command;

use serde_json::Value;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_xgc")
}

fn run_plan() -> String {
    let output = Command::new(binary())
        .arg("plan")
        .output()
        .expect("xgc plan 起不来");
    assert!(
        output.status.success(),
        "xgc plan 退出码非零：{:?}",
        output.stderr
    );
    String::from_utf8(output.stdout).expect("plan 输出不是 UTF-8")
}

#[test]
fn plan_is_deterministic() {
    assert_eq!(run_plan(), run_plan());
}

#[test]
fn plan_matches_default_schedule() {
    let doc: Value = serde_json::from_str(&run_plan()).expect("plan 不是合法 JSON");
    let systems = doc["systems"].as_array().expect("缺 systems");
    let names: Vec<&str> = systems
        .iter()
        .map(|system| system["name"].as_str().expect("系统缺 name"))
        .collect();
    assert_eq!(names, xgc_sim::default_system_names());

    let stages = doc["stages"].as_array().expect("缺 stages");
    assert_eq!(
        stages
            .iter()
            .filter(|stage| stage["kind"] == "host")
            .count(),
        1,
        "宿主阶段必须恰好一次"
    );
    assert_ne!(
        stages.last().expect("stages 为空")["kind"],
        "host",
        "默认调度的宿主阶段不在最后（动画系统在其后）"
    );

    let mut order = Vec::new();
    for stage in stages {
        match stage["kind"].as_str().expect("阶段缺 kind") {
            "batch" => {
                let indices: Vec<usize> = stage["indices"]
                    .as_array()
                    .expect("批缺 indices")
                    .iter()
                    .map(|value| value.as_u64().expect("下标不是整数") as usize)
                    .collect();
                assert!(
                    indices.windows(2).all(|pair| pair[1] == pair[0] + 1),
                    "批次必须连续：{indices:?}"
                );
                order.extend(indices);
            }
            "host" => {}
            other => panic!("未知阶段：{other}"),
        }
    }
    assert_eq!(order, (0..systems.len()).collect::<Vec<_>>());

    // 宿主阶段之后的第一个系统必须是 animation_state（旧端 Update 钩子先于动画 tick）。
    let mut seen = 0usize;
    for stage in stages {
        if stage["kind"] == "host" {
            break;
        }
        seen += stage["indices"]
            .as_array()
            .map_or(0, |indices| indices.len());
    }
    assert_eq!(names[order[seen]], "animation_state");
}

#[test]
fn plan_resolves_every_component_name() {
    let doc: Value = serde_json::from_str(&run_plan()).expect("plan 不是合法 JSON");
    for system in doc["systems"].as_array().expect("缺 systems") {
        for key in ["reads", "writes"] {
            for name in system[key].as_array().expect("缺读写集") {
                let name = name.as_str().expect("组件名不是字符串");
                assert!(
                    !name.starts_with("component#"),
                    "{} 的 {key} 里有未注册的组件：{name}",
                    system["name"]
                );
            }
        }
    }
}

#[test]
fn plan_ignores_extra_arguments() {
    let output = Command::new(binary())
        .arg("plan")
        .arg("--wat")
        .output()
        .expect("xgc plan 起不来");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), run_plan());
}
