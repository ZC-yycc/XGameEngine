//! 固定步预览只消费编辑世界的数据副本，宿主与派生缓存每次重新构建。

use std::path::{Path, PathBuf};

use serde_json::{json, Value as Json};
use xgc_base::EntityId;
use xgc_host::{asset_paths, ScriptedRunner};
use xgc_sim::Simulation;
use xgc_store::ProjectSession;
use xgc_world::{World, WorldData};

use crate::{query::FieldRef, required_str, required_u64, Agent};

pub(super) const MAX_FRAMES: u64 = 300;
pub(super) const MAX_WATCHES: usize = 16;
pub(super) const MIN_DT: f64 = 1.0 / 240.0;
pub(super) const MAX_DT: f64 = 1.0;

/// 显式记录的起点和逐帧观察。连接只保留最近一次成功记录。
pub(super) struct RecordedPreview {
    id: String,
    initial: WorldData,
    dt: Option<f32>,
    project: Option<PathBuf>,
    hashes: Vec<u64>,
    watch: Vec<Watch>,
}

pub(super) fn run(agent: &mut Agent, params: &Json) -> Result<Json, String> {
    let params = params
        .as_object()
        .ok_or_else(|| "sim.run 参数必须是包含 frames 的对象".to_string())?;
    if let Some(key) = params.keys().find(|key| {
        !matches!(
            key.as_str(),
            "frames" | "dt" | "project" | "watch" | "record"
        )
    }) {
        return Err(format!("sim.run 不支持参数 {key}"));
    }
    let frames = params
        .get("frames")
        .and_then(Json::as_u64)
        .filter(|&frames| frames <= MAX_FRAMES)
        .ok_or_else(|| format!("frames 必须是 0 到 {MAX_FRAMES} 之间的整数"))?;
    let dt = params.get("dt").map(parse_dt).transpose()?;
    let project = params.get("project").map(parse_project).transpose()?;
    let record = match params.get("record") {
        Some(value) => value.as_bool().ok_or("record 必须是布尔值")?,
        None => false,
    };
    let samples = frames as usize + 1;
    let mut watch = parse_watch(agent, params.get("watch"), samples)?;
    let initial = agent.world().world_data();
    let mut preview = Preview::new(&initial, dt, project)?;
    let mut hashes = Vec::with_capacity(samples);
    for frame in 0..=frames {
        hashes.push(preview.frame_hash(frame)?);
        for watch in &mut watch {
            let value = watch.read(preview.world())?;
            watch.values.push(value);
        }
    }
    let checkpoints: Vec<_> = hashes
        .iter()
        .enumerate()
        .map(|(frame, hash)| json!({"frame": frame, "state_hash": format!("{hash:016x}")}))
        .collect();
    let observations: Vec<_> = watch.iter().map(Watch::json).collect();
    let mut result =
        json!({"frames_run": frames, "state_hash": checkpoints[frames as usize]["state_hash"]});
    result["checkpoints"] = Json::Array(checkpoints);
    result["watch"] = Json::Array(observations);
    if record {
        let id = format!("tr-{}", agent.next_trace);
        agent.next_trace = agent.next_trace.checked_add(1).ok_or("记录编号已用尽")?;
        result["trace_id"] = json!(id);
        agent.recorded_preview = Some(RecordedPreview {
            id,
            initial,
            dt,
            project: project.map(Path::to_path_buf),
            hashes,
            watch,
        });
    }
    Ok(result)
}

pub(super) fn replay(agent: &Agent, params: &Json) -> Result<Json, String> {
    let id = required_str(params, "trace_id")?;
    if params.as_object().is_some_and(|params| params.len() != 1) {
        return Err("sim.replay 只接受 trace_id 参数".into());
    }
    let trace = agent
        .recorded_preview
        .as_ref()
        .filter(|trace| trace.id == id)
        .ok_or_else(|| format!("当前连接找不到记录 {id}"))?;
    let mut preview = Preview::new(&trace.initial, trace.dt, trace.project.as_deref())?;
    for (frame, &expected_hash) in trace.hashes.iter().enumerate() {
        let actual_hash = preview.frame_hash(frame as u64)?;
        if actual_hash == expected_hash {
            continue;
        }
        let mut diff = Vec::new();
        for watch in &trace.watch {
            let actual = watch.read(preview.world())?;
            let expected = &watch.values[frame];
            if &actual != expected {
                diff.push(json!({"id": watch.id, "path": watch.field.label, "expected": expected, "actual": actual}));
            }
        }
        return Ok(json!({
            "trace_id": id, "matched": false, "frames_run": frame,
            "first_divergence": {"frame": frame, "expected_hash": format!("{expected_hash:016x}"),
                                 "actual_hash": format!("{actual_hash:016x}"), "watch": diff}
        }));
    }
    Ok(json!({
        "trace_id": id, "matched": true, "frames_run": trace.hashes.len() - 1, "first_divergence": null
    }))
}

