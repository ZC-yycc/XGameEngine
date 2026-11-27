//! 项目场景切换：清单登记名 → 对应场景世界。
//!
//! 脏场景与 Play 期间一律拒绝：静默丢改动救不回来，Play 世界和编辑世界是两份数据。

use xgc_store::load_world_with_bytes;
use xgc_world::World;

use crate::{EditorState, SceneFile};

impl EditorState {
    /// 项目登记的全部场景；单场景打开时为空。
    pub fn scenes(&self) -> &[String] {
        self.project
            .as_ref()
            .map_or(&[], |session| session.manifest().scenes.as_slice())
    }

    /// 当前场景在项目清单里的名字；单场景打开时为 None。
    pub fn current_scene(&self) -> Option<&str> {
        self.scene.as_deref()
    }

    /// 切换到清单登记的另一关。
    ///
    /// 脏场景拒绝切换（拒绝保存还能救，静默丢改动救不回来）；Play 期间拒绝，
    /// 因为运行时副本和编辑世界是两份数据。
    pub fn open_scene(&mut self, scene: &str) -> Result<(), String> {
        if self.play.is_some() {
            return Err("Play 期间不能切换场景".to_string());
        }
        if self.dirty {
            return Err("有未保存改动，先保存再切换场景".to_string());
        }
        let session = self
            .project
            .as_ref()
            .ok_or_else(|| "当前不是项目，无法切换场景".to_string())?;
        let path = session
            .canonical_scene_path(scene)
            .map_err(|err| format!("读取场景失败：{err}"))?;
        let (data, bytes) =
            load_world_with_bytes(&path).map_err(|err| format!("读取场景失败：{err}"))?;
        let mut world = World::new();
        world
            .restore_world_data(&data)
            .map_err(|err| format!("恢复世界失败：{err}"))?;
        self.world = world;
        self.file = Some(SceneFile { path, bytes });
        self.scene = Some(scene.to_string());
        self.selected = None;
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        Ok(())
    }
}
