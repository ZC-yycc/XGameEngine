//! XGC Agent 协议最小集：一行一条 JSON-RPC 2.0，stdin/stdout。
//!
//! 方法：`engine.describe` / `world.query` / `world.load` / `world.save` /
//! `edit.plan` / `edit.apply` / `edit.revert`。会话持有内存世界；命令与数据都
//! 走已经可序列化的 `Command` / `WorldData`，不另造一套。

#![deny(unsafe_code)]

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value as Json};
use xgc_reflect::{ComponentId, Registry};
use xgc_store::{load_world, save_world};
use xgc_world::{Applied, Command, Journal, Plan, World, WorldSnapshot};

mod query;
mod snapshot;

use query::{axis_index, FieldRef, Filter, FilterOp};

pub const PROTOCOL: &str = "xgc.agent/1";

pub struct Agent {
    world: World,
    registry: Registry,
    plans: HashMap<u64, Plan>,
    applied: HashMap<u64, Applied>,
    journal: Journal,
    base: WorldSnapshot,
    next_plan: u64,
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}

impl Agent {
    pub fn new() -> Self {
        let registry = xgc_world::builtin_registry();
        let world = World::new();
        let base = world.snapshot();
        Self {
            world,
            registry,
            plans: HashMap::new(),
            applied: HashMap::new(),
            journal: Journal::new(),
            base,
            next_plan: 0,
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
            "engine.diagnostics" => self.diagnostics(),
            "engine.history" => self.history(),
            "engine.replay" => self.replay(),
            "world.query" => self.query(params),
            "world.get" => self.get(params),
            "world.digest" => self.digest(),
            "world.stats" => self.stats(),
            "world.validate" => self.validate(),
            "world.load" => self.load(params),
            "world.save" => self.save(params),
            "world.snapshot" => self.snapshot(),
            "world.restore" => self.restore(params),
            "edit.plan" => self.plan(params),
            "edit.apply" => self.apply(params),
            "edit.revert" => self.revert(params),
            "edit.undo" => self.undo(),
            other => Err(format!("未知方法：{other}")),
        }
    }

