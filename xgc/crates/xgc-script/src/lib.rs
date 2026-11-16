//! Lua 脚本宿主：只开放安全标准库，脚本按名字加载、执行与调用。

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{FromLua, FromLuaMulti, IntoLua, IntoLuaMulti, Lua, LuaOptions, StdLib, Value};
use xgc_base::EntityId;
use xgc_world::{Transform, World};

pub struct LuaHost {
    lua: Lua,
    world: Option<Rc<RefCell<World>>>,
}

impl LuaHost {
    /// 安全沙箱：table / string / math / utf8 / coroutine；不开 io / os / package / debug。
    pub fn new() -> Result<Self, String> {
        let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE;
        let lua = Lua::new_with(libs, LuaOptions::default())
            .map_err(|err| format!("创建 Lua 运行时失败：{err}"))?;
        Ok(Self { lua, world: None })
    }

    /// 绑定世界：脚本可读实体数 / 读写 Transform；World 由调用方持有以便检查结果。
    pub fn with_world(world: Rc<RefCell<World>>) -> Result<Self, String> {
        let host = Self::new()?;
        host.register_world(&world)?;
        Ok(Self {
            lua: host.lua,
            world: Some(world),
        })
    }

    fn register_world(&self, world: &Rc<RefCell<World>>) -> Result<(), String> {
        let api = self
            .lua
            .create_table()
            .map_err(|err| format!("创建 xgc 表失败：{err}"))?;
        let count_world = Rc::clone(world);
        let count = self
            .lua
            .create_function(move |_, ()| Ok(count_world.borrow().len() as i64))
            .map_err(|err| format!("创建 entity_count 失败：{err}"))?;
        api.set("entity_count", count)
            .map_err(|err| format!("注册 entity_count 失败：{err}"))?;

        let get_world = Rc::clone(world);
        let get_position = self
            .lua
            .create_function(move |lua, id: u64| {
                let world = get_world.borrow();
                let entity = EntityId::from_raw(id);
                let key = world
                    .key(entity)
                    .ok_or_else(|| mlua::Error::RuntimeError(format!("实体 {id} 不存在")))?;
                let transform = world.get::<Transform>(key).ok_or_else(|| {
                    mlua::Error::RuntimeError(format!("实体 {id} 没有 Transform"))
                })?;
                let table = lua.create_table()?;
                table.set("x", transform.position[0])?;
                table.set("y", transform.position[1])?;
                table.set("z", transform.position[2])?;
                Ok(table)
            })
            .map_err(|err| format!("创建 get_position 失败：{err}"))?;
        api.set("get_position", get_position)
            .map_err(|err| format!("注册 get_position 失败：{err}"))?;

        let set_world = Rc::clone(world);
        let set_position = self
            .lua
            .create_function(move |_, (id, x, y, z): (u64, f32, f32, f32)| {
                let mut world = set_world.borrow_mut();
                let entity = EntityId::from_raw(id);
                let key = world
                    .key(entity)
                    .ok_or_else(|| mlua::Error::RuntimeError(format!("实体 {id} 不存在")))?;
                let transform = world.get_mut::<Transform>(key).ok_or_else(|| {
                    mlua::Error::RuntimeError(format!("实体 {id} 没有 Transform"))
                })?;
                transform.position = [x, y, z];
                Ok(())
            })
            .map_err(|err| format!("创建 set_position 失败：{err}"))?;
        api.set("set_position", set_position)
            .map_err(|err| format!("注册 set_position 失败：{err}"))?;

        let log = self
            .lua
            .create_function(|_, message: String| {
                println!("[lua] {message}");
                Ok(())
            })
            .map_err(|err| format!("创建 log 失败：{err}"))?;
        api.set("log", log)
            .map_err(|err| format!("注册 log 失败：{err}"))?;

        self.lua
            .globals()
            .set("xgc", api)
            .map_err(|err| format!("注册 xgc 表失败：{err}"))
    }

    pub fn world(&self) -> Option<&Rc<RefCell<World>>> {
        self.world.as_ref()
    }

    pub fn run(&self, name: &str, source: &str) -> Result<(), String> {
        self.lua
            .load(source)
            .set_name(name)
            .exec()
            .map_err(|err| format!("脚本 {name} 执行失败：{err}"))
    }

    pub fn eval<T: FromLuaMulti>(&self, source: &str) -> Result<T, String> {
        self.lua
            .load(source)
            .eval()
            .map_err(|err| format!("Lua 求值失败：{err}"))
    }

    pub fn call<T: FromLua>(&self, name: &str, args: impl IntoLuaMulti) -> Result<T, String> {
        let function: mlua::Function = self
            .lua
            .globals()
            .get(name)
            .map_err(|err| format!("读取 Lua 函数 {name} 失败：{err}"))?;
        function
            .call(args)
            .map_err(|err| format!("Lua 函数 {name} 调用失败：{err}"))
    }

    pub fn call_multi<T: FromLuaMulti>(
        &self,
        name: &str,
        args: impl IntoLuaMulti,
    ) -> Result<T, String> {
        let function: mlua::Function = self
            .lua
            .globals()
            .get(name)
            .map_err(|err| format!("读取 Lua 函数 {name} 失败：{err}"))?;
        function
            .call(args)
            .map_err(|err| format!("Lua 函数 {name} 调用失败：{err}"))
    }

    pub fn set_global(&self, name: &str, value: impl IntoLua) -> Result<(), String> {
        self.lua
            .globals()
            .set(name, value)
            .map_err(|err| format!("写入 Lua 全局 {name} 失败：{err}"))
    }

    pub fn global_is_nil(&self, name: &str) -> Result<bool, String> {
        let value: Value = self
            .lua
            .globals()
            .get(name)
            .map_err(|err| format!("读取 Lua 全局 {name} 失败：{err}"))?;
        Ok(matches!(value, Value::Nil))
    }

    /// 只读脚本名列表：`package.loaded` 不可用，这里用宿主登记表代替。
    pub fn lua(&self) -> &Lua {
        &self.lua
    }
}

/// `LuaHost` 不是 `Send`，显式声明以便调用方知道不能跨线程共享。
impl std::fmt::Debug for LuaHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaHost").finish_non_exhaustive()
    }
}
