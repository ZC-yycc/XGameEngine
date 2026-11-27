//! 模块顶层初始化与运行期状态分开，候选模块成功后才替换活动环境。

use mlua::Table;

use crate::{compile, sandbox, LuaHost};

pub(super) struct PreparedModule {
    name: String,
    source: String,
    environment: Table,
    initial_state: Option<Table>,
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
        let mut previous_state = None;
        let environment = self.with_budget(
            || format!("脚本 {name} 执行失败"),
            || {
                let environment = sandbox::module_environment(&self.lua, &self.load_factory)?;
                let function =
                    compile::compile_text(&self.lua, name, source, Some(environment.clone()))?;
                if let Some(state) = &self.state {
                    // 任意 Lua 值都能进持久状态。用临时表初始化，避免通过表或闭包改到正在运行的状态。
                    previous_state = Some((state, state.replace(self.lua.create_table()?)));
                }
                function.call::<()>(())?;
                Ok(environment)
            },
        );
        let initialized = previous_state.map(|(state, previous)| state.replace(previous));
        Ok(PreparedModule {
            name: name.to_owned(),
            source: source.to_owned(),
            environment: environment?,
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