    fn describe(&self) -> Result<Json, String> {
        Ok(json!({
            "engine": "xgc",
            "protocol": PROTOCOL,
            "version": env!("CARGO_PKG_VERSION"),
            "components": self.registry.schema(),
            "commands": [
                "set_field",
                "add_component",
                "remove_component",
                "spawn",
                "despawn"
            ],
            "methods": [
                "engine.describe",
                "engine.diagnostics",
                "engine.history",
                "engine.replay",
                "world.query",
                "world.get",
                "world.digest",
                "world.stats",
                "world.validate",
                "world.load",
                "world.save",
                "world.snapshot",
                "world.restore",
                "edit.plan",
                "edit.apply",
                "edit.revert",
                "edit.undo"
            ],
            "world": {
                "revision": self.world.revision(),
                "entities": self.world.len(),
            },
            "capabilities": {
                "render": ["camera", "depth", "instancing", "glb", "lod"],
                "sim": xgc_sim::default_system_names(),
                "migration": ["all_old_component_types"],
                "agent": ["query", "plan", "apply", "revert", "save_load"],
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
        let select = self.parse_select(params)?;
        let filter = self.parse_filter(params)?;

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
            .filter(|entity| filter.as_ref().is_none_or(|filter| filter.matches(entity)))
            .collect();
        let total = entities.len();
        let start = offset.min(total);
        let end = start.saturating_add(limit).min(total);
        let page: Vec<_> = entities.drain(start..end).collect();
        let next_offset = if end < total { Some(end) } else { None };

        if let Some(fields) = select {
            let rows: Vec<_> = page
                .iter()
                .map(|entity| {
                    let mut values = serde_json::Map::new();
                    for field in &fields {
                        let value = entity
                            .components
                            .iter()
                            .find(|component| component.component == field.component)
                            .and_then(|component| {
                                component
                                    .fields
                                    .iter()
                                    .find(|(name, _)| name == &field.field)
                                    .map(|(_, value)| value)
                            });
                        values.insert(
                            field.label(),
                            value
                                .and_then(|value| serde_json::to_value(value).ok())
                                .unwrap_or(Json::Null),
                        );
                    }
                    json!({"id": entity.id.to_string(), "values": values})
                })
                .collect();
            return Ok(json!({
                "revision": data.revision,
                "total": total,
                "offset": start,
                "rows": rows,
                "next_offset": next_offset,
            }));
        }

        Ok(json!({
            "revision": data.revision,
            "total": total,
            "offset": start,
            "entities": page,
            "next_offset": next_offset,
        }))
    }

    fn parse_select(&self, params: &Json) -> Result<Option<Vec<FieldRef>>, String> {
        let Some(select) = params.get("select") else {
            return Ok(None);
        };
        if select.is_null() {
            return Ok(None);
        }
        let items = select
            .as_array()
            .ok_or_else(|| "select 必须是字符串数组".to_string())?;
        let mut fields = Vec::with_capacity(items.len());
        for item in items {
            let path = item
                .as_str()
                .ok_or_else(|| "select 的条目必须是字符串".to_string())?;
            fields.push(self.resolve_field(path)?);
        }
        Ok(Some(fields))
    }

    fn parse_filter(&self, params: &Json) -> Result<Option<Filter>, String> {
        let Some(filter) = params.get("filter") else {
            return Ok(None);
        };
        if filter.is_null() {
            return Ok(None);
        }
        let path = filter
            .get("path")
            .and_then(Json::as_str)
            .ok_or_else(|| "filter.path 必须是字符串".to_string())?;
        let path = self.resolve_field(path)?;
        let op = match filter
            .get("op")
            .and_then(Json::as_str)
            .ok_or_else(|| "filter.op 必须是字符串".to_string())?
        {
            "eq" => FilterOp::Eq,
            "ne" => FilterOp::Ne,
            "lt" => FilterOp::Lt,
            "le" => FilterOp::Le,
            "gt" => FilterOp::Gt,
            "ge" => FilterOp::Ge,
            other => return Err(format!("未知过滤操作：{other}")),
        };
        let axis = match filter.get("axis").and_then(Json::as_str) {
            Some(axis) => Some(axis_index(axis)?),
            None => None,
        };
        let value = filter
            .get("value")
            .cloned()
            .ok_or_else(|| "filter.value 缺失".to_string())?;
        Ok(Some(Filter {
            path,
            axis,
            op,
            value,
        }))
    }

    fn resolve_field(&self, path: &str) -> Result<FieldRef, String> {
        let (component, field) = path
            .split_once(':')
            .ok_or_else(|| format!("字段路径必须是 组件:字段：{path}"))?;
        let schema = self
            .registry
            .by_name(component)
            .ok_or_else(|| format!("未知组件：{component}"))?;
        if !schema.fields.iter().any(|item| item.name == field) {
            return Err(format!("组件 {component} 没有字段 {field}"));
        }
        Ok(FieldRef {
            component: ComponentId(schema.id),
            component_name: component.to_string(),
            field: field.to_string(),
        })
    }

    fn get(&self, params: &Json) -> Result<Json, String> {
        let id = required_u64(params, "id")?;
        let entity = self.world.entity_data(xgc_base::EntityId::from_raw(id));
        Ok(json!({"entity": entity}))
    }

    fn replay(&mut self) -> Result<Json, String> {
        let hash = self
            .journal
            .replay(&mut self.world, &self.base)
            .map_err(|err| format!("重放失败：{err}"))?;
        self.applied.clear();
        Ok(json!({
            "revision": self.world.revision(),
            "hash": hash,
            "records": self.journal.len(),
            "entities": self.world.len(),
        }))
    }

    fn history(&self) -> Result<Json, String> {
        Ok(json!({
            "records": self.journal.records(),
            "count": self.journal.len(),
            "next_tx": self.journal.records().last().map_or(0, |record| record.tx_id + 1),
        }))
    }

    fn diagnostics(&self) -> Result<Json, String> {
        let codes: Vec<Json> = xgc_base::diagnostic_catalog()
            .iter()
            .map(|code| json!({"code": code.code, "summary": code.summary}))
            .collect();
        Ok(json!({"codes": codes}))
    }

    fn digest(&self) -> Result<Json, String> {
        Ok(json!({
            "revision": self.world.revision(),
            "entities": self.world.len(),
            "state_hash": format!("{:016x}", self.world.state_hash()),
        }))
    }

    fn stats(&self) -> Result<Json, String> {
        let data = self.world.world_data();
        let mut counts: HashMap<u32, usize> = HashMap::new();
        for entity in &data.entities {
            for component in &entity.components {
                *counts.entry(component.component.0).or_default() += 1;
            }
        }
        let mut components: Vec<_> = counts
            .into_iter()
            .map(|(id, count)| {
                let name = self
                    .registry
                    .get(xgc_reflect::ComponentId(id))
                    .map(|schema| schema.name.clone())
                    .unwrap_or_else(|| format!("unknown:{id}"));
                json!({"id": id, "component": name, "count": count})
            })
            .collect();
        components.sort_by_key(|row| row["id"].as_u64());
        Ok(json!({
            "revision": data.revision,
            "entities": data.entities.len(),
            "components": components,
        }))
    }

    fn validate(&self) -> Result<Json, String> {
        let diagnostics = self.world.validate(&self.registry);
        Ok(json!({
            "valid": diagnostics.is_empty(),
            "diagnostics": diagnostics.iter().map(|diagnostic| json!({
                "code": diagnostic.code,
                "severity": diagnostic.severity.to_string(),
                "subject": diagnostic.subject.to_string(),
                "message": diagnostic.message,
            })).collect::<Vec<_>>(),
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
        self.journal = Journal::new();
        self.base = self.world.snapshot();
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
        let preview: Vec<_> = plan
            .commands
            .iter()
            .map(|command| match command {
                Command::SetField {
                    entity,
                    component,
                    field,
                    ..
                } => json!({
                    "kind": "set_field",
                    "entity": entity.to_string(),
                    "component": component.0,
                    "field": field,
                    "before": self.world.get_field(*entity, *component, field),
                }),
                Command::AddComponent { entity, component } => json!({
                    "kind": "add_component",
                    "entity": entity.to_string(),
                    "component": component.component.0,
                    "fields": component.fields,
                }),
                Command::RemoveComponent { entity, component } => json!({
                    "kind": "remove_component",
                    "entity": entity.to_string(),
                    "component": component.0,
                }),
                Command::Spawn { data } => json!({
                    "kind": "spawn",
                    "entity": data.id.to_string(),
                    "components": data.components.iter().map(|component| component.component.0).collect::<Vec<_>>(),
                }),
                Command::Despawn { entity } => json!({
                    "kind": "despawn",
                    "entity": entity.to_string(),
                    "components": self.world.entity_data(*entity)
                        .map(|data| data.components.iter().map(|component| component.component.0).collect::<Vec<_>>())
                        .unwrap_or_default(),
                }),
            })
            .collect();
        let response = json!({
            "plan_id": plan_id,
            "base_revision": plan.base_revision,
            "valid": plan.is_valid(),
            "commands": plan.len(),
            "diagnostics": diagnostics,
            "preview": preview,
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
            .apply_logged(&plan, &mut self.journal)
            .map_err(|diagnostics| format!("应用失败：{diagnostics:?}"))?;
        let tx_id = self
            .journal
            .records()
            .last()
            .map_or(0, |record| record.tx_id);
        let response = json!({
            "tx_id": tx_id,
            "revision": applied.revision,
            "inverse": applied.inverse,
        });
        self.applied.insert(tx_id, applied);
        Ok(response)
    }

    fn undo(&mut self) -> Result<Json, String> {
        let tx_id = self
            .applied
            .keys()
            .copied()
            .max()
            .ok_or_else(|| "没有可撤销的事务".to_string())?;
        let applied = self.applied.remove(&tx_id).expect("刚查到的事务");
        let revision = self
            .world
            .revert(&applied)
            .map_err(|diagnostics| format!("撤销失败：{diagnostics:?}"))?;
        Ok(json!({"tx_id": tx_id, "revision": revision}))
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
    match params.get(key) {
        Some(Json::Number(number)) => number
            .as_u64()
            .ok_or_else(|| format!("参数 {key} 必须是非负整数")),
        Some(Json::String(text)) => text
            .parse::<u64>()
            .map_err(|err| format!("参数 {key} 不是十进制 u64：{err}")),
        _ => Err(format!("缺少整数参数 {key}")),
    }
}

fn error_response(id: Json, message: String) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": -32000, "message": message},
    })
    .to_string()
}
