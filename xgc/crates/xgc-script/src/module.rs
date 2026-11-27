//! 模块顶层初始化与运行期状态分开，候选模块成功后才替换活动环境。

use mlua::{Function, Table, Value};

use crate::{compile, sandbox, LuaHost};

enum Preparation<'a> {
    Standalone,
    LegacyMount,
    DetectMount(&'a Table),
}

pub(super) struct PreparedModule {
    name: String,
    source: String,
    environment: Table,
    initial_state: Option<Table>,
    pub(super) batch_update: Option<Function>,
}

impl LuaHost {
    /// 先执行顶层，再登记源码和环境。失败时仍可调用已加载的模块。
    pub fn load_script(&self, name: &str, source: &str) -> Result<(), String> {
        let module = self.prepare_module(name, source)?;
        self.commit_module(module);
        Ok(())
    }

    /// 用已登记的源码重新构造活动模块。
    pub fn reload(&self, name: &str) -> Result<(), String> {
        let source = self
            .scripts
            .borrow()
            .get(name)
            .cloned()
            .ok_or_else(|| format!("没有登记脚本 {name}"))?;
        self.load_script(name, &source)
    }

    pub fn script_names(&self) -> Vec<String> {
        self.scripts.borrow().keys().cloned().collect()
    }

    pub(super) fn prepare_module(
        &self,
        name: &str,
        source: &str,
    ) -> Result<PreparedModule, String> {
        self.prepare(name, source, Preparation::Standalone)
    }

    pub(super) fn prepare_mount(
        &self,
        name: &str,
        source: &str,
        base: Option<&Table>,
    ) -> Result<PreparedModule, String> {
        let preparation = base.map_or(Preparation::LegacyMount, Preparation::DetectMount);
        self.prepare(name, source, preparation)
    }

    fn prepare(
        &self,
        name: &str,
        source: &str,
        preparation: Preparation<'_>,
    ) -> Result<PreparedModule, String> {
        let mut previous_state = None;
        let result = self.with_budget(
            || format!("脚本 {name} 执行失败"),
            || {
                let mount = match preparation {
                    Preparation::DetectMount(base) => Some(sandbox::MountEnvironment::new(
                        &self.lua,
                        &self.load_factory,
                        base,
                    )?),
                    Preparation::Standalone | Preparation::LegacyMount => None,
                };
                let environment = match &mount {
                    Some(mount) => mount.table(),
                    None => sandbox::module_environment(&self.lua, &self.load_factory)?,
                };
                let function =
                    compile::compile_text(&self.lua, name, source, Some(environment.clone()))?;
                if let Some(state) = &self.state {
                    // 任意 Lua 值都能进持久状态。用临时表初始化，避免通过表或闭包改到正在运行的状态。
                    previous_state = Some((state, state.replace(self.lua.create_table()?)));
                }
                let returned = function.call::<Value>(())?;
                match preparation {
                    Preparation::DetectMount(_) => {
                        let update = batch_update(&environment, returned)?;
                        let mount = mount.expect("已建立挂载环境");
                        Ok((mount.finish(update.is_some())?, update))
                    }
                    Preparation::LegacyMount => {
                        if batch_update(&environment, returned)?.is_some() {
                            return Err(mlua::Error::RuntimeError(
                                "切换到批回调后需要重新进入播放".into(),
                            ));
                        }
                        Ok((environment, None))
                    }
                    Preparation::Standalone => Ok((environment, None)),
                }
            },
        );
        let initialized = previous_state.map(|(state, previous)| state.replace(previous));
        let (environment, batch_update) = result?;
        Ok(PreparedModule {
            name: name.to_owned(),
            source: source.to_owned(),
            environment,
            batch_update,
            initial_state: if self.environment.borrow().is_none() {
                initialized
            } else {
                None
            },
        })
    }

    pub(super) fn commit_module(&self, module: PreparedModule) {
        if let (Some(state), Some(initial)) = (&self.state, module.initial_state) {
            state.replace(initial);
        }
        self.environment.replace(Some(module.environment));
        self.scripts.borrow_mut().insert(module.name, module.source);
    }

    pub(super) fn environment(&self) -> Table {
        self.environment
            .borrow()
            .as_ref()
            .cloned()
            .unwrap_or_else(|| self.lua.globals())
    }
}

fn batch_update(environment: &Table, returned: Value) -> mlua::Result<Option<Function>> {
    let Value::Table(module) = returned else {
        return Ok(None);
    };
    let Value::Function(update) = module.raw_get::<Value>("update")? else {
        return Ok(None);
    };
    if crate::lifecycle::has_mounted_hook(environment)? {
        // 现有挂载可返回带函数的元数据。它的全局钩子仍是挂载生命周期入口。
        return Ok(None);
    }
    Ok(Some(update))
}
