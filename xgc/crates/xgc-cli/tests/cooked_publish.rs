use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use xgc_store::save_world;
use xgc_world::{Script, Transform, World};

const MARKER: &str = ".xgc-pack-transaction";
const PREVIOUS: &str = ".xgc-pack-previous-cooked";
const LOCK: &str = ".xgc-pack.lock";

#[test]
fn a_failed_repack_keeps_the_previous_package_runnable() {
    let fixture = Fixture::new();
    let scene = fixture.scene();
    let source = fixture.source();
    let cooked = fixture.cooked();
    fs::write(source.join("Scripts/a.lua"), first_script(1)).unwrap();
    save_scene(&scene, &["Scripts/a.lua"]);
    success(fixture.pack(&cooked));
    let before = success(cooked_run(&cooked));
    let bytes = package_bytes(&cooked);

    // 先复制已更新的脚本，再遇到和二进制世界产物冲突的目录。
    fs::write(source.join("Scripts/a.lua"), first_script(2)).unwrap();
    fs::create_dir(source.join("world.xgcb")).unwrap();
    fs::write(source.join("world.xgcb/b.lua"), first_script(3)).unwrap();
    save_scene(&scene, &["Scripts/a.lua", "world.xgcb/b.lua"]);
    let failed = fixture.pack(&cooked);
    assert_eq!(failed.status.code(), Some(1), "{failed:?}");
    assert!(String::from_utf8_lossy(&failed.stderr).contains("写入二进制 cooked world 失败"));
    assert_eq!(success(cooked_run(&cooked)), before);
    assert_eq!(package_bytes(&cooked), bytes);
}

#[test]
fn repeats_publish_and_serializes_concurrent_releases() {
    let fixture = Fixture::new();
    let build = fixture.source().join("Build");
    let cooked = build.join("cooked");
    fixture.set_source(1);
    assert_eq!(
        success(fixture.pack(&cooked)).trim_end(),
        format!("pack: 3 个文件（含 world.json） -> {}", cooked.display())
    );
    let bytes = package_bytes(&cooked);
    let before = success(cooked_run(&cooked));
    assert!(build.join(LOCK).is_file());

    let spawn = || {
        fixture
            .pack_command(&cooked)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = spawn();
    let second = spawn();
    success(first.wait_with_output().unwrap());
    success(second.wait_with_output().unwrap());
    assert_eq!(package_bytes(&cooked), bytes);
    assert_eq!(success(cooked_run(&cooked)), before);
    check_transaction_is_clean(&cooked);

    fixture.set_source(2);
    success(fixture.pack(&cooked));
    assert_ne!(success(cooked_run(&cooked)), before);
    check_transaction_is_clean(&cooked);
}

#[test]
fn recovers_reserved_boundaries_before_a_failed_new_pack() {
    let fixture = Fixture::new();
    let cooked = fixture.cooked();
    let previous = fixture.0.join(PREVIOUS);
    let staged_new = fixture.0.join("next");
    let unrelated_staging = fixture.0.join(".xgc-pack-staging-7-0");
    fs::create_dir(&unrelated_staging).unwrap();
    fs::write(unrelated_staging.join("keep.txt"), b"unowned").unwrap();
    let stranded_staging = fixture.0.join(".xgc-pack-staging-8-0");
    fs::create_dir_all(stranded_staging.join(MARKER)).unwrap();
    fs::write(stranded_staging.join("package-fragment"), b"partial").unwrap();

    fixture.set_source(1);
    success(fixture.pack(&cooked));
    let old = success(cooked_run(&cooked));
    let (source, scene) = (fixture.source(), fixture.scene());
    fs::write(source.join("Scripts/a.lua"), first_script(2)).unwrap();
    success(fixture.pack(&staged_new));
    let new = success(cooked_run(&staged_new));
    assert_ne!(old, new);
    fs::write(&scene, "{broken:").unwrap();

    fs::create_dir(cooked.join(MARKER)).unwrap();
    assert_eq!(fixture.pack(&cooked).status.code(), Some(2));
    assert_eq!(success(cooked_run(&cooked)), old);
    assert!(!cooked.join(MARKER).exists());

    fs::create_dir(cooked.join(MARKER)).unwrap();
    fs::rename(&cooked, &previous).unwrap();
    fs::remove_file(&scene).unwrap();
    let missing = fixture.pack(&cooked);
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("读取世界失败"));
    assert_eq!(success(cooked_run(&cooked)), old);
    assert!(!previous.exists());
    assert!(!cooked.join(MARKER).exists());

    fs::create_dir(cooked.join(MARKER)).unwrap();
    fs::rename(&cooked, &previous).unwrap();
    fs::rename(&staged_new, &cooked).unwrap();
    assert_eq!(fixture.pack(&cooked).status.code(), Some(2));
    assert_eq!(success(cooked_run(&cooked)), new);
    assert!(!previous.exists());
    assert_eq!(
        fs::read(unrelated_staging.join("keep.txt")).unwrap(),
        b"unowned"
    );
    assert_eq!(
        fs::read(stranded_staging.join("package-fragment")).unwrap(),
        b"partial"
    );
}

