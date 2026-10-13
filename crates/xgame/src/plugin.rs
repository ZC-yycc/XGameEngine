//! 插件边界：引擎宿主与用户 cdylib 之间的**唯一过境点**（UE 模式）。
//!
//! # 这一层在防什么
//!
//! Rust 的 ABI 不保证跨编译单元兼容（结构体布局、trait 对象 vtable、std 类型
//! 的内存表示都可能随编译单元而异）。用户插件与引擎是**两个独立编译产物**，
//! 直接互传 Rust 值等于把稳定性押在"碰巧一致"上。三道防线：
//!
//! 1. **C ABI 入口符号**：`export_plugin!` 只导出一个 `extern "C"` 函数
//!    （dlsym 按名字能找到的东西），函数表与句柄全部 `#[repr(C)]`；
//! 2. **版本握手**：`EngineApi::version` 必须等于宿主的 [`ENGINE_VERSION_CODE`]。
//!    门面 crate 版本一致 ⇒ 同 rustc、同依赖、同 build flags 编译 ⇒ 这些
//!    "Rust ABI"（`BehaviorRegistry` 的内存布局、`Lifecycle` vtable）实践上
//!    一致 —— UE 对 C++ 模块要求同款 MSVC 是同一个道理；
//! 3. **对象纪律**：插件构造的组件对象，其 vtable 指回插件代码；宿主只经
//!    注册表里的函数指针调用，永不跨边界释放对象（分配器纪律）。
//!
//! 握手失败的两侧行为：插件拒载（[`PLUGIN_ERR_VERSION`]）并经日志打出自己的
//! 版本；宿主把返回码翻译成可读报错（[`load_plugin`]）。Rust ABI 不稳定是
//! 已知风险，这道闸门是整个 UE 式插件模式能成立的前提。
//!
//! # 能力从哪来：`Ctx`，不是函数表
//!
//! 计划文档（`Docs/双轨语言架构计划.md` C4）最初列了一张 ~10 函数的表
//! （pos/set_rot/action_pressed/delta/time……）。方法层（C1 `GameCtx`）落地后
//! 这些**全部是 `Ctx` 的方法**，而四个钩子本来就拿到 `&mut Ctx` —— 再铺一张
//! 裸函数表等于给同一套能力造第二份真相源，违反"两轨共用一套 `GameCtx` 方法"
//! 的原则。所以 `EngineApi` 只保留钩子之外用得到的东西：版本码 + 日志。
//! 扩表时机：出现"入口期 / 非钩子上下文"确实需要的能力再加，且只在表尾追加。
//!
//! # 用法（用户视角）
//!
//! ```ignore
//! xgame::export_plugin!(|registry| {
//!     registry.register::<MyComponent>("MyComponent");
//! });
//! ```
//!
//! 宿主侧：
//!
//! ```ignore
//! let mut registry = BehaviorRegistry::new();
//! let _plugin = xgame::plugin::load_plugin("path/to/my_plugin.dll", &mut registry)?;
//! let world = World::new(scene, root, config).with_behavior_registry(registry);
//! ```

use std::ffi::{c_char, c_void};
use std::fmt;
use std::ops::Deref;
use std::path::Path;

use libloading::Library;

use xgame_runtime::BehaviorRegistry;

/// 入口返回：版本握手失败（宿主的版本码 ≠ 插件的 [`ENGINE_VERSION_CODE`]）。
pub const PLUGIN_ERR_VERSION: u32 = 1;
/// 入口返回：宿主块是空指针（宿主侧缺陷）。
pub const PLUGIN_ERR_NULL: u32 = 2;

/// 把 `major.minor.patch` 打包成版本码。
///
/// 布局：`0x58`（"X" 的 ASCII，让"这是个版本码"可辨识）+ 三个 8 位字段。
/// 魔数保证结果**永不为 0** —— 0 附近的值留给了入口错误码，不能与合法
/// 版本码相撞。
///
/// 预发布后缀（`0.1.0-rc.1`）不参与：只认 release 三段号（P0 的握手语义，
/// 真要区分预发布再扩位）。
pub const fn version_code(v: &str) -> u32 {
    let b = v.as_bytes();
    let mut fields = [0u32; 3];
    let mut f = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c == b'.' {
            if f == fields.len() - 1 {
                break; // 第四段起忽略
            }
            f += 1;
        } else if c.is_ascii_digit() {
            fields[f] = fields[f] * 10 + (c - b'0') as u32;
        } else {
            break; // 预发布 / 构建元数据后缀，握手不看
        }
        i += 1;
    }
    0x5800_0000 | ((fields[0] & 0xFF) << 16) | ((fields[1] & 0xFF) << 8) | (fields[2] & 0xFF)
}

