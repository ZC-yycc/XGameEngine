//! Lua 宿主回归：安全标准库、执行、函数调用。

use xgc_script::LuaHost;

#[test]
fn evaluates_arithmetic_and_functions() {
    let host = LuaHost::new().unwrap();
    assert_eq!(host.eval::<i64>("return 1 + 2").unwrap(), 3);
    host.run("defs", "function add(a, b) return a + b end")
        .unwrap();
    assert_eq!(host.call::<i64>("add", (2, 5)).unwrap(), 7);
}

#[test]
fn sandbox_does_not_expose_io_or_os() {
    let host = LuaHost::new().unwrap();
    assert!(host.global_is_nil("io").unwrap());
    assert!(host.global_is_nil("os").unwrap());
    assert!(host.global_is_nil("package").unwrap());
}

#[test]
fn syntax_errors_are_reported() {
    let host = LuaHost::new().unwrap();
    let error = host.run("bad", "function (").unwrap_err();
    assert!(error.contains("bad"));
}
