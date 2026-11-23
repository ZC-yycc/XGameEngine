//! `xgc plan`：打印默认调度的执行计划（系统访问声明 + 阶段划分）。
//!
//! 输出是确定性 JSON：两次运行逐字节相同，既给 Agent / 工具读，也是「计划即
//! 执行顺序」的证据——批次连续（摊平 == 注册顺序）、宿主阶段恒在最后。

use std::process::ExitCode;

use xgc_reflect::Registry;
use xgc_sim::{PlanStage, Schedule};
use xgc_world::{builtin_registry, ComponentId};

pub fn plan() -> ExitCode {
    let schedule = Schedule::default_schedule(1);
    let plan = schedule.plan();
    let registry = builtin_registry();

    let systems: Vec<serde_json::Value> = schedule
        .names()
        .iter()
        .zip(schedule.accesses())
        .enumerate()
        .map(|(index, (name, access))| {
            serde_json::json!({
                "index": index,
                "name": name,
                "reads": component_names(&access.reads, &registry),
                "writes": component_names(&access.writes, &registry),
            })
        })
        .collect();

    let stages: Vec<serde_json::Value> = plan
        .stages()
        .iter()
        .map(|stage| match stage {
            PlanStage::Batch(indices) => serde_json::json!({
                "kind": "batch",
                "indices": indices,
            }),
            PlanStage::Host => serde_json::json!({ "kind": "host" }),
        })
        .collect();

    let doc = serde_json::json!({ "systems": systems, "stages": stages });
    match serde_json::to_string_pretty(&doc) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("plan 序列化失败：{err}");
            ExitCode::from(1)
        }
    }
}

fn component_names(ids: &[ComponentId], registry: &Registry) -> Vec<String> {
    ids.iter()
        .map(|id| {
            registry
                .get(*id)
                .map(|schema| schema.name.clone())
                .unwrap_or_else(|| format!("component#{}", id.0))
        })
        .collect()
}
