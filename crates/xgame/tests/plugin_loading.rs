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
//! 两个插件都是 workspace 成员，产出在 `target/*/<插件名>.dll`。**注意
//! `cargo test` 并不会产出 cdylib** —— 它只要 rlib 来链接测试，所以
//! `target/*.dll` 停在上次 `cargo build` 的那一刻。
//!
//! 而"过期的插件 DLL"在本测试里不是"跑得旧"而是**直接崩进程**：插件注册表
//! 是跨编译单元的内存布局，DLL 侧写、宿主侧读，结构一变（P1 给
//! `BehaviorEntry` 加过一个字段）就会读歪 `HashMap`，再经构造函数指针跳过去
//! = `STATUS_ACCESS_VIOLATION`。所以每次取 DLL 都先嵌套构建一次
//! （见 [`build_package_once`]），把"新鲜"交给 cargo 的完整指纹去判断。
//! 带超时，防止与外层 cargo 抢目标目录锁时无限挂起。
//!
//! # 为什么同时测两个插件
//!
//! `spinner-plugin`（`examples/`）是引擎仓库自带的最小示例，四钩子 + 计数器；
//! `behaviors-demo`（`projects/`）是**用户项目形态**的插件 —— 它就在游戏项目
//! 里面，正是编辑器 P0.5 会探测并自动编译的那一种。两条路径的 dlopen 逻辑
//! 相同，但"用户项目"这一侧还需要确认：workspace 成员产出的产物名、
//! `crate-type = ["cdylib", "rlib"]` 时的文件名，都与示例不同。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use xgame::plugin::{load_plugin, LoadedPlugin};
use xgame::prelude::*;

/// 探测示例插件的 cdylib 路径；找不到就现场构建一次。
fn example_plugin_dll() -> PathBuf {
    workspace_plugin_dll("spinner_plugin", "spinner-plugin")
}

/// 探测演示项目 `projects/BehaviorsDemo` 的 cdylib 路径；找不到就现场构建一次。
fn behaviors_demo_dll() -> PathBuf {
    workspace_plugin_dll("behaviors_demo", "behaviors-demo")
}

/// 按 cdylib 名字（下划线形式）与 cargo 包名（连字符形式）定位 workspace 成员的
/// 动态库，找不到就嵌套构建一次。
///
/// 两个名字分开传是因为"包名 → 产物文件名"的换算规则是 cargo 的地盘
/// （`-` → `_`，再按平台加 `lib` 前缀 / `.dll` 后缀）—— 在测试里照抄一遍
/// 规则比猜可靠，而一旦猜错，报错会是"找不到文件"这种毫无信息量的话。
fn workspace_plugin_dll(lib_name: &str, package: &str) -> PathBuf {
    let file = format!(
        "{}{lib_name}{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let mut candidates: Vec<PathBuf> = Vec::new();
    // 1) 当前测试二进制的 target 子目录（CARGO_TARGET_TMPDIR = target/<profile>/tmp）。
    if let Ok(tmp) = std::env::var("CARGO_TARGET_TMPDIR") {
        candidates.push(PathBuf::from(tmp).parent().unwrap().join(&file));
    }
    // 2) 仓库根的 target/{当前 profile, 另一 profile}（`env!` 保证路径与本测试同源）。
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap().to_owned();
    let (first, second) = if cfg!(debug_assertions) {
        ("debug", "release")
    } else {
        ("release", "debug")
    };
    candidates.push(repo_root.join("target").join(first).join(&file));
    candidates.push(repo_root.join("target").join(second).join(&file));

    // **先保证产物与源码一致，再找产物** —— 顺序反了就会捡到一个"存在但过期"
    // 的 DLL，而过期的插件会把测试进程打崩（见 `build_package_once`）。
    build_package_once(&repo_root, package);

    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| {
            panic!(
                "构建后仍未找到插件动态库 {file}；请手动运行 `cargo build -p {package}` \
                 并检查 target 目录布局"
            )
        })
}

/// 嵌套构建一个插件包，**每个包每进程至多一次**（并发的其余测试等它建完）。
///
/// # 为什么必须真的构建，而不是"探到就用"
///
/// `cargo test` **不会重新产出 cdylib** —— 它只要 rlib 来链接测试二进制，
/// 于是 `target/*.dll` 停在上次 `cargo build` 的那一刻。而插件注册表是
/// **跨编译单元的内存布局**：DLL 那侧写 `BehaviorEntry`，宿主这侧读它。
/// P1 给 `BehaviorEntry` 加了 `fields: &'static [FieldDesc]` —— 旧 DLL 按三段写、
/// 新宿主按四段解，`HashMap` 的桶直接读歪，下一次经构造函数指针跳过去就是
/// `STATUS_ACCESS_VIOLATION`：**进程暴毙，连 panic 都没有**，测试输出里只能看到
/// 一个 0xc0000005。
///
/// P0.5 没踩到纯属运气 —— 那会儿 DLL 刚由验证流程里的 `cargo build` 产出、
/// 恰好新鲜。产物与源码是否一致不该靠运气，所以这里无条件交给 cargo
/// （它有完整指纹：新鲜时零点几秒、**且不会重写产物文件**）。
///
/// # 为什么每包只构建一次
///
/// 本文件的四个测试并行跑，各自起一次嵌套 cargo 除了白排队，还有 Windows
/// 文件锁竞态：`load_plugin` 直接 dlopen `target/*.dll`（本测试**刻意**不走
/// 编辑器的临时副本路径 —— 那是 P0.5 给"编辑器反复重载"设计的，走它就测不到
/// 原始加载了），而已被加载的文件 cargo 重写不了，会以 "Access denied" 失败。
/// 把构建串成进程内一次，后面几个测试读到的必定是同一批已建好的产物。
///
/// 持锁跨整次构建是有意的：第二个测试要的正是"第一个建完了"。
fn build_package_once(repo_root: &std::path::Path, package: &str) {
    static DONE: std::sync::OnceLock<Mutex<Vec<String>>> = std::sync::OnceLock::new();
    let done = DONE.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = done.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.iter().any(|p| p == package) {
        return;
    }
    build_plugin_package(repo_root, package);
    guard.push(package.to_owned());
}

