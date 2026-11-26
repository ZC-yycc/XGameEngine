//! 工具参数 schema 描述 Agent 接受的 JSON 形状。运行时校验由各方法负责。

use serde_json::{json, Value as Json};

use crate::query::{QUERY_DEFAULT_LIMIT, QUERY_MAX_LIMIT};

pub(super) fn no_params() -> Json {
    json!({"type": "object", "properties": {}, "description": "无需参数。"})
}

pub(super) fn query() -> Json {
    let field_path = json!({
        "type": "string",
        "description": "组件名:字段路径，例如 core.transform:position。支持结构字段、列表索引和映射键。"
    });
    json!({
        "type": "object",
        "properties": {
            "component": {
                "type": ["string", "null"],
                "description": "指定注册组件名时只保留带该组件的实体。"
            },
            "offset": {
                "type": ["integer", "null"], "minimum": 0, "maximum": u64::MAX, "default": 0,
                "description": "从 0 起算的分页偏移。缺少或为 null 时从首项开始；有 cursor 时以 cursor 为准。"
            },
            "cursor": {
                "type": ["string", "null"],
                "description": "上一页的 next_cursor，从上一页的最后一个实体 ID 之后继续。沿用筛选条件；会话或世界版本变更后须重新查询。"
            },
            "limit": {
                "type": ["integer", "null"], "minimum": 0, "maximum": u64::MAX,
                "default": QUERY_DEFAULT_LIMIT,
                "description": format!("每页实体数。缺少或为 null 时取 {QUERY_DEFAULT_LIMIT}，超过 {QUERY_MAX_LIMIT} 时取 {QUERY_MAX_LIMIT}。")
            },
            "select": {
                "type": ["array", "null"],
                "items": field_path,
                "description": "指定字段时返回 rows，缺少或为 null 时返回完整 entities。"
            },
            "filter": {
                "type": ["object", "null"],
                "properties": {
                    "path": field_path,
                    "op": {"enum": ["eq", "ne", "lt", "le", "gt", "ge"]},
                    "axis": {
                        "enum": ["x", "y", "z", "w", null],
                        "description": "比较向量或四元数的一个分量。"
                    },
                    "value": {
                        "description": "比较目标。数值和字符串支持排序。相等比较也接受带 kind 和 value 的反射值。"
                    }
                },
                "required": ["path", "op", "value"]
            }
        }
    })
}

pub(super) fn get() -> Json {
    required_param("id", id_param("持久实体 ID。"))
}

pub(super) fn world_path() -> Json {
    required_param(
        "path",
        json!({
            "type": "string",
            "description": "服务端文件系统路径。相对路径以服务进程的工作目录为基准。"
        }),
    )
}

pub(super) fn apply() -> Json {
    required_param("plan_id", id_param("edit.plan 返回的计划 ID。"))
}

pub(super) fn revert() -> Json {
    required_param("tx_id", id_param("edit.apply 返回的事务 ID。"))
}

pub(super) fn restore() -> Json {
    let data = object(
        json!({
            "revision": unsigned(u64::MAX),
            "next_id": unsigned(u64::MAX),
            "entities": {"type": "array", "items": {"$ref": "#/$defs/EntityData"}},
            "environment": scene_environment()
        }),
        &["revision", "next_id", "entities"],
    );
    let mut schema = required_param("data", data);
    schema["$defs"] = entity_defs();
    schema
}

pub(super) fn plan() -> Json {
    let entity = decimal_id();
    let component = unsigned(u64::from(u32::MAX));
    let data = || object(json!({"data": {"$ref": "#/$defs/EntityData"}}), &["data"]);
    let commands = json!({
        "type": "array",
        "items": {"oneOf": [
            command("SetField", object(json!({
                "entity": entity,
                "component": component,
                "field": {"type": "string", "description": "组件内字段路径。"},
                "value": {"$ref": "#/$defs/Value"}
            }), &["entity", "component", "field", "value"])),
            command("AddComponent", object(json!({
                "entity": entity,
                "component": {"$ref": "#/$defs/ComponentData"}
            }), &["entity", "component"])),
            command("RemoveComponent", object(json!({
                "entity": entity,
                "component": component
            }), &["entity", "component"])),
            command("Spawn", data()),
            command("Despawn", object(json!({"entity": entity}), &["entity"])),
            command("Replace", data())
        ]}
    });
    let mut schema = required_param("commands", commands);
    schema["$defs"] = entity_defs();
    schema
}

