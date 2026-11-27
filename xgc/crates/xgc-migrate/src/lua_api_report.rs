//! 项目迁移的 Lua 名单对照。旧名单随迁移器构建，XGC 名单由当前宿主提供。

use std::collections::BTreeSet;
use std::fmt::Write as _;

const LEGACY_API: &str = include_str!("legacy_lua_api.txt");

/// `registered` 统计实际同名引擎绑定。`missing` 包含仅有 Lua 标准函数的同名项。
pub fn render_lua_api_report(
    engine_globals: &BTreeSet<String>,
    standard_globals: &BTreeSet<String>,
    namespace: &BTreeSet<String>,
) -> String {
    let mut legacy = BTreeSet::new();
    let mut registered_count = 0;
    let mut standard_only_count = 0;
    let mut categories = String::new();
    for line in LEGACY_API.lines().filter(|line| !line.starts_with('#')) {
        let mut names = line.split_whitespace();
        let Some(category) = names.next() else {
            continue;
        };
        let mut registered = Vec::new();
        let mut absent = Vec::new();
        let mut standard_only = Vec::new();
        for name in names {
            legacy.insert(name);
            if engine_globals.contains(name) {
                registered.push(name);
            } else if standard_globals.contains(name) {
                standard_only.push(name);
            } else {
                absent.push(name);
            }
        }
        registered_count += registered.len();
        standard_only_count += standard_only.len();
        write!(
            categories,
            "\n    [{category}] registered=[{}] absent=[{}]",
            registered.join(","),
            absent.join(","),
        )
        .expect("字符串格式化不会失败");
        if !standard_only.is_empty() {
            write!(categories, " standard_only=[{}]", standard_only.join(","))
                .expect("字符串格式化不会失败");
        }
    }

    let extra: Vec<_> = engine_globals
        .iter()
        .map(String::as_str)
        .filter(|name| !legacy.contains(name))
        .collect();
    let namespace: Vec<_> = namespace.iter().map(String::as_str).collect();
    let mut report = format!(
        "  lua-api: legacy={} registered={registered_count} missing={} standard_only={standard_only_count} extra={}\n    extra=[{}] xgc=[{}]{categories}",
        legacy.len(),
        legacy.len() - registered_count,
        extra.len(),
        extra.join(","),
        namespace.join(","),
    );
    report.push_str("\n    missing 包含无绑定的 absent 以及仅有 Lua 标准函数的 standard_only。registered 表示同名绑定。");
    report.push_str(SETTLEMENT);
    report
}

// 结算差异来自旧 World::update 与 XGC Simulation / ScriptRuntime 的执行顺序。
// physics_scripts、legacy_look_at、ui_spawn、anim_event_hook 和 host_order 回归钉住相应路径。
const SETTLEMENT: &str = "
    timing [hooks] 旧端渲染帧先运行 start（delta 为 0），每个固定步执行 fixed_update、物理、碰撞，最后每渲染帧运行一次 update；XGC 在每个固定步先完成物理与 UI，再按实体运行 start（首次）、update、fixed_update，三个钩子在该步读取同一个 delta。
    timing [physics] 旧端 fixed_update 内的 set_velocity 能影响紧接的物理步；XGC 的动态刚体 velocity 在同一钩子可读回新值，Rapier 在下一固定步消费它。
    timing [collision] 旧端在物理之后、渲染帧 update 之前派发碰撞；XGC 在当步的 update、fixed_update 后派发碰撞，碰撞钩子的世界命令与普通钩子同批提交。
    timing [spawn] 旧端生成实体或预制体时立即加入场景；UI 生成亦立即入场景。XGC 的 ui_spawn_* 返回后可在 Lua 内按 ID 查找和编辑，ECS 实体在本步脚本批次提交时加入世界，下一步参加 UI 布局和命中。
    timing [transform] 旧端 set_pos 立即写世界位置，set_rot 立即写局部旋转；XGC 在本步脚本批次提交局部位置和旋转。pos、entity_pos 与 look_at 能读取待提交的局部位置；rot 只返回脚本阶段开始时局部旋转的偏航角。
    timing [animation] 旧端在动画推进的同一渲染帧派发 on_anim_event；XGC 在动画推进后的下一固定步，继普通钩子和碰撞后派发。";
