//! 方法目录同时驱动 JSON-RPC 分派、协议发现和 MCP 工具发现。

use serde_json::{json, Value as Json};

use crate::{explain, schemas, script_check, sim_run, Agent};

pub(super) struct Method {
    pub(super) name: &'static str,
    pub(super) description: &'static str,
    pub(super) input_schema: fn() -> Json,
    handler: fn(&mut Agent, &Json) -> Result<Json, String>,
}

impl Method {
    const fn new(
        name: &'static str,
        description: &'static str,
        input_schema: fn() -> Json,
        handler: fn(&mut Agent, &Json) -> Result<Json, String>,
    ) -> Self {
        Self {
            name,
            description,
            input_schema,
            handler,
        }
    }

    pub(super) fn call(&self, agent: &mut Agent, params: &Json) -> Result<Json, String> {
        (self.handler)(agent, params)
    }

    pub(super) fn tool_name(&self) -> String {
        self.name.replace('.', "_")
    }

    pub(super) fn describe(&self) -> Json {
        json!({
            "name": self.name,
            "description": self.description,
            "input_schema": (self.input_schema)(),
        })
    }
}

pub(super) static METHODS: &[Method] = &[
    Method::new(
        "engine.describe",
        "查看协议版本、方法、组件与能力目录，以及当前世界的摘要。",
        schemas::no_params,
        |agent, _| agent.describe(),
    ),
    Method::new(
        "engine.diagnostics",
        "列出诊断码及其含义。用 world.validate 检查当前世界。",
        schemas::no_params,
        |agent, _| agent.diagnostics(),
    ),
    Method::new(
        "engine.history",
        "读取当前会话的事务日志和下一事务 ID。日志包括已提交的回滚。",
        schemas::no_params,
        |agent, _| agent.history(),
    ),
    Method::new(
        "engine.replay",
        "从会话基准快照重放事务日志并替换当前世界。成功后清空可回滚事务。",
        schemas::no_params,
        |agent, _| agent.replay(),
    ),
    Method::new(
        "world.query",
        "分页查询当前世界的实体，可按组件和字段过滤，或投影返回指定字段。",
        schemas::query,
        |agent, params| agent.query(params),
    ),
    Method::new(
        "world.get",
        "按持久实体 ID 读取实体及其组件。该 ID 不存在时返回 entity 为 null。",
        schemas::get,
        |agent, params| agent.get(params),
    ),
    Method::new(
        "world.digest",
        "读取当前世界的 revision、实体总数和十六进制 state_hash。",
        schemas::no_params,
        |agent, _| agent.digest(),
    ),
    Method::new(
        "world.stats",
        "统计当前世界的实体总数及各组件的实例数。",
        schemas::no_params,
        |agent, _| agent.stats(),
    ),
    Method::new(
        "world.validate",
        "按组件注册表检查当前世界，返回 valid 与诊断列表。",
        schemas::no_params,
        |agent, _| agent.validate(),
    ),
    Method::new(
        "world.load",
        "读取 XGC 世界 JSON 并重置计划与事务历史。已登记的项目场景还返回十进制 next_entity_id。",
        schemas::world_path,
        Agent::load,
    ),
    Method::new(
        "world.save",
        "保存 XGC 世界 JSON。写回项目场景时须先加载该场景，并核对加载后的磁盘原文。",
        schemas::world_path,
        Agent::save,
    ),
    Method::new(
        "world.snapshot",
        "在响应的 data 字段中返回当前 WorldData，可直接用于 world.restore。",
        schemas::no_params,
        |agent, _| agent.snapshot(),
    ),
    Method::new(
        "world.restore",
        "用 world.snapshot 的 data 替换当前世界，重置计划和事务历史。",
        schemas::restore,
        Agent::restore,
    ),
    Method::new(
        "edit.plan",
        "按序检查编辑命令，返回 plan_id、预览和诊断。用 edit.apply 提交计划。",
        schemas::plan,
        Agent::plan,
    ),
    Method::new(
        "edit.apply",
        "按计划提交编辑，可指定当前基准版本。成功返回 tx_id、revision 和逆命令，同一幂等键可重取回执。",
        schemas::apply,
        Agent::apply,
    ),
    Method::new(
        "edit.revert",
        "按 tx_id 提交回滚。目标须为当前会话可回滚的事务，且世界版本须与其提交版本一致。",
        schemas::revert,
        Agent::revert,
    ),
    Method::new(
        "edit.undo",
        "尝试回滚当前会话最后一笔可回滚的事务，仍会检查世界版本。",
        schemas::no_params,
        |agent, _| agent.undo(),
    ),
    Method::new(
        "script.check",
        "编译内联 Lua 源码，返回语法检查结果和诊断。",
        schemas::script_check,
        |_, params| script_check::check(params),
    ),
    Method::new(
        "world.explain",
        "解释实体的渲染模型抽取，返回选中的模型引用或排除原因。可指定 physics_with 解释当前场景快照的一对刚体形状重叠。",
        schemas::explain,
        |agent, params| explain::explain(agent.world(), params),
    ),
    Method::new(
        "sim.run",
        "从当前世界的独立副本预览固定步，返回逐帧哈希与指定字段。可选择记录以供回放。",
        schemas::sim_run,
        sim_run::run,
    ),
    Method::new(
        "sim.replay",
        "从当前连接的记录起点重新跑固定步，返回首个哈希分歧和该帧采样字段差异。项目资产按当前文件读取。",
        schemas::sim_replay,
        |agent, params| sim_run::replay(agent, params),
    ),
];

pub(super) fn by_name(name: &str) -> Option<&'static Method> {
    METHODS.iter().find(|method| method.name == name)
}

pub(super) fn by_tool_name(name: &str) -> Option<&'static Method> {
    METHODS.iter().find(|method| method.tool_name() == name)
}
