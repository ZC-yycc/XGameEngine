//! 根运动：把基础剪辑里某个显式节点的平移增量交给实体位置。
//!
//! 语义逐条对齐旧端 `crates/xgame-core/src/animation.rs` 的 `RootMotion` 与
//! `crates/xgame-core/src/animation_runtime.rs::root_motion_delta`：
//!
//! - 节点名由场景显式写（glTF 常同时有场景根 / 骨架包装根 / 髋骨，猜第一个根
//!   节点会把不同 DCC 导出的资产解释成不同结果）。
//! - 只取**基础通道**（`Animator.clip` + 交叉淡入的旧剪辑）；additive / layers
//!   属骨架姿势，不参与根运动。
//! - 增量是**模型局部空间**的位移，交给宿主乘实体世界矩阵的线性部分；
//!   渲染侧同时把这些轴的髋骨平移改回绑定姿态（`xgc-render/src/glb/root_motion.rs`），
//!   否则实体会前进两次。
//! - 循环剪辑必须按「展开轨迹」相减：先对两端取模再减会把跨循环边界的那一帧
//!   算成瞬移回起点。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

mod spline;
pub use spline::CubicSplineSegment;

/// 根运动只取水平面：走路剪辑的髋部起伏不该把角色抬离地面。
pub const ROOT_MOTION_AXES_DEFAULT: [bool; 3] = [true, false, true];

/// 一个实体的根运动配置（落在 `animation.animator.root_motion` 字段的 RON 里）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RootMotion {
    /// 提取平移的 glTF 节点名；空串或找不到节点时不产生位移。
    pub node: String,
    /// X / Y / Z 逐轴开关。
    pub axes: [bool; 3],
}

impl Default for RootMotion {
    fn default() -> Self {
        Self {
            node: String::new(),
            axes: ROOT_MOTION_AXES_DEFAULT,
        }
    }
}

impl RootMotion {
    /// 解析组件字段里的 RON 字符串；坏数据一律当成「关闭」，不让一条配置错误
    /// 把实体位置写飞。
    pub fn parse(text: &str) -> Option<Self> {
        let motion: Self = ron::from_str(text).ok()?;
        if motion.node.is_empty() {
            return None;
        }
        Some(motion)
    }

    /// 序列化成组件字段的 RON 形状（与旧端 `RootMotion` 同布局）。
    pub fn to_ron(&self) -> Result<String, String> {
        ron::ser::to_string(self).map_err(|err| format!("根运动配置序列化失败：{err}"))
    }

    pub fn is_active(&self) -> bool {
        !self.node.is_empty()
    }
}

/// 一条根节点平移轨道。关键帧值与时间轴等长，逐帧三维。
#[derive(Clone, Debug, PartialEq)]
pub struct RootMotionTrack {
    times: Vec<f32>,
    values: RootMotionValues,
}

#[derive(Clone, Debug, PartialEq)]
enum RootMotionValues {
    Linear(Vec<[f32; 3]>),
    Step(Vec<[f32; 3]>),
    /// 每帧依次是入切线、值、出切线。
    CubicSpline(Vec<[[f32; 3]; 3]>),
}

impl RootMotionTrack {
    pub fn new(times: Vec<f32>, values: Vec<[f32; 3]>, step: bool) -> Option<Self> {
        if !Self::valid_times(&times, values.len()) {
            return None;
        }
        let values = if step {
            RootMotionValues::Step(values)
        } else {
            RootMotionValues::Linear(values)
        };
        Some(Self { times, values })
    }

    /// 接收 glTF 按入切线、值、出切线排列的关键帧。
    pub fn new_cubic_spline(times: Vec<f32>, keys: Vec<[[f32; 3]; 3]>) -> Option<Self> {
        if !Self::valid_times(&times, keys.len()) || times.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return None;
        }
        Some(Self {
            times,
            values: RootMotionValues::CubicSpline(keys),
        })
    }

    fn valid_times(times: &[f32], keys: usize) -> bool {
        !times.is_empty() && times.len() == keys && times.iter().all(|time| time.is_finite())
    }

    /// 两端钳到首末关键值，样条和渲染姿势使用相同的段定位与求值。
    pub fn sample(&self, time: f32) -> [f32; 3] {
        let (values, step) = match &self.values {
            RootMotionValues::Linear(values) => (values, false),
            RootMotionValues::Step(values) => (values, true),
            RootMotionValues::CubicSpline(keys) => {
                let segment = CubicSplineSegment::at(&self.times, time);
                let [_, start, outgoing] = keys[segment.lower];
                let [incoming, end, _] = keys[segment.upper];
                return segment.sample(start, outgoing, end, incoming);
            }
        };
        if time.is_nan() || time <= self.times[0] {
            return values[0];
        }
        let last = self.times.len() - 1;
        if time >= self.times[last] {
            return values[last];
        }
        let upper = self.times.partition_point(|&key| key < time).min(last);
        let lower = upper.saturating_sub(1);
        let span = self.times[upper] - self.times[lower];
        if step || span <= 0.0 {
            return values[lower];
        }
        let u = ((time - self.times[lower]) / span).clamp(0.0, 1.0);
        lerp3(values[lower], values[upper], u)
    }
}

