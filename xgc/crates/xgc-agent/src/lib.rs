//! XGC Agent 协议最小集：一行一条 JSON-RPC 2.0，stdin/stdout。
//!
//! 方法：`engine.describe` / `world.query` / `world.load` / `world.save` /
//! `edit.plan` / `edit.apply` / `edit.revert`。会话持有内存世界；命令与数据都
//! 走已经可序列化的 `Command` / `WorldData`，不另造一套。

#![deny(unsafe_code)]

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value as Json};
use xgc_reflect::Registry;
use xgc_store::{load_world, save_world};
use xgc_world::{Applied, Command, Plan, World};

pub const PROTOCOL: &str = "xgc.agent/1";

pub struct Agent {
    world: World,
    registry: Registry,
    plans: HashMap<u64, Plan>,
    applied: HashMap<u64, Applied>,
    next_plan: u64,
    next_tx: u64,
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}

impl Agent {
    pub fn new() -> Self {
        let mut registry = Registry::new();
        let _ = registry.register::<xgc_world::Transform>();
        let _ = registry.register::<xgc_world::Velocity>();
        let _ = registry.register::<xgc_world::Name>();
        Self {
            world: World::new(),
            registry,
            plans: HashMap::new(),
            applied: HashMap::new(),
            next_plan: 0,
            next_tx: 0,
        }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// 处理一行；通知（无 id）返回 None。
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let request: Json = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(err) => {
                return Some(error_response(Json::Null, format!("JSON 解析失败：{err}")));
            }
        };
        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let params = request.get("params").cloned().unwrap_or(Json::Null);

        let result = self.dispatch(&method, &params);
        let id = id?;
        Some(match result {
            Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}).to_string(),
            Err(message) => error_response(id, message),
        })
    }

    fn dispatch(&mut self, method: &str, params: &Json) -> Result<Json, String> {
        match method {
            "engine.describe" => self.describe(),
            "world.query" => self.query(params),
            "world.load" => self.load(params),
            "world.save" => self.save(params),
            "edit.plan" => self.plan(params),
            "edit.apply" => self.apply(params),
            "edit.revert" => self.revert(params),
            other => Err(format!("未知方法：{other}")),
        }
    }

    fn describe(&self) -> Result<Json, String> {
        Ok(json!({
            "engine": "xgc",
            "protocol": PROTOCOL,
            "version": env!("CARGO_PKG_VERSION"),
            "components": self.registry.schema(),
            "commands": ["set_field", "spawn", "despawn"],
            "world": {
                "revision": self.world.revision(),
                "entities": self.world.len(),
            },
            "limits": {"query_default": 100, "query_max": 1000},
        }))
    }

    fn query(&self, params: &Json) -> Result<Json, String> {
        let offset = params.get("offset").and_then(Json::as_u64).unwrap_or(0) as usize;
        let limit = params
            .get("limit")
            .and_then(Json::as_u64)
            .unwrap_or(100)
            .min(1000) as usize;
        let component = match params.get("component").and_then(Json::as_str) {
            Some(name) => Some(
                self.registry
                    .by_name(name)
                    .ok_or_else(|| format!("未知组件：{name}"))?
                    .id,
            ),
            None => None,
        };

        let data = self.world.world_data();
        let mut entities: Vec<_> = data
            .entities
            .into_iter()
            .filter(|entity| {
                component.is_none_or(|id| {
                    entity
                        .components
                        .iter()
                        .any(|component| component.component.0 == id)
                })
            })
            .collect();
        let total = entities.len();
        let start = offset.min(total);
        let end = start.saturating_add(limit).min(total);
        let page: Vec<_> = entities.drain(start..end).collect();
        Ok(json!({
            "revision": data.revision,
            "total": total,
            "offset": start,
            "entities": page,
            "next_offset": if end < total { Some(end) } else { None },
        }))
    }

    fn load(&mut self, params: &Json) -> Result<Json, String> {
        let path = required_str(params, "path")?;
        let data = load_world(Path::new(path)).map_err(|err| err.to_string())?;
        let mut world = World::new();
        world
            .restore_world_data(&data)
            .map_err(|message| format!("恢复失败：{message}"))?;
        self.world = world;
        self.plans.clear();
        self.applied.clear();
        Ok(json!({
            "revision": self.world.revision(),
            "entities": self.world.len(),
        }))
    }

    fn save(&self, params: &Json) -> Result<Json, String> {
        let path = required_str(params, "path")?;
        let data = self.world.world_data();
        save_world(Path::new(path), &data).map_err(|err| err.to_string())?;
        Ok(json!({
            "path": path,
            "revision": data.revision,
            "entities": data.entities.len(),
        }))
    }

    fn plan(&mut self, params: &Json) -> Result<Json, String> {
        let commands = params
            .get("commands")
            .cloned()
            .ok_or_else(|| "缺少 commands".to_string())?;
        let commands: Vec<Command> =
            serde_json::from_value(commands).map_err(|err| format!("命令解析失败：{err}"))?;

        let plan = self.world.plan(commands);
        let plan_id = self.next_plan;
        self.next_plan += 1;
        let diagnostics: Vec<_> = plan
            .diagnostics
            .iter()
            .map(|diagnostic| {
                json!({
                    "code": diagnostic.code,
                    "severity": diagnostic.severity.to_string(),
                    "subject": diagnostic.subject.to_string(),
                    "message": diagnostic.message,
                })
            })
            .collect();
        let response = json!({
            "plan_id": plan_id,
            "base_revision": plan.base_revision,
            "valid": plan.is_valid(),
            "commands": plan.len(),
            "diagnostics": diagnostics,
        });
        self.plans.insert(plan_id, plan);
        Ok(response)
    }

    fn apply(&mut self, params: &Json) -> Result<Json, String> {
        let plan_id = required_u64(params, "plan_id")?;
        let plan = self
            .plans
            .remove(&plan_id)
            .ok_or_else(|| format!("未知 plan_id：{plan_id}"))?;
        let applied = self
            .world
            .apply(&plan)
            .map_err(|diagnostics| format!("应用失败：{diagnostics:?}"))?;
        let tx_id = self.next_tx;
        self.next_tx += 1;
        let response = json!({
            "tx_id": tx_id,
            "revision": applied.revision,
            "inverse": applied.inverse,
        });
        self.applied.insert(tx_id, applied);
        Ok(response)
    }

    fn revert(&mut self, params: &Json) -> Result<Json, String> {
        let tx_id = required_u64(params, "tx_id")?;
        let applied = self
            .applied
            .remove(&tx_id)
            .ok_or_else(|| format!("未知 tx_id：{tx_id}"))?;
        let revision = self
            .world
            .revert(&applied)
            .map_err(|diagnostics| format!("回滚失败：{diagnostics:?}"))?;
        Ok(json!({"tx_id": tx_id, "revision": revision}))
    }
}

fn required_str<'a>(params: &'a Json, key: &str) -> Result<&'a str, String> {
    params
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("缺少字符串参数 {key}"))
}

fn required_u64(params: &Json, key: &str) -> Result<u64, String> {
    params
        .get(key)
        .and_then(Json::as_u64)
        .ok_or_else(|| format!("缺少整数参数 {key}"))
}

fn error_response(id: Json, message: String) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": -32000, "message": message},
    })
    .to_string()
}
