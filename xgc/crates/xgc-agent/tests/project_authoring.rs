//! 用真实协议请求验证项目编辑与独立世界快照的边界。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value as Json};
use xgc_agent::Agent;
use xgc_base::EntityId;
use xgc_store::{
    load_project, load_world, save_project, save_world, ProjectContent, ProjectManifest,
};
use xgc_world::{EntityData, WorldData};

const FIRST: &str = "scenes/a.json";
const SECOND: &str = "scenes/b.json";

#[test]
fn project_spawn_claims_are_unique_across_connections() {
    let fixture = Project::new();
    let (first_path, second_path) = (fixture.path(FIRST), fixture.path(SECOND));
    let mut first = Agent::new();
    let mut second = Agent::new();
    let first_load = load(&mut first, &first_path);
    let second_load = load(&mut second, &second_path);
    let first_plan = spawn(&mut first, 10);
    let second_plan = spawn(&mut second, 10);
    let params = json!({"plan_id": first_plan, "base_revision": 3, "idempotency_key": "first"});
    let receipt = result(&mut first, "edit.apply", params.clone());
    let prior = result(&mut second, "world.digest", Json::Null);
    let conflict = response(&mut second, "edit.apply", json!({"plan_id": second_plan}));
    assert_eq!(conflict["error"]["code"], -32000, "{conflict}");
    assert!(conflict["error"]["message"]
        .as_str()
        .unwrap()
        .contains("11"));
    assert_eq!(result(&mut second, "world.digest", Json::Null), prior);
    assert_eq!(
        result(&mut second, "engine.history", Json::Null)["count"],
        0
    );
    assert_eq!(first_load["next_entity_id"], "10");
    assert_eq!(second_load["next_entity_id"], "10");
    assert_eq!(fixture.next_id(), 11);
    assert_eq!(result(&mut first, "edit.apply", params), receipt);
    assert_eq!(fixture.next_id(), 11);

    let retry = spawn(&mut second, 11);
    let second_receipt = result(&mut second, "edit.apply", json!({"plan_id": retry}));
    assert_eq!(fixture.next_id(), 12);
    assert_eq!(receipt["revision"], 4);
    assert_eq!(second_receipt["revision"], 4);
    assert_eq!(ids_on_disk(&first_path), [1]);
    assert_eq!(ids_on_disk(&second_path), [2]);
    save(&mut first, &first_path);
    save(&mut second, &second_path);
    assert_eq!(ids_on_disk(&first_path), [1, 10]);
    assert_eq!(ids_on_disk(&second_path), [2, 11]);
    assert_eq!(load(&mut Agent::new(), &first_path)["next_entity_id"], "12");
}

#[test]
fn project_scene_save_checks_the_loaded_document_and_baseline() {
    let fixture = Project::new();
    let (first_path, second_path) = (fixture.path(FIRST), fixture.path(SECOND));
    let mut first = Agent::new();
    let mut stale = Agent::new();
    load(&mut first, &first_path);
    load(&mut stale, &first_path);
    let plan = spawn(&mut first, 10);
    result(&mut first, "edit.apply", json!({"plan_id": plan}));
    save(&mut first, &first_path);
    let saved = std::fs::read(&first_path).unwrap();
    let other = std::fs::read(&second_path).unwrap();
    for (agent, path) in [(&mut stale, &first_path), (&mut first, &second_path)] {
        let error = response(agent, "world.save", json!({"path": path}));
        assert_eq!(error["error"]["code"], -32000, "{error}");
    }
    assert_eq!(std::fs::read(&second_path).unwrap(), other);

    let detached = fixture.path("draft.json");
    let snapshot = result(&mut first, "world.snapshot", Json::Null);
    result(
        &mut first,
        "world.restore",
        json!({"data": snapshot["data"]}),
    );
    let error = response(&mut first, "world.save", json!({"path": first_path}));
    assert_eq!(error["error"]["code"], -32000, "{error}");
    assert_eq!(std::fs::read(&first_path).unwrap(), saved);
    save(&mut first, &detached);
    assert_eq!(ids_on_disk(&detached), [1, 10]);
    assert_eq!(fixture.next_id(), 11);
}

