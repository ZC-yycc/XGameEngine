//! 固定步预览只消费编辑世界的数据副本，宿主与派生缓存每次重新构建。

use std::path::Path;

use serde_json::{json, Value as Json};
use xgc_base::EntityId;
use xgc_host::{asset_paths, ScriptedRunner};
use xgc_sim::Simulation;
use xgc_store::ProjectSession;
use xgc_world::World;

use crate::{query::FieldRef, required_str, required_u64, Agent};

pub(super) const MAX_FRAMES: u64 = 300;
pub(super) const MAX_WATCHES: usize = 16;
pub(super) const MIN_DT: f64 = 1.0 / 240.0;
pub(super) const MAX_DT: f64 = 1.0;

pub(super) fn run(agent: &Agent, params: &Json) -> Result<Json, String> {
    let params = params
        .as_object()
        .ok_or_else(|| "sim.run 参数必须是包含 frames 的对象".to_string())?;
    if let Some(key) = params
        .keys()
        .find(|key| !matches!(key.as_str(), "frames" | "dt" | "project" | "watch"))
    {
        return Err(format!("sim.run 不支持参数 {key}"));
    }
    let frames = params
        .get("frames")
        .and_then(Json::as_u64)
        .filter(|&frames| frames <= MAX_FRAMES)
        .ok_or_else(|| format!("frames 必须是 0 到 {MAX_FRAMES} 之间的整数"))?;
    let dt = params.get("dt").map(parse_dt).transpose()?;
    let project = params.get("project").map(parse_project).transpose()?;
    let samples = frames as usize + 1;
    let mut watch = parse_watch(agent, params.get("watch"), samples)?;

    let data = agent.world().world_data();
    let mut world = World::new();
    world
        .restore_world_data(&data)
        .map_err(|err| format!("无法创建预览世界：{err}"))?;
    let simulation = match dt {
        Some(dt) => Simulation::with_fixed_dt(world, dt),
        None => Simulation::new(world),
    };
    let mut preview = Preview::new(simulation, project)?;

    let mut state_hash = String::new();
    let mut checkpoints = Vec::with_capacity(samples);
    for frame in 0..=frames {
        if frame > 0 {
            preview.step()?;
        }
        let world = preview.world();
        state_hash = format!("{:016x}", world.state_hash());
        checkpoints.push(json!({"frame": frame, "state_hash": state_hash}));
        for watch in &mut watch {
            watch.sample(world)?;
        }
    }
    let watch: Vec<_> = watch.into_iter().map(Watch::into_json).collect();
    Ok(
        json!({"frames_run": frames, "state_hash": state_hash, "checkpoints": checkpoints, "watch": watch}),
    )
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
    fn new(simulation: Simulation, project: Option<&Path>) -> Result<Self, String> {
        let Some(path) = project else {
            // 无项目时连脚本宿主也不创建，场景中的相对引用不会读到工作目录。
            return Ok(Self::Systems(Box::new(simulation)));
        };
        let project = ProjectSession::open(path)
            .map_err(|err| format!("project {} 打不开：{err}", path.display()))?;
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

    fn step(&mut self) -> Result<(), String> {
        match self {
            Self::Systems(simulation) => simulation
                .advance_with(Vec::new(), |_| {})
                .map_err(|err| format!("预览步进失败：{err:?}")),
            Self::Project(runner) => runner.step(),
        }
        .map(|_| ())
    }
}

struct Watch {
    id: EntityId,
    field: FieldRef,
    values: Vec<Json>,
}

impl Watch {
    fn sample(&mut self, world: &World) -> Result<(), String> {
        let value = self
            .field
            .read(world, self.id, |value| serde_json::to_value(value));
        let value = value
            .transpose()
            .map_err(|err| format!("watch {} 序列化失败：{err}", self.field.label))?;
        self.values.push(value.unwrap_or(Json::Null));
        Ok(())
    }

    fn into_json(self) -> Json {
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
            Ok(Watch {
                id: EntityId::from_raw(required_u64(item, "id")?),
                field: agent.resolve_field(required_str(item, "path")?)?,
                values: Vec::with_capacity(samples),
            })
        })
        .collect()
}