/// 一个节点的平移轨道（按节点名索引，不在依赖节点下标——渲染侧与运行时
/// 对同一模型的节点编号没有跨进程契约）。
#[derive(Clone, Debug, PartialEq)]
pub struct RootMotionNodeTrack {
    pub node: String,
    pub track: RootMotionTrack,
}

/// 一个剪辑的根运动轨道集合（通常只有一条）。
#[derive(Clone, Debug, PartialEq)]
pub struct RootMotionClip {
    pub name: String,
    /// 剪辑时长（秒）；循环展开用。
    pub duration: f32,
    pub tracks: Vec<RootMotionNodeTrack>,
}

impl RootMotionClip {
    pub fn track(&self, node: &str) -> Option<&RootMotionTrack> {
        self.tracks
            .iter()
            .find(|entry| entry.node == node)
            .map(|entry| &entry.track)
    }
}

/// 一个模型位移动画集合。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RootMotionSet {
    pub clips: Vec<RootMotionClip>,
}

impl RootMotionSet {
    pub fn clip(&self, name: &str) -> Option<&RootMotionClip> {
        self.clips.iter().find(|clip| clip.name == name)
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }
}

/// 按模型串缓存的根运动轨道表（宿主在加载世界时建一次）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RootMotionLibrary {
    sets: BTreeMap<String, RootMotionSet>,
}

impl RootMotionLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, model: String, set: RootMotionSet) {
        if !set.is_empty() {
            self.sets.insert(model, set);
        }
    }

    pub fn set(&self, model: &str) -> Option<&RootMotionSet> {
        self.sets.get(model)
    }

    pub fn len(&self) -> usize {
        self.sets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }
}

/// 基础通道这一帧走的时间区间（秒）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSpan<'a> {
    pub clip: &'a str,
    pub from: f32,
    pub to: f32,
    pub looping: bool,
}

/// 交叉淡入里旧剪辑这一帧走的时间区间与混合系数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionFade<'a> {
    pub clip: &'a str,
    pub from: f32,
    pub to: f32,
    pub looping: bool,
    pub alpha: f32,
}

