//! Lua 脚本宿主：只开放安全标准库，脚本按名字加载、执行与调用。

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use mlua::{
    FromLua, FromLuaMulti, HookTriggers, IntoLua, IntoLuaMulti, Lua, LuaOptions, StdLib,
    Value as LuaValue, VmState,
};
use xgc_base::EntityId;
use xgc_world::{builtin_registry, Command, Transform, Value, World};

mod runtime;
pub use runtime::ScriptRuntime;

pub struct LuaHost {
    lua: Lua,
    world: Option<Rc<RefCell<World>>>,
    scripts: RefCell<BTreeMap<String, String>>,
    budget: Option<(Arc<AtomicUsize>, usize)>,
    started: Cell<bool>,
    commands: Option<Rc<RefCell<Vec<Command>>>>,
}

impl LuaHost {
    /// 安全沙箱：table / string / math / utf8 / coroutine；不开 io / os / package / debug。
    pub fn new() -> Result<Self, String> {
        let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE;
        let lua = Lua::new_with(libs, LuaOptions::default())
            .map_err(|err| format!("创建 Lua 运行时失败：{err}"))?;
        Ok(Self {
            lua,
            world: None,
            scripts: RefCell::new(BTreeMap::new()),
            budget: None,
            started: Cell::new(false),
            commands: None,
        })
    }

    /// 脚本只产生命令；由调用方在帧末 plan / apply，保证事务边界。
    pub fn with_commands(commands: Rc<RefCell<Vec<Command>>>) -> Result<Self, String> {
        let host = Self::new()?;
        host.register_commands(&commands)?;
        Ok(Self {
            lua: host.lua,
            world: None,
            scripts: RefCell::new(BTreeMap::new()),
            budget: None,
            started: Cell::new(false),
            commands: Some(commands),
        })
    }

