//! 事务日志：记录 plan / applied / inverse，支持 JSON 往返与重放。
//!
//! M1 先做内存日志 + JSON 文本；落盘（fsx 原子写）与跨进程恢复下一刀接上。
//! 重放从 `WorldSnapshot` 开始，逐条重新 plan / apply，并用 revision 与 state hash
//! 校验确定性。

use serde::{Deserialize, Serialize};

use super::{Applied, Command, Plan, World, WorldSnapshot};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TxRecord {
    pub tx_id: u64,
    pub base_revision: u64,
    pub revision: u64,
    pub commands: Vec<Command>,
    pub inverse: Vec<Command>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    next_tx: u64,
    records: Vec<TxRecord>,
}

impl Journal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, plan: &Plan, applied: &Applied) -> u64 {
        let tx_id = self.next_tx;
        self.next_tx += 1;
        self.records.push(TxRecord {
            tx_id,
            base_revision: plan.base_revision,
            revision: applied.revision,
            commands: plan.commands.clone(),
            inverse: applied.inverse.clone(),
        });
        tx_id
    }

    pub fn records(&self) -> &[TxRecord] {
        &self.records
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|err| format!("日志序列化失败：{err}"))
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|err| format!("日志解析失败：{err}"))
    }

    /// 从初始快照重放全部事务；返回最终 state hash。
    ///
    /// 校验点：每条记录必须从它记录的 `base_revision` 开始，应用后必须达到它
    /// 记录的 `revision`。对不上就是日志或命令语义漂移，直接报错。
    pub fn replay(&self, world: &mut World, initial: &WorldSnapshot) -> Result<u64, String> {
        world.restore_snapshot(initial);
        for record in &self.records {
            if world.revision() != record.base_revision {
                return Err(format!(
                    "事务 {} 的 base_revision={}，重放世界里是 {}",
                    record.tx_id,
                    record.base_revision,
                    world.revision()
                ));
            }
            let plan = world.plan(record.commands.clone());
            if !plan.is_valid() {
                return Err(format!(
                    "事务 {} 重放计划无效：{:?}",
                    record.tx_id, plan.diagnostics
                ));
            }
            let applied = world.apply(&plan).map_err(|diagnostics| {
                format!("事务 {} 重放失败：{diagnostics:?}", record.tx_id)
            })?;
            if applied.revision != record.revision {
                return Err(format!(
                    "事务 {} 重放后 revision={}，日志记录 {}",
                    record.tx_id, applied.revision, record.revision
                ));
            }
        }
        Ok(world.state_hash())
    }
}

impl World {
    /// 应用并记账；日志记在内存里，落盘由上层负责。
    pub fn apply_logged(
        &mut self,
        plan: &Plan,
        journal: &mut Journal,
    ) -> Result<Applied, Vec<xgc_base::Diagnostic>> {
        let applied = self.apply(plan)?;
        journal.record(plan, &applied);
        Ok(applied)
    }
}