/// 本帧的模型局部根位移（已按轴过滤）。
///
/// `None` = 配置 / 剪辑 / 节点缺失或数据非有限，调用方原样不动实体。
pub fn root_motion_delta(
    set: &RootMotionSet,
    motion: &RootMotion,
    current: MotionSpan<'_>,
    fade: Option<MotionFade<'_>>,
) -> Option<[f32; 3]> {
    if !motion.is_active() {
        return None;
    }
    let current_delta = span_delta(set, motion.node.as_str(), current)?;
    let delta = match fade {
        Some(fade) => match span_delta(
            set,
            motion.node.as_str(),
            MotionSpan {
                clip: fade.clip,
                from: fade.from,
                to: fade.to,
                looping: fade.looping,
            },
        ) {
            // 旧剪辑缺失时退回新剪辑（与交叉淡入渲染侧的兜底一致）：
            // 少算一帧位移比卡住不动更接近真实意图。
            None => current_delta,
            Some(old_delta) => {
                let alpha = if fade.alpha.is_finite() {
                    fade.alpha.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let mut blended = [0.0; 3];
                for axis in 0..3 {
                    blended[axis] = old_delta[axis] * (1.0 - alpha) + current_delta[axis] * alpha;
                }
                blended
            }
        },
        None => current_delta,
    };
    if delta.iter().any(|value| !value.is_finite()) {
        return None;
    }
    Some(filter_axes(delta, motion.axes))
}

fn span_delta(set: &RootMotionSet, node: &str, span: MotionSpan<'_>) -> Option<[f32; 3]> {
    let clip = set.clip(span.clip)?;
    let track = clip.track(node)?;
    if !clip.duration.is_finite() || clip.duration <= 0.0 {
        return None;
    }
    if !span.from.is_finite() || !span.to.is_finite() {
        return None;
    }
    if span.looping {
        Some(sub3(
            unfolded(track, clip.duration, span.to),
            unfolded(track, clip.duration, span.from),
        ))
    } else {
        Some(sub3(track.sample(span.to), track.sample(span.from)))
    }
}

/// 循环剪辑的展开轨迹：`(last - first) * floor(t / duration) + (sample(t) - first)`。
fn unfolded(track: &RootMotionTrack, duration: f32, time: f32) -> [f32; 3] {
    let time = time.max(0.0);
    let first = track.sample(0.0);
    let last = track.sample(duration);
    let cycles = (time / duration).floor();
    let local = track.sample(time % duration);
    add3(scale3(sub3(last, first), cycles), sub3(local, first))
}

/// 按配置的轴掩码清零。
pub fn filter_axes(delta: [f32; 3], axes: [bool; 3]) -> [f32; 3] {
    [
        if axes[0] { delta[0] } else { 0.0 },
        if axes[1] { delta[1] } else { 0.0 },
        if axes[2] { delta[2] } else { 0.0 },
    ]
}

/// 世界位置 → 父节点局部位置（`to_world_delta` 的逆运算）。
///
/// 与 `to_world_delta` 同一处近似：TRS 分解无法表达父级的剪切。
pub fn to_local_position(parent: &crate::GlobalTransform, world: [f32; 3]) -> [f32; 3] {
    let offset = [
        world[0] - parent.position[0],
        world[1] - parent.position[1],
        world[2] - parent.position[2],
    ];
    let inverse = [
        -parent.rotation[0],
        -parent.rotation[1],
        -parent.rotation[2],
        parent.rotation[3],
    ];
    let rotated = crate::components::quat_rotate(inverse, offset);
    [
        rotated[0] / parent.scale[0],
        rotated[1] / parent.scale[1],
        rotated[2] / parent.scale[2],
    ]
}

/// 模型局部位移 → 世界位移（近似 `R_global × (S_global × local)`）。
///
/// `GlobalTransform` 只有 TRS，没有剪切项；祖先「非均匀缩放 + 子级旋转」的精确
/// 线性部分无法表示。旧端用完整世界矩阵，这里接受这处近似（现代场景的父级缩放
/// 基本都是均匀的），并在文档里写明。
pub fn to_world_delta(global: &crate::GlobalTransform, local: [f32; 3]) -> [f32; 3] {
    let scaled = [
        local[0] * global.scale[0],
        local[1] * global.scale[1],
        local[2] * global.scale[2],
    ];
    crate::components::quat_rotate(global.rotation, scaled)
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(value: [f32; 3], factor: f32) -> [f32; 3] {
    [value[0] * factor, value[1] * factor, value[2] * factor]
}

fn lerp3(a: [f32; 3], b: [f32; 3], u: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * u,
        a[1] + (b[1] - a[1]) * u,
        a[2] + (b[2] - a[2]) * u,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(times: &[f32], values: &[[f32; 3]]) -> RootMotionTrack {
        RootMotionTrack::new(times.to_vec(), values.to_vec(), false).expect("轨道应有效")
    }

    fn clip(
        name: &str,
        duration: f32,
        node: &str,
        times: &[f32],
        values: &[[f32; 3]],
    ) -> RootMotionClip {
        RootMotionClip {
            name: name.to_string(),
            duration,
            tracks: vec![RootMotionNodeTrack {
                node: node.to_string(),
                track: track(times, values),
            }],
        }
    }

    #[test]
    fn root_motion_round_trips_through_ron() {
        let motion = RootMotion {
            node: "b_Hip_01".to_string(),
            axes: [true, false, true],
        };
        let text = motion.to_ron().expect("序列化应成功");
        assert_eq!(RootMotion::parse(&text), Some(motion));
        assert_eq!(RootMotion::parse(""), None);
        assert_eq!(RootMotion::parse("(node: \"\")"), None);
        assert_eq!(RootMotion::parse("not ron"), None);
    }

    #[test]
    fn sample_interpolates_and_clamps() {
        let track = track(&[0.0, 1.0], &[[0.0, 0.0, 0.0], [2.0, 4.0, 6.0]]);
        assert_eq!(track.sample(-1.0), [0.0, 0.0, 0.0]);
        assert_eq!(track.sample(0.5), [1.0, 2.0, 3.0]);
        assert_eq!(track.sample(2.0), [2.0, 4.0, 6.0]);
    }

    #[test]
    fn non_looping_delta_is_the_pairwise_difference() {
        let set = RootMotionSet {
            clips: vec![clip(
                "Walk",
                1.0,
                "hip",
                &[0.0, 0.5, 1.0],
                &[[0.0, 0.0, 0.0], [0.5, 0.0, 0.0], [1.0, 0.0, 0.5]],
            )],
        };
        let motion = RootMotion {
            node: "hip".to_string(),
            axes: [true, true, true],
        };
        let delta = root_motion_delta(
            &set,
            &motion,
            MotionSpan {
                clip: "Walk",
                from: 0.25,
                to: 0.75,
                looping: false,
            },
            None,
        );
        assert_eq!(delta, Some([0.5, 0.0, 0.25]));
    }

    #[test]
    fn looping_delta_unfolds_instead_of_wrapping() {
        // 一秒走 2 单位：0.5 → 1.5 跨过一次循环边界，应得到 2 而不是 0。
        let set = RootMotionSet {
            clips: vec![clip(
                "Walk",
                1.0,
                "hip",
                &[0.0, 1.0],
                &[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            )],
        };
        let motion = RootMotion {
            node: "hip".to_string(),
            axes: [true, false, true],
        };
        let delta = root_motion_delta(
            &set,
            &motion,
            MotionSpan {
                clip: "Walk",
                from: 0.5,
                to: 1.5,
                looping: true,
            },
            None,
        );
        assert_eq!(delta, Some([2.0, 0.0, 0.0]));
    }

    #[test]
    fn fade_blends_the_two_spans() {
        let set = RootMotionSet {
            clips: vec![
                clip(
                    "Walk",
                    1.0,
                    "hip",
                    &[0.0, 1.0],
                    &[[0.0; 3], [1.0, 0.0, 0.0]],
                ),
                clip("Run", 1.0, "hip", &[0.0, 1.0], &[[0.0; 3], [3.0, 0.0, 0.0]]),
            ],
        };
        let motion = RootMotion {
            node: "hip".to_string(),
            axes: [true, true, true],
        };
        let delta = root_motion_delta(
            &set,
            &motion,
            MotionSpan {
                clip: "Run",
                from: 0.0,
                to: 1.0,
                looping: false,
            },
            Some(MotionFade {
                clip: "Walk",
                from: 0.0,
                to: 1.0,
                looping: false,
                alpha: 0.25,
            }),
        );
        assert_eq!(delta, Some([1.5, 0.0, 0.0]));
    }

    #[test]
    fn axes_filter_and_missing_data_stay_quiet() {
        let set = RootMotionSet {
            clips: vec![clip(
                "Walk",
                1.0,
                "hip",
                &[0.0, 1.0],
                &[[0.0, 0.0, 0.0], [1.0, 2.0, 3.0]],
            )],
        };
        let motion = RootMotion {
            node: "hip".to_string(),
            axes: [true, false, true],
        };
        let span = MotionSpan {
            clip: "Walk",
            from: 0.0,
            to: 1.0,
            looping: false,
        };
        assert_eq!(
            root_motion_delta(&set, &motion, span, None),
            Some([1.0, 0.0, 3.0])
        );
        let mut missing = motion.clone();
        missing.node = "missing".to_string();
        assert_eq!(root_motion_delta(&set, &missing, span, None), None);
        let mut no_clip = motion.clone();
        no_clip.node = "hip".to_string();
        assert_eq!(
            root_motion_delta(
                &set,
                &motion,
                MotionSpan {
                    clip: "Nope",
                    ..span
                },
                None
            ),
            None
        );
    }

    #[test]
    fn local_position_inverts_scale_and_rotation() {
        let half = std::f32::consts::FRAC_PI_4;
        let parent = crate::GlobalTransform {
            position: [10.0, 0.0, 0.0],
            scale: [0.5, 0.5, 0.5],
            rotation: [0.0, half.sin(), 0.0, half.cos()],
        };
        // 世界 +X 1.0 是在父局部 +Z 2.0（旋转 90°、缩放 0.5 的逆）。
        let local = to_local_position(&parent, [11.0, 0.0, 0.0]);
        assert!(local[0].abs() < 1e-5, "{local:?}");
        assert!((local[2] - 2.0).abs() < 1e-5, "{local:?}");
    }

    #[test]
    fn world_delta_applies_scale_and_rotation() {
        // 绕 Y 转 90°：局部 +Z 变成世界 +X。
        let half = std::f32::consts::FRAC_PI_4;
        let global = crate::GlobalTransform {
            scale: [0.5, 0.5, 0.5],
            rotation: [0.0, half.sin(), 0.0, half.cos()],
            ..crate::GlobalTransform::default()
        };
        let world = to_world_delta(&global, [0.0, 0.0, 2.0]);
        assert!((world[0] - 1.0).abs() < 1e-5, "{world:?}");
        assert!(world[2].abs() < 1e-5, "{world:?}");
    }
}
