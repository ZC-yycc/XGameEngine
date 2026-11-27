//! XGC Agent 协议最小集：一行一条 JSON-RPC 2.0，stdin/stdout。
//!
//! `engine.describe` 从方法目录公开协议能力。会话持有内存世界，编辑与快照
//! 共用 `Command` / `WorldData` 的序列化格式。

#![deny(unsafe_code)]

use std::collections::HashMap;

use serde_json::{json, Value as Json};
use xgc_reflect::{ComponentId, FieldPath, Registry};
use xgc_world::{Applied, Command, Journal, Plan, World, WorldSnapshot};

mod edit_apply;
mod explain;
mod mcp;
mod methods;
mod project_authoring;
mod query;
mod schemas;
mod script_check;
mod sim_run;
mod snapshot;

#[cfg(test)]
mod container_tests;

pub use mcp::Mcp;
use query::{
    axis_index, omit_entity_defaults, projected_columns, projected_rows, FieldRef, Filter,
    FilterOp, Projection, QueryEpoch, QUERY_DEFAULT_LIMIT, QUERY_MAX_LIMIT,
};

pub const PROTOCOL: &str = "xgc.agent/1";

pub struct Agent {
    world: World,
    query_epoch: QueryEpoch,
    registry: Registry,
    plans: HashMap<u64, Plan>,
    applied: HashMap<u64, Applied>,
    apply_receipts: HashMap<String, edit_apply::ApplyReceipt>,
    journal: Journal,
    base: WorldSnapshot,
    project_scene: Option<project_authoring::ProjectScene>,
    next_plan: u64,
    recorded_preview: Option<sim_run::RecordedPreview>,
    next_trace: u64,
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
            query_epoch: QueryEpoch::new(),
            registry,
            plans: HashMap::new(),
            applied: HashMap::new(),
            apply_receipts: HashMap::new(),
            journal: Journal::new(),
            base,
            project_scene: None,
            next_plan: 0,
            recorded_preview: None,
            next_trace: 0,
        }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    fn call(&mut self, method: &str, params: &Json) -> Result<Json, String> {
        methods::by_name(method)
            .ok_or_else(|| format!("未知方法：{method}"))?
            .call(self, params)
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

        let result = self.call(&method, &params);
        let id = id?;
        Some(match result {
            Ok(value) => {
                let mut response = json!({"jsonrpc": "2.0", "id": id});
                response["result"] = value;
                response.to_string()
            }
            Err(message) => error_response(id, message),
        })
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
            "methods": methods::METHODS.iter().map(|method| method.name).collect::<Vec<_>>(),
            "method_details": methods::METHODS.iter().map(methods::Method::describe).collect::<Vec<_>>(),
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
            "limits": {"query_default": QUERY_DEFAULT_LIMIT, "query_max": QUERY_MAX_LIMIT},
        }))
    }

    fn query(&self, params: &Json) -> Result<Json, String> {
        let offset = params.get("offset").and_then(Json::as_u64).unwrap_or(0) as usize;
        let limit = params
            .get("limit")
            .and_then(Json::as_u64)
            .unwrap_or(QUERY_DEFAULT_LIMIT)
            .min(QUERY_MAX_LIMIT) as usize;
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
        let include_defaults = params
            .get("include_defaults")
            .filter(|value| !value.is_null())
            .map(|value| value.as_bool().ok_or("include_defaults 必须是布尔值"))
            .transpose()?
            .unwrap_or(true);
        let defaults = (!include_defaults).then_some(&self.registry);
        let columns = match params.get("format").filter(|value| !value.is_null()) {
            None => false,
            Some(value) => match value.as_str() {
                Some("rows") => false,
                Some("columns") if select.is_some() => true,
                Some("columns") => return Err("列式查询需要 select".into()),
                _ => return Err("format 只接受 rows 或 columns".into()),
            },
        };
        let filter = self.parse_filter(params)?;
        let revision = self.world.revision();
        let after = params
            .get("cursor")
            .filter(|value| !value.is_null())
            .map(|value| self.query_epoch.resume(revision, value))
            .transpose()?;

        // 总量与偏移沿用精确计数，先只收集匹配的 ID。完整实体和投影值都到分页后才读取。
        let mut ids: Vec<_> = self
            .world
            .entity_ids(component.map(ComponentId))
            .filter(|&id| {
                filter
                    .as_ref()
                    .is_none_or(|filter| filter.matches(&self.world, id))
            })
            .collect();
        ids.sort_unstable();
        let total = ids.len();
        let start = after.map_or_else(
            || offset.min(total),
            |last| ids.partition_point(|&id| id <= last),
        );
        let end = start.saturating_add(limit).min(total);
        let page = &ids[start..end];
        let next_offset = (end < total).then_some(end);
        let next_cursor = next_offset.and_then(|_| {
            page.last()
                .copied()
                .or(after)
                .map(|last| self.query_epoch.cursor(revision, last))
        });
        let mut result = json!({
            "revision": revision,
            "total": total,
            "offset": start,
            "next_offset": next_offset,
            "next_cursor": next_cursor,
        });
        if let Some(fields) = select {
            let projections: Vec<_> = fields
                .iter()
                .map(|field| Projection::new(field, defaults))
                .collect();
            if columns {
                let (columns, rows) =
                    projected_columns(&self.world, page, &projections, !include_defaults);
                result["columns"] = columns;
                result["rows"] = rows;
            } else {
                result["rows"] = projected_rows(&self.world, page, &projections);
            }
        } else {
            let mut entities: Vec<_> = page
                .iter()
                .filter_map(|&id| self.world.entity_data(id))
                .collect();
            if let Some(registry) = defaults {
                for entity in &mut entities {
                    omit_entity_defaults(entity, registry);
                }
            }
            result["entities"] = json!(entities);
        }
        Ok(result)
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
        let field = FieldPath::parse(field)?;
        let root = schema
            .fields
            .iter()
            .find(|item| item.name == field.root())
            .ok_or_else(|| format!("组件 {component} 没有字段 {}", field.root()))?;
        let schema_field = field.schema(root)?;
        Ok(FieldRef {
            component: ComponentId(schema.id),
            label: path.to_string(),
            ty: schema_field.ty.clone(),
            path: field,
        })
    }

    fn get(&self, params: &Json) -> Result<Json, String> {
        let id = required_u64(params, "id")?;
        let entity = self.world.entity_data(xgc_base::EntityId::from_raw(id));
        Ok(json!({"entity": entity}))
    }

    fn replay(&mut self) -> Result<Json, String> {
        // 重放先恢复基准再执行事务，失败也可能已经换过当前世界。
        self.query_epoch = QueryEpoch::new();
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
                Command::Replace { data } => json!({
                    "kind": "replace",
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

    fn undo(&mut self) -> Result<Json, String> {
        let tx_id = self
            .applied
            .keys()
            .copied()
            .max()
            .ok_or_else(|| "没有可撤销的事务".to_string())?;
        self.revert_transaction(tx_id, "撤销失败")
    }

    fn revert(&mut self, params: &Json) -> Result<Json, String> {
        let tx_id = required_u64(params, "tx_id")?;
        self.revert_transaction(tx_id, "回滚失败")
    }

    fn revert_transaction(&mut self, tx_id: u64, error_prefix: &str) -> Result<Json, String> {
        let applied = self
            .applied
            .get(&tx_id)
            .ok_or_else(|| format!("未知 tx_id：{tx_id}"))?;
        let revision = self
            .world
            .revert_logged(applied, &mut self.journal)
            .map_err(|diagnostics| format!("{error_prefix}：{diagnostics:?}"))?;
        self.applied.remove(&tx_id);
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
