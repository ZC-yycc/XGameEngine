//! 真正的 MCP stdio 会话同时验证脚本边界、逐行输出和项目宿主预览。

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value as Json};
use xgc_world::{Name, Script, Transform, World};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("xgc-mcp-sim-{}-{nonce}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn project(root: &Path, first_script: &str) {
    let scripts = root.join("Fallback/Scripts");
    fs::create_dir_all(&scripts).unwrap();
    // 清单入口不存在，证明预览只读取清单上下文，继续运行会话中的场景。
    fs::write(
        root.join("project.xgc"),
        r#"(
            format_version: 2,
            next_entity_id: 1,
            name: "MCP preview",
            entry_scene: "scenes/unused.json",
            scenes: ["scenes/unused.json"],
            content: (models: "Models", textures: "Textures", audio: "Audio",
                      prefabs: "Prefabs", scripts: "Scripts"),
            search_paths: ["Fallback"],
        )"#,
    )
    .unwrap();
    fs::write(scripts.join("first.lua"), first_script).unwrap();
    fs::write(
        scripts.join("last.lua"),
        r#"
        local ticks = 0
        function update(ctx)
            ticks = ticks + 1
            set_pos(ctx, ticks + delta(ctx), 0, 0)
        end
        "#,
    )
    .unwrap();
    fs::write(
        scripts.join("last.lua.meta"),
        "(guid: \"0123456789abcdef\",)",
    )
    .unwrap();
}