/// 本引擎的版本码（`ENGINE_VERSION` 的打包形式，握手的比对基准）。
pub const ENGINE_VERSION_CODE: u32 = version_code(crate::ENGINE_VERSION);

/// 插件日志级别（C 布局 u32）。
///
/// 只在入口期用（钩子内请用 `Ctx::log`，那是同一套日志总线，归属到组件名）。
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginLog {
    Info = 0,
    Warn = 1,
    Error = 2,
}

/// 引擎能力函数表（C 布局）。字段含义见模块文档"能力从哪来"一节。
#[repr(C)]
pub struct EngineApi {
    /// 宿主版本码。插件第一件事就是比对它（见 [`run_entry`]）。
    pub version: u32,
    /// 宿主日志入口。裸指针 + 长度的 C 签名；插件侧用 [`EngineApi::write_log`] 发。
    pub log_fn: extern "C" fn(level: u32, msg: *const c_char, len: usize),
}

impl EngineApi {
    /// 经宿主日志通道发一条（入口期唯一可用的日志通道）。
    pub fn write_log(&self, level: PluginLog, args: fmt::Arguments<'_>) {
        let text = args.to_string();
        (self.log_fn)(level as u32, text.as_ptr().cast::<c_char>(), text.len());
    }
}

/// 插件入口收到的宿主块（C 布局）。
#[repr(C)]
pub struct PluginHost {
    /// 引擎能力表（版本码 + 日志）。
    pub api: EngineApi,
    /// `BehaviorRegistry` 借出的裸句柄：只在入口调用期间解引用。
    pub registry: *mut c_void,
}

/// 入口符号的类型（`export_plugin!` 生成的函数即此签名）。
///
/// `unsafe` 是 dlsym 的固有语义：符号名对不上、宿主块伪造，都是调用方的
/// 责任 —— `load_plugin` 负担这份契约，用户永远只写 `export_plugin!`。
pub type PluginEntryFn = unsafe extern "C" fn(*const PluginHost) -> u32;

/// 插件入口的公共骨架：校验宿主块 → 版本握手 → 把注册表借给插件的注册闭包。
///
/// 返回值是握手协议的一部分：成功返回本插件的 [`ENGINE_VERSION_CODE`]（宿主
/// 据此核对）；失败返回 [`PLUGIN_ERR_VERSION`] / [`PLUGIN_ERR_NULL`]。
///
/// # Safety
///
/// `host` 必须指向宿主构造、且在整个调用期间存活的 [`PluginHost`]，其中的
/// `registry` 句柄必须是有效的 `BehaviorRegistry`。`export_plugin!` 生成的
/// 入口把"宿主保证这一点"作为调用契约。
pub unsafe fn run_entry(
    host: *const PluginHost,
    register: impl FnOnce(&mut BehaviorRegistry),
) -> u32 {
    if host.is_null() {
        return PLUGIN_ERR_NULL;
    }
    // SAFETY: 上面刚判过非空；契约保证指针在本次调用期间有效。
    let host = unsafe { &*host };
    if host.api.version != ENGINE_VERSION_CODE {
        // 插件侧自卫：版本不同 ⇒ Rust ABI 不保证 ⇒ 绝不让对方驱动自己。
        host.api.write_log(
            PluginLog::Error,
            format_args!(
                "本插件按 xgame v{}（版本码 0x{:08X}）编译，宿主版本码 0x{:08X}，拒载",
                crate::ENGINE_VERSION,
                ENGINE_VERSION_CODE,
                host.api.version
            ),
        );
        return PLUGIN_ERR_VERSION;
    }
    if host.registry.is_null() {
        return PLUGIN_ERR_NULL;
    }
    // SAFETY: 契约保证 registry 句柄有效；写入的构造/分发函数指针指向本插件
    // （这份动态库）的代码 —— 所以 LoadedPlugin 必须活到注册表销毁。
    let registry = unsafe { &mut *(host.registry as *mut BehaviorRegistry) };
    register(registry);
    ENGINE_VERSION_CODE
}

