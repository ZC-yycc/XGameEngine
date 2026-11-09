//! 组件身份属于实体内部；来源 ID 则属于预制体资产里的对应实体。

use serde::{Deserialize, Deserializer, Serialize};

/// 与 `EntityId` 合用才构成全局地址；0 与最大值保留给坏数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ComponentId(u64);

impl ComponentId {
    pub fn from_raw(raw: u64) -> Result<Self, String> {
        if raw == 0 || raw == u64::MAX {
            Err(format!("组件 ID {raw} 无效"))
        } else {
            Ok(Self(raw))
        }
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ComponentId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_raw(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// 预制体实例的本地身份不等于它在资产中对应组件的 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentIdentity {
    pub id: ComponentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_component_id: Option<ComponentId>,
}

/// `next_id` 必须随 v2 实体持久化；只从当前组件列表的最大值恢复会复用已删除的 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ComponentIdSequence {
    next_id: u64,
}

impl<'de> Deserialize<'de> for ComponentIdSequence {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            next_id: u64,
        }
        Self::from_next(Wire::deserialize(deserializer)?.next_id).map_err(serde::de::Error::custom)
    }
}

impl Default for ComponentIdSequence {
    fn default() -> Self {
        Self { next_id: 1 }
    }
}

impl ComponentIdSequence {
    pub fn from_next(next_id: u64) -> Result<Self, String> {
        if next_id == 0 {
            return Err(format!("下一个组件 ID {next_id} 无效"));
        }
        Ok(Self { next_id })
    }

    pub const fn next_id(self) -> u64 {
        self.next_id
    }

    /// 调用方在读盘后检查所有本地 ID；来源 ID 不属于这个序列。
    pub fn validate(self, ids: impl IntoIterator<Item = ComponentId>) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        for id in ids {
            if id.raw() >= self.next_id {
                return Err(format!(
                    "组件 ID {} 不小于下一个 ID {}",
                    id.raw(),
                    self.next_id
                ));
            }
            if !seen.insert(id) {
                return Err(format!("重复的组件 ID {}", id.raw()));
            }
        }
        Ok(())
    }

    pub fn allocate(&mut self) -> Result<ComponentId, String> {
        let id = ComponentId::from_raw(self.next_id)?;
        self.next_id = self.next_id.checked_add(1).ok_or("组件 ID 耗尽")?;
        Ok(id)
    }
}