fn request(id: u64, method: &str, params: Json) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn tool(id: u64, name: &str, arguments: Json) -> Json {
    request(
        id,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
}

fn tool_result(response: &Json) -> Json {
    assert_eq!(response["result"]["isError"], false, "{response}");
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn sim_preview_runs_in_real_mcp_stdio_without_leaking_script_output() {
    let scratch = Scratch::new();
    let cwd = scratch.0.join("cwd");
    fs::create_dir_all(cwd.join("Scripts")).unwrap();
    fs::write(
        cwd.join("Scripts/first.lua"),
        "error('unexpected cwd script')",
    )
    .unwrap();

    let good = scratch.0.join("project");
    project(
        &good,
        r#"
        function update(ctx)
            print('preview print')
            log(ctx, 'preview log')
            xgc.log('preview xgc.log')
            xgc.set(self_id(ctx), 'core.name', 'value', 'first mount ran')
            set_pos(ctx, 99, 0, 0)
        end
        "#,
    );
    let broken = scratch.0.join("broken");
    project(
        &broken,
        "function update(ctx) set_pos(ctx, 99, 0, 0); error('preview failed') end",
    );

    let mut world = World::new();
    let key = world.spawn((
        Name {
            value: "edit world".into(),
        },
        Transform::default(),
        Script {
            paths: vec!["Scripts/first.lua".into(), "guid://0123456789abcdef".into()],
        },
    ));
    let id = world.entity(key).unwrap();
    let plain = json!({"frames": 2, "dt": 0.25, "watch": [
        {"id": id, "path": "core.transform:position"},
        {"id": id, "path": "core.name:value"}
    ]});
    let mut scripted = plain.clone();
    scripted["project"] = json!(good.join("project.xgc"));
    let requests = [
        request(1, "initialize", json!({"protocolVersion": "2025-06-18"})),
        request(2, "tools/list", json!({})),
        tool(3, "world_restore", json!({"data": world.world_data()})),
        tool(4, "world_snapshot", json!({})),
        tool(5, "sim_run", plain),
        tool(6, "sim_run", scripted.clone()),
        tool(6, "sim_run", scripted),
        tool(7, "sim_run", json!({"frames": 1, "project": broken})),
        tool(8, "world_snapshot", json!({})),
    ];
    let mut child = Command::new(env!("CARGO_BIN_EXE_xgc"))
        .arg("mcp")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for request in &requests {
            writeln!(stdin, "{request}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), requests.len(), "{stdout}");
    let responses: Vec<Json> = lines
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for (response, request) in responses.iter().zip(requests) {
        assert_eq!(response["id"], request["id"], "{response}");
    }
    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    let sim = tools.iter().find(|tool| tool["name"] == "sim_run").unwrap();
    assert_eq!(sim["inputSchema"]["required"], json!(["frames"]));
    assert_eq!(sim["inputSchema"]["properties"]["frames"]["maximum"], 300);
    assert_eq!(sim["inputSchema"]["properties"]["watch"]["maxItems"], 16);
    tool_result(&responses[2]);
    let before = tool_result(&responses[3]);
    let plain = tool_result(&responses[4]);
    let scripted = tool_result(&responses[5]);
    assert_eq!(plain["checkpoints"][0], scripted["checkpoints"][0]);
    assert_eq!(plain["watch"][0]["values"], json!(vec![vec3(0.0); 3]));
    assert_eq!(
        scripted["watch"][0]["values"],
        json!([vec3(0.0), vec3(1.25), vec3(2.25)])
    );
    assert_eq!(scripted["watch"][1]["values"][0]["value"], "edit world");
    assert_eq!(
        scripted["watch"][1]["values"][1]["value"],
        "first mount ran"
    );
    assert_eq!(lines[5], lines[6]);
    assert_eq!(responses[7]["result"]["isError"], true);
    let error = responses[7]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    for expected in ["第 1 帧", "Scripts/first.lua", "preview failed"] {
        assert!(error.contains(expected), "{error}");
    }
    assert_eq!(tool_result(&responses[8]), before);
    for expected in ["preview print", "preview log", "preview xgc.log"] {
        assert!(stderr.contains(expected), "{stderr}");
    }
    assert!(!stderr.contains("unexpected cwd script"), "{stderr}");
}

#[test]
fn replay_reports_the_first_actual_script_divergence_in_mcp() {
    let scratch = Scratch::new();
    let baseline = r#"
        local ticks = 0
        function update(ctx)
            ticks = ticks + 1
            set_pos(ctx, ticks, 0, 0)
        end
    "#;
    let changed = r#"
        local ticks = 0
        function update(ctx)
            ticks = ticks + 1
            if ticks >= 2 then set_pos(ctx, ticks + 9, 0, 0)
            else set_pos(ctx, ticks, 0, 0) end
            if ticks >= 3 then error('replay ran past the first divergence') end
        end
    "#;
    project(&scratch.0, baseline);
    let mut world = World::new();
    let key = world.spawn((
        Transform::default(),
        Script {
            paths: vec!["Scripts/first.lua".into()],
        },
    ));
    let id = world.entity(key).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_xgc"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut call = |request: Json| -> Json {
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        assert_ne!(stdout.read_line(&mut line).unwrap(), 0);
        let response: Json = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], request["id"], "{response}");
        response
    };
    let result = || {
        let handshake = call(request(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18"}),
        ));
        assert_eq!(handshake["result"]["protocolVersion"], "2025-06-18");
        tool_result(&call(tool(
            2,
            "world_restore",
            json!({"data": world.world_data()}),
        )));
        let before = tool_result(&call(tool(3, "world_snapshot", json!({}))));
        let recorded = tool_result(&call(tool(
            4,
            "sim_run",
            json!({
                "frames": 3, "record": true, "project": scratch.0,
                "watch": [{"id": id, "path": "core.transform:position"}]
            }),
        )));
        assert_eq!(
            recorded["watch"][0]["values"],
            json!([vec3(0.0), vec3(1.0), vec3(2.0), vec3(3.0)])
        );
        let replay_args = json!({"trace_id": recorded["trace_id"]});
        let matched = tool_result(&call(tool(5, "sim_replay", replay_args.clone())));
        assert_eq!(matched["matched"], true);
        assert_eq!(matched["frames_run"], 3);
        assert_eq!(matched["first_divergence"], Json::Null);
        fs::write(scratch.0.join("Fallback/Scripts/first.lua"), changed).unwrap();
        let control = tool_result(&call(tool(
            6,
            "sim_run",
            json!({
                "frames": 2, "project": scratch.0,
                "watch": [{"id": id, "path": "core.transform:position"}]
            }),
        )));
        assert_eq!(control["checkpoints"][1], recorded["checkpoints"][1]);
        assert_ne!(control["checkpoints"][2], recorded["checkpoints"][2]);
        let diverged = tool_result(&call(tool(7, "sim_replay", replay_args)));
        assert_eq!(diverged["matched"], false);
        assert_eq!(diverged["frames_run"], 2);
        assert_eq!(
            diverged["first_divergence"],
            json!({
                "frame": 2,
                "expected_hash": recorded["checkpoints"][2]["state_hash"],
                "actual_hash": control["checkpoints"][2]["state_hash"],
                "watch": [{"id": id, "path": "core.transform:position", "expected": vec3(2.0), "actual": vec3(11.0)}]
            })
        );
        assert_eq!(
            tool_result(&call(tool(8, "world_snapshot", json!({})))),
            before
        );
    };
    // 断言失败也要关闭 stdin，避免失败用例在 Windows 锁住 xgc.exe。
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(result));
    drop(stdin);
    drop(stdout);
    let output = child.wait_with_output().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
}

fn vec3(x: f32) -> Json {
    json!({"kind": "vec3", "value": [x, 0.0, 0.0]})
}
