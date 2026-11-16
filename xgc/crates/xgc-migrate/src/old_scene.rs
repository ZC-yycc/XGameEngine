//! 旧场景顶层结构；字段只在本 crate 内使用。

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct OldScene {
    pub(crate) format_version: u32,
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) entities: Vec<OldEntity>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OldEntity {
    pub(crate) id: OldId,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) parent: Option<OldId>,
    pub(crate) transform: OldTransform,
    pub(crate) components: super::OldComponents,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct OldId(pub(crate) u64);

#[derive(Debug, Deserialize)]
pub(crate) struct OldTransform {
    pub(crate) position: glam::Vec3,
    pub(crate) rotation: glam::Quat,
    pub(crate) scale: glam::Vec3,
}
