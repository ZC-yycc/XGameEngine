//! 旧场景的角色参数投影。
//!
//! 旧端 `xgame_core::CharacterController` 的六个字段都带 `#[serde(default = "default_*")]`，
//! 所以作者手写的 RON 可以只写关心的字段。迁移这一侧必须复刻同一批缺省值：
//! 不给缺省的话，这类场景会在迁移入口直接解析失败（`missing field`），
//! 等于把"没写就按旧端缺省"这条语义挡在门外。

use serde::Deserialize;

/// 旧场景里的角色参数（六个字段的 serde 缺省见文件头）。
#[derive(Debug, Deserialize)]
pub(crate) struct OldCharacter {
    #[serde(default = "default_character_height")]
    pub(crate) height: f32,
    #[serde(default = "default_character_radius")]
    pub(crate) radius: f32,
    #[serde(default = "default_character_step_offset")]
    pub(crate) step_offset: f32,
    #[serde(default = "default_character_slope_limit")]
    pub(crate) slope_limit: f32,
    #[serde(default = "default_character_skin_width")]
    pub(crate) skin_width: f32,
    #[serde(default = "default_character_snap_to_ground")]
    pub(crate) snap_to_ground: f32,
}

pub(crate) fn default_character_height() -> f32 {
    1.8
}

pub(crate) fn default_character_radius() -> f32 {
    0.3
}

pub(crate) fn default_character_step_offset() -> f32 {
    0.35
}

pub(crate) fn default_character_slope_limit() -> f32 {
    45.0
}

pub(crate) fn default_character_skin_width() -> f32 {
    0.02
}

pub(crate) fn default_character_snap_to_ground() -> f32 {
    0.3
}
