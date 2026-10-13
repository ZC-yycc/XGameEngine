//! 端到端：dlopen 示例插件 → 版本握手 → 注册组件 → `World` 驱动四钩子。
//!
//! 对应双轨计划 P0 验收第 2、6 条。示例插件是 `examples/spinner-plugin`
//! （cdylib），它的两个组件由**跨动态库边界**的构造/分发函数指针驱动 ——
//! 这条路径没有单元测试能替代：`run_entry` 的单测走的是同进程内调用，
//! 覆盖不了"真实 dlsym + 两个编译产物互操作"。
//!
//! # 计数器为什么用导出符号读
//!
//! 插件组件的内部状态（本例的钩子计数）活在动态库里，宿主看不见其类型。
//! 最朴素的观测通道是 `#[no_mangle] pub static` 导出 + libloading 按符号名
//! 读 —— 与引擎从 C ABI 表拿能力是同一种边界纪律。
//!
//! # dll 在哪
//!
//! `spinner-plugin` 是 workspace 成员，`cargo test --workspace` 会顺带产出
//! cdylib；单独跑 `cargo test -p xgame` 时未必。所以先探 `target/*`，
//! 找不到就触发一次嵌套 `cargo build -p spinner-plugin`（带超时，防止与
//! 外层 cargo 抢目标目录锁时无限挂起）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use xgame::plugin::{load_plugin, LoadedPlugin};
use xgame::prelude::*;

/// 探测示例插件的 cdylib 路径；找不到就现场构建一次。
fn example_plugin_dll() -> PathBuf {
    let lib_name = format!(
        "{}spinner_plugin{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let mut candidates: Vec<PathBuf> = Vec::new();
    // 1) 当前测试二进制的 target 子目录（CARGO_TARGET_TMPDIR = target/<profile>/tmp）。
    if let Ok(tmp) = std::env::var("CARGO_TARGET_TMPDIR") {
        candidates.push(PathBuf::from(tmp).parent().unwrap().join(&lib_name));
    }
    // 2) 仓库根的 target/{当前 profile, 另一 profile}（`env!` 保证路径与本测试同源）。
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap().to_owned();
    let (first, second) = if cfg!(debug_assertions) {
        ("debug", "release")
    } else {
        ("release", "debug")
    };
    candidates.push(repo_root.join("target").join(first).join(&lib_name));
    candidates.push(repo_root.join("target").join(second).join(&lib_name));

    if let Some(found) = candidates.iter().find(|p| p.exists()) {
        return found.clone();
    }

    build_example_plugin(&repo_root);
    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| {
            panic!(
                "构建后仍未找到示例插件动态库；请手动运行 `cargo build -p spinner-plugin` \
                 并检查 target 目录布局"
            )
        })
}

/// 嵌套跑一次 `cargo build -p spinner-plugin`（在工作线程里等结果，
/// 超时即判"与外层 cargo 死锁"而不是无限挂起测试）。
fn build_example_plugin(repo_root: &std::path::Path) {
    let cargo = cargo_exe();
    let mut cmd = std::process::Command::new(&cargo);
    cmd.args(["build", "-p", "spinner-plugin"])
        .current_dir(repo_root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if !cfg!(debug_assertions) {
        cmd.arg("--release");
    }
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("无法启动 {cargo:?} 构建示例插件：{e}"));

    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    panic!("`cargo build -p spinner-plugin` 失败（{status}），示例插件无法构建");
                }
                return;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    panic!(
                        "嵌套 `cargo build -p spinner-plugin` 超时 —— 多半是与外层 cargo \
                         抢目标目录锁；请手动构建后再跑本测试"
                    );
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => panic!("等待示例插件构建失败：{e}"),
        }
    }
}

