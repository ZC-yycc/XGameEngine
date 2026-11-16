//! 诊断码目录：所有对外可见的 code 在这里定义一次，命令 / 校验收敛引用。

pub const EDIT_UNKNOWN_ENTITY: &str = "EDIT.UNKNOWN_ENTITY";
pub const EDIT_UNKNOWN_ARCHETYPE: &str = "EDIT.UNKNOWN_ARCHETYPE";
pub const EDIT_UNKNOWN_COMPONENT: &str = "EDIT.UNKNOWN_COMPONENT";
pub const EDIT_UNKNOWN_FIELD: &str = "EDIT.UNKNOWN_FIELD";
pub const EDIT_TYPE_MISMATCH: &str = "EDIT.TYPE_MISMATCH";
pub const EDIT_ID_IN_USE: &str = "EDIT.ID_IN_USE";
pub const EDIT_INVALID_SPAWN: &str = "EDIT.INVALID_SPAWN";
pub const EDIT_ENTITY_DEAD_IN_PLAN: &str = "EDIT.ENTITY_DEAD_IN_PLAN";
pub const EDIT_COMPONENT_EXISTS: &str = "EDIT.COMPONENT_EXISTS";
pub const EDIT_COMPONENT_MISSING: &str = "EDIT.COMPONENT_MISSING";
pub const EDIT_INVALID_COMPONENT: &str = "EDIT.INVALID_COMPONENT";
pub const EDIT_CAPTURE_FAILED: &str = "EDIT.CAPTURE_FAILED";
pub const EDIT_APPLY_FAILED: &str = "EDIT.APPLY_FAILED";
pub const EDIT_REVERT_FAILED: &str = "EDIT.REVERT_FAILED";
pub const EDIT_REVISION_CONFLICT: &str = "EDIT.REVISION_CONFLICT";

pub const WORLD_DUP_ENTITY: &str = "WORLD.DUP_ENTITY";
pub const WORLD_DUP_COMPONENT: &str = "WORLD.DUP_COMPONENT";
pub const WORLD_UNKNOWN_COMPONENT: &str = "WORLD.UNKNOWN_COMPONENT";
pub const WORLD_DUP_FIELD: &str = "WORLD.DUP_FIELD";
pub const WORLD_UNKNOWN_FIELD: &str = "WORLD.UNKNOWN_FIELD";
pub const WORLD_UNKNOWN_TYPE: &str = "WORLD.UNKNOWN_TYPE";
pub const WORLD_TYPE_MISMATCH: &str = "WORLD.TYPE_MISMATCH";
pub const WORLD_RANGE: &str = "WORLD.RANGE";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagnosticCode {
    pub code: &'static str,
    pub summary: &'static str,
}

pub const CATALOG: &[DiagnosticCode] = &[
    DiagnosticCode {
        code: EDIT_UNKNOWN_ENTITY,
        summary: "命令 / 查询引用了不存在的实体",
    },
    DiagnosticCode {
        code: EDIT_UNKNOWN_ARCHETYPE,
        summary: "实体数据没有对应的组件组合",
    },
    DiagnosticCode {
        code: EDIT_UNKNOWN_COMPONENT,
        summary: "组件 ID 未在注册表登记",
    },
    DiagnosticCode {
        code: EDIT_UNKNOWN_FIELD,
        summary: "组件没有该字段",
    },
    DiagnosticCode {
        code: EDIT_TYPE_MISMATCH,
        summary: "字段值与 schema 类型不匹配",
    },
    DiagnosticCode {
        code: EDIT_ID_IN_USE,
        summary: "spawn 指定的实体 ID 已存在",
    },
    DiagnosticCode {
        code: EDIT_INVALID_SPAWN,
        summary: "spawn 数据不合法",
    },
    DiagnosticCode {
        code: EDIT_ENTITY_DEAD_IN_PLAN,
        summary: "同一计划里先删除了实体，后续命令仍引用它",
    },
    DiagnosticCode {
        code: EDIT_COMPONENT_EXISTS,
        summary: "add_component 的组件已存在",
    },
    DiagnosticCode {
        code: EDIT_COMPONENT_MISSING,
        summary: "remove_component / set_field 的组件不存在",
    },
    DiagnosticCode {
        code: EDIT_INVALID_COMPONENT,
        summary: "组件数据无法构造",
    },
    DiagnosticCode {
        code: EDIT_CAPTURE_FAILED,
        summary: "revert 前无法捕获实体快照",
    },
    DiagnosticCode {
        code: EDIT_APPLY_FAILED,
        summary: "命令应用失败",
    },
    DiagnosticCode {
        code: EDIT_REVERT_FAILED,
        summary: "命令回退失败",
    },
    DiagnosticCode {
        code: EDIT_REVISION_CONFLICT,
        summary: "计划基于的 revision 与当前世界不一致",
    },
    DiagnosticCode {
        code: WORLD_DUP_ENTITY,
        summary: "WorldData 中实体 ID 重复",
    },
    DiagnosticCode {
        code: WORLD_DUP_COMPONENT,
        summary: "同一实体重复出现同一组件",
    },
    DiagnosticCode {
        code: WORLD_UNKNOWN_COMPONENT,
        summary: "实体数据含未知组件",
    },
    DiagnosticCode {
        code: WORLD_DUP_FIELD,
        summary: "组件字段重复",
    },
    DiagnosticCode {
        code: WORLD_UNKNOWN_FIELD,
        summary: "组件数据含未知字段",
    },
    DiagnosticCode {
        code: WORLD_UNKNOWN_TYPE,
        summary: "字段值类型无法识别",
    },
    DiagnosticCode {
        code: WORLD_TYPE_MISMATCH,
        summary: "字段值与组件 schema 不匹配",
    },
    DiagnosticCode {
        code: WORLD_RANGE,
        summary: "字段值超出允许范围",
    },
];

pub fn catalog() -> &'static [DiagnosticCode] {
    CATALOG
}
