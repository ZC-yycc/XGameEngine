//! # 八个工具 —— 智能体操纵引擎的最小完备集
//!
//! 刻意小：**读清单 → 读场景 → 改（整体或单实体）→ 验证** 四步闭环，
//! 覆盖"从零建关卡"的全部写路径。能用文本的地方用文本（场景就是 RON 原文），
//! 智能体拿得到、改得动、写得回，不需要我们发明私有格式。验证有两条腿：
//! `project_check` 管静态引用（模型 / 脚本在不在盘上），`run_headless` 管
//! 动态实跑（真的跑 N 帧、脚本真的没报错）。
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

/// 一次工具调用的输出：一段文本 +（可选的）随附图片。
///
/// 八个工具里只有 `run_headless`（`screenshot: true`）会带图：无头渲染一帧
/// 直接把 PNG 以 MCP `image` 内容块回给客户端 —— 视觉模型看到的是画面本身，
/// 而不是"渲染没报错"的口头保证。图片在协议层是独立 content 块（见
/// `xgame-mcp` 库的 `call_tool`），这里只负责编好 base64。
#[derive(Debug)]
pub struct ToolOutput {
    pub text: String,
    pub images: Vec<ToolImage>,
}

/// 一张随附图片（MCP `image` 内容块的内容）。
#[derive(Debug)]
pub struct ToolImage {
    /// MIME 类型，如 `image/png`。
    pub mime: &'static str,
    /// 标准 base64（不带 data URL 前缀）。
    pub base64: String,
}

impl ToolOutput {
    /// 只有文本（绝大多数工具）。
    fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }
}

/// 8 个工具的 JSON Schema（`tools/list` 的返回内容）。
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
            "name": "asset_list",
            "description": "列出项目里已登记的资产（有 .meta 的模型 / 纹理 / 音频 / 脚本 / 预制体），返回 JSON：每项的路径、类型、GUID 与导入设置，外加资产库诊断（缺 .meta / 孤儿 .meta / 坏 .meta / GUID 冲突）。智能体先用它「看见」资产树，再用路径去写 entity_upsert 的 model / scripts 参数 —— 不需要猜项目里有什么。只读，不生成任何文件。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": project_prop,
                    "kind": { "type": "string", "description": "只看某一类：Model / Texture / Audio / Script / Prefab / Shader（不传 = 全部）" },
                },
                "required": ["project"],
            },
        }),
        json!({
            "name": "project_check",
            "description": "无头验证项目：清单可解析、入口场景可加载、实体引用的模型/脚本在盘上真实存在，并检查资产身份证（缺 .meta / 孤儿 .meta / GUID 冲突 / 悬空 GUID）。返回 JSON 报告（missing_* / dangling_guids / missing_metas 列出问题，has_issues 汇总）。改完就跑，自证没改坏 —— 这是 Agent 闭环的验证步。",
            "inputSchema": {
                "type": "object",
                "properties": { "project": project_prop },
                "required": ["project"],
            },
        }),
        json!({
            "name": "run_headless",
            "description": "无头跑项目：不开窗口把项目模拟 N 帧（默认 300），逐帧固定 dt，返回 JSON 报告 —— 帧耗时分布、实体/刚体计数、脚本与组件日志（含 error 样本）、非有限变换检测、音频状态。project_check 的自证搭档：它管静态引用，这个管「真的跑得动」。report.ok=false（退出码 2）表示跑完但观察到问题（脚本 error 日志等），报告照常返回；项目打不开才是工具失败。include_scene=true 时另带回跑完后的场景 RON（观察模拟结果：物理落点、脚本改过的变换）。screenshot=true 时离屏渲染一帧（默认取景 + 顺光机位，1280x720）并以图片块返回 —— 接得上模型的视觉通道就能直接看画面。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": project_prop,
                    "frames": { "type": "integer", "minimum": 1, "maximum": 100000, "description": "跑多少帧（默认 300；60Hz 下 300 帧 = 5 秒模拟）" },
                    "dt": { "type": "number", "description": "每帧推进的秒数（默认 = 项目固定步长；非必要不传 —— 默认值才保证确定性）" },
                    "include_scene": { "type": "boolean", "description": "true 时在返回信封里附上跑完后的场景 RON（默认 false，响应更小）" },
                    "screenshot": { "type": "boolean", "description": "true 时跑完额外离屏渲染一帧，PNG 以图片内容块随结果返回（需要 GPU；默认 false）" },
                },
                "required": ["project"],
            },
        }),
    ]
}

