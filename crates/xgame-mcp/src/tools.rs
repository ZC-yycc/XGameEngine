//! # 六个工具 —— 智能体操纵引擎的最小完备集
//!
//! 刻意小：**读清单 → 读场景 → 改（整体或单实体）→ 验证** 四步闭环，
//! 覆盖"从零建关卡"的全部写路径。能用文本的地方用文本（场景就是 RON 原文），
//! 智能体拿得到、改得动、写得回，不需要我们发明私有格式。
//!
//! # 并发写警告（所有写工具共有）
//!
//! 编辑器**不监视文件**（已知差距），外部写入不会热加载；反过来，编辑器里
//! 未保存的改动也不感知外部写入 —— 谁后保存谁覆盖。约定：**同一时刻只让
//! 一边写**（要么纯 MCP 改，要么纯编辑器改），该警告也写进了工具描述里。
//!
//! # 路径安全
//!
//! `scene` 参数必须是**项目内相对路径**：拒绝 `..`、绝对路径、盘符 ——
//! 这是"智能体能把引擎玩坏到什么程度"的边界之一，宁可拒绝也别写出项目外。

use glam::{EulerRot, Quat, Vec3};
use serde_json::{json, Value};

use xgame_core::{check_project, Entity, ProjectHandle, Scene};

/// 调用失败的两种（MCP 语义不同，别混 —— 详见库文档"错误分两层"）。
#[derive(Debug, PartialEq, Eq)]
pub enum CallError {
    /// 请求点名了不存在的工具：**协议错误**（JSON-RPC -32602）。
    UnknownTool(String),
    /// 工具跑了但没干成：**工具结果错误**（`result.isError: true` + 文本）。
    Failed(String),
}

/// 快捷构造 [`CallError::Failed`]。
fn fail(message: impl Into<String>) -> CallError {
    CallError::Failed(message.into())
}

/// 6 个工具的 JSON Schema（`tools/list` 的返回内容）。
///
/// 描述是**给智能体看的说明书** —— 它没读过我们的代码，只靠这段文字决定
/// 怎么用工具，所以语义（覆盖式、部分更新、并发写警告）都写在描述里。
pub fn schemas() -> Vec<Value> {
    let project_prop = json!({
        "type": "string",
        "description": "项目根目录（含 project.ron 的目录），绝对路径或相对 server 工作目录",
    });
    let scene_prop = json!({
        "type": "string",
        "description": "项目内相对场景路径（如 Scenes/scene.ron；缺 .ron 会自动补全）；缺省 = 清单里的入口场景。绝对路径与 `..` 会被拒绝",
    });
    let write_caveat = "注意：编辑器不监视文件，写入不会热加载到已打开的编辑器；编辑器之后的保存也会覆盖本工具的修改。同一时刻只应有一边在写。";

    vec![
        json!({
            "name": "project_info",
            "description": "读取项目概要：名字、根路径、入口场景、窗口与运行时配置（重力/固定步长）、场景/脚本/预制体清单。探索项目的第一步。",
            "inputSchema": {
                "type": "object",
                "properties": { "project": project_prop },
                "required": ["project"],
            },
        }),
        json!({
            "name": "scene_read",
            "description": format!("读出一个场景的完整 RON 文本（与文件内容同构，可原样传回 scene_write 写回）。返回的就是场景真身，不是摘要。{write_caveat}"),
            "inputSchema": {
                "type": "object",
                "properties": { "project": project_prop, "scene": scene_prop },
                "required": ["project"],
            },
        }),
        json!({
            "name": "scene_write",
            "description": format!("用给定 RON 文本**整体替换**一个场景。先解析验证：坏数据一个字节都不落盘。适合成批改结构（加实体、改层级、动灯光）。{write_caveat}"),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": project_prop,
                    "scene": scene_prop,
                    "ron": { "type": "string", "description": "完整场景的 RON 文本（通常来自 scene_read 的输出）" },
                },
                "required": ["project", "ron"],
            },
        }),
        json!({
            "name": "entity_upsert",
            "description": format!("按名字创建或更新场景里的实体，**部分更新**：只传要改的字段，没传的保持原值（新建实体未指定的字段取默认）。变换是局部坐标；先挂父再改变换，所以给的局部值就是最终值。父子挂接保持世界坐标不变。{write_caveat}"),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": project_prop,
                    "scene": scene_prop,
                    "name": { "type": "string", "description": "实体名字（场景内重名多个会拒绝，消歧后才能动）" },
                    "model": { "type": "string", "description": "模型资产相对路径，或 builtin://cube / builtin://sphere 等内建几何；传入即覆盖" },
                    "scripts": { "type": "array", "items": { "type": "string" }, "description": "挂接的脚本相对路径列表（整表覆盖）" },
                    "position": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "局部位置 [x, y, z]" },
                    "rotation_euler_deg": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "局部旋转，欧拉角（度，XYZ 顺序）" },
                    "scale": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "局部缩放 [x, y, z]" },
                    "parent": { "type": "string", "description": "父实体名字（按名字查找）；空串 = 断开挂到根" },
                },
                "required": ["project", "name"],
            },
        }),
        json!({
            "name": "entity_remove",
            "description": format!("按名字删除一个实体。同名多个实体时拒绝（歧义）。子节点不会被连坐删除，会提升到被删实体的父节点下。{write_caveat}"),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": project_prop,
                    "scene": scene_prop,
                    "name": { "type": "string", "description": "要删除的实体名字" },
                },
                "required": ["project", "name"],
            },
        }),
        json!({
            "name": "project_check",
            "description": "无头验证项目：清单可解析、入口场景可加载、实体引用的模型/脚本在盘上真实存在。返回 JSON 报告（missing_* 列出悬空引用，has_issues 汇总）。改完就跑，自证没改坏 —— 这是 Agent 闭环的验证步。",
            "inputSchema": {
                "type": "object",
                "properties": { "project": project_prop },
                "required": ["project"],
            },
        }),
    ]
}