    fn register_commands(&self, commands: &Rc<RefCell<Vec<Command>>>) -> Result<(), String> {
        let api = self
            .lua
            .create_table()
            .map_err(|err| format!("创建 xgc 表失败：{err}"))?;
        let set_commands = Rc::clone(commands);
        let set_field = self
            .lua
            .create_function(
                move |_, (id, component, field, value): (u64, String, String, mlua::Value)| {
                    let component = component_id(&component)?;
                    let value = lua_to_value(&value)?;
                    set_commands.borrow_mut().push(Command::SetField {
                        entity: EntityId::from_raw(id),
                        component,
                        field,
                        value,
                    });
                    Ok(())
                },
            )
            .map_err(|err| format!("创建 set 失败：{err}"))?;
        api.set("set", set_field)
            .map_err(|err| format!("注册 set 失败：{err}"))?;
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

    pub fn take_commands(&self) -> Vec<Command> {
        self.commands
            .as_ref()
            .map(|commands| std::mem::take(&mut *commands.borrow_mut()))
            .unwrap_or_default()
    }
    pub fn with_world(world: Rc<RefCell<World>>) -> Result<Self, String> {
        let host = Self::new()?;
        host.register_world(&world)?;
        Ok(Self {
            lua: host.lua,
            world: Some(world),
            scripts: RefCell::new(BTreeMap::new()),
            budget: None,
            started: Cell::new(false),
            commands: None,
        })
    }

    /// 指令预算：每 N 条指令检查一次，超限返回 Lua 错误；每次 run / eval / call 前重置。
    pub fn set_instruction_limit(&mut self, limit: usize) {
        let counter = Arc::new(AtomicUsize::new(0));
        let hook_counter = Arc::clone(&counter);
        let budget = Arc::new(limit);
        self.lua.set_hook(
            HookTriggers::new().every_nth_instruction(1000),
            move |_, _| {
                let used = hook_counter.fetch_add(1000, Ordering::Relaxed) + 1000;
                if used > *budget {
                    Err(mlua::Error::RuntimeError("Lua 指令预算超限".to_string()))
                } else {
                    Ok(VmState::Continue)
                }
            },
        );
        self.budget = Some((counter, limit));
    }

    fn reset_budget(&self) {
        if let Some((counter, _)) = &self.budget {
            counter.store(0, Ordering::Relaxed);
        }
    }

    /// 加载脚本：执行并登记源码，供 `reload` 使用。
    pub fn load_script(&self, name: &str, source: &str) -> Result<(), String> {
        self.scripts
            .borrow_mut()
            .insert(name.to_string(), source.to_string());
        self.run(name, source)
    }

    /// 热重载：重新执行已登记脚本。
    pub fn reload(&self, name: &str) -> Result<(), String> {
        let source = self
            .scripts
            .borrow()
            .get(name)
            .cloned()
            .ok_or_else(|| format!("没有登记脚本 {name}"))?;
        self.run(name, &source)
    }

    pub fn script_names(&self) -> Vec<String> {
        self.scripts.borrow().keys().cloned().collect()
    }

    /// 调用生命周期钩子；脚本没有该函数时返回 false。
    pub fn call_hook(&self, name: &str, dt: f32) -> Result<bool, String> {
        let value: LuaValue = self
            .lua
            .globals()
            .get(name)
            .map_err(|err| format!("读取钩子 {name} 失败：{err}"))?;
        let LuaValue::Function(function) = value else {
            return Ok(false);
        };
        self.reset_budget();
        if name == "on_update" {
            function
                .call::<()>(dt)
                .map_err(|err| format!("Lua 钩子 {name} 失败：{err}"))?;
        } else {
            function
                .call::<()>(())
                .map_err(|err| format!("Lua 钩子 {name} 失败：{err}"))?;
        }
        Ok(true)
    }

    /// 首次调用触发 `on_start`；重复调用返回 false。
    pub fn start(&self) -> Result<bool, String> {
        if self.started.replace(true) {
            return Ok(false);
        }
        self.call_hook("on_start", 0.0)
    }

    pub fn update(&self, dt: f32) -> Result<bool, String> {
        self.call_hook("on_update", dt)
    }

    pub fn destroy(&self) -> Result<bool, String> {
        self.call_hook("on_destroy", 0.0)
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

        let get_world = Rc::clone(world);
        let get_field = self
            .lua
            .create_function(move |lua, (id, component, field): (u64, String, String)| {
                let component = component_id(&component)?;
                let world = get_world.borrow();
                let value = world
                    .get_field(EntityId::from_raw(id), component, &field)
                    .ok_or_else(|| {
                        mlua::Error::RuntimeError(format!(
                            "实体 {id} 没有字段 {}.{field}",
                            component.0
                        ))
                    })?;
                value_to_lua(lua, &value)
            })
            .map_err(|err| format!("创建 get_field 失败：{err}"))?;
        api.set("get", get_field)
            .map_err(|err| format!("注册 get 失败：{err}"))?;

        let set_world = Rc::clone(world);
        let set_field = self
            .lua
            .create_function(
                move |_, (id, component, field, value): (u64, String, String, mlua::Value)| {
                    let component = component_id(&component)?;
                    let value = lua_to_value(&value)?;
                    let mut world = set_world.borrow_mut();
                    world
                        .set_field(EntityId::from_raw(id), component, &field, &value)
                        .map_err(mlua::Error::RuntimeError)
                },
            )
            .map_err(|err| format!("创建 set_field 失败：{err}"))?;
        api.set("set", set_field)
            .map_err(|err| format!("注册 set 失败：{err}"))?;

        let spawn_world = Rc::clone(world);
        let spawn = self
            .lua
            .create_function(move |_, ()| {
                let mut world = spawn_world.borrow_mut();
                let key = world.spawn((Transform::default(),));
                world
                    .entity(key)
                    .map(|id| id.raw())
                    .ok_or_else(|| mlua::Error::RuntimeError("spawn 失败".to_string()))
            })
            .map_err(|err| format!("创建 spawn 失败：{err}"))?;
        api.set("spawn", spawn)
            .map_err(|err| format!("注册 spawn 失败：{err}"))?;

        let despawn_world = Rc::clone(world);
        let despawn = self
            .lua
            .create_function(move |_, id: u64| {
                let mut world = despawn_world.borrow_mut();
                let key = world
                    .key(EntityId::from_raw(id))
                    .ok_or_else(|| mlua::Error::RuntimeError(format!("实体 {id} 不存在")))?;
                if world.despawn(key) {
                    Ok(true)
                } else {
                    Err(mlua::Error::RuntimeError(format!("实体 {id} 删除失败")))
                }
            })
            .map_err(|err| format!("创建 despawn 失败：{err}"))?;
        api.set("despawn", despawn)
            .map_err(|err| format!("注册 despawn 失败：{err}"))?;

        self.lua
            .globals()
            .set("xgc", api)
            .map_err(|err| format!("注册 xgc 表失败：{err}"))
    }

    pub fn world(&self) -> Option<&Rc<RefCell<World>>> {
        self.world.as_ref()
    }

    pub fn run(&self, name: &str, source: &str) -> Result<(), String> {
        self.reset_budget();
        self.lua
            .load(source)
            .set_name(name)
            .exec()
            .map_err(|err| format!("脚本 {name} 执行失败：{err}"))
    }

    pub fn eval<T: FromLuaMulti>(&self, source: &str) -> Result<T, String> {
        self.reset_budget();
        self.lua
            .load(source)
            .eval()
            .map_err(|err| format!("Lua 求值失败：{err}"))
    }

    pub fn call<T: FromLua>(&self, name: &str, args: impl IntoLuaMulti) -> Result<T, String> {
        self.reset_budget();
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
        self.reset_budget();
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
        let value: LuaValue = self
            .lua
            .globals()
            .get(name)
            .map_err(|err| format!("读取 Lua 全局 {name} 失败：{err}"))?;
        Ok(matches!(value, LuaValue::Nil))
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

fn component_id(name: &str) -> mlua::Result<xgc_world::ComponentId> {
    builtin_registry()
        .by_name(name)
        .map(|schema| xgc_world::ComponentId(schema.id))
        .ok_or_else(|| mlua::Error::RuntimeError(format!("未知组件：{name}")))
}

fn value_to_lua(lua: &Lua, value: &Value) -> mlua::Result<mlua::Value> {
    Ok(match value {
        Value::Bool(value) => mlua::Value::Boolean(*value),
        Value::I64(value) => mlua::Value::Integer(*value),
        Value::F64(value) => mlua::Value::Number(*value),
        Value::String(value) => mlua::Value::String(lua.create_string(value)?),
        Value::Vec2(value) => vec_table(lua, &[value[0], value[1]], &["x", "y"])?,
        Value::Vec3(value) => vec_table(lua, &[value[0], value[1], value[2]], &["x", "y", "z"])?,
        Value::Vec4(value) | Value::Quat(value) => vec_table(
            lua,
            &[value[0], value[1], value[2], value[3]],
            &["x", "y", "z", "w"],
        )?,
        Value::Entity(value) => mlua::Value::Integer(*value as i64),
    })
}

fn vec_table(lua: &Lua, values: &[f64], names: &[&str]) -> mlua::Result<mlua::Value> {
    let table = lua.create_table()?;
    for (name, value) in names.iter().zip(values) {
        table.set(*name, *value)?;
    }
    Ok(mlua::Value::Table(table))
}

fn lua_to_value(value: &mlua::Value) -> mlua::Result<Value> {
    match value {
        mlua::Value::Boolean(value) => Ok(Value::Bool(*value)),
        mlua::Value::Integer(value) => Ok(Value::I64(*value)),
        mlua::Value::Number(value) => Ok(Value::F64(*value)),
        mlua::Value::String(value) => Ok(Value::String(value.to_str()?.to_string())),
        mlua::Value::Table(table) => {
            let x = table.get::<Option<f64>>("x")?;
            let y = table.get::<Option<f64>>("y")?;
            let z = table.get::<Option<f64>>("z")?;
            let w = table.get::<Option<f64>>("w")?;
            match (x, y, z, w) {
                (Some(x), Some(y), Some(z), Some(w)) => Ok(Value::Vec4([x, y, z, w])),
                (Some(x), Some(y), Some(z), None) => Ok(Value::Vec3([x, y, z])),
                (Some(x), Some(y), None, None) => Ok(Value::Vec2([x, y])),
                _ => Err(mlua::Error::RuntimeError(
                    "向量表需要 x/y[/z[/w]] 字段".to_string(),
                )),
            }
        }
        other => Err(mlua::Error::RuntimeError(format!(
            "不支持的 Lua 值：{}",
            other.type_name()
        ))),
    }
}