/// 分发一次工具调用：`Ok` 是工具输出（文本，可能带图）；`Err` 见 [`CallError`]。
pub fn call(name: &str, args: &Value) -> Result<ToolOutput, CallError> {
    match name {
        "project_info" => project_info(args).map(ToolOutput::text),
        "scene_read" => scene_read(args).map(ToolOutput::text),
        "scene_write" => scene_write(args).map(ToolOutput::text),
        "entity_upsert" => entity_upsert(args).map(ToolOutput::text),
        "entity_remove" => entity_remove(args).map(ToolOutput::text),
        "project_check" => project_check(args).map(ToolOutput::text),
        "asset_list" => asset_list(args).map(ToolOutput::text),
        "run_headless" => run_headless(args),
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
// 八个工具
// ---------------------------------------------------------------------------

/// `project_info` —— 清单 + 各目录清单，全走 [`ProjectHandle`] 的现成 API。
fn project_info(args: &Value) -> Result<String, CallError> {
    let project = open_project(args)?;
    let manifest = project.manifest();
    // NaN/超限在入口夹成安全值（sanitized 契约）—— JSON 本身也不许出现 NaN。
    let runtime = manifest.runtime.sanitized();
    let render = manifest.render.sanitized();
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
        // 画面设置（色调映射 / 曝光 / 泛光）：智能体改画面就是改这几项，
        // 读报告时也知道 `run_headless` 截的图是按什么出的。
        "render": {
            "exposure": render.exposure,
            "tone_map": render.tone_map,
            "bloom": {
                "intensity": render.bloom.intensity,
                "threshold": render.bloom.threshold,
                "knee": render.bloom.knee,
            },
            "sky": {
                "enabled": render.sky.enabled,
                "intensity": render.sky.intensity,
                "zenith": render.sky.zenith,
                "horizon": render.sky.horizon,
                "ground": render.sky.ground,
                "sun_size": render.sky.sun_size,
                "sun_intensity": render.sky.sun_intensity,
            },
            "fxaa": {
                "enabled": render.fxaa.enabled,
            },
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
        "audio_refs": report.audio_refs,
        "assets": report.assets,
        "missing_models": report.missing_models,
        "missing_scripts": report.missing_scripts,
        "missing_audio": report.missing_audio,
        "missing_prefabs": report.missing_prefabs,
        "dangling_guids": report.dangling_guids,
        "missing_metas": report.missing_metas,
        "orphan_metas": report.orphan_metas,
        "invalid_metas": report.invalid_metas,
        "guid_conflicts": report.guid_conflicts,
        "has_issues": report.has_issues(),
        "issue_count": report.issue_count(),
        "render": report.render(),
    })))
}

/// `asset_list` —— 智能体的"资产树视野"（P0 资产管线的读侧）。
///
/// 只读：打开项目会扫一次资产库（不写 `.meta`，那是编辑器的活），
/// 因此 `missing_meta` 会如实列出来 —— 智能体据此决定是报告问题还是
/// 让用户跑一次 `--migrate-assets`。
fn asset_list(args: &Value) -> Result<String, CallError> {
    let project = open_project(args)?;
    let kind_filter = str_arg(args, "kind")
        .map(|k| k.trim().to_lowercase())
        .filter(|k| !k.is_empty());

    let assets = project.assets();
    let mut items = Vec::new();
    for (guid, record) in assets.sorted_entries() {
        let kind_label = format!("{:?}", record.kind).to_lowercase();
        if let Some(filter) = &kind_filter {
            if &kind_label != filter {
                continue;
            }
        }
        items.push(json!({
            "path": record.path,
            "kind": record.kind,
            "guid": guid.to_string(),
            "import": record.import,
        }));
    }

    let diagnostics = assets.diagnostics();
    let missing_meta = diagnostics.missing_meta.clone();
    let orphan_meta = diagnostics.orphan_meta.clone();
    let invalid_meta: Vec<Value> = diagnostics
        .invalid_meta
        .iter()
        .map(|bad| json!({ "path": bad.path, "reason": bad.reason }))
        .collect();
    let conflicts: Vec<Value> = diagnostics
        .conflicts
        .iter()
        .map(|c| json!({ "guid": c.guid.to_string(), "paths": c.paths }))
        .collect();
    drop(assets);

    Ok(pretty(&json!({
        "count": items.len(),
        "assets": items,
        "diagnostics": {
            "missing_meta": missing_meta,
            "orphan_meta": orphan_meta,
            "invalid_meta": invalid_meta,
            "guid_conflicts": conflicts,
            "has_issues": !(missing_meta.is_empty()
                && orphan_meta.is_empty()
                && invalid_meta.is_empty()
                && conflicts.is_empty()),
        },
    })))
}

