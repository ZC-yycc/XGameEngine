//! 场景从项目加载后，编辑身份与磁盘基准一起绑定到会话。

use std::path::{Path, PathBuf};

use serde_json::{json, Value as Json};
use xgc_store::{load_world, load_world_with_bytes, save_snapshot, ProjectSession};
use xgc_world::{Command, Plan};

use crate::{required_str, Agent};

pub(super) struct ProjectScene {
    session: ProjectSession,
    scene: String,
    path: PathBuf,
    bytes: Vec<u8>,
}

impl Agent {
    pub(super) fn load(&mut self, params: &Json) -> Result<Json, String> {
        let path = Path::new(required_str(params, "path")?);
        let project = ProjectSession::containing_scene(path).map_err(|err| err.to_string())?;
        let (data, binding) = if let Some((session, scene)) = project {
            let path = session
                .canonical_scene_path(&scene)
                .map_err(|err| err.to_string())?;
            let (data, bytes) = load_world_with_bytes(&path).map_err(|err| err.to_string())?;
            let binding = ProjectScene {
                session,
                scene,
                path,
                bytes,
            };
            (data, Some(binding))
        } else {
            (load_world(path).map_err(|err| err.to_string())?, None)
        };
        let mut result = self.replace_edit_world(&data, "恢复失败")?;
        if let Some(binding) = &binding {
            result["next_entity_id"] = json!(binding.session.manifest().next_entity_id.to_string());
        }
        self.project_scene = binding;
        Ok(result)
    }

    pub(super) fn save(&mut self, params: &Json) -> Result<Json, String> {
        let path = required_str(params, "path")?;
        let target = Path::new(path);
        let data = self.world.world_data();
        let binding = match &mut self.project_scene {
            Some(binding)
                if binding
                    .session
                    .matches_scene_location(&binding.scene, &binding.path, target)
                    .map_err(|err| err.to_string())? =>
            {
                Some(binding)
            }
            _ => None,
        };
        if let Some(binding) = binding {
            binding.bytes = binding
                .session
                .save_scene_if_unchanged(&binding.scene, &binding.path, &data, &binding.bytes)
                .map_err(|err| err.to_string())?;
        } else {
            save_snapshot(target, &data).map_err(|err| err.to_string())?;
        }
        Ok(json!({
            "path": path,
            "revision": data.revision,
            "entities": data.entities.len(),
        }))
    }

    pub(super) fn claim_plan_ids(&mut self, plan: &Plan) -> Result<(), String> {
        let Some(binding) = &mut self.project_scene else {
            return Ok(());
        };
        // 只为可应用的新计划领号。Undo 与日志回放直接还原原有身份。
        if !plan.is_valid() || plan.base_revision != self.world.revision() {
            return Ok(());
        }
        let mut ids = plan.commands.iter().filter_map(|command| match command {
            Command::Spawn { data } => Some(data.id),
            _ => None,
        });
        if let Some(first) = ids.next() {
            let (first, last) = ids.fold((first, first), |(low, high), id| {
                (low.min(id), high.max(id))
            });
            binding
                .session
                .claim_scene_entity_ids(
                    &binding.scene,
                    &binding.path,
                    first,
                    last,
                    self.world.next_entity_id(),
                )
                .map_err(|err| err.to_string())?;
        }
        Ok(())
    }
}
