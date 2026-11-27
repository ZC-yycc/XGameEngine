//! XGC 独立播放器核心：从原生项目或 cooked 目录加载世界并驱动脚本。

use std::path::{Path, PathBuf};

use xgc_asset::{AssetPaths, CookRuntime};
use xgc_host::{asset_paths, ScriptedRunner};
use xgc_render::{RenderQuality, RenderSettings};
use xgc_store::{load_world, ProjectSession};
use xgc_world::{World, WorldData};

pub mod window;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerStats {
    pub frames: u64,
    pub entities: usize,
    pub scripts: usize,
    pub state_hash: u64,
}

/// 从项目目录、`project.xgc` 或 cooked 目录读取入口场景。
pub fn load_world_dir(path: &Path) -> Result<WorldData, String> {
    load_world_dir_scene(path, None)
}

/// 原生项目可以按清单登记名或唯一文件名主干选场景。
pub fn load_world_dir_scene(path: &Path, scene: Option<&str>) -> Result<WorldData, String> {
    Ok(load_scene(path, scene)?.data)
}

struct LoadedScene {
    data: WorldData,
    source: SceneSource,
}

impl LoadedScene {
    fn runner(&self) -> Result<(ScriptedRunner, xgc_render::MeshCache), String> {
        let mut world = World::new();
        world
            .restore_world_data(&self.data)
            .map_err(|err| format!("恢复世界失败：{err}"))?;
        match &self.source {
            SceneSource::Project(project) => world.set_collision_layers(&project.manifest().layers),
            SceneSource::Cooked {
                runtime: Some(runtime),
                ..
            } => world.set_collision_layers(&runtime.layers),
            SceneSource::Cooked { runtime: None, .. } => {}
        }
        Ok(ScriptedRunner::with_assets(
            world,
            self.source.asset_paths(),
        ))
    }
}

enum SceneSource {
    Project(Box<ProjectSession>),
    Cooked {
        root: PathBuf,
        runtime: Option<Box<CookRuntime>>,
    },
}

impl SceneSource {
    fn root(&self) -> &Path {
        match self {
            Self::Project(project) => project.root(),
            Self::Cooked { root, .. } => root,
        }
    }

    fn render_settings(&self) -> (RenderSettings, RenderQuality) {
        match self {
            Self::Project(project) => {
                let manifest = project.manifest();
                (manifest.render, manifest.quality)
            }
            Self::Cooked { runtime, .. } => {
                runtime.as_ref().map_or_else(Default::default, |runtime| {
                    (runtime.render, runtime.quality)
                })
            }
        }
    }

    fn asset_paths(&self) -> AssetPaths {
        let search: &[String] = match self {
            Self::Project(project) => &project.manifest().search_paths,
            Self::Cooked { runtime, .. } => runtime
                .as_ref()
                .map_or(&[][..], |runtime| runtime.search_paths.as_slice()),
        };
        asset_paths(Some(self.root()), search)
    }
}

fn load_scene(path: &Path, scene: Option<&str>) -> Result<LoadedScene, String> {
    if path.file_name().is_some_and(|name| name == "project.xgc")
        || path.join("project.xgc").exists()
    {
        let project = ProjectSession::open(path).map_err(|err| format!("读取项目失败：{err}"))?;
        let scene = match scene {
            Some(name) => resolve_scene_name(&project, name)?,
            None => project.entry_scene().to_string(),
        };
        let data = project
            .load_scene(&scene)
            .map_err(|err| format!("读取场景 {scene} 失败：{err}"))?;
        return Ok(LoadedScene {
            data,
            source: SceneSource::Project(Box::new(project)),
        });
    }
    let dir = path;
    if scene.is_some() {
        return Err("cooked 目录没有场景清单，不能按名字选择场景".to_string());
    }
    let manifest = xgc_asset::verify_manifest(dir)
        .map_err(|differences| format!("cooked 文件校验失败：{}", differences.join("；")))?;
    let binary = dir.join("world.xgcb");
    let data = if binary.exists() {
        xgc_store::load_world_binary(&binary)
    } else {
        load_world(&dir.join("world.json"))
    }
    .map_err(|err| format!("读取 cooked world 失败：{err}"))?;
    Ok(LoadedScene {
        data,
        source: SceneSource::Cooked {
            root: dir.to_path_buf(),
            runtime: manifest.map(|manifest| Box::new(manifest.runtime)),
        },
    })
}

/// 场景名解析：先按清单登记名精确匹配，再按唯一文件名主干匹配（`character` →
/// `scenes/character.json`）。多个同名主干必须写出完整登记名。
fn resolve_scene_name(session: &ProjectSession, requested: &str) -> Result<String, String> {
    let scenes = &session.manifest().scenes;
    if scenes.iter().any(|scene| scene == requested) {
        return Ok(requested.to_string());
    }
    let mut matches = scenes.iter().filter(|scene| {
        Path::new(scene)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem == requested)
    });
    match (matches.next(), matches.next()) {
        (Some(scene), None) => Ok(scene.clone()),
        (Some(_), Some(_)) => Err(format!(
            "场景名 {requested} 匹配到多个文件，请写清单里的完整路径"
        )),
        _ => Err(format!(
            "项目清单里没有场景 {requested}；已登记：{}",
            scenes.join(", ")
        )),
    }
}

pub fn run_cooked(dir: &Path, frames: u64) -> Result<PlayerStats, String> {
    run_cooked_scene(dir, frames, None)
}

/// 固定 60Hz 跑指定帧数。场景选择仅适用于原生项目。
pub fn run_cooked_scene(
    path: &Path,
    frames: u64,
    scene: Option<&str>,
) -> Result<PlayerStats, String> {
    let loaded = load_scene(path, scene)?;
    let (mut runner, _) = loaded.runner()?;
    for _ in 0..frames {
        runner.step()?;
    }
    let world = runner.world();
    Ok(PlayerStats {
        frames,
        entities: world.len(),
        scripts: runner.script_count(),
        state_hash: world.state_hash(),
    })
}