/// 分发一次工具调用：`Ok(text)` = 成功文本；`Err` 见 [`CallError`]。
pub fn call(name: &str, args: &Value) -> Result<String, CallError> {
    match name {
        "project_info" => project_info(args),
        "scene_read" => scene_read(args),
        "scene_write" => scene_write(args),
        "entity_upsert" => entity_upsert(args),
        "entity_remove" => entity_remove(args),
        "project_check" => project_check(args),
        other => Err(CallError::UnknownTool(other.to_string())),
    }
}

// ---------------------------------------------------------------------------
// 参数与文件的公共处理
// ---------------------------------------------------------------------------

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, CallError> {
    str_arg(args, key)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fail(format!("`{key}` 是必填字符串参数")))
}

fn open_project(args: &Value) -> Result<ProjectHandle, CallError> {
    let root = require_str(args, "project")?;
    ProjectHandle::open(root).map_err(|e| fail(format!("打开项目失败（{root}）: {e}")))
}

/// 解析 `scene` 参数 → 项目内相对路径（保证带 `.ron`）；缺省用入口场景。
fn scene_rel(args: &Value, default: &str) -> Result<String, CallError> {
    let raw = match str_arg(args, "scene").map(str::trim) {
        None => default,
        Some("") => return Err(fail("`scene` 不能为空串")),
        Some(s) => s,
    };
    if raw.contains("..") || raw.contains(':') || raw.starts_with('/') || raw.starts_with('\\') {
        return Err(fail(format!(
            "scene 必须是项目内相对路径（拒绝绝对路径 / `..` 穿越）: {raw}"
        )));
    }
    // Windows 智能体爱给反斜杠：统一成 `/` 再判断扩展名。
    let raw = raw.replace('\\', "/");
    Ok(if raw.to_lowercase().ends_with(".ron") {
        raw
    } else {
        format!("{raw}.ron")
    })
}

fn load_scene_of(project: &ProjectHandle, rel: &str) -> Result<Scene, CallError> {
    project
        .load_scene(rel)
        .map_err(|e| fail(format!("加载场景 {rel} 失败: {e}")))
}

fn save_scene_of(project: &ProjectHandle, rel: &str, scene: &Scene) -> Result<(), CallError> {
    project
        .write_scene(rel, scene)
        .map_err(|e| fail(format!("写场景 {rel} 失败: {e}")))
}