/// `run_headless` —— 把"无头跑项目"透传给编辑器二进制（`--headless`）。
///
/// # 为什么是 spawn
///
/// 无头运行的实现住在编辑器二进制里（`--check` 的邻居）：跑的是
/// `World::update`，与编辑器 ▶ Play、独立游戏同一段代码。MCP 侧只做透传 ——
/// 两处共用一条进化路径，不做第二份帧循环。server 装在哪编辑器就在哪
/// （`target/debug/` 与 `dist/` 都是同目录布局，见 [`find_editor_exe`]）。
///
/// # 校验的归属
///
/// `frames` / `dt` 的合法性由编辑器侧（`headless::parse_args`）**唯一**负责，
/// 这里只做 JSON 类型检查 —— 两处都做校验必然漂移，报错文案也会不同步。
fn run_headless(args: &Value) -> Result<ToolOutput, CallError> {
    let root = require_str(args, "project")?;
    let exe_dir = std::env::current_exe()
        .map_err(|e| fail(format!("拿不到 server 可执行文件路径: {e}")))?
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| fail("server 可执行文件没有上级目录（不该发生）"))?;
    let exe = find_editor_exe(&exe_dir).ok_or_else(|| {
        fail(format!(
            "找不到 xgame-editor 可执行文件（找过 {} 及其上一级）—— 它应与 xgame-mcp 装在同一目录（target/debug 或 dist/）",
            exe_dir.display()
        ))
    })?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--headless");
    // MCP server 常被 GUI 客户端（无控制台）拉起：不给子进程 CREATE_NO_WINDOW，
    // 每次工具调用都会闪出一个控制台窗口（stdout/stderr 管道不受影响）。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    if let Some(v) = args.get("frames").filter(|v| !v.is_null()) {
        let n = v.as_u64().ok_or_else(|| fail("`frames` 必须是正整数"))?;
        cmd.arg("--frames").arg(n.to_string());
    }
    if let Some(v) = args.get("dt").filter(|v| !v.is_null()) {
        let dt = v.as_f64().ok_or_else(|| fail("`dt` 必须是数字（秒）"))?;
        cmd.arg("--dt").arg(dt.to_string());
    }
    let include_scene = args
        .get("include_scene")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let dump_path = include_scene.then(|| temp_report_path("headless-scene", "ron"));
    if let Some(path) = &dump_path {
        cmd.arg("--dump-scene").arg(path);
    }
    let want_shot = args
        .get("screenshot")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // 截图落在临时 PNG，读完即删 —— 图会以内联图片块进信封，不需要留盘。
    let shot_path = want_shot.then(|| temp_report_path("headless-shot", "png"));
    if let Some(path) = &shot_path {
        cmd.arg("--screenshot").arg(path);
    }
    cmd.arg(root);

    let output = cmd
        .output()
        .map_err(|e| fail(format!("启动 {} 失败: {e}", exe.display())))?;
    let stderr = tail_chars(&String::from_utf8_lossy(&output.stderr), 2000);
    let exit = output.status.code();
    match exit {
        Some(0 | 2) => {}
        Some(1) => return Err(fail(format!("无头运行失败（退出码 1）: {stderr}"))),
        other => {
            return Err(fail(format!(
                "无头运行异常退出（退出码 {other:?}）: {stderr}"
            )))
        }
    }

    let report: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| fail(format!("无头运行输出不是 JSON: {e}")))?;
    let mut envelope = json!({ "exit_code": exit, "report": report });
    if let Some(path) = &dump_path {
        // `include_scene` 是给智能体的承诺：读不到就整体失败，别给半个信封。
        let text = std::fs::read_to_string(path)
            .map_err(|e| fail(format!("读场景落盘 {} 失败: {e}", path.display())))?;
        let _ = std::fs::remove_file(path);
        envelope["scene_ron"] = json!(text);
    }
    let mut images = Vec::new();
    if let Some(path) = &shot_path {
        // 读不到截图 = 编辑器宣称画了却没落盘：整体失败，别给半个信封。
        let bytes = std::fs::read(path)
            .map_err(|e| fail(format!("读截图 {} 失败: {e}", path.display())))?;
        let _ = std::fs::remove_file(path);
        // 报告的 `screenshot` 节带实际机位与尺寸（ShotInfo）；信封这里只留
        // "图片块里有几个字节"这个元数据，图本身在 content 块里。
        envelope["screenshot"] = json!({ "bytes": bytes.len() });
        images.push(ToolImage {
            mime: "image/png",
            base64: base64_encode(&bytes),
        });
    }
    Ok(ToolOutput {
        text: pretty(&envelope),
        images,
    })
}

