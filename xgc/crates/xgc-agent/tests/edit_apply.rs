//! 走 JSON-RPC 验证编辑回执和计划的生命周期。

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value as Json};
use xgc_agent::Agent;
use xgc_base::diagnostics::EDIT_REVISION_CONFLICT;

fn call(agent: &mut Agent, method: &str, params: Json) -> Json {
    static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let response: Json =
        serde_json::from_str(&agent.handle_line(&request.to_string()).unwrap()).unwrap();
    assert_eq!(response["id"], id);
    response
}

fn result(agent: &mut Agent, method: &str, params: Json) -> Json {
    let response = call(agent, method, params);
    assert!(response.get("error").is_none(), "{method}: {response}");
    response["result"].clone()
}

fn error(agent: &mut Agent, method: &str, params: Json) -> String {
    let response = call(agent, method, params);
    assert_eq!(response["error"]["code"], -32000, "{method}: {response}");
    response["error"]["message"].as_str().unwrap().to_owned()
}

fn spawn_plan(agent: &mut Agent, id: u64) -> Json {
    let plan = result(
        agent,
        "edit.plan",
        json!({"commands": [{"Spawn": {"data": {
            "id": id.to_string(), "revision": 0, "components": []
        }}}]}),
    );
    assert_eq!(plan["valid"], true, "{plan}");
    plan["plan_id"].clone()
}

#[test]
fn retries_return_the_original_receipt_across_later_edits() {
    let mut agent = Agent::new();
    let first = spawn_plan(&mut agent, 1);
    let receipt = result(
        &mut agent,
        "edit.apply",
        json!({
            "plan_id": first, "idempotency_key": "spawn-1", "base_revision": 0
        }),
    );
    assert_eq!(
        receipt,
        json!({"tx_id": 0, "revision": 1, "inverse": [{"Despawn": {"entity": "1"}}]})
    );
    let later = spawn_plan(&mut agent, 2);
    result(&mut agent, "edit.apply", json!({"plan_id": later}));
    let digest = result(&mut agent, "world.digest", Json::Null);
    let history = result(&mut agent, "engine.history", Json::Null);
    assert_eq!(history["count"], 2);
    assert_eq!(digest["entities"], 2);

    // 基准版本只约束第一次提交。已有回执可用原版本、现版本或省略版本重取。
    let mut retry = json!({"plan_id": first.to_string(), "idempotency_key": "spawn-1"});
    assert_eq!(result(&mut agent, "edit.apply", retry.clone()), receipt);
    for revision in [json!("0"), digest["revision"].clone()] {
        retry["base_revision"] = revision;
        assert_eq!(result(&mut agent, "edit.apply", retry.clone()), receipt);
    }
    assert_eq!(result(&mut agent, "world.digest", Json::Null), digest);
    assert_eq!(result(&mut agent, "engine.history", Json::Null), history);
    assert!(error(&mut agent, "edit.apply", json!({"plan_id": first})).contains("未知 plan_id"));
    assert!(error(
        &mut agent,
        "edit.apply",
        json!({"plan_id": first, "idempotency_key": "other"})
    )
    .contains("未知 plan_id"));
}

#[test]
fn a_committed_key_cannot_consume_another_plan() {
    let mut agent = Agent::new();
    let first = spawn_plan(&mut agent, 1);
    let receipt = result(
        &mut agent,
        "edit.apply",
        json!({"plan_id": first, "idempotency_key": "reserved"}),
    );
    let next = spawn_plan(&mut agent, 2);
    let failed = error(
        &mut agent,
        "edit.apply",
        json!({"plan_id": next, "idempotency_key": "reserved"}),
    );
    assert!(failed.contains("idempotency_key"), "{failed}");
    assert_eq!(result(&mut agent, "engine.history", Json::Null)["count"], 1);
    assert_eq!(
        result(
            &mut agent,
            "edit.apply",
            json!({"plan_id": first, "idempotency_key": "reserved"})
        ),
        receipt
    );
    assert_eq!(
        result(
            &mut agent,
            "edit.apply",
            json!({"plan_id": next, "idempotency_key": "separate"})
        )["revision"],
        2
    );
}

#[test]
fn a_bad_requested_revision_leaves_the_plan_available_for_retry() {
    let mut agent = Agent::new();
    let first = spawn_plan(&mut agent, 1);
    let unchanged = result(&mut agent, "world.digest", Json::Null);
    let mut params =
        json!({"plan_id": first, "idempotency_key": "correctable", "base_revision": 7});
    let conflict = error(&mut agent, "edit.apply", params.clone());
    assert!(conflict.contains(EDIT_REVISION_CONFLICT), "{conflict}");
    assert!(conflict.contains("当前 revision=0"), "{conflict}");
    assert_eq!(result(&mut agent, "world.digest", Json::Null), unchanged);
    assert_eq!(result(&mut agent, "engine.history", Json::Null)["count"], 0);

    params["base_revision"] = json!(-1);
    assert!(error(&mut agent, "edit.apply", params.clone()).contains("base_revision"));
    params["base_revision"] = json!("0");
    params["idempotency_key"] = json!(8);
    assert!(error(&mut agent, "edit.apply", params.clone()).contains("idempotency_key"));
    params["idempotency_key"] = json!("correctable");
    let receipt = result(&mut agent, "edit.apply", params.clone());
    assert_eq!(receipt["revision"], 1);
    assert_eq!(result(&mut agent, "edit.apply", params), receipt);
}

