//! 工具参数 schema 描述 Agent 接受的 JSON 形状。运行时校验由各方法负责。

use serde_json::{json, Value as Json};

use crate::explain::DEFAULT_LOD_BIAS;
use crate::query::{QUERY_DEFAULT_LIMIT, QUERY_MAX_LIMIT};
use crate::script_check::INLINE_NAME;

pub(super) fn no_params() -> Json {
    json!({"type": "object", "properties": {}, "description": "无需参数。"})
}

pub(super) fn script_check() -> Json {
    object(
        json!({
            "source": {"type": "string", "description": "待编译的 Lua 文本。"},
            "name": {
                "type": ["string", "null"], "default": INLINE_NAME,
                "description": "诊断显示名。缺少或为 null 时用缺省名。"
            }
        }),
        &["source"],
    )
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
            "format": {
                "enum": ["rows", "columns", null], "default": "rows",
                "description": "columns 需要 select，返回类型目录 columns 和按列排列的 rows。首列为实体 ID，其余列按 select 排列，缺失值为 null。"
            },
            "include_defaults": {
                "type": ["boolean", "null"], "default": true,
                "description": "false 省略等于 schema 默认值的字段。列描述的 default_rows 是本页零起始的 [start,end) 区间，区间里的 null 或缺省尾格可补默认值；其他 null 仍表示缺失。"
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

pub(super) fn explain() -> Json {
    let max = f64::from(f32::MAX);
    let mut schema = object(
        json!({
            "id": id_param("要检查的持久实体 ID。"),
            "camera": {
                "description": "带 render.camera 和 core.global_transform 的相机实体 ID。缺少或为 null 时按眼点或默认相机计算。",
                "oneOf": [unsigned(u64::MAX), decimal_id(), {"type": "null"}]
            },
            "eye": {
                "type": ["array", "null"], "minItems": 3, "maxItems": 3,
                "items": {"type": "number", "minimum": -max, "maximum": max},
                "description": "世界空间的眼点，与 camera 互斥。都未提供时使用首台活动相机，或回退到原点。"
            },
            "lod_bias": {
                "type": ["number", "null"], "minimum": -max, "maximum": max,
                "default": DEFAULT_LOD_BIAS,
                "description": "渲染器使用的有效 LOD 距离倍率，非正值固定使用首个可用级。"
            },
            "physics_with": {
                "description": "另一实体的持久 ID。指定后返回当前场景快照中两个 physics.body 正常建体后的形状重叠和传感器状态。传感器重叠不会产生分离冲量。历史轨迹需用仿真查看。",
                "oneOf": [unsigned(u64::MAX), decimal_id(), {"type": "null"}]
            }
        }),
        &["id"],
    );
    schema["not"] = json!({
        "required": ["camera", "eye"],
        "properties": {"camera": {"type": ["integer", "string"]}, "eye": {"type": "array"}}
    });
    schema
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
    object(
        json!({
            "plan_id": id_param("edit.plan 返回的计划 ID。"),
            "base_revision": {
                "oneOf": [unsigned(u64::MAX), decimal_id(), {"type": "null"}],
                "description": "首次提交时要求当前世界版本等于此值。缺少或为 null 时仍检查计划的基准版本。"
            },
            "idempotency_key": {
                "type": ["string", "null"],
                "description": "当前编辑会话内，凭此键和原 plan_id 重取成功回执。一个键只用于一份已提交的计划，加载或恢复世界会重置。"
            }
        }),
        &["plan_id"],
    )
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

pub(super) fn sim_run() -> Json {
    use crate::sim_run::{MAX_DT, MAX_FRAMES, MAX_WATCHES, MIN_DT};

    let mut watch = object(
        json!({
            "id": id_param("采样的持久实体 ID。"),
            "path": {"type": "string", "description": "与 world.query 相同的组件:字段路径。"}
        }),
        &["id", "path"],
    );
    watch["additionalProperties"] = json!(false);
    let mut schema = object(
        json!({
            "frames": unsigned(MAX_FRAMES),
            "dt": {"type": "number", "minimum": MIN_DT, "maximum": MAX_DT,
                   "description": "固定步秒数，省略时使用引擎默认值。"},
            "project": {"type": "string", "minLength": 1,
                        "description": "服务端原生项目目录或 project.xgc 路径。省略时只跑无资产系统。"},
            "watch": {"type": "array", "maxItems": MAX_WATCHES, "items": watch,
                      "description": "从第 0 帧起采样的实体字段，缺失值为 null。"},
            "record": {"type": "boolean", "default": false,
                       "description": "成功后返回 trace_id，并替换本连接上一次成功记录。"}
        }),
        &["frames"],
    );
    schema["additionalProperties"] = json!(false);
    schema
}

pub(super) fn sim_replay() -> Json {
    let mut schema = required_param(
        "trace_id",
        json!({"type": "string", "description": "本连接 sim.run 返回的记录 ID。"}),
    );
    schema["additionalProperties"] = json!(false);
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
