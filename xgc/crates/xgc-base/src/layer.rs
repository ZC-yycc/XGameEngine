//! 项目碰撞层设置与 32 位物理过滤共用同一个索引边界。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerIndex(u8);

impl LayerIndex {
    const COUNT: usize = u32::BITS as usize;
    pub const MAX: u8 = (Self::COUNT - 1) as u8;

    pub const fn new(index: u8) -> Option<Self> {
        if index <= Self::MAX {
            Some(Self(index))
        } else {
            None
        }
    }

    pub const fn raw(self) -> u8 {
        self.0
    }

    pub const fn member(self) -> u32 {
        1u32 << self.0
    }
}

/// 项目只保存改名和掩码覆盖。缺省掩码全通，显式零值表示拒绝所有层。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollisionLayers {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<u8, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub masks: BTreeMap<u8, u32>,
}

impl CollisionLayers {
    pub fn is_default(&self) -> bool {
        self.names.is_empty() && self.masks.is_empty()
    }

    pub fn validate(&self) -> Result<(), String> {
        for &index in self.names.keys().chain(self.masks.keys()) {
            if LayerIndex::new(index).is_none() {
                return Err(format!("碰撞层 {index} 超过 {}", LayerIndex::MAX));
            }
        }
        Ok(())
    }

    /// 旧项目读入时的既有清洗规则。掩码两侧允许各自独立表达。
    pub fn sanitized(&self) -> Self {
        let names = self.names.iter().filter_map(|(&index, name)| {
            let name = name.trim();
            (LayerIndex::new(index).is_some() && !name.is_empty()).then(|| (index, name.to_owned()))
        });
        let masks = self.masks.iter().filter_map(|(&index, &mask)| {
            (LayerIndex::new(index).is_some() && mask != u32::MAX).then_some((index, mask))
        });
        Self {
            names: names.collect(),
            masks: masks.collect(),
        }
    }

    /// 在宿主装载时展开覆盖，固定步直接按层索引读取。
    pub fn collision_masks(&self) -> CollisionMasks {
        let mut masks = [u32::MAX; LayerIndex::COUNT];
        let mut filtered = false;
        for (&index, &mask) in &self.masks {
            if let Some(layer) = LayerIndex::new(index) {
                masks[usize::from(layer.raw())] = mask;
                filtered |= mask != u32::MAX;
            }
        }
        CollisionMasks(filtered.then_some(masks))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CollisionMasks(Option<[u32; LayerIndex::COUNT]>);

impl CollisionMasks {
    pub const fn all_enabled(&self) -> bool {
        self.0.is_none()
    }

    pub fn mask(&self, layer: LayerIndex) -> u32 {
        self.0
            .as_ref()
            .map_or(u32::MAX, |masks| masks[usize::from(layer.raw())])
    }

    pub fn allows_pair(&self, a: LayerIndex, b: LayerIndex) -> bool {
        self.mask(a) & b.member() != 0 && self.mask(b) & a.member() != 0
    }
}
