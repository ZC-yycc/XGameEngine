//! 实体动画组件（声明式播放配置）。
//!
//! 这里只放**要落盘的配置**：播哪个剪辑、速度、是否循环、是否自动播放。
//! 采样时钟与蒙皮矩阵计算属于渲染层（见 `xgame_render::animation`）——
//! 动画数据（glTF 里的骨骼与关键帧）本身就是渲染资产，把它拆到 core 之外
//! 可以让 core 继续对 GPU / glTF 一无所知。
//!
//! # 为什么是 `Entity` 的一等字段，而不是 `ComponentMap`
//!
//! `ComponentMap` 是 `#[serde(skip)]` 的运行时容器（装 `Box<dyn Any>`，不可比、
//! 不落盘）。而动画配置必须：进场景文件（声明式、可版本管理）、参与脏检查
//! （`Entity` 的 `PartialEq`）、被编辑器指纹看到（改了剪辑要触发重绘）——
//! 这三条都要求它像 `physics` / `scripts` 一样是一等字段。

use serde::{Deserialize, Serialize};

/// 挂在实体上的动画播放器配置。
///
/// 默认值 = 「播第一个剪辑、原速、循环、自动播放」——给一个带动画的模型
/// 挂上组件后不改任何字段就应当能看到它动起来。
///
/// `#[serde(default)]` 在**结构体**级别：反序列化时缺哪个字段就从
/// [`Default`] 补哪个字段。于是旧场景文件（没有 `animator`）反序列化成
/// `None`（`Option` 缺省），手写的半截配置（比如只有 `clip`）也能补全。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Animator {
    /// 要播放的剪辑名（glTF `animations[].name`）。空串 / 找不到 = 不动画（绑定姿态）。
    pub clip: String,
    /// 播放速度倍率。`1.0` = 原速；`0` = 冻结在当前帧；负值 v1 停在第 0 帧（倒放留到 v2）。
    pub speed: f32,
    /// 是否循环。`false` 播到末尾停在最后一帧（时间被钳住，不回跳）。
    pub looping: bool,
    /// 是否自动开始播放。`false` = 静止在剪辑第 0 帧（用于「先摆好再播」的制作流程）。
    pub autoplay: bool,
}

impl Default for Animator {
    fn default() -> Self {
        Self {
            clip: String::new(),
            speed: 1.0,
            looping: true,
            autoplay: true,
        }
    }
}

impl Animator {
    /// 指定剪辑、其余取默认的便捷构造。
    pub fn new(clip: impl Into<String>) -> Self {
        Self {
            clip: clip.into(),
            ..Self::default()
        }
    }

    /// 这一帧该不该推进时钟（`viewport_animating` / 帧策略据此判断）。
    ///
    /// 三个条件缺一不可：没开自动播放就没有"在播"这回事；速度 ≤ 0 时时间不前进
    /// ——负速的语义是"从第 0 帧起倒放"，而第 0 帧倒着走只会钳回 0，画面纹丝不动，
    /// 继续请求出帧就是白烧 CPU（倒放要从剪辑末尾起步才有意义，留到 v2）。
    pub fn is_active(&self) -> bool {
        self.autoplay && self.speed > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认值的语义承诺：挂上就播、原速、循环。
    #[test]
    fn default_plays_the_first_clip_in_a_loop() {
        let a = Animator::default();
        assert!(a.autoplay, "默认应当自动播放");
        assert!(a.looping, "默认应当循环");
        assert!((a.speed - 1.0).abs() < f32::EPSILON, "默认应当原速");
        assert!(a.is_active());
    }

    /// 旧场景文件没有 `animator` 字段 → `None`；半截配置由结构体默认补全。
    #[test]
    fn partial_ron_config_fills_missing_fields_from_defaults() {
        #[derive(Deserialize)]
        struct Wrap {
            #[serde(default)]
            animator: Animator,
        }
        let partial: Wrap = ron::from_str("(animator: (clip: \"Run\"))").unwrap();
        assert_eq!(partial.animator.clip, "Run");
        assert!(partial.animator.looping, "缺失的 looping 应补成默认 true");
        assert!(partial.animator.autoplay, "缺失的 autoplay 应补成默认 true");
        assert!(
            (partial.animator.speed - 1.0).abs() < f32::EPSILON,
            "缺失的 speed 应补成 1.0"
        );

        // 整个字段都缺失 → 用整套默认（clip 空 = 不动画）。
        let none: Wrap = ron::from_str("()").unwrap();
        assert_eq!(none.animator.clip, "", "完全没有 animator 字段时应是默认值");
        assert!(none.animator.looping);
    }

    /// 没开自动播放 / 速度不为正 → 不请求出帧（否则编辑器会满速空转）。
    #[test]
    fn inactive_when_paused_or_stationary() {
        let mut a = Animator::new("Walk");
        assert!(a.is_active());
        a.autoplay = false;
        assert!(!a.is_active(), "关掉自动播放就不该再请求出帧");
        a.autoplay = true;
        a.speed = 0.0;
        assert!(!a.is_active(), "速度 0 时时间不前进，不该白烧帧");
    }

    /// 负速 = 从第 0 帧起倒放 → 钳回 0、画面不动 → **不该**请求出帧。
    /// （真正的倒放需要"从剪辑末尾起步"，是 v2 的事；v1 在这里显式声明语义，
    /// 免得未来有人以为是 bug 又把满速空转加回来。）
    #[test]
    fn negative_speed_is_stationary_in_v1_and_must_not_spin_frames() {
        let mut a = Animator::new("Walk");
        a.speed = -1.0;
        assert!(!a.is_active(), "v1 负速停在第 0 帧，不该请求出帧");
    }
}