/// `Scene` 真身的 RON 文本（与 `write_scene` 同一序列化配置 → 读写同构）。
fn scene_to_ron(scene: &Scene) -> Result<String, CallError> {
    ron::ser::to_string_pretty(scene, ron::ser::PrettyConfig::default())
        .map_err(|e| fail(format!("场景序列化失败: {e}")))
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("手工构造的 Value 必可序列化")
}

/// 取 `[x, y, z]`；缺省/null = 不改该字段。
fn vec3_arg(args: &Value, key: &str) -> Result<Option<Vec3>, CallError> {
    let Some(value) = args.get(key) else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let arr = value
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or_else(|| fail(format!("`{key}` 必须是三个数的数组 [x, y, z]")))?;
    let mut out = [0.0f32; 3];
    for (item, slot) in arr.iter().zip(out.iter_mut()) {
        let n = item
            .as_f64()
            .ok_or_else(|| fail(format!("`{key}` 里有不是数字的元素")))?
            as f32;
        if !n.is_finite() {
            return Err(fail(format!("`{key}` 含非有限值（NaN / Infinity / 超出 f32 范围）")));
        }
        *slot = n;
    }
    Ok(Some(Vec3::from_array(out)))
}

/// 场景重名数量检查：更新/删除遇到多个同名必须拒绝 —— 智能体按名字寻址，
/// 名字歧义时"猜一个"迟早删错东西。
fn count_named(scene: &Scene, name: &str) -> Result<usize, CallError> {
    let n = scene.entities.iter().filter(|e| e.name == name).count();
    if n > 1 {
        return Err(fail(format!(
            "名为 {name:?} 的实体有 {n} 个，按名字寻址有歧义 —— 先用 scene_read/scene_write 消歧（改名或删到唯一）"
        )));
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// 六个工具
// ---------------------------------------------------------------------------

/// `project_info` —— 清单 + 各目录清单，全走 [`ProjectHandle`] 的现成 API。
fn project_info(args: &Value) -> Result<String, CallError> {
    let project = open_project(args)?;
    let manifest = project.manifest();
    // NaN/超限在入口夹成安全值（sanitized 契约）—— JSON 本身也不许出现 NaN。
    let runtime = manifest.runtime.sanitized();
    let info = json!({
        "name": manifest.name,
        "root": project.root().display().to_string(),
        "format_version": manifest.format_version,
        "entry_scene": manifest.entry_scene,
        "window": {
            "title": manifest.window.title,
            "width": manifest.window.width,
            "height": manifest.window.height,
            "resizable": manifest.window.resizable,
            "vsync": manifest.window.vsync,
        },
        "runtime": {
            "gravity": runtime.gravity,
            "fixed_dt": runtime.fixed_dt,
        },
        "scenes": list_scenes(&project),
        "scripts": project.list_scripts(),
        "prefabs": project.list_prefabs(),
    });
    Ok(pretty(&info))
}

/// `Scenes/`（清单 `scenes_dir`）下的 `*.ron`，项目相对路径、正斜杠、已排序。
/// 目录不存在 → 空表（合法状态：新项目还没落过场景文件）。
fn list_scenes(project: &ProjectHandle) -> Vec<String> {
    let dir = project.scenes_dir();
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("ron") {
                if let Some(rel) = project.relative_path(&path) {
                    out.push(rel.replace('\\', "/"));
                }
            }
        }
    }
    out.sort();
    out
}

/// `scene_read` —— 场景真身的 RON 文本（读写同构，round-trip 安全）。
fn scene_read(args: &Value) -> Result<String, CallError> {
    let project = open_project(args)?;
    let rel = scene_rel(args, &project.manifest().entry_scene)?;
    let scene = load_scene_of(&project, &rel)?;
    scene_to_ron(&scene)
}

