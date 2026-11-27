use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use xgc_base::EntityId;
use xgc_store::project::PrefabSceneChange;
use xgc_store::{
    save_prefab, save_project, save_world, ProjectContent, ProjectManifest, ProjectSession,
    PROJECT_FORMAT_VERSION,
};
use xgc_world::{EntityData, WorldData};

const FIRST: &str = "scenes/first.json";
const SECOND: &str = "scenes/second.json";
const PREFAB: &str = "prefabs/widget.prefab.json";
const PENDING: &str = ".xgc-prefab-apply.pending";

struct Fixture {
    root: PathBuf,
    session: ProjectSession,
}

impl Fixture {
    fn new(scenes: &[&str]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let root = std::env::temp_dir().join(format!(
            "xgc-prefab-apply-{}-{}-{}",
            std::process::id(),
            time.as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        save_world(&root.join(FIRST), &world(1, &[10])).unwrap();
        save_world(&root.join(SECOND), &world(1, &[20])).unwrap();
        save_prefab(&root.join(PREFAB), &world(1, &[1, 2])).unwrap();
        save_project(
            &root.join("project.xgc"),
            &ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                next_entity_id: 100,
                name: "Prefab Apply".into(),
                entry_scene: FIRST.into(),
                scenes: scenes.iter().map(|scene| (*scene).into()).collect(),
                content: ProjectContent {
                    models: "models".into(),
                    textures: "textures".into(),
                    audio: "audio".into(),
                    shaders: None,
                    prefabs: "prefabs".into(),
                    scripts: "scripts".into(),
                },
                search_paths: Vec::new(),
                input: None,
                render: Default::default(),
                quality: Default::default(),
                layers: Default::default(),
            },
        )
        .unwrap();
        let session = ProjectSession::open(&root).unwrap();
        Self { root, session }
    }

    fn read(&self, relative: &str) -> Vec<u8> {
        fs::read(self.root.join(relative)).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn world(revision: u64, ids: &[u64]) -> WorldData {
    WorldData {
        revision,
        next_id: 100,
        entities: ids
            .iter()
            .map(|&id| EntityData::new(EntityId::from_raw(id)))
            .collect(),
        ..WorldData::default()
    }
}

fn guard<'a>(scene: &'a str, path: &'a Path, original: &'a [u8]) -> PrefabSceneChange<'a> {
    PrefabSceneChange {
        scene,
        path,
        original,
        replacement: None,
    }
}

#[test]
fn structural_apply_checks_latest_scene_coverage_and_preserves_exact_guard_bytes() {
    let fixture = Fixture::new(&[FIRST]);
    let mut manifest = fixture.session.manifest().clone();
    manifest.scenes.push(SECOND.into());
    save_project(&fixture.root.join("project.xgc"), &manifest).unwrap();
    let first = fixture.root.join(FIRST).canonicalize().unwrap();
    let second = fixture.root.join(SECOND).canonicalize().unwrap();
    let prefab = fixture.root.join(PREFAB);
    let before_prefab = fixture.read(PREFAB);
    let before_first = fixture.read(FIRST);
    let stale_second = fixture.read(SECOND);
    let mut before_second = stale_second.clone();
    before_second.extend_from_slice(b"\r\n");
    fs::write(&second, &before_second).unwrap();
    let replacement = world(2, &[10]);
    let changed_first = PrefabSceneChange {
        replacement: Some(&replacement),
        ..guard(FIRST, &first, &before_first)
    };
    let asset = world(2, &[1]);
    let error = fixture
        .session
        .apply_prefab_if_unchanged(
            &prefab,
            &asset,
            &before_prefab,
            std::slice::from_ref(&changed_first),
            || Ok(()),
        )
        .unwrap_err();
    assert!(error.to_string().contains("所有登记场景"), "{error}");
    let repeated = [
        guard(FIRST, &first, &before_first),
        guard("./scenes/first.json", &first, &before_first),
    ];
    let error = fixture
        .session
        .apply_prefab_if_unchanged(&prefab, &asset, &before_prefab, &repeated, || Ok(()))
        .unwrap_err();
    assert!(error.to_string().contains("目标路径重复"), "{error}");
    let mut changes = [guard(SECOND, &second, &stale_second), changed_first];
    let error = fixture
        .session
        .apply_prefab_if_unchanged(&prefab, &asset, &before_prefab, &changes, || Ok(()))
        .unwrap_err();
    assert!(error.to_string().contains("已被外部修改"), "{error}");
    assert_eq!(fixture.read(PREFAB), before_prefab);
    assert_eq!(fixture.read(FIRST), before_first);
    assert_eq!(fixture.read(SECOND), before_second);
    assert!(!fixture.root.join(PENDING).exists());

    changes[0].original = &before_second;
    let baselines = fixture
        .session
        .apply_prefab_if_unchanged(&prefab, &asset, &before_prefab, &changes, || Ok(()))
        .unwrap();
    assert_eq!(baselines, [before_second, fixture.read(FIRST)]);
    assert_eq!(xgc_store::load_world(&first).unwrap(), replacement);
    assert_eq!(xgc_store::load_prefab(&prefab).unwrap(), asset);
    assert_eq!(baselines[0], fixture.read(SECOND));
    assert!(!fixture.root.join(PENDING).exists());
}

fn with_revision(bytes: &[u8], revision: u64) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value["payload"]["revision"] = revision.into();
    serde_json::to_vec(&value).unwrap()
}

