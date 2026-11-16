//! 关节轨道：clip 里按 joint 索引的本地变换关键帧。

use super::{
    loop_time, normalize_quat, slerp, AnimationClip, RotationKey, ScaleKey, TranslationKey,
};
use xgc_base::EntityId;
use xgc_world::{Parent, World};
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JointTrack {
    pub joint: u32,
    pub translation: Vec<TranslationKey>,
    pub rotation: Vec<RotationKey>,
    pub scale: Vec<ScaleKey>,
}

impl AnimationClip {
    pub fn joint_track(&self, joint: u32) -> Option<&JointTrack> {
        self.joints.iter().find(|track| track.joint == joint)
    }

    pub fn sample_joint_looped(
        &self,
        joint: u32,
        time: f32,
    ) -> Option<([f32; 3], [f32; 4], [f32; 3])> {
        let track = self.joint_track(joint)?;
        let time = loop_time(time, self.duration);
        Some((
            sample_translation_keys(&track.translation, time),
            sample_rotation_keys(&track.rotation, time),
            sample_scale_keys(&track.scale, time),
        ))
    }
}
pub(super) fn sample_translation_keys(keys: &[TranslationKey], time: f32) -> [f32; 3] {
    let Some(first) = keys.first() else {
        return [0.0; 3];
    };
    let last = keys.last().expect("非空");
    if !time.is_finite() || time <= first.time {
        return first.value;
    }
    if time >= last.time {
        return last.value;
    }
    let upper = keys.partition_point(|key| key.time <= time);
    let (left, right) = (&keys[upper - 1], &keys[upper]);
    let span = right.time - left.time;
    let t = if span > f32::EPSILON {
        (time - left.time) / span
    } else {
        0.0
    };
    [
        left.value[0] + (right.value[0] - left.value[0]) * t,
        left.value[1] + (right.value[1] - left.value[1]) * t,
        left.value[2] + (right.value[2] - left.value[2]) * t,
    ]
}

pub(super) fn sample_rotation_keys(keys: &[RotationKey], time: f32) -> [f32; 4] {
    let Some(first) = keys.first() else {
        return [0.0, 0.0, 0.0, 1.0];
    };
    let last = keys.last().expect("非空");
    if !time.is_finite() || time <= first.time {
        return normalize_quat(first.value);
    }
    if time >= last.time {
        return normalize_quat(last.value);
    }
    let upper = keys.partition_point(|key| key.time <= time);
    let (left, right) = (&keys[upper - 1], &keys[upper]);
    let span = right.time - left.time;
    let t = if span > f32::EPSILON {
        (time - left.time) / span
    } else {
        0.0
    };
    slerp(left.value, right.value, t)
}

pub(super) fn sample_scale_keys(keys: &[ScaleKey], time: f32) -> [f32; 3] {
    let Some(first) = keys.first() else {
        return [1.0; 3];
    };
    let last = keys.last().expect("非空");
    if !time.is_finite() || time <= first.time {
        return first.value;
    }
    if time >= last.time {
        return last.value;
    }
    let upper = keys.partition_point(|key| key.time <= time);
    let (left, right) = (&keys[upper - 1], &keys[upper]);
    let span = right.time - left.time;
    let t = if span > f32::EPSILON {
        (time - left.time) / span
    } else {
        0.0
    };
    [
        left.value[0] + (right.value[0] - left.value[0]) * t,
        left.value[1] + (right.value[1] - left.value[1]) * t,
        left.value[2] + (right.value[2] - left.value[2]) * t,
    ]
}

pub(crate) fn descendant_of(world: &World, key: xgc_base::EntityKey, root: EntityId) -> bool {
    let mut current = key;
    for _ in 0..128 {
        let Some(parent) = world.get::<Parent>(current) else {
            return false;
        };
        if parent.entity == root {
            return true;
        }
        let Some(next) = world.key(parent.entity) else {
            return false;
        };
        current = next;
    }
    false
}