#[test]
fn project_claims_stay_issued_after_undo_and_replay() {
    let fixture = Project::new();
    let path = fixture.path(FIRST);
    let mut agent = Agent::new();
    let initial = load(&mut agent, &path);
    assert_eq!(initial["next_entity_id"], "10");
    let old_plan = spawn(&mut agent, 30);
    let first = spawn(&mut agent, 12);
    result(&mut agent, "edit.apply", json!({"plan_id": first}));
    assert_eq!(fixture.next_id(), 13);
    let rejected = response(&mut agent, "edit.apply", json!({"plan_id": old_plan}));
    assert_eq!(rejected["error"]["code"], -32000);
    assert_eq!(fixture.next_id(), 13);

    let undo = result(&mut agent, "edit.undo", Json::Null);
    assert_eq!(undo["revision"], 5);
    let replay = result(&mut agent, "engine.replay", Json::Null);
    assert_eq!(replay["revision"], 5);
    assert_eq!(replay["entities"], 1);
    let reused = spawn(&mut agent, 12);
    let rejected = response(&mut agent, "edit.apply", json!({"plan_id": reused}));
    assert_eq!(rejected["error"]["code"], -32000);
    assert_eq!(fixture.next_id(), 13);
    save(&mut agent, &path);
    assert_eq!(ids_on_disk(&path), [1]);
}

#[test]
fn project_scene_save_rejects_a_removed_file_or_registration() {
    let fixture = Project::new();
    let path = fixture.path(SECOND);
    let mut author = Agent::new();
    load(&mut author, &path);
    let baseline = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let removed = response(&mut author, "world.save", json!({"path": path}));
    assert_eq!(removed["error"]["code"], -32000);
    assert!(!path.exists());

    std::fs::write(&path, &baseline).unwrap();
    let manifest_path = fixture.path("project.xgc");
    let mut manifest = load_project(&manifest_path).unwrap();
    manifest.scenes.retain(|scene| scene != SECOND);
    save_project(&manifest_path, &manifest).unwrap();
    let plan = spawn(&mut author, 12);
    let unregistered = response(&mut author, "edit.apply", json!({"plan_id": plan}));
    assert_eq!(unregistered["error"]["code"], -32000);
    assert_eq!(fixture.next_id(), 10);
    let unregistered = response(&mut author, "world.save", json!({"path": path}));
    assert_eq!(unregistered["error"]["code"], -32000);
    assert_eq!(std::fs::read(&path).unwrap(), baseline);
}

fn response(agent: &mut Agent, method: &str, params: Json) -> Json {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let line = agent.handle_line(&request.to_string()).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn result(agent: &mut Agent, method: &str, params: Json) -> Json {
    let response = response(agent, method, params);
    assert!(response.get("error").is_none(), "{method}: {response}");
    response["result"].clone()
}

fn load(agent: &mut Agent, path: &Path) -> Json {
    result(agent, "world.load", json!({"path": path}))
}

fn save(agent: &mut Agent, path: &Path) -> Json {
    result(agent, "world.save", json!({"path": path}))
}

fn spawn(agent: &mut Agent, id: u64) -> Json {
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

fn ids_on_disk(path: &Path) -> Vec<u64> {
    load_world(path)
        .unwrap()
        .entities
        .iter()
        .map(|data| data.id.raw())
        .collect()
}

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let fixture = Self(std::env::temp_dir().join(format!(
            "xgc-agent-project-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        for (index, relative) in [(1, FIRST), (2, SECOND)] {
            save_world(
                &fixture.path(relative),
                &WorldData {
                    revision: 3,
                    next_id: 10,
                    environment: Default::default(),
                    entities: vec![EntityData::new(EntityId::from_raw(index))],
                },
            )
            .unwrap();
        }
        let manifest = ProjectManifest {
            format_version: xgc_store::PROJECT_FORMAT_VERSION,
            next_entity_id: 10,
            name: "Agent 两场景事务".into(),
            entry_scene: FIRST.into(),
            scenes: vec![FIRST.into(), SECOND.into()],
            content: ProjectContent {
                models: "Resources".into(),
                textures: "Resources".into(),
                audio: "Resources".into(),
                shaders: None,
                prefabs: "Prefabs".into(),
                scripts: "Scripts".into(),
            },
            search_paths: Vec::new(),
            input: None,
            render: Default::default(),
            quality: Default::default(),
            layers: Default::default(),
        };
        save_project(&fixture.path("project.xgc"), &manifest).unwrap();
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    fn next_id(&self) -> u64 {
        load_project(&self.path("project.xgc"))
            .unwrap()
            .next_entity_id
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