fn parse_dt(value: &Json) -> Result<f32, String> {
    value
        .as_f64()
        .filter(|dt| (MIN_DT..=MAX_DT).contains(dt))
        .map(|dt| dt as f32)
        .ok_or_else(|| "dt 必须是 1/240 到 1 之间的有限秒数".to_string())
}

fn parse_project(value: &Json) -> Result<&Path, String> {
    value
        .as_str()
        .filter(|path| !path.trim().is_empty())
        .map(Path::new)
        .ok_or_else(|| "project 必须是非空项目目录或 project.xgc 路径".to_string())
}

enum Preview {
    Systems(Box<Simulation>),
    Project(Box<ScriptedRunner>),
}

impl Preview {
    fn new(initial: &WorldData, dt: Option<f32>, project: Option<&Path>) -> Result<Self, String> {
        let mut world = World::new();
        world
            .restore_world_data(initial)
            .map_err(|err| format!("无法创建预览世界：{err}"))?;
        let project = project
            .map(|path| {
                ProjectSession::open(path)
                    .map_err(|err| format!("project {} 打不开：{err}", path.display()))
            })
            .transpose()?;
        if let Some(project) = &project {
            world.set_collision_layers(&project.manifest().layers);
        }
        let simulation = match dt {
            Some(dt) => Simulation::with_fixed_dt(world, dt),
            None => Simulation::new(world),
        };
        let Some(project) = project else {
            // 无项目时连脚本宿主也不创建，场景中的相对引用不会读到工作目录。
            return Ok(Self::Systems(Box::new(simulation)));
        };
        let assets = asset_paths(Some(project.root()), &project.manifest().search_paths);
        let (runner, _) = ScriptedRunner::with_simulation(simulation, assets);
        Ok(Self::Project(Box::new(runner)))
    }

    fn world(&self) -> &World {
        match self {
            Self::Systems(simulation) => simulation.world(),
            Self::Project(runner) => runner.world(),
        }
    }

    fn frame_hash(&mut self, frame: u64) -> Result<u64, String> {
        if frame > 0 {
            match self {
                Self::Systems(simulation) => simulation
                    .advance_with(Vec::new(), |_| {})
                    .map_err(|err| format!("预览步进失败：{err:?}")),
                Self::Project(runner) => runner.step(),
            }?;
        }
        Ok(self.world().state_hash())
    }
}

struct Watch {
    id: EntityId,
    field: FieldRef,
    values: Vec<Json>,
}

impl Watch {
    fn read(&self, world: &World) -> Result<Json, String> {
        self.field
            .read(world, self.id, |value| serde_json::to_value(value))
            .transpose()
            .map(|value| value.unwrap_or(Json::Null))
            .map_err(|err| format!("watch {} 序列化失败：{err}", self.field.label))
    }

    fn json(&self) -> Json {
        json!({"id": self.id, "path": self.field.label, "values": self.values})
    }
}

fn parse_watch(agent: &Agent, watch: Option<&Json>, samples: usize) -> Result<Vec<Watch>, String> {
    let Some(watch) = watch else {
        return Ok(Vec::new());
    };
    let items = watch
        .as_array()
        .filter(|items| items.len() <= MAX_WATCHES)
        .ok_or_else(|| format!("watch 必须是最多 {MAX_WATCHES} 项的数组"))?;
    items
        .iter()
        .map(|item| {
            let object = item
                .as_object()
                .ok_or_else(|| "watch 条目必须是包含 id 和 path 的对象".to_string())?;
            if let Some(key) = object
                .keys()
                .find(|key| !matches!(key.as_str(), "id" | "path"))
            {
                return Err(format!("watch 条目不支持字段 {key}"));
            }
            let id = EntityId::from_raw(required_u64(item, "id")?);
            id.validate()
                .map_err(|err| format!("watch id 无效：{err}"))?;
            Ok(Watch {
                id,
                field: agent.resolve_field(required_str(item, "path")?)?,
                values: Vec::with_capacity(samples),
            })
        })
        .collect()
}