#[test]
fn an_invalid_or_stale_plan_does_not_claim_its_key() {
    let mut agent = Agent::new();
    let invalid = result(
        &mut agent,
        "edit.plan",
        json!({"commands": [{"Despawn": {"entity": "88"}}]}),
    );
    assert_eq!(invalid["valid"], false);
    assert!(error(
        &mut agent,
        "edit.apply",
        json!({"plan_id": invalid["plan_id"], "idempotency_key": "retry"})
    )
    .contains("EDIT.UNKNOWN_ENTITY"));

    let stale = spawn_plan(&mut agent, 2);
    let next = spawn_plan(&mut agent, 1);
    result(&mut agent, "edit.apply", json!({"plan_id": next}));
    let conflict = error(
        &mut agent,
        "edit.apply",
        json!({
            "plan_id": stale, "idempotency_key": "retry", "base_revision": 1
        }),
    );
    assert!(conflict.contains(EDIT_REVISION_CONFLICT), "{conflict}");
    // 计划内建的版本检查仍拒绝已经过时的计划，重规划后可继续用同一个键。
    let fresh = spawn_plan(&mut agent, 2);
    let receipt = result(
        &mut agent,
        "edit.apply",
        json!({
            "plan_id": fresh, "idempotency_key": "retry", "base_revision": 1
        }),
    );
    assert_eq!(receipt["tx_id"], 1);
    assert_eq!(receipt["revision"], 2);
}

#[test]
fn successful_world_replacement_clears_keys_and_failure_keeps_them() {
    let path = std::env::temp_dir().join(format!("xgc-edit-receipt-{}.json", std::process::id()));
    let mut agent = Agent::new();
    let empty = result(&mut agent, "world.snapshot", Json::Null);
    let path_params = json!({"path": path});
    result(&mut agent, "world.save", path_params.clone());
    struct SavedWorld(std::path::PathBuf);
    impl Drop for SavedWorld {
        fn drop(&mut self) {
            std::fs::remove_file(&self.0).ok();
        }
    }
    let _saved_world = SavedWorld(path.clone());

    for (method, replace_params, bad_params) in [
        (
            "world.restore",
            json!({"data": empty["data"]}),
            json!({"data": "无效快照"}),
        ),
        (
            "world.load",
            path_params,
            json!({"path": path.with_extension("missing")}),
        ),
    ] {
        let plan = spawn_plan(&mut agent, 1);
        let params = json!({"plan_id": plan, "idempotency_key": "session-key"});
        let receipt = result(&mut agent, "edit.apply", params.clone());
        assert_eq!(receipt["tx_id"], 0);
        let _ = error(&mut agent, method, bad_params);
        assert_eq!(result(&mut agent, "edit.apply", params.clone()), receipt);
        result(&mut agent, "engine.replay", Json::Null);
        assert_eq!(result(&mut agent, "edit.apply", params.clone()), receipt);

        assert_eq!(
            result(&mut agent, method, replace_params),
            json!({"revision": 0, "entities": 0})
        );
        assert!(error(&mut agent, "edit.apply", params).contains("未知 plan_id"));
        assert_eq!(result(&mut agent, "engine.history", Json::Null)["count"], 0);
    }
    let new = spawn_plan(&mut agent, 7);
    assert_eq!(
        result(
            &mut agent,
            "edit.apply",
            json!({"plan_id": new, "idempotency_key": "session-key"})
        )["revision"],
        1
    );
}

#[test]
fn empty_plans_keep_their_receipts_and_unkeyed_applies_are_single_use() {
    let mut agent = Agent::new();
    let empty = result(&mut agent, "edit.plan", json!({"commands": []}));
    let params =
        json!({"plan_id": empty["plan_id"], "idempotency_key": "empty", "base_revision": null});
    let receipt = result(&mut agent, "edit.apply", params.clone());
    assert_eq!(receipt, json!({"tx_id": 0, "revision": 0, "inverse": []}));
    let legacy_plan = spawn_plan(&mut agent, 1);
    let legacy = json!({"plan_id": format!("+{legacy_plan}")});
    assert_eq!(result(&mut agent, "edit.apply", legacy.clone())["tx_id"], 1);
    assert!(error(&mut agent, "edit.apply", legacy).contains("未知 plan_id"));
    assert_eq!(result(&mut agent, "edit.apply", params), receipt);
    assert_eq!(result(&mut agent, "engine.history", Json::Null)["count"], 2);
}