fn journal_entry(relative: &str, original: &[u8], applied: &[u8]) -> serde_json::Value {
    json!({
        "relative": relative,
        "original": std::str::from_utf8(original).unwrap(),
        "applied": std::str::from_utf8(applied).unwrap(),
    })
}

#[test]
fn interrupted_apply_preflights_recovery_and_next_scene_write_restores_exact_originals() {
    let fixture = Fixture::new(&[FIRST, SECOND]);
    let before_prefab = fixture.read(PREFAB);
    let before_first = fixture.read(FIRST);
    let before_second = fixture.read(SECOND);
    let applied_prefab = with_revision(&before_prefab, 2);
    let applied_first = with_revision(&before_first, 2);
    let applied_second = with_revision(&before_second, 2);
    let external_second = with_revision(&before_second, 3);
    let current: serde_json::Value = serde_json::from_slice(&before_prefab).unwrap();
    let pending = json!({
        "format_version": current["format_version"],
        "kind": "prefab_apply",
        "payload": {
            "prefab": journal_entry(PREFAB, &before_prefab, &applied_prefab),
            "scenes": [
                journal_entry(FIRST, &before_first, &applied_first),
                journal_entry(SECOND, &before_second, &applied_second),
            ],
        },
    });
    let pending = serde_json::to_vec(&pending).unwrap();
    let pending_path = fixture.root.join(PENDING);
    let prefab = fixture.root.join(PREFAB);
    let first = fixture.root.join(FIRST).canonicalize().unwrap();
    let second = fixture.root.join(SECOND);
    // 模拟已发布日志、写过资产和第一关，第二关随后被外部编辑。
    fs::write(&pending_path, &pending).unwrap();
    fs::write(prefab, &applied_prefab).unwrap();
    fs::write(&first, &applied_first).unwrap();
    fs::write(&second, &external_second).unwrap();
    let reader = ProjectSession::open(&fixture.root).unwrap();
    assert_eq!(reader.load_scene(FIRST).unwrap().revision, 2);
    assert_eq!(fixture.read(PENDING), pending);
    let error = reader.recover_prefab_apply().unwrap_err();
    assert!(error.to_string().contains(SECOND), "{error}");
    assert_eq!(fixture.read(PENDING), pending);
    assert_eq!(fixture.read(PREFAB), applied_prefab);
    assert_eq!(fixture.read(FIRST), applied_first);
    assert_eq!(fixture.read(SECOND), external_second);

    fs::write(second, before_second).unwrap();
    let saved = reader
        .save_scene_if_unchanged(FIRST, &first, &world(4, &[10]), &before_first)
        .unwrap();
    assert_eq!(fixture.read(PREFAB), before_prefab);
    assert_eq!(fixture.read(FIRST), saved);
    assert_eq!(reader.load_scene(FIRST).unwrap().revision, 4);
    assert!(!pending_path.exists());
}

#[test]
fn value_apply_uses_one_guard_and_empty_recovery_creates_no_lock() {
    let fixture = Fixture::new(&[FIRST, SECOND]);
    let lock = fixture.root.join(".xgc-project.lock");
    fs::remove_file(&lock).unwrap();
    assert!(!fixture.session.recover_prefab_apply().unwrap());
    assert!(!lock.exists());

    let original = fixture.read(PREFAB);
    let first = fixture.root.join(FIRST).canonicalize().unwrap();
    let before_first = fixture.read(FIRST);
    let before_second = fixture.read(SECOND);
    let asset = world(2, &[1, 2]);
    let prefab = fixture.root.join(PREFAB);
    let result = fixture
        .session
        .apply_prefab_if_unchanged(
            &prefab,
            &asset,
            &original,
            &[guard(FIRST, &first, &before_first)],
            || panic!("仅写回组件值无需检查删除的来源"),
        )
        .unwrap();
    assert_eq!(result, [before_first]);
    assert_eq!(fixture.read(FIRST), result[0]);
    assert_eq!(fixture.read(SECOND), before_second);
    assert_eq!(xgc_store::load_prefab(&prefab).unwrap(), asset);
    assert!(!fixture.root.join(PENDING).exists());
}

#[cfg(windows)]
#[test]
fn scene_replacement_failure_rolls_back_prior_asset_write() {
    use std::os::windows::fs::OpenOptionsExt;

    let fixture = Fixture::new(&[FIRST]);
    let first = fixture.root.join(FIRST).canonicalize().unwrap();
    let prefab = fixture.root.join(PREFAB);
    let before_prefab = fixture.read(PREFAB);
    let before_first = fixture.read(FIRST);
    // 允许读取基准，但没有 FILE_SHARE_DELETE，替换这关时 rename 会失败。
    let open_scene = fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(&first)
        .unwrap();
    let replacement = world(2, &[10]);
    let change = PrefabSceneChange {
        replacement: Some(&replacement),
        ..guard(FIRST, &first, &before_first)
    };
    let error = fixture
        .session
        .apply_prefab_if_unchanged(&prefab, &world(2, &[1]), &before_prefab, &[change], || {
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(error, xgc_store::StoreError::Io(_)), "{error}");
    assert_eq!(fixture.read(PREFAB), before_prefab);
    assert_eq!(fixture.read(FIRST), before_first);
    assert!(!fixture.root.join(PENDING).exists());
    drop(open_scene);
}