/// 已完成握手的插件句柄。
///
/// **必须**活得比"注册了该插件组件的注册表"久：注册表里的构造/分发函数指针
/// 指向这份动态库的代码，句柄 drop = 卸载 = 悬垂函数指针。惯用法是与使用它的
/// 注册表 / 世界同作用域持有（集成测试即是此写法）。
#[derive(Debug)]
pub struct LoadedPlugin {
    _lib: Library,
}

impl Deref for LoadedPlugin {
    type Target = Library;
    fn deref(&self) -> &Library {
        &self._lib
    }
}

/// 宿主侧加载器：打开动态库 → 调入口 → 握手 → 结果交给调用方的注册表。
///
/// 成功返回的 [`LoadedPlugin`] 必须比注册表活得久（见其文档）。
pub fn load_plugin(
    path: impl AsRef<Path>,
    registry: &mut BehaviorRegistry,
) -> Result<LoadedPlugin, String> {
    let path = path.as_ref();
    // SAFETY: 库句柄随返回值交给调用方持有（LoadedPlugin 记载了存活期约定）。
    let lib = unsafe { Library::new(path) }
        .map_err(|e| format!("无法打开插件 {}: {e}", path.display()))?;
    let ret = {
        let entry: libloading::Symbol<'_, PluginEntryFn> = unsafe { lib.get(b"xgame_plugin_entry") }
            .map_err(|_| {
                format!(
                    "插件 {} 缺少入口符号 xgame_plugin_entry（没有用 xgame::export_plugin! 导出？）",
                    path.display()
                )
            })?;
        let host = PluginHost {
            api: EngineApi {
                version: ENGINE_VERSION_CODE,
                log_fn: engine_log_sink,
            },
            registry: registry as *mut BehaviorRegistry as *mut c_void,
        };
        // SAFETY: `entry` 来自 libloading 对 `xgame_plugin_entry` 的按名查找；
        // `host` 是刚构造的栈上块，调用期间存活 —— 即 run_entry 的契约。
        unsafe { (*entry)(&host) }
    };
    handshake(ret)?;
    log::info!(
        "已加载插件 {}（xgame v{}，版本码 0x{:08X}）",
        path.display(),
        crate::ENGINE_VERSION,
        ENGINE_VERSION_CODE
    );
    Ok(LoadedPlugin { _lib: lib })
}

/// 把入口返回码翻译成宿主侧的加载结果（版本不匹配 ⇒ 可读报错）。
fn handshake(ret: u32) -> Result<(), String> {
    if ret == ENGINE_VERSION_CODE {
        return Ok(());
    }
    let ver = crate::ENGINE_VERSION;
    let code = ENGINE_VERSION_CODE;
    match ret {
        PLUGIN_ERR_VERSION => Err(format!(
            "插件拒载：版本不匹配 —— 宿主为 xgame v{ver}（版本码 0x{code:08X}），插件是用另一个版本的 \
             xgame 编译的（其版本已在插件日志中打出）。请改用 v{ver} 的门面 crate 重编插件"
        )),
        PLUGIN_ERR_NULL => Err("插件拒载：入口收到空宿主块（宿主侧缺陷，请上报）".to_owned()),
        other => Err(format!(
            "插件拒载：版本码不符 —— 插件返回 0x{other:08X}，宿主期望 0x{code:08X}（v{ver}）；\
             插件多半是用另一个版本的 xgame 编译的"
        )),
    }
}

/// 宿主侧日志入口：把插件的 `write_log` 转进引擎的 `log` 总线。
extern "C" fn engine_log_sink(level: u32, msg: *const c_char, len: usize) {
    if msg.is_null() {
        return;
    }
    // SAFETY: 契约：指针/长度来自 `write_log` 刚构造好的 String（UTF-8 可信）。
    let bytes = unsafe { std::slice::from_raw_parts(msg.cast::<u8>(), len) };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return;
    };
    match level {
        2 => log::error!("[插件] {text}"),
        1 => log::warn!("[插件] {text}"),
        _ => log::info!("[插件] {text}"),
    }
}