/// 找 cargo：优先环境变量，其次 rustup 的默认安装位置（本机有"cargo 不在
/// PATH 上"的情况，见 tools/build-release.ps1 的 Resolve-Cargo）。
fn cargo_exe() -> PathBuf {
    if let Some(c) = std::env::var_os("CARGO") {
        return PathBuf::from(c);
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let p = PathBuf::from(home).join(".cargo").join("bin").join("cargo.exe");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("cargo")
}

/// 从插件库里按符号名读一个 `AtomicU32` 计数器。
///
/// SAFETY 说明：符号必须是插件侧 `#[no_mangle] pub static ...: AtomicU32`；
/// 引用的生命周期被压在 `&plugin` 上 —— 库句柄活着，静态就活着。
fn counter(plugin: &LoadedPlugin, name: &[u8]) -> u32 {
    let sym: libloading::Symbol<'_, *const AtomicU32> = unsafe { plugin.get(name) }
        .unwrap_or_else(|e| panic!("插件缺少计数符号 {:?}：{e}", String::from_utf8_lossy(name)));
    unsafe { &**sym }.load(Ordering::Relaxed)
}

/// 全链路：加载 → 注册 → World 驱动 → 四钩子都有可观测证据。
#[test]
fn example_plugin_loads_and_drives_entities() {
    let dll = example_plugin_dll();
    let mut registry = BehaviorRegistry::new();
    let plugin = load_plugin(&dll, &mut registry).unwrap_or_else(|e| panic!("{e}"));

    // 握手通过后，插件声明的两个组件出现在宿主注册表里。
    assert!(registry.contains("Spinner"), "插件的 Spinner 应当已注册");
    assert!(registry.contains("Reaper"), "插件的 Reaper 应当已注册");

    // 场景：host 挂 Spinner + Reaper；target 只挂 Spinner，会被 Reaper 销毁。
    let mut scene = Scene::new("plugin");
    let mut host = Entity::new("host");
    host.behaviors.push("Spinner".to_owned());
    host.behaviors.push("Reaper".to_owned());
    scene.add_entity(host);
    let mut target = Entity::new("target");
    target.behaviors.push("Spinner".to_owned());
    scene.add_entity(target);

    let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
        .with_behavior_registry(registry);

    for _ in 0..5 {
        world.update(1.0 / 60.0);
    }

    // 组件真的驱动了实体：Spinner 的 update 把 host 绕 Y 转了起来。
    let host = world.scene().find_by_name("host").expect("host 应当还在");
    assert_ne!(
        host.transform.rotation,
        Quat::IDENTITY,
        "插件组件的 update 应当已经旋转 host"
    );
    // Reaper 把 target 销毁了。
    assert!(
        world.scene().find_by_name("target").is_none(),
        "Reaper 应当已销毁 target"
    );

    // 四钩子的调用计数（start / on_destroy 精确，update / fixed_update 取下界）。
    let starts = counter(&plugin, b"SPINNER_STARTS");
    let updates = counter(&plugin, b"SPINNER_UPDATES");
    let fixed = counter(&plugin, b"SPINNER_FIXED_UPDATES");
    let destroys = counter(&plugin, b"SPINNER_DESTROYS");
    assert_eq!(starts, 2, "两个 Spinner 各件各 start 一次");
    assert_eq!(destroys, 1, "target 的 on_destroy 应当恰好跑一次");
    // update：第 1 帧两个实体都在（2 次），此后 4 帧只剩 host（4 次）= 6。
    assert!(updates >= 5, "update 钩子应当每帧都在跑，实际 {updates}");
    // fixed_update 与固定步同频，允许步数有浮点抖动，只取下界。
    assert!(fixed >= 3, "fixed_update 钩子应当跑起来，实际 {fixed}");
}

/// 加载失败的路径也要给出可读报错（验收第 6 条的"报错可读"部分）。
#[test]
fn unreadable_plugin_fails_with_readable_error() {
    let missing = std::env::temp_dir().join("definitely_missing_xgame_plugin.dll");
    let mut registry = BehaviorRegistry::new();
    let err = load_plugin(&missing, &mut registry).unwrap_err();
    assert!(err.contains("无法打开插件"), "{err}");
    assert!(err.contains("definitely_missing_xgame_plugin.dll"), "{err}");
    // 握手失败的报错文案（版本不匹配 / 空指针）由 xgame::plugin 的单元测试覆盖。
}