/// 在 `dir` 及其上一级找 `xgame-editor` 可执行文件。
///
/// 上一级是给 `cargo test` 兜底：测试二进制在 `target/<profile>/deps/` 下跑，
/// 编辑器 exe 在它的上一级（`cargo build` 的常规产物布局）。
fn find_editor_exe(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let name = format!("xgame-editor{}", std::env::consts::EXE_SUFFIX);
    for base in [dir.to_path_buf(), dir.join("..")] {
        let candidate = base.join(&name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 进程内唯一的临时文件路径（pid + 纳秒）；调用方读完即删。
fn temp_report_path(tag: &str, ext: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "xgame-mcp-{tag}-{}-{}.{ext}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

/// 标准 base64（RFC 4648，带 `=` 填充）—— 只为内联截图存在。
///
/// 没引 `base64` crate：全仓库只此一处编码、且是单向（解码从不需要），
/// 30 行查表法配一组已知向量测试足够钉住。
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let third = *chunk.get(2).unwrap_or(&0);
        let n = ((chunk[0] as u32) << 16) | ((*chunk.get(1).unwrap_or(&0) as u32) << 8) | third as u32;
        out.push(TABLE[(n >> 18 & 63) as usize] as char);
        out.push(TABLE[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 取文本尾巴（≤ `max` 个字符，按字符切不按字节切）。
///
/// 孩子的失败原因在 stderr 的最后几行；如果调用环境的 `RUST_LOG` 开着，
/// 头部会混进成片日志 —— 原样转发等于拿噪声灌智能体的上下文。
fn tail_chars(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    if count <= max {
        return trimmed.to_string();
    }
    let skipped = count - max;
    format!(
        "…（前 {skipped} 字符略）{}",
        trimmed.chars().skip(skipped).collect::<String>()
    )
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
    /// 只返回文本部分；要断言随附图片用 [`run_full`]。
    fn run(project: &ProjectHandle, name: &str, args: Value) -> Result<String, CallError> {
        run_full(project, name, args).map(|out| out.text)
    }

    /// 调工具并返回完整输出（文本 + 随附图片块）。
    fn run_full(
        project: &ProjectHandle,
        name: &str,
        mut args: Value,
    ) -> Result<ToolOutput, CallError> {
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
        // 画面设置节：默认 ACES + 曝光 1.0 + 泛光关（智能体改画面读的三项）。
        assert_eq!(info["render"]["tone_map"], "Aces");
        assert_eq!(info["render"]["exposure"], 1.0);
        assert_eq!(info["render"]["bloom"]["intensity"], 0.0);
        assert_eq!(info["render"]["bloom"]["threshold"], 1.0);
        // f32 → f64 是精确加宽（0.6000000238…），别拿十进制字面量比。
        assert_eq!(info["render"]["bloom"]["knee"], f64::from(0.6f32));
        // 天空默认关；字段完整，智能体据此判断截图的背景来自天空还是清屏色。
        assert_eq!(info["render"]["sky"]["enabled"], false);
        assert_eq!(info["render"]["sky"]["intensity"], 1.0);
        assert_eq!(info["render"]["sky"]["sun_size"], f64::from(1.5f32));
        assert_eq!(
            info["render"]["sky"]["zenith"]
                .as_array()
                .expect("zenith 数组")
                .len(),
            3
        );
        // FXAA 默认关；字段完整，智能体据此解释截图的边缘像素差异。
        assert_eq!(info["render"]["fxaa"]["enabled"], false);
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

    // -- run_headless（spawn 编辑器二进制）---------------------------------

    /// 嵌套构建"当前"编辑器二进制并返回其路径。
    ///
    /// 与 `xgame/tests/plugin_loading.rs` 同一套纪律：`cargo test` 不会把 bin
    /// 目标装成 exe，`target/*/xgame-editor.exe` 可能停在上次 `cargo build` 的
    /// 那一刻 —— 而过期的编辑器**不认 `--headless`**：旧入口会把它当未知参数
    /// 继续走 eframe，弹出一个窗口把测试进程挂死。所以先无条件嵌套跑一次
    /// `cargo build -p xgame-editor`（指纹新鲜时零点几秒），再拼路径。
    fn editor_exe_for_tests() -> std::path::PathBuf {
        static BUILT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        BUILT.get_or_init(|| {
            let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(|p| p.parent())
                .expect("crates/xgame-mcp 的上两级 = 仓库根");
            let cargo = match std::env::var_os("CARGO") {
                Some(c) => std::path::PathBuf::from(c),
                None => std::path::PathBuf::from("cargo"),
            };
            let mut cmd = std::process::Command::new(&cargo);
            cmd.args(["build", "-p", "xgame-editor"])
                .current_dir(repo_root)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if !cfg!(debug_assertions) {
                cmd.arg("--release");
            }
            let mut child = cmd
                .spawn()
                .unwrap_or_else(|e| panic!("无法启动 {cargo:?} 构建编辑器：{e}"));
            // 超时防"与外层 cargo 抢目标目录锁"无限挂起（同 plugin_loading）。
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) if status.success() => break,
                    Ok(Some(status)) => panic!("`cargo build -p xgame-editor` 失败（{status}）"),
                    Ok(None) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                    Ok(None) => {
                        let _ = child.kill();
                        panic!(
                            "嵌套 `cargo build -p xgame-editor` 超时 —— 多半是与外层 cargo \
                             抢目标目录锁；请手动构建后再跑本测试"
                        );
                    }
                    Err(e) => panic!("等待编辑器构建失败：{e}"),
                }
            }
        });

        // 测试二进制在 target/<profile>/deps/ 下，编辑器 exe 在其上一级。
        let exe = std::env::current_exe()
            .expect("拿测试可执行文件路径")
            .parent()
            .expect("测试二进制有上级目录")
            .join("..")
            .join(format!("xgame-editor{}", std::env::consts::EXE_SUFFIX));
        assert!(exe.is_file(), "构建后仍未找到编辑器：{}", exe.display());
        exe
    }

    /// run_headless 干净跑：信封 = 退出码 0 + 报告 ok + 帧数透传；默认不带场景快照。
    #[test]
    fn run_headless_runs_a_clean_project() {
        editor_exe_for_tests(); // 先保证产物新鲜（过期编辑器不认 --headless）
        let project = temp_project("headless");
        let out = run(&project, "run_headless", json!({ "frames": 30 })).expect("应成功");
        let envelope: Value = serde_json::from_str(&out).expect("信封必须是 JSON");
        assert_eq!(envelope["exit_code"], 0);
        assert_eq!(envelope["report"]["frames"], 30);
        assert_eq!(envelope["report"]["ok"], true, "{}", envelope["report"]);
        assert!(
            envelope.get("scene_ron").is_none(),
            "include_scene 默认关闭，不该带场景快照"
        );
        cleanup(&project);
    }

    /// 脚本 error：跑完但脏 —— 退出码 2、report.ok=false、error 计数透传。
    /// **退出码 2 不是工具失败**，问题表达在报告里（智能体读报告定位）。
    #[test]
    fn run_headless_reports_script_errors_in_envelope() {
        editor_exe_for_tests();
        let project = temp_project("headless_dirty");
        let scripts = project.scripts_dir();
        std::fs::create_dir_all(&scripts).expect("建 Scripts 目录");
        std::fs::write(
            scripts.join("boom.lua"),
            "function update(ctx)\n    error(ctx, 'boom')\nend\n",
        )
        .expect("写脚本");
        let mut scene = entry_scene(&project);
        scene
            .entities
            .push(Entity::new("Boomer").with_script("Scripts/boom.lua"));
        let rel = project.manifest().entry_scene.clone();
        project.write_scene(&rel, &scene).expect("写场景");

        let out = run(&project, "run_headless", json!({ "frames": 5 })).expect("跑完即成功");
        let envelope: Value = serde_json::from_str(&out).expect("JSON");
        assert_eq!(envelope["exit_code"], 2);
        assert_eq!(envelope["report"]["ok"], false);
        assert_eq!(envelope["report"]["logs"]["error"], 5);
        assert!(
            !envelope["report"]["error_samples"]
                .as_array()
                .expect("error_samples 数组")
                .is_empty()
        );
        cleanup(&project);
    }

    /// include_scene=true：信封带回跑完后的场景 RON，且能读回、实体在。
    #[test]
    fn run_headless_can_include_scene_snapshot() {
        editor_exe_for_tests();
        let project = temp_project("headless_scene");
        let out = run(
            &project,
            "run_headless",
            json!({ "frames": 3, "include_scene": true }),
        )
        .expect("应成功");
        let envelope: Value = serde_json::from_str(&out).expect("JSON");
        let ron_text = envelope["scene_ron"]
            .as_str()
            .expect("include_scene=true 必须带 scene_ron");
        let scene: Scene = ron::from_str(ron_text).expect("scene_ron 必须能读回");
        assert!(scene.find_by_name("Main Camera").is_some());
        cleanup(&project);
    }

    /// base64：RFC 4648 的已知向量（含填充的三种长度）。编码错了图片块就
    /// 是坏数据 —— 客户端解不开只会看到"图打不开"，极难联想到是编码 bug。
    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // 每 3 字节 → 4 字符（含填充）的定长关系，随机长度扫一遍。
        for len in 0..64usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            assert_eq!(base64_encode(&bytes).len(), len.div_ceil(3) * 4, "len={len}");
        }
    }

    /// screenshot=true：信封带字节数、随附块是一张真 PNG（签名开头的 base64）。
    #[test]
    fn run_headless_screenshot_returns_inline_image() {
        editor_exe_for_tests();
        let project = temp_project("headless_shot");
        let out = match run_full(&project, "run_headless", json!({ "frames": 3, "screenshot": true }))
        {
            Ok(out) => out,
            // 无 GPU 环境跳过（与渲染层测试同一套约定）：截图拿下适配器失败
            // 走退出码 1，错误文本里带"GPU 适配器"。
            Err(CallError::Failed(m)) if m.contains("GPU") || m.contains("适配器") => {
                eprintln!("跳过 run_headless_screenshot_returns_inline_image：{m}");
                cleanup(&project);
                return;
            }
            Err(e) => panic!("截图运行不该失败：{e:?}"),
        };
        let envelope: Value = serde_json::from_str(&out.text).expect("信封必须是 JSON");
        assert_eq!(envelope["exit_code"], 0);
        let bytes = envelope["screenshot"]["bytes"].as_u64().expect("截图字节数");
        assert!(bytes > 1000, "PNG 太小（{bytes} 字节），多半没画出东西");
        // 报告的 screenshot 节带实际机位（自动取景的结果必须是可复现的具体值）。
        let report_shot = &envelope["report"]["screenshot"];
        assert_eq!(report_shot["width"], 1280);
        assert_eq!(report_shot["height"], 720);

        assert_eq!(out.images.len(), 1, "必须恰好一张图");
        let image = &out.images[0];
        assert_eq!(image.mime, "image/png");
        // `iVBORw0KGgo` = PNG 魔数（89 50 4E 47 0D 0A 1A 0A）的 base64 前缀：
        // 不解码也能确认块里装的是 PNG 而不是随便什么字节。
        assert!(
            image.base64.starts_with("iVBORw0KGgo"),
            "图片块不是 PNG：{}…",
            &image.base64[..image.base64.len().min(24)]
        );
        assert_eq!(image.base64.len(), (bytes as usize).div_ceil(3) * 4);
        cleanup(&project);
    }

    /// 项目打不开：孩子退出码 1 → 工具失败（isError 层），消息里带孩子的 stderr。
    #[test]
    fn run_headless_fails_on_unloadable_project() {
        editor_exe_for_tests();
        let err = call(
            "run_headless",
            &json!({ "project": "Z:/definitely/not/a/project", "frames": 1 }),
        )
        .expect_err("打不开的项目必须失败");
        assert!(
            matches!(&err, CallError::Failed(m) if m.contains("退出码 1")),
            "{err:?}"
        );
    }

    /// `asset_list`：智能体的"资产树视野"—— 列路径 / 类型 / GUID / 导入设置，
    /// 并把"还没导入（缺 .meta）"如实报出来；**全程只读**。
    #[test]
    fn asset_list_reports_assets_and_diagnostics() {
        let project = temp_project("assets");
        std::fs::create_dir_all(project.path("Resources")).unwrap();
        std::fs::create_dir_all(project.path("Scripts")).unwrap();
        std::fs::write(project.path("Resources/Box.glb"), b"glb").unwrap();
        std::fs::write(project.path("Scripts/A.lua"), b"-- lua").unwrap();

        // 还没有 .meta：0 个资产，但诊断指出两个待导入文件，且不写盘。
        let text = run(&project, "asset_list", json!({})).expect("asset_list 应成功");
        let out: Value = serde_json::from_str(&text).expect("返回必须是 JSON");
        assert_eq!(out["count"], 0);
        assert_eq!(out["diagnostics"]["missing_meta"].as_array().unwrap().len(), 2);
        assert_eq!(out["diagnostics"]["has_issues"], true);
        assert!(
            !project.path("Resources/Box.glb.meta").exists(),
            "asset_list 必须只读：它不该顺手生成 .meta"
        );

        // 走了导入（生成 .meta）之后：两项都在，GUID 是 32 位十六进制。
        project.ensure_asset_metas().expect("生成 meta");
        let text = run(&project, "asset_list", json!({})).expect("asset_list 应成功");
        let out: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(out["count"], 2);
        let kinds: Vec<&str> = out["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["kind"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"Model"), "{kinds:?}");
        assert!(kinds.contains(&"Script"), "{kinds:?}");
        for asset in out["assets"].as_array().unwrap() {
            assert_eq!(asset["guid"].as_str().unwrap().len(), 32);
            assert!(asset["import"]["model_scale"].as_f64().is_some());
        }

        // kind 过滤（大小写不敏感）。
        let text = run(&project, "asset_list", json!({ "kind": "model" })).unwrap();
        let out: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(out["count"], 1);
        assert_eq!(out["assets"][0]["path"], "Resources/Box.glb");

        cleanup(&project);
    }

    /// 场景里悬空的 GUID 要在 `project_check` 的 JSON 里单独出现 ——
    /// 智能体据此知道"引用断了"，而不是只看到 `missing_models` 里一串 guid。
    #[test]
    fn project_check_reports_dangling_guids() {
        let project = temp_project("dangling_guid");
        std::fs::create_dir_all(project.path("Resources")).unwrap();
        std::fs::write(project.path("Resources/Box.glb"), b"glb").unwrap();

        run(
            &project,
            "entity_upsert",
            json!({ "name": "Box", "model": "Resources/Box.glb" }),
        )
        .expect("建实体");

        // 删掉资产与 .meta：场景里留着 guid://，库里查不到。
        std::fs::remove_file(project.path("Resources/Box.glb")).unwrap();
        std::fs::remove_file(project.path("Resources/Box.glb.meta")).unwrap();

        let text = run(&project, "project_check", json!({})).expect("检查应成功");
        let out: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(out["has_issues"], true);
        assert_eq!(out["dangling_guids"].as_array().unwrap().len(), 1, "{out}");
        assert!(
            out["dangling_guids"][0].as_str().unwrap().contains("guid://"),
            "{out}"
        );
        assert!(out["missing_models"].as_array().unwrap().is_empty());

        cleanup(&project);
    }

    /// 可执行文件搜索：空目录树找不到；同目录命中；`deps/` 下跑时上一级命中。
    #[test]
    fn editor_exe_search_covers_same_and_parent_dir() {
        let dir = std::env::temp_dir().join(format!(
            "xgame_mcp_exe_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let deps = dir.join("deps");
        std::fs::create_dir_all(&deps).expect("建目录");
        assert_eq!(find_editor_exe(&deps), None, "空目录树必须找不到");

        let fake = dir.join(format!("xgame-editor{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&fake, b"stub").expect("放桩文件");
        assert_eq!(find_editor_exe(&dir), Some(fake.clone()), "同目录应命中");
        // 上一级命中的路径拼写带 `..`（`deps/../xgame-editor.exe`）——按解析后
        // 的同一文件断言，别锁死字节拼写（那是 `join` 的实现细节）。
        let found = find_editor_exe(&deps).expect("上一级应命中");
        assert_eq!(
            found.canonicalize().expect("解析命中路径"),
            fake.canonicalize().expect("解析桩路径"),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