/// 导出插件入口符号（用户插件 crate 的最后一行）。
///
/// 展开成一个 `#[no_mangle] pub unsafe extern "C" fn xgame_plugin_entry` ——
/// 宿主（`load_plugin`）dlsym 的唯一目标。闭包参数是宿主注册表的借用，
/// 在这里 `register::<T>("Name")` 把组件写进宿主的表。
#[macro_export]
macro_rules! export_plugin {
    (|$registry:ident| $body:block) => {
        /// 插件入口符号（由 `xgame::export_plugin!` 生成）。
        ///
        /// # Safety
        ///
        /// 宿主必须传入自己构造、且在调用期间存活的 `PluginHost`。
        #[no_mangle]
        pub unsafe extern "C" fn xgame_plugin_entry(
            host: *const $crate::plugin::PluginHost,
        ) -> u32 {
            // SAFETY: 宿主保证指针在本次调用期间有效（run_entry 的调用契约）。
            unsafe { $crate::plugin::run_entry(host, |$registry| $body) }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::slice;
    use std::sync::Mutex;

    use xgame_runtime::Lifecycle;

    #[derive(Default)]
    struct Noop;
    impl Lifecycle for Noop {}

    /// 插件日志的捕获槽（`extern "C"` 函数不能带状态，只能经静态收集）。
    static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    extern "C" fn capture_log(_level: u32, msg: *const c_char, len: usize) {
        if msg.is_null() {
            return;
        }
        // SAFETY: 契约同 engine_log_sink：指针来自 write_log 刚建好的 String。
        let bytes = unsafe { slice::from_raw_parts(msg.cast::<u8>(), len) };
        CAPTURED
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(bytes).into_owned());
    }

    #[test]
    fn version_code_is_stable_and_never_zero() {
        assert_eq!(version_code("0.1.0"), 0x5800_0100);
        assert_ne!(version_code("0.0.0"), 0, "魔数保证版本码永不为 0（0 是错误码）");
        assert_ne!(version_code("0.1.0"), version_code("0.2.0"));
        // 预发布后缀不参与握手：只认 release 三段号。
        assert_eq!(version_code("0.1.0-rc.1"), version_code("0.1.0"));
        assert_eq!(ENGINE_VERSION_CODE, version_code(crate::ENGINE_VERSION));
    }

    #[test]
    fn entry_registers_components_when_version_matches() {
        let mut registry = BehaviorRegistry::new();
        let host = PluginHost {
            api: EngineApi {
                version: ENGINE_VERSION_CODE,
                log_fn: engine_log_sink,
            },
            registry: &mut registry as *mut BehaviorRegistry as *mut c_void,
        };
        let ret = unsafe { run_entry(&host, |r| r.register::<Noop>("Noop")) };
        assert_eq!(ret, ENGINE_VERSION_CODE, "成功路径应当返回本插件版本码");
        assert!(registry.contains("Noop"), "握手通过后注册表应当被写入");
    }

    #[test]
    fn entry_refuses_mismatched_host_and_logs_its_own_version() {
        CAPTURED.lock().unwrap().clear();
        let mut registry = BehaviorRegistry::new();
        let host = PluginHost {
            api: EngineApi {
                version: ENGINE_VERSION_CODE + 1,
                log_fn: capture_log,
            },
            registry: &mut registry as *mut BehaviorRegistry as *mut c_void,
        };
        let ret = unsafe { run_entry(&host, |r| r.register::<Noop>("Noop")) };
        assert_eq!(ret, PLUGIN_ERR_VERSION, "版本不匹配时插件必须拒载");
        assert!(!registry.contains("Noop"), "握手失败不该写入注册表");
        let captured = CAPTURED.lock().unwrap();
        assert!(
            captured.iter().any(|m| m.contains(crate::ENGINE_VERSION)),
            "插件应当经日志报出自己的版本，实际日志：{captured:?}"
        );
    }

    #[test]
    fn null_host_is_rejected_without_panicking() {
        let ret = unsafe { run_entry(std::ptr::null(), |_| {}) };
        assert_eq!(ret, PLUGIN_ERR_NULL);
    }

    #[test]
    fn handshake_rejections_are_readable() {
        assert!(handshake(ENGINE_VERSION_CODE).is_ok());

        let e = handshake(PLUGIN_ERR_VERSION).unwrap_err();
        assert!(
            e.contains("版本不匹配") && e.contains(crate::ENGINE_VERSION),
            "{e}"
        );

        let e = handshake(0xDEAD_BEEF).unwrap_err();
        assert!(e.contains("0xDEADBEEF"), "{e}");

        let e = handshake(PLUGIN_ERR_NULL).unwrap_err();
        assert!(e.contains("空宿主块"), "{e}");
    }
}
