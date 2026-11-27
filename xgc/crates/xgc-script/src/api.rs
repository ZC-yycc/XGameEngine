//! 从真正的挂载环境读取接口，区分 Lua 自带函数和引擎绑定。

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use mlua::{Function, Table, Value};

use crate::{sandbox, LuaHost, ScriptView};

/// 项目 Lua 模块可见的函数名，按名称排序。
#[derive(Debug)]
pub struct MountedLuaApi {
    pub globals: BTreeSet<String>,
    pub standard_globals: BTreeSet<String>,
    pub namespace: BTreeSet<String>,
}

/// 只启动空沙箱和生产绑定，无需装载项目或运行其脚本。
pub fn mounted_lua_api() -> Result<MountedLuaApi, String> {
    let mut host = LuaHost::new()?;
    let standard = functions(&host.lua.globals()).map_err(inspect_error)?;
    host.register_mounted(
        Default::default(),
        Rc::new(RefCell::new(ScriptView::default())),
        0,
    )?;
    inspect(&host, &standard).map_err(inspect_error)
}

fn inspect(host: &LuaHost, standard: &BTreeMap<String, Function>) -> mlua::Result<MountedLuaApi> {
    let environment = sandbox::module_environment(&host.lua, &host.load_factory)?;
    let mut api = MountedLuaApi {
        globals: BTreeSet::new(),
        standard_globals: BTreeSet::new(),
        namespace: BTreeSet::new(),
    };
    for (name, installed) in functions(&host.lua.globals())? {
        if !matches!(environment.get::<Value>(name.as_str())?, Value::Function(_)) {
            continue;
        }
        // 用同一 VM 中安装前后的函数身份判定，覆盖标准函数的新绑定也应计入引擎接口。
        // 从模块查可见性时不能用 pairs(_G)，绑定通过其元表查找。
        if standard
            .get(&name)
            .is_some_and(|original| original.to_pointer() == installed.to_pointer())
        {
            api.standard_globals.insert(name);
        } else {
            api.globals.insert(name);
        }
    }
    if let Value::Table(namespace) = environment.get::<Value>("xgc")? {
        api.namespace = functions(&namespace)?.into_keys().collect();
    }
    Ok(api)
}

fn functions(table: &Table) -> mlua::Result<BTreeMap<String, Function>> {
    let mut functions = BTreeMap::new();
    for pair in table.pairs::<String, Value>() {
        let (name, value) = pair?;
        if let Value::Function(function) = value {
            functions.insert(name, function);
        }
    }
    Ok(functions)
}

fn inspect_error(error: mlua::Error) -> String {
    format!("读取项目 Lua 接口失败：{error}")
}