/// `scene_write` —— 先解析、后落盘：坏数据一个字节都不写。
fn scene_write(args: &Value) -> Result<String, CallError> {
    let project = open_project(args)?;
    let rel = scene_rel(args, &project.manifest().entry_scene)?;
    let text = require_str(args, "ron")?;
    let scene: Scene = ron::from_str(text)
        .map_err(|e| fail(format!("RON 解析失败，未写入任何文件: {e}")))?;
    save_scene_of(&project, &rel, &scene)?;
    Ok(pretty(&json!({
        "scene": rel,
        "path": project.path(&rel).display().to_string(),
        "entities": scene.entities.len(),
        "lights": scene.lights.len(),
    })))
}

/// `entity_upsert` —— 按名字创建/补丁更新，整场景写回。
fn entity_upsert(args: &Value) -> Result<String, CallError> {
    let project = open_project(args)?;
    let rel = scene_rel(args, &project.manifest().entry_scene)?;
    let mut scene = load_scene_of(&project, &rel)?;
    let name = require_str(args, "name")?;

    count_named(&scene, name)?;
    let existing = scene.find_by_name(name).map(|e| e.id);
    let index = match existing {
        Some(id) => scene.find_entity(id).expect("id 来自本场景内查找"),
        None => {
            scene.add_entity(Entity::new(name));
            scene.entities.len() - 1
        }
    };

    // 顺序契约：**先挂父、再改变换**。`set_parent` 会保世界坐标重算局部
    // 变换，先改再挂会把智能体给的局部值搅掉 —— 反过来，给的值就是最终值。
    if let Some(parent_name) = str_arg(args, "parent") {
        let parent_name = parent_name.trim();
        if parent_name.is_empty() {
            scene.set_parent(index, None);
        } else {
            let pindex = scene
                .entities
                .iter()
                .position(|e| e.name == parent_name)
                .ok_or_else(|| fail(format!("找不到父实体 {parent_name:?}（场景里没有）")))?;
            if pindex == index {
                return Err(fail("实体不能把 `parent` 指向自己"));
            }
            if !scene.set_parent(index, Some(pindex)) {
                return Err(fail("挂接被拒绝：目标是自己的子孙（会形成层级环）"));
            }
        }
    }

    {
        let entity = &mut scene.entities[index];
        if let Some(model) = str_arg(args, "model") {
            entity.model = model.to_string();
        }
        if let Some(scripts) = args.get("scripts") {
            if !scripts.is_null() {
                let arr = scripts
                    .as_array()
                    .ok_or_else(|| fail("`scripts` 必须是字符串数组"))?;
                let mut list = Vec::with_capacity(arr.len());
                for item in arr {
                    list.push(
                        item.as_str()
                            .ok_or_else(|| fail("`scripts` 里有不是字符串的元素"))?
                            .to_string(),
                    );
                }
                entity.scripts = list;
            }
        }
        if let Some(v) = vec3_arg(args, "position")? {
            entity.transform.position = v;
        }
        if let Some(deg) = vec3_arg(args, "rotation_euler_deg")? {
            let r = deg * (std::f32::consts::PI / 180.0);
            entity.transform.rotation = Quat::from_euler(EulerRot::XYZ, r.x, r.y, r.z);
        }
        if let Some(v) = vec3_arg(args, "scale")? {
            entity.transform.scale = v;
        }
    }

    save_scene_of(&project, &rel, &scene)?;
    Ok(pretty(&json!({
        "action": if existing.is_some() { "updated" } else { "created" },
        "name": name,
        "scene": rel,
        "entities": scene.entities.len(),
    })))
}

/// `entity_remove` —— 按名字删单个；`remove_entity` 把子节点提升到被删
/// 实体的父节点（不连坐，子树不散架）。
fn entity_remove(args: &Value) -> Result<String, CallError> {
    let project = open_project(args)?;
    let rel = scene_rel(args, &project.manifest().entry_scene)?;
    let mut scene = load_scene_of(&project, &rel)?;
    let name = require_str(args, "name")?;

    match count_named(&scene, name)? {
        0 => Err(fail(format!("场景里没有名为 {name:?} 的实体"))),
        _ => {
            let id = scene
                .find_by_name(name)
                .expect("count_named 刚确认唯一存在")
                .id;
            scene.remove_entity(id);
            save_scene_of(&project, &rel, &scene)?;
            Ok(pretty(&json!({
                "action": "removed",
                "name": name,
                "scene": rel,
                "entities": scene.entities.len(),
            })))
        }
    }
}