#[test]
fn refuses_to_replace_source_or_unowned_content() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let cooked = fixture.cooked();
    fixture.set_source(1);
    success(fixture.pack(&cooked));
    let package = package_bytes(&cooked);
    let source_bytes = fs::read(fixture.scene()).unwrap();
    for out in [&source, &fixture.0, &source.join("Scripts")] {
        assert_eq!(fixture.pack(out).status.code(), Some(1));
    }
    assert_eq!(fs::read(fixture.scene()).unwrap(), source_bytes);
    assert_eq!(
        fs::read_to_string(source.join("Scripts/a.lua")).unwrap(),
        first_script(1)
    );

    let previous = fixture.0.join(PREVIOUS);
    fs::create_dir(&previous).unwrap();
    fs::write(previous.join("keep.txt"), b"untouched").unwrap();
    assert_eq!(fixture.pack(&cooked).status.code(), Some(1));
    assert_eq!(fs::read(previous.join("keep.txt")).unwrap(), b"untouched");
    assert_eq!(package_bytes(&cooked), package);

    fs::create_dir(previous.join(MARKER)).unwrap();
    fs::write(
        previous.join(MARKER).join("keep.txt"),
        b"not an empty marker",
    )
    .unwrap();
    assert_eq!(fixture.pack(&cooked).status.code(), Some(1));
    assert_eq!(
        fs::read(previous.join(MARKER).join("keep.txt")).unwrap(),
        b"not an empty marker"
    );

    let other = fixture.0.join("other-package");
    success(fixture.pack(&other));
    fs::write(other.join("notes.txt"), b"keep in place").unwrap();
    assert_eq!(fixture.pack(&other).status.code(), Some(1));
    assert_eq!(fs::read(other.join("notes.txt")).unwrap(), b"keep in place");
    assert_eq!(package_bytes(&other), package);

    let world_inside_output = other.join("user-scene.json");
    save_scene(&world_inside_output, &["Scripts/a.lua"]);
    let rejected = xgc()
        .arg("pack")
        .args([&world_inside_output, &source, &other])
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("发布路径覆盖源项目或世界"));
    assert!(world_inside_output.is_file());
}

fn first_script(x: u32) -> String {
    format!("function update(ctx) set_pos(ctx, {x}, 0, 0) end")
}

fn save_scene(path: &Path, scripts: &[&str]) {
    let mut world = World::new();
    for path in scripts {
        world.spawn((
            Script {
                paths: vec![(*path).into()],
            },
            Transform::default(),
        ));
    }
    save_world(path, &world.world_data()).unwrap();
}

fn package_bytes(path: &Path) -> Vec<Vec<u8>> {
    ["cook.json", "world.json", "world.xgcb", "Scripts/a.lua"]
        .map(|name| fs::read(path.join(name)).unwrap())
        .into()
}

fn xgc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xgc"))
}

fn cooked_run(path: &Path) -> std::process::Output {
    xgc().arg("run-cooked").arg(path).arg("1").output().unwrap()
}

fn success(output: std::process::Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "xgc-cooked-publish-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let fixture = Self(dir);
        fs::create_dir_all(fixture.source().join("Scripts")).unwrap();
        fixture
    }

    fn source(&self) -> PathBuf {
        self.0.join("source")
    }

    fn scene(&self) -> PathBuf {
        self.source().join("scene.json")
    }

    fn cooked(&self) -> PathBuf {
        self.0.join("cooked")
    }

    fn pack_command(&self, out: &Path) -> Command {
        let mut command = xgc();
        command
            .arg("pack")
            .args([self.scene().as_path(), self.source().as_path(), out]);
        command
    }

    fn pack(&self, out: &Path) -> std::process::Output {
        self.pack_command(out).output().unwrap()
    }

    fn set_source(&self, position: u32) {
        fs::write(self.source().join("Scripts/a.lua"), first_script(position)).unwrap();
        save_scene(&self.scene(), &["Scripts/a.lua"]);
    }
}

fn check_transaction_is_clean(cooked: &Path) {
    assert!(!cooked.join(MARKER).exists());
    let parent = cooked.parent().unwrap();
    assert!(!parent.join(PREVIOUS).exists());
    assert!(fs::read_dir(parent).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".xgc-pack-staging-")));
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