fn object(properties: Json, required: &[&str]) -> Json {
    json!({"type": "object", "properties": properties, "required": required})
}

fn required_param(name: &str, schema: Json) -> Json {
    object(json!({(name): schema}), &[name])
}

fn unsigned(maximum: u64) -> Json {
    json!({"type": "integer", "minimum": 0, "maximum": maximum})
}

fn decimal_id() -> Json {
    json!({
        "type": "string", "pattern": "^\\+?[0-9]+$",
        "description": "十进制 u64 字符串。"
    })
}

fn id_param(description: &str) -> Json {
    json!({
        "description": description,
        "oneOf": [unsigned(u64::MAX), decimal_id()]
    })
}

fn command(name: &str, payload: Json) -> Json {
    let mut schema = required_param(name, payload);
    schema["additionalProperties"] = json!(false);
    schema
}

fn entity_defs() -> Json {
    json!({
        "EntityData": object(json!({
            "id": decimal_id(),
            "revision": unsigned(u64::MAX),
            "components": {"type": "array", "items": {"$ref": "#/$defs/ComponentData"}}
        }), &["id", "revision", "components"]),
        "ComponentData": object(json!({
            "component": unsigned(u64::from(u32::MAX)),
            "fields": {"type": "array", "items": {
                "type": "array", "minItems": 2, "maxItems": 2,
                "prefixItems": [{"type": "string"}, {"$ref": "#/$defs/Value"}]
            }}
        }), &["component", "fields"]),
        "Value": value_schema()
    })
}

// WorldData 与 Command 都带递归的 Value，引用在每个工具的 schema 根上解析。
fn value_schema() -> Json {
    let nested = json!({"$ref": "#/$defs/Value"});
    let fields = json!({"type": "object", "additionalProperties": nested});
    json!({"oneOf": [
        tagged("bool", json!({"type": "boolean"})),
        tagged("i64", json!({"type": "integer", "minimum": i64::MIN, "maximum": i64::MAX})),
        tagged("f64", json!({"type": "number"})),
        tagged("string", json!({"type": "string"})),
        tagged("vec2", vector(2)),
        tagged("vec3", vector(3)),
        tagged("vec4", vector(4)),
        tagged("quat", vector(4)),
        tagged("entity", unsigned(u64::MAX)),
        tagged("list", json!({"type": "array", "items": nested})),
        tagged("struct", fields.clone()),
        tagged("map", fields),
        tagged("enum", object(json!({
            "variant": {"type": "string"},
            "payload": nested
        }), &["variant", "payload"]))
    ]})
}

fn tagged(kind: &str, data: Json) -> Json {
    object(
        json!({"kind": {"const": kind}, "value": data}),
        &["kind", "value"],
    )
}

fn vector(len: usize) -> Json {
    json!({"type": "array", "items": {"type": "number"}, "minItems": len, "maxItems": len})
}

fn scene_environment() -> Json {
    let light = object(
        json!({
            "name": {"type": "string"},
            "kind": {"enum": ["Directional", "Point"]},
            "direction": vector(3),
            "position": vector(3),
            "color": vector(3),
            "intensity": {"type": "number"},
            "range": {"type": "number"},
            "casts_shadow": {"type": "boolean"},
            "shadow_resolution": unsigned(u64::from(u32::MAX))
        }),
        &["name"],
    );
    json!({"type": "object", "properties": {
        "lights": {"type": "array", "items": light},
        "ambient": vector(3)
    }})
}
