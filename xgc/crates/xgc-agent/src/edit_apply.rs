//! 编辑提交和同一会话内的幂等回执。

use serde_json::{json, Value as Json};
use xgc_base::diagnostics::EDIT_REVISION_CONFLICT;
use xgc_world::TxRecord;

use crate::{required_u64, Agent};

pub(super) struct ApplyReceipt {
    plan_id: u64,
    // 日志只追加，直到换世界时与回执索引一起重置，无需在回执中再存一份逆命令。
    record_index: usize,
}

impl Agent {
    pub(super) fn apply(&mut self, params: &Json) -> Result<Json, String> {
        let plan_id = required_u64(params, "plan_id")?;
        let base_revision = params
            .get("base_revision")
            .filter(|value| !value.is_null())
            .map(|_| required_u64(params, "base_revision"))
            .transpose()?;
        let key = match params.get("idempotency_key") {
            None | Some(Json::Null) => None,
            Some(Json::String(key)) => Some(key.as_str()),
            Some(_) => return Err("参数 idempotency_key 必须是字符串".to_string()),
        };
        // 回执属于已经提交的计划。重试时当前世界可以已经推进了多个版本。
        if let Some(receipt) = key.and_then(|key| self.apply_receipts.get(key)) {
            if receipt.plan_id != plan_id {
                return Err(format!(
                    "idempotency_key 已用于 plan_id：{}",
                    receipt.plan_id
                ));
            }
            return Ok(receipt_json(&self.journal.records()[receipt.record_index]));
        }

        // 请求中的版本填错时，客户端可以更正参数后重试原计划。
        let current = self.world.revision();
        if let Some(expected) = base_revision.filter(|&revision| revision != current) {
            return Err(format!(
                "应用失败：{EDIT_REVISION_CONFLICT} 请求 base_revision={expected}，当前 revision={current}"
            ));
        }
        let plan = self
            .plans
            .remove(&plan_id)
            .ok_or_else(|| format!("未知 plan_id：{plan_id}"))?;
        self.claim_plan_ids(&plan)?;
        let record_index = self.journal.len();
        let applied = self
            .world
            .apply_logged(&plan, &mut self.journal)
            .map_err(|diagnostics| format!("应用失败：{diagnostics:?}"))?;
        let record = &self.journal.records()[record_index];
        let response = receipt_json(record);
        self.applied.insert(record.tx_id, applied);
        if let Some(key) = key {
            self.apply_receipts.insert(
                key.to_owned(),
                ApplyReceipt {
                    plan_id,
                    record_index,
                },
            );
        }
        Ok(response)
    }
}

fn receipt_json(record: &TxRecord) -> Json {
    json!({
        "tx_id": record.tx_id,
        "revision": record.revision,
        "inverse": record.inverse,
    })
}