/// 嵌套跑一次 `cargo build -p <package>`（在工作线程里等结果，
/// 超时即判"与外层 cargo 死锁"而不是无限挂起测试）。
fn build_plugin_package(repo_root: &std::path::Path, package: &str) {
    let cargo = cargo_exe();
    let mut cmd = std::process::Command::new(&cargo);
    cmd.args(["build", "-p", package])
        .current_dir(repo_root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if !cfg!(debug_assertions) {
        cmd.arg("--release");
    }
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("无法启动 {cargo:?} 构建插件包 {package}：{e}"));

    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    panic!("`cargo build -p {package}` 失败（{status}），插件无法构建");
                }
                return;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    panic!(
                        "嵌套 `cargo build -p {package}` 超时 —— 多半是与外层 cargo \
                         抢目标目录锁；请手动构建后再跑本测试"
                    );
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => panic!("等待插件构建失败：{e}"),
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
        let p = PathBuf::from(home)
            .join(".cargo")
            .join("bin")
            .join("cargo.exe");
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

// ---------------------------------------------------------------------------
// 演示项目（projects/BehaviorsDemo）—— P0.5 的"用户项目"这一侧
// ---------------------------------------------------------------------------

/// 演示插件跨边界加载后，组件名与它自己单元测试里断言的**完全一致**。
///
/// 这条测试守着一件很容易漂移的事：`behaviors-demo` 的单测是在**同进程**里
/// 直接调 `register_all` 验证的，而编辑器走的是 dlopen + 握手。两边必须是
/// 同一张表 —— 否则会出现"单测绿、场景里挂组件却纹丝不动"。
#[test]
fn behaviors_demo_plugin_registers_across_the_boundary() {
    let dll = behaviors_demo_dll();
    let mut registry = BehaviorRegistry::new();
    let plugin = load_plugin(&dll, &mut registry).unwrap_or_else(|e| panic!("{e}"));

    assert!(registry.contains("Spinner"), "Spinner 应当已注册");
    assert!(registry.contains("Bobber"), "Bobber 应当已注册");
    assert_eq!(
        registry.names().len(),
        2,
        "注册表里应当正好是这两个组件，实际 {:?}",
        registry.names()
    );

    // 保活到测试结束：句柄 drop 不卸载，但断言期间必须活着。
    let _keep = plugin.path();
}

/// 演示插件的组件真的能驱动实体（**经真实 dlopen** 的整条链路）。
///
/// 与 `behaviors-demo` 自己的单测互补：那条测的是组件逻辑，这条测的是
/// "组件逻辑经跨边界函数指针 + `World` 分发后仍然成立"。
#[test]
fn behaviors_demo_plugin_drives_entities() {
    let dll = behaviors_demo_dll();
    let mut registry = BehaviorRegistry::new();
    let _plugin = load_plugin(&dll, &mut registry).unwrap_or_else(|e| panic!("{e}"));

    let mut scene = Scene::new("e2e");
    let mut spinner = Entity::new("spinner");
    spinner.behaviors.push("Spinner".to_owned());
    scene.add_entity(spinner);
    let mut bobber = Entity::new("bobber");
    bobber.behaviors.push("Bobber".to_owned());
    bobber.transform.position = Vec3::new(0.0, 3.0, 0.0);
    scene.add_entity(bobber);

    let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
        .with_behavior_registry(registry);

    // 一边跑一边记录 Bobber 的最低点：跑满整周期（BOB_HZ=0.5 ⇒ 周期 2s）会正好
    // 回到起点，只看终点会得到"没动"的假象 —— 第一版就是这么写的。
    let mut bob_min = f32::MAX;
    for _ in 0..120 {
        world.update(1.0 / 60.0);
        let y = world
            .scene()
            .find_by_name("bobber")
            .expect("实体应当还在")
            .transform
            .position
            .y;
        bob_min = bob_min.min(y);
    }

    let scene = world.scene();
    let s = scene.find_by_name("spinner").expect("实体应当还在");
    let (yaw, _, _) = s.transform.rotation.to_euler(glam::EulerRot::YXZ);
    assert!(
        (yaw.to_degrees() - 150.0).abs() < 1.0,
        "Spinner 应当在 2 秒里转过约 150°，实际 {:.2}°",
        yaw.to_degrees()
    );
    // 起点 3.0、幅度 0.6 ⇒ 最低点应当接近 2.4。
    assert!(
        (bob_min - 2.4).abs() < 0.05,
        "Bobber 应当浮到 2.4 附近，最低只到 {bob_min:.3}"
    );
}
