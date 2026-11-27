//! 批回调只在当前固定步内借用同一份世界，字段写入交给常规事务。

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    sync::LazyLock,
};

use mlua::{Function, Table, Value as LuaValue};
use xgc_base::EntityId;
use xgc_world::{
    builtin_registry, Command, ComponentId, FieldPath, FieldSchema, Registry, Value, World,
};

use crate::{lua_value, LuaHost};

pub(super) struct BatchModule {
    pub(super) name: String,
    pub(super) update: Function,
    factory: Function,
    pub(super) members: Vec<EntityId>,
}

impl BatchModule {
    pub(super) fn new(name: String, host: &LuaHost, update: Function) -> Result<Self, String> {
        let factory = host
            .lua
            .load(BATCH_FACTORY)
            .set_name("=xgc:batch")
            .eval::<Function>()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            name,
            update,
            factory,
            members: Vec::new(),
        })
    }

    pub(super) fn run(
        &self,
        host: &LuaHost,
        world: &World,
        commands: &mut Vec<Command>,
    ) -> Result<(), String> {
        let writes = RefCell::new(BTreeMap::new());
        let selected = Cell::new(None);
        let active = || selected.get().ok_or_else(|| runtime_error("批行尚未选定"));
        host.with_budget(
            || format!("批脚本 {} 执行失败", self.name),
            || {
                host.lua.scope(|scope| {
                    let get =
                        scope.create_function(|lua, (_row, name): (Table, mlua::String)| {
                            let entity = active()?;
                            let name = name.to_str()?;
                            let field = BatchField::resolve(&name)?;
                            if let Some(staged) = writes.borrow().get(&field.key(entity)) {
                                return field.to_lua(lua, staged);
                            }
                            world
                                .get_field(entity, field.component, &field.root.name)
                                .map_or(Ok(LuaValue::Nil), |value| field.to_lua(lua, &value))
                        })?;
                    let set = scope.create_function(
                        |_, (_row, name, value): (Table, mlua::String, LuaValue)| {
                            let entity = active()?;
                            let name = name.to_str()?;
                            let field = BatchField::resolve(&name)?;
                            let value = lua_value::lua_to_field_value(value, field.schema)?;
                            let key = field.key(entity);
                            let mut writes = writes.borrow_mut();
                            let value = if let Some(path) = &field.path {
                                // 整根覆盖才能让随后读写其他子路径也看见本次回调的改动。
                                let mut root = writes
                                    .get(&key)
                                    .cloned()
                                    .map_or_else(|| field.read_root(world, entity), Ok)?;
                                path.replace(&mut root, value).map_err(runtime_error)?;
                                root
                            } else {
                                if !writes.contains_key(&key) {
                                    field.read_root(world, entity)?;
                                }
                                value
                            };
                            writes.insert(key, value);
                            Ok(())
                        },
                    )?;
                    let row = self.factory.call::<Table>((get, set))?;
                    let selected = &selected;
                    let entity =
                        scope.create_function(move |_, (_batch, index): (Table, usize)| {
                            let entity = index
                                .checked_sub(1)
                                .and_then(|index| self.members.get(index))
                                .copied()
                                .ok_or_else(|| {
                                    runtime_error(format!("批行索引超出范围：{index}"))
                                })?;
                            selected.set(Some(entity));
                            Ok(row.clone())
                        })?;
                    let len = scope.create_function(|_, (): ()| Ok(self.members.len()))?;
                    let batch = self.factory.call::<Table>((len, entity, true))?;
                    self.update.call::<()>(batch)
                })
            },
        )?;
        commands.extend(writes.into_inner().into_iter().map(
            |((entity, component, field), value)| Command::SetField {
                entity,
                component,
                field: field.to_owned(),
                value,
            },
        ));
        Ok(())
    }
}

struct BatchField<'a> {
    component: ComponentId,
    root: &'a FieldSchema,
    schema: &'a FieldSchema,
    path: Option<FieldPath>,
}

impl BatchField<'static> {
    fn resolve(name: &str) -> mlua::Result<Self> {
        static REGISTRY: LazyLock<Registry> = LazyLock::new(builtin_registry);
        for (separator, _) in name.rmatch_indices('.') {
            let (component, path) = name.split_at(separator);
            let Some(descriptor) = REGISTRY.by_name(component) else {
                continue;
            };
            let path = &path[1..];
            let root_name = path.split(['.', '[']).next().unwrap_or_default();
            let root = descriptor
                .fields
                .iter()
                .find(|field| field.name == root_name)
                .ok_or_else(|| runtime_error(format!("未知批字段：{name}")))?;
            let path = (path != root_name)
                .then(|| FieldPath::parse(path).map_err(runtime_error))
                .transpose()?;
            let schema = match &path {
                Some(path) => path.schema(root).map_err(runtime_error)?,
                None => root,
            };
            return Ok(Self {
                component: ComponentId(descriptor.id),
                root,
                schema,
                path,
            });
        }
        Err(runtime_error(format!("未知批字段：{name}")))
    }

    fn key(&self, entity: EntityId) -> (EntityId, ComponentId, &'static str) {
        (entity, self.component, &self.root.name)
    }

    fn read_root(&self, world: &World, entity: EntityId) -> mlua::Result<Value> {
        world
            .get_field(entity, self.component, &self.root.name)
            .ok_or_else(|| runtime_error(format!("实体 {entity} 缺少批字段 {}", self.root.name)))
    }

    fn to_lua(&self, lua: &mlua::Lua, root: &Value) -> mlua::Result<LuaValue> {
        match &self.path {
            Some(path) => path.read(root).map_or(Ok(LuaValue::Nil), |value| {
                lua_value::value_to_lua(lua, value)
            }),
            None => lua_value::value_to_lua(lua, root),
        }
    }
}

fn runtime_error(message: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(message.into())
}

// 一次调用复用同一个行句柄，每次 entity(i) 改变当前行；脚本在本轮循环内使用它。
// scope 在回调返回后撤销借用，Lua 保存下来的句柄也不能再读写当前世界。
const BATCH_FACTORY: &str = r#"
    local function readonly()
        error('批句柄只读，请用 row:set() 写字段', 2)
    end
    return function(first, second, batch)
        local methods
        if batch then
            methods = {len = first, entity = second}
        else
            methods = {get = first, set = second}
        end
        return setmetatable({}, {
            __metatable = false,
            __newindex = readonly,
            __index = methods,
        })
    end
"#;
