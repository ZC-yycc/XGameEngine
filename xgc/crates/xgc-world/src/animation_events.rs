//! 动画事件的瞬态载荷。
//!
//! 作者在 `Animator.events` 里写下"第几秒发什么名字"；本模块只放**运行期**
//! 已经判定命中的那一条，供脚本宿主在固定步里消费。事件不进 `WorldData`、
//! 不进快照、也不参与 `state_hash` —— 它描述的是"这一帧发生了什么"，不是
//! 世界状态；重放时应当由时钟重新推导，而不是从存档里读回来。

use serde::{Deserialize, Serialize};
use xgc_base::EntityId;

/// 作者写下的动画事件：从剪辑 0 起算的秒数 + 事件名。
///
/// 越界、非有限、负时间、空名一律由运行时跳过（与旧端 `crossed_events`
/// 同一口径）：宁可这条事件不触发，也不猜作者想要"下一圈的同一相位"。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimEvent {
    /// 触发时刻（秒）。
    pub time: f32,
    /// 事件名；空名不派发。
    pub name: String,
}

impl AnimEvent {
    pub fn new(name: impl Into<String>, time: f32) -> Self {
        Self {
            name: name.into(),
            time,
        }
    }
}

/// 某一固定步里命中的动画事件；`entity` 是挂了 `Animator` 的实体。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiredAnimEvent {
    pub entity: EntityId,
    pub name: String,
}

impl super::World {
    /// 清空本步事件。`Simulation::step_with` 在推进任何系统之前调用。
    pub fn clear_animation_events(&mut self) {
        self.animation_events.clear();
    }

    /// 追加一条已命中的事件（动画系统在推进时钟后调用）。
    pub fn push_animation_event(&mut self, event: FiredAnimEvent) {
        self.animation_events.push(event);
    }

    /// 本步动画事件；脚本宿主在读完后由下一次 `step_with` 统一清空。
    pub fn animation_events(&self) -> &[FiredAnimEvent] {
        &self.animation_events
    }
}