/// `project_check` —— 复用 xgame-core 的检查（与编辑器 `--check` 同一份逻辑）。
fn project_check(args: &Value) -> Result<String, CallError> {
    let project = open_project(args)?;
    let report = check_project(project.root()).map_err(|e| fail(format!("检查失败: {e}")))?;
    Ok(pretty(&json!({
        "entities": report.entities,
        "lights": report.lights,
        "script_refs": report.script_refs,
        "missing_models": report.missing_models,
        "missing_scripts": report.missing_scripts,
        "has_issues": report.has_issues(),
        "render": report.render(),
    })))
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时项目（用完记得删）。
    fn temp_project(tag: &str) -> ProjectHandle {
        let base = std::env::temp_dir().join(format!(
            "xgame_mcp_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        ProjectHandle::create(&base, &format!("Mcp{tag}")).expect("创建临时项目")
    }

    fn cleanup(project: &ProjectHandle) {
        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 调工具：自动注入 `project` 参数（测试里没人想手写五遍绝对路径）。
    fn run(project: &ProjectHandle, name: &str, mut args: Value) -> Result<String, CallError> {
        let obj = args.as_object_mut().expect("测试参数必须是对象");
        obj.insert("project".into(), json!(project.root().display().to_string()));
        call(name, &args)
    }

    /// 拿入口场景（测试断言用）。
    fn entry_scene(project: &ProjectHandle) -> Scene {
        project.load_entry_scene().expect("入口场景应可加载")
    }

    /// project_info：清单字段与场景文件列表都到位，路径统一正斜杠。
    #[test]
    fn project_info_reports_manifest_and_files() {
        let project = temp_project("info");
        let text = run(&project, "project_info", json!({})).expect("project_info 应成功");
        let info: Value = serde_json::from_str(&text).expect("返回必须是 JSON");
        assert_eq!(info["name"], "Mcpinfo");
        assert_eq!(info["entry_scene"], project.manifest().entry_scene);
        let scenes: Vec<&str> = info["scenes"]
            .as_array()
            .expect("scenes 数组")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            scenes.contains(&"Scenes/scene.ron"),
            "默认场景应在列表里: {scenes:?}"
        );
        assert!(info["runtime"]["fixed_dt"].as_f64().unwrap() > 0.0);
        cleanup(&project);
    }

    /// scene_read 返回的文本必须能被 RON 解析回场景（读写同构是承诺）。
    #[test]
    fn scene_read_round_trips() {
        let project = temp_project("read");
        let text = run(&project, "scene_read", json!({})).expect("scene_read 应成功");
        let scene: Scene = ron::from_str(&text).expect("返回文本必须是合法场景 RON");
        assert!(!scene.entities.is_empty(), "默认场景至少一个实体");
        cleanup(&project);
    }

    /// 坏 RON：报错且**一个字节都不落盘**（先解析后写的契约）。
    #[test]
    fn scene_write_rejects_garbage_without_touching_file() {
        let project = temp_project("garbage");
        let before = std::fs::read_to_string(project.entry_scene_path()).expect("读原文件");
        let err = run(
            &project,
            "scene_write",
            json!({ "ron": "this is (( not ron" }),
        )
        .expect_err("坏 RON 必须被拒绝");
        match err {
            CallError::Failed(msg) => assert!(msg.contains("未写入"), "{msg}"),
            other => panic!("应是 Failed，得到 {other:?}"),
        }
        let after = std::fs::read_to_string(project.entry_scene_path()).expect("读原文件");
        assert_eq!(before, after, "被拒的写请求不能改动文件");
        cleanup(&project);
    }

    /// scene_read → 改 → scene_write 的纯文本往返。
    #[test]
    fn scene_write_accepts_modified_text() {
        let project = temp_project("roundtrip");
        let mut scene = entry_scene(&project);
        scene.entities.push(Entity::new("AgentAdded"));
        let ron_text = scene_to_ron(&scene).expect("序列化");
        let out = run(&project, "scene_write", json!({ "ron": ron_text })).expect("应写入");
        let reply: Value = serde_json::from_str(&out).expect("JSON");
        assert!(reply["entities"].as_u64().unwrap() >= 2);
        assert!(entry_scene(&project).find_by_name("AgentAdded").is_some());
        cleanup(&project);
    }

    /// 路径穿越与绝对路径必须被拒绝 —— 智能体的破坏半径止于项目目录。
    #[test]
    fn scene_path_traversal_rejected() {
        let project = temp_project("traversal");
        for bad in ["../evil.ron", "C:/abs/evil.ron", "/abs/evil.ron"] {
            let err = run(&project, "scene_read", json!({ "scene": bad }))
                .expect_err("路径穿越必须被拒");
            match err {
                CallError::Failed(msg) => assert!(msg.contains("相对路径"), "{bad}: {msg}"),
                other => panic!("应是 Failed，得到 {other:?}"),
            }
        }
        // 反斜杠的"项目内"路径不穿越，反而是合法的（统一成 / 后补 .ron）。
        let ok = run(&project, "scene_read", json!({ "scene": "Scenes\\scene" }));
        assert!(ok.is_ok(), "{ok:?}");
        cleanup(&project);
    }

    /// entity_upsert：新建 + 全字段；再部分更新只动一个字段，其余保持。
    #[test]
    fn entity_upsert_creates_then_patches() {
        let project = temp_project("upsert");
        let out = run(
            &project,
            "entity_upsert",
            json!({
                "name": "Crate",
                "model": "builtin://cube",
                "position": [1.0, 2.0, 3.0],
                "rotation_euler_deg": [0.0, 90.0, 0.0],
                "scale": [2.0, 2.0, 2.0],
            }),
        )
        .expect("新建应成功");
        let reply: Value = serde_json::from_str(&out).expect("JSON");
        assert_eq!(reply["action"], "created");

        let scene = entry_scene(&project);
        let e = scene.find_by_name("Crate").expect("新实体应在场景里");
        assert_eq!(e.model, "builtin://cube");
        assert_eq!(e.transform.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(e.transform.scale, Vec3::new(2.0, 2.0, 2.0));
        assert!(
            (e.transform.rotation.angle_between(Quat::IDENTITY)
                - std::f32::consts::FRAC_PI_2)
                .abs()
                < 1e-4,
            "90° 应转对，得到 {:?}",
            e.transform.rotation
        );

        // 部分更新：只改 scale，位置与旋转必须原样保留。
        run(
            &project,
            "entity_upsert",
            json!({ "name": "Crate", "scale": [3.0, 3.0, 3.0] }),
        )
        .expect("补丁更新应成功");
        let scene = entry_scene(&project);
        let e = scene.find_by_name("Crate").expect("仍在");
        assert_eq!(e.transform.position, Vec3::new(1.0, 2.0, 3.0), "未传字段不能被动");
        assert_eq!(e.transform.scale, Vec3::new(3.0, 3.0, 3.0));
        cleanup(&project);
    }

    /// 父子挂接：先挂父再改变换 → 给的局部值是最终值；世界坐标 = 父 + 局部。
    #[test]
    fn entity_upsert_parenting() {
        let project = temp_project("parent");
        run(
            &project,
            "entity_upsert",
            json!({ "name": "Root", "position": [10.0, 0.0, 0.0] }),
        )
        .expect("建父");
        run(
            &project,
            "entity_upsert",
            json!({ "name": "Child", "position": [1.0, 0.0, 0.0], "parent": "Root" }),
        )
        .expect("建子");

        let scene = entry_scene(&project);
        let child_idx = scene
            .entities
            .iter()
            .position(|e| e.name == "Child")
            .expect("Child 在");
        let root = scene.find_by_name("Root").expect("Root 在");
        let child = &scene.entities[child_idx];
        assert_eq!(child.parent, Some(root.id), "父子关系应建立");
        assert_eq!(
            child.transform.position,
            Vec3::new(1.0, 0.0, 0.0),
            "局部值 = 智能体给的值（先挂父再改变换的顺序契约）"
        );
        let world = scene.world_matrix(child_idx);
        assert!(
            (world.w_axis.x - 11.0).abs() < 1e-4,
            "世界坐标 = 父(10) + 局部(1)，得到 {}",
            world.w_axis.x
        );

        // 找不到的父 → 拒绝；自己当父 → 拒绝。
        let err = run(
            &project,
            "entity_upsert",
            json!({ "name": "Child", "parent": "Nope" }),
        )
        .expect_err("不存在的父必须拒绝");
        assert!(matches!(&err, CallError::Failed(m) if m.contains("找不到父实体")), "{err:?}");
        let err = run(
            &project,
            "entity_upsert",
            json!({ "name": "Child", "parent": "Child" }),
        )
        .expect_err("自己当父必须拒绝");
        assert!(matches!(&err, CallError::Failed(m) if m.contains("自己")), "{err:?}");
        cleanup(&project);
    }

    /// 重名歧义：更新与删除都必须拒绝（按名字寻址，猜一个迟早出事）。
    #[test]
    fn ambiguous_names_refused() {
        let project = temp_project("ambiguous");
        let mut scene = entry_scene(&project);
        scene.entities.push(Entity::new("Dup"));
        scene.entities.push(Entity::new("Dup"));
        let rel = project.manifest().entry_scene.clone();
        project.write_scene(&rel, &scene).expect("写入重名场景");

        for tool in ["entity_upsert", "entity_remove"] {
            let err = run(&project, tool, json!({ "name": "Dup" }))
                .expect_err("重名必须被拒");
            assert!(
                matches!(&err, CallError::Failed(m) if m.contains("歧义")),
                "{tool}: {err:?}"
            );
        }
        cleanup(&project);
    }

    /// 删除：成功路径 + 不存在报错 + 子节点提升（不连坐）。
    #[test]
    fn entity_remove_semantics() {
        let project = temp_project("remove");
        run(&project, "entity_upsert", json!({ "name": "Parent" })).expect("建父");
        run(
            &project,
            "entity_upsert",
            json!({ "name": "Leaf", "parent": "Parent" }),
        )
        .expect("建子");

        let err = run(&project, "entity_remove", json!({ "name": "Ghost" }))
            .expect_err("不存在的实体删除应报错");
        assert!(matches!(&err, CallError::Failed(m) if m.contains("没有名为")), "{err:?}");

        run(&project, "entity_remove", json!({ "name": "Parent" })).expect("删父");
        let scene = entry_scene(&project);
        assert!(
            scene.find_by_name("Parent").is_none(),
            "父已删"
        );
        let leaf = scene.find_by_name("Leaf").expect("子节点不连坐，必须还在");
        assert_eq!(leaf.parent, None, "子节点提升到根");
        cleanup(&project);
    }

    /// project_check：默认项目干净；加悬空引用后被点名。
    #[test]
    fn project_check_tool_reports_dangling() {
        let project = temp_project("check");
        let out = run(&project, "project_check", json!({})).expect("检查应成功");
        let report: Value = serde_json::from_str(&out).expect("JSON");
        assert_eq!(report["has_issues"], false, "{}", report["render"]);

        let mut scene = entry_scene(&project);
        scene
            .entities
            .push(Entity::new("Ghost").with_model("Nowhere/model.glb"));
        let rel = project.manifest().entry_scene.clone();
        project.write_scene(&rel, &scene).expect("写入悬空引用");

        let out = run(&project, "project_check", json!({})).expect("检查应成功");
        let report: Value = serde_json::from_str(&out).expect("JSON");
        assert_eq!(report["has_issues"], true);
        assert_eq!(report["missing_models"].as_array().unwrap().len(), 1);
        cleanup(&project);
    }

    /// 未知工具名走 UnknownTool 变体（协议层转成 -32602，见 lib 测试）。
    #[test]
    fn unknown_tool_is_distinct_error() {
        let project = temp_project("unknown");
        let err = run(&project, "definitely_not_a_tool", json!({}))
            .expect_err("未知工具必须报错");
        assert_eq!(err, CallError::UnknownTool("definitely_not_a_tool".into()));
        cleanup(&project);
    }
}
