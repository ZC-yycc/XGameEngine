//! 音频子系统的**纯数据层**：组件、命令束、监听者姿态。
//!
//! # 分层（为什么这些类型住在 core 而不是 xgame-audio）
//!
//! ```text
//! core（本模块：AudioSource / AudioPort / AudioCommand）
//!   ↑
//! xgame-audio（kira 封装 AudioEngine：解码 / 混音 / 空间化）
//!   ↑
//! runtime::World（持 port + engine，帧末 drain 一次）
//! ```
//!
//! 命令束住在这里的直接收益：
//!
//! - **脚本层不引 kira**（`xgame-script` 只依赖 core）——脚本 API 生成命令、
//!   World 帧末消费，两层之间只传纯数据。kira 的链接产物（cpal / symphonia）
//!   与它自带的 glam 版本（0.33，引擎是 0.29）都关在 `xgame-audio` 一道墙里；
//! - **无声降级免费**：没有声卡的环境（CI、无头测试）里 `AudioEngine` 是
//!   silent 模式，但命令照常受理、计数照常递增 —— 上层（World 胶水、
//!   脚本 API）的回归测试完全不需要音频设备；
//! - 命令束天然成了**可观测断面**：测试断言"这一帧产生了什么命令"，
//!   不用碰任何音频后端。
//!
//! # 坐标约定（kira 监听者姿态）
//!
//! kira 的约定：监听者未旋转时 **-Z 朝前、+X 在右、+Y 在上**。引擎里有两个
//! 互相矛盾的"右"：`Transform::right()` = `up × forward`，而相机帧
//! （`EditorCamera::pan_right`）= `forward × up` —— 两者互为相反数。音频
//! 统一采用**相机/物理约定**（`right = forward × up`）：监听者就是"玩家的
//! 眼睛"，它属于相机帧。实体帧的右耳在左的问题由此避开。

use std::fmt;

use glam::{Mat3, Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::id::EntityId;

// ---------------------------------------------------------------------------
// 组件
// ---------------------------------------------------------------------------

/// 实体上的音频源组件：一个可循环、可空间化的音效声明。
///
/// `None`（实体没挂）= 纯静默实体。与 `physics` / `animator` 完全同款的
/// 兼容策略：`#[serde(default)]` 让旧场景文件加载成 `None`，
/// `skip_serializing_if` 让没配音频的实体不写这一行。
///
/// `autoplay` 默认 `true` 是刻意的：给实体挂音频源的作者意图几乎总是
/// "进场景就响"（背景氛围、机关音）；要"等脚本触发"就把 `autoplay`
/// 关掉、脚本里 `play_sound` 播。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSource {
    /// 音频文件的**项目相对路径**（如 `Resources/Audio/Beep.wav`）。
    pub clip: String,
    /// 线性音量（`1.0` = 原音量，`0.0` = 静音）。
    pub volume: f32,
    /// 播放速率（`1.0` = 原速；同时改变速度与音高，与 kira 语义一致）。
    pub pitch: f32,
    /// 是否循环。
    pub looping: bool,
    /// 进场景后自动播放一次。
    pub autoplay: bool,
    /// 是否空间化（3D 定位，随实体移动更新发射位置）。
    pub spatial: bool,
    /// 空间化：满音量的最近距离。
    pub min_distance: f32,
    /// 空间化：听不见的最远距离。
    pub max_distance: f32,
}

impl Default for AudioSource {
    fn default() -> Self {
        Self {
            clip: String::new(),
            volume: 1.0,
            pitch: 1.0,
            looping: false,
            autoplay: true,
            spatial: false,
            min_distance: 1.0,
            max_distance: 50.0,
        }
    }
}

impl AudioSource {
    pub fn new(clip: impl Into<String>) -> Self {
        Self {
            clip: clip.into(),
            ..Self::default()
        }
    }

    pub fn with_volume(mut self, volume: f32) -> Self {
        self.volume = volume;
        self
    }

    pub fn with_pitch(mut self, pitch: f32) -> Self {
        self.pitch = pitch;
        self
    }

    pub fn with_looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }

    pub fn with_autoplay(mut self, autoplay: bool) -> Self {
        self.autoplay = autoplay;
        self
    }

    pub fn with_spatial(mut self, spatial: bool) -> Self {
        self.spatial = spatial;
        self
    }

    /// 把组件参数摊成一次播放请求。
    ///
    /// `position` 是世界坐标（空间化时用）；`follow` 由调用方（World）按需
    /// 填 —— 组件自己不知道"该跟着哪个实体"，但 autoplay 的空间声源
    /// 必然跟着挂载实体，World 那边补上。
    pub fn to_play_sound(&self, position: Option<[f32; 3]>) -> PlaySound {
        PlaySound {
            clip: self.clip.clone(),
            volume: self.volume,
            pitch: self.pitch,
            looping: self.looping,
            position,
            min_distance: self.min_distance,
            max_distance: self.max_distance,
            follow: None,
        }
    }
}

// ---------------------------------------------------------------------------
// 播放请求与命令
// ---------------------------------------------------------------------------

/// 一次播放请求（脚本 `play_sound*` 与 `AudioSource` autoplay 的共同载体）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlaySound {
    /// 音频文件的项目相对路径。
    pub clip: String,
    /// 线性音量（`0.0..=1.0`，超界会被引擎 clamp）。
    pub volume: f32,
    /// 播放速率（`1.0` = 原速）。
    pub pitch: f32,
    /// 是否循环。
    pub looping: bool,
    /// 空间化位置（世界坐标）。`None` = 非空间化（UI 音、全局音）。
    pub position: Option<[f32; 3]>,
    /// 空间化最近距离。
    pub min_distance: f32,
    /// 空间化最远距离。
    pub max_distance: f32,
    /// 跟随的实体：**无论是否空间化**，它都把声音绑到该实体上
    /// （`StopEntity` 按它匹配 —— 实体删除 / 组件摘除时停音）；
    /// 空间化声源还按实体世界坐标逐帧更新发射位置（**覆盖** `position`）。
    /// 移动平台的脚步声、载具引擎声都靠它。
    pub follow: Option<EntityId>,
}

impl PlaySound {
    pub fn new(clip: impl Into<String>) -> Self {
        Self {
            clip: clip.into(),
            volume: 1.0,
            pitch: 1.0,
            looping: false,
            position: None,
            min_distance: 1.0,
            max_distance: 50.0,
            follow: None,
        }
    }

    /// 摆到世界坐标上（空间化）。
    pub fn at(mut self, position: [f32; 3]) -> Self {
        self.position = Some(position);
        self
    }

    pub fn with_volume(mut self, volume: f32) -> Self {
        self.volume = volume;
        self
    }

    pub fn with_pitch(mut self, pitch: f32) -> Self {
        self.pitch = pitch;
        self
    }

    pub fn looping(mut self) -> Self {
        self.looping = true;
        self
    }

    /// 跟随一个实体（`follow` 覆盖 `position`）。
    pub fn following(mut self, entity: EntityId) -> Self {
        self.follow = Some(entity);
        self
    }
}

/// 帧末要引擎执行的音频命令。
#[derive(Debug, Clone, PartialEq)]
pub enum AudioCommand {
    /// 播放一次。引擎返回的句柄记进"活动声音"表（停止、GC 都靠它）。
    Play(PlaySound),
    /// 更新某实体关联的发射位置（空间化移动声源）。
    EmitterPosition {
        entity: EntityId,
        position: [f32; 3],
    },
    /// 停止某个 clip 的全部实例（UI 关窗就把对应音停掉）。
    StopClip(String),
    /// 停止某实体关联的全部声音（实体被销毁时）。
    StopEntity(EntityId),
    /// 停止一切（静音开关）。
    StopAll,
}

// ---------------------------------------------------------------------------
// 监听者姿态
// ---------------------------------------------------------------------------

/// 监听者（"玩家的耳朵"）在世界里的位置与朝向。
///
/// 只暴露 `(position, forward)` 两个自由度：世界 up 恒为 `+Y`。脚本作者
/// 填一个"我在哪、我看哪"就够了，roll（歪头）在游戏音频里没有用武之地，
/// 少一个自由度就少一整类"耳机左右反了"的 bug。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListenerPose {
    /// 世界坐标。
    pub position: [f32; 3],
    /// 朝向（会被归一化；零向量退化为 -Z）。
    pub forward: [f32; 3],
}

impl Default for ListenerPose {
    /// 与 `xgame-game` 的默认相机同款：站在 `(5,5,5)` 看向原点 ——
    /// 编辑器忘了每帧更新监听者时，听到的也大差不差。forward 存归一化值
    /// （与 `new()` 一致：字段不变量是"单位向量"）。
    fn default() -> Self {
        Self::new([5.0, 5.0, 5.0], [-1.0, -1.0, -1.0])
    }
}

impl ListenerPose {
    pub fn new(position: [f32; 3], forward: [f32; 3]) -> Self {
        Self {
            position,
            forward: normalize_or_back(forward),
        }
    }

    /// 由 `(position, forward)` 算出的朝向四元数 `[x, y, z, w]`。
    ///
    /// 基向量：`up = +Y`（世界恒定），`right = forward × up`（相机帧，
    /// 见模块文档），`z = right × up = −forward`（kira 的 +Z 在脑后）。
    /// 旋转矩阵按列排 `[right, up, z]` 即得"未旋转时 -Z 朝前、+X 在右"。
    ///
    /// 朝向接近竖直（forward ≈ ±Y）时 `right` 退化，按惯例取 `+X`：
    /// 矩阵不塌、监听者水平朝前 —— 仰望天空时声音定位短暂失去上下感，
    /// 这个取舍远好于四元数变成 NaN。
    pub fn orientation(&self) -> [f32; 4] {
        let forward = Vec3::from(normalize_or_back(self.forward));
        let up = Vec3::Y;
        let right = {
            let r = forward.cross(up);
            if r.length_squared() < 1e-8 { Vec3::X } else { r.normalize() }
        };
        let z = right.cross(up);
        let q = Quat::from_mat3(&Mat3::from_cols(right, up, z));
        [q.x, q.y, q.z, q.w]
    }
}

/// 归一化；长度可忽略时退回 `-Z`（kira 的"向前"）。
fn normalize_or_back(v: [f32; 3]) -> [f32; 3] {
    let v = Vec3::from(v);
    if v.length_squared() < 1e-12 {
        [0.0, 0.0, -1.0]
    } else {
        v.normalize().to_array()
    }
}

// ---------------------------------------------------------------------------
// 命令束
// ---------------------------------------------------------------------------

/// 一帧音频命令的收集器：World 每帧把它 `drain` 给 `AudioEngine`。
///
/// 与脚本层的 `ScriptCommand` 缓冲完全同款的模式 —— 产生方（组件、脚本）
/// 只管往里塞，消费方（World 帧末）一次取走。
#[derive(Debug, Default)]
pub struct AudioPort {
    /// 监听者姿态（每帧由宿主写入：编辑器相机 / 游戏相机）。
    pub listener: ListenerPose,
    /// 主音量（线性 `0.0..=1.0`，引擎 clamp）。
    pub master_volume: f32,
    /// 待执行命令。私有：只能经方法进 —— 保证每个命令都带完整的默认语义。
    commands: Vec<AudioCommand>,
}

impl AudioPort {
    pub fn new() -> Self {
        Self::default()
    }

    /// 更新监听者（位置 + 朝向，up 恒 +Y）。
    pub fn set_listener(&mut self, position: [f32; 3], forward: [f32; 3]) {
        self.listener = ListenerPose::new(position, forward);
    }

    /// 设置主音量（clamp 到 `0.0..=1.0`）。
    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = volume.clamp(0.0, 1.0);
    }

    /// 请求播放一次。
    pub fn play(&mut self, sound: PlaySound) {
        self.commands.push(AudioCommand::Play(sound));
    }

    /// 更新某实体的空间化声源位置。
    pub fn emitter_position(&mut self, entity: EntityId, position: [f32; 3]) {
        self.commands.push(AudioCommand::EmitterPosition { entity, position });
    }

    /// 停止某 clip 的全部实例。
    pub fn stop_clip(&mut self, clip: impl Into<String>) {
        self.commands.push(AudioCommand::StopClip(clip.into()));
    }

    /// 停止某实体的全部声音。
    pub fn stop_entity(&mut self, entity: EntityId) {
        self.commands.push(AudioCommand::StopEntity(entity));
    }

    /// 停止一切声音。
    pub fn stop_all(&mut self) {
        self.commands.push(AudioCommand::StopAll);
    }

    /// 取走全部待执行命令（帧末调用）。
    ///
    /// 直接 `std::mem::take` 而不复用缓冲：一帧的命令数通常是个位数，
    /// 为省这点分配去维护"借出窗口"不值 —— 与 `LuaHost::take_commands`
    /// 同款取舍。
    pub fn drain(&mut self) -> Vec<AudioCommand> {
        std::mem::take(&mut self.commands)
    }

    /// 待执行命令数（测试 / 调试用）。
    pub fn pending(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

// ---------------------------------------------------------------------------
// 统计
// ---------------------------------------------------------------------------

/// 音频子系统的可观测状态（编辑器状态栏 / 测试断言用）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioStats {
    /// 是否真的连上了输出设备（`false` = 无声降级：照常受理，只是不出声）。
    pub available: bool,
    /// 累计受理的播放请求（**解码前**递增 —— clip 缺失也算，World 胶水的
    /// 回归测试因此不需要真实音频文件）。
    pub requests: u64,
    /// 当前活动声音数。
    pub active: usize,
    /// 累计结束 / 被停的声音数。
    pub stopped: u64,
    /// 已缓存的 clip 数。
    pub cached_clips: usize,
    /// 解码失败过的 clip 数（失败会记住，不逐帧重试解码）。
    pub failed_clips: usize,
    /// 上一批 drain 进来的命令数（"这帧音频层干没干活"一眼可见）。
    pub last_batch: usize,
}

impl fmt::Display for AudioStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "audio: {} requests={} active={} stopped={} clips={}/{} last_batch={}",
            if self.available { "device" } else { "silent" },
            self.requests,
            self.active,
            self.stopped,
            self.cached_clips,
            self.failed_clips,
            self.last_batch,
        )
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 组件 RON 往返：全字段写入 → 解析 → 相等。
    #[test]
    fn audio_source_ron_round_trips() {
        let source = AudioSource::new("Resources/Audio/Beep.wav")
            .with_volume(0.5)
            .with_pitch(1.5)
            .with_looping(true)
            .with_autoplay(false)
            .with_spatial(true);
        let text = ron::to_string(&source).expect("序列化");
        let back: AudioSource = ron::from_str(&text).expect("反序列化");
        assert_eq!(source, back);
    }

    /// 省略字段的 RON：结构体级 `#[serde(default)]` 逐字段补默认 ——
    /// 旧场景文件（没有 audio 字段的年代）与手写精简场景都靠这条。
    #[test]
    fn audio_source_ron_defaults_when_fields_omitted() {
        let source: AudioSource = ron::from_str(r#"(clip: "Resources/Audio/Beep.wav")"#)
            .expect("省略字段应可解析");
        assert_eq!(source.clip, "Resources/Audio/Beep.wav");
        assert_eq!(source.volume, 1.0);
        assert_eq!(source.pitch, 1.0);
        assert!(!source.looping);
        assert!(source.autoplay, "autoplay 默认应为 true");
        assert!(!source.spatial);
        assert_eq!(source.min_distance, 1.0);
        assert_eq!(source.max_distance, 50.0);
    }

    /// 命令束：各种命令按序进、drain 一次取空；主音量 clamp。
    #[test]
    fn audio_port_collects_commands_in_order() {
        let mut port = AudioPort::new();
        let id = EntityId::new();

        port.set_listener([1.0, 2.0, 3.0], [0.0, 0.0, -1.0]);
        port.play(PlaySound::new("a.wav").at([0.0, 0.0, 0.0]));
        port.emitter_position(id, [4.0, 0.0, 0.0]);
        port.stop_clip("a.wav");
        port.stop_entity(id);
        port.stop_all();
        assert_eq!(port.pending(), 5);
        assert!(!port.is_empty());

        let commands = port.drain();
        assert!(matches!(&commands[0], AudioCommand::Play(p) if p.clip == "a.wav"));
        assert!(matches!(
            &commands[1],
            AudioCommand::EmitterPosition { entity, position: [4.0, 0.0, 0.0] } if *entity == id
        ));
        assert!(matches!(&commands[2], AudioCommand::StopClip(c) if c == "a.wav"));
        assert!(matches!(&commands[3], AudioCommand::StopEntity(e) if *e == id));
        assert!(matches!(&commands[4], AudioCommand::StopAll));
        assert!(port.is_empty(), "drain 之后应取空");

        port.set_master_volume(3.0);
        assert_eq!(port.master_volume, 1.0, "主音量必须 clamp");
        port.set_master_volume(-1.0);
        assert_eq!(port.master_volume, 0.0);
    }

    /// 监听者姿态的坐标约定（与 kira"未旋转 = -Z 朝前、+X 在右"对齐）：
    /// 看向 -Z 时是单位四元数；看向 +Z 时是绕 Y 轴 180°。
    #[test]
    fn listener_orientation_matches_camera_convention() {
        // 看向 -Z（引擎的"前"）：监听者不旋转。
        let pose = ListenerPose::new([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]);
        let q = pose.orientation();
        assert!(q[0].abs() < 1e-5 && q[1].abs() < 1e-5 && q[2].abs() < 1e-5);
        assert!((q[3] - 1.0).abs() < 1e-5, "看向 -Z 应为单位四元数: {q:?}");

        // 看向 +Z：绕 Y 轴 180°（y=1, w=0）。
        let q = ListenerPose::new([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]).orientation();
        assert!(q[1].abs() > 0.999, "看向 +Z 应绕 Y 轴 180°: {q:?}");
        assert!(q[3].abs() < 1e-5);

        // 右耳检验：把监听者姿态的 quat 作用到局部 +X（kira 约定的"右"）上。
        // 看向 -Z：不旋转，局部 +X 就是世界 +X；看向 +Z：绕 Y 轴 180°，
        // 局部 +X 翻到世界 -X —— 转身后世界 +X 确实到了左手边，物理正确。
        let q_front = ListenerPose::new([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]).orientation();
        let right = Quat::from_xyzw(q_front[0], q_front[1], q_front[2], q_front[3]) * Vec3::X;
        assert!((right - Vec3::X).length() < 1e-5, "转身前右耳在 +X: {right:?}");
        let right = Quat::from_xyzw(q[0], q[1], q[2], q[3]) * Vec3::X;
        assert!((right + Vec3::X).length() < 1e-5, "转身后 +X 到左手边: {right:?}");

        // 默认姿态：站在 (5,5,5) 看向原点 —— forward 会被归一化。
        let pose = ListenerPose::default();
        assert_eq!(pose.position, [5.0, 5.0, 5.0]);
        let len = pose.forward.iter().map(|c| c * c).sum::<f32>().sqrt();
        assert!((len - 1.0).abs() < 1e-5, "默认 forward 应为单位向量");
        assert!(pose.forward.iter().all(|c| *c < 0.0), "看向原点应各分量为负");
    }

    /// 竖直朝向（forward ≈ +Y）不产生 NaN：退化分支必须给出合法四元数。
    #[test]
    fn vertical_forward_does_not_produce_nan() {
        let q = ListenerPose::new([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]).orientation();
        assert!(q.iter().all(|c| c.is_finite()), "竖直朝向不能出 NaN: {q:?}");

        // 零向量 forward 退回 -Z（单位四元数）。
        let q = ListenerPose::new([0.0, 0.0, 0.0], [0.0, 0.0, 0.0]).orientation();
        assert!(q[3].is_finite() && (q[3] - 1.0).abs() < 1e-5);
    }

    /// `AudioSource::to_play_sound` 把组件参数摊平到播放请求。
    #[test]
    fn source_maps_to_play_sound() {
        let source = AudioSource::new("x.wav").with_volume(0.3).with_pitch(2.0).with_looping(true);
        let play = source.to_play_sound(Some([1.0, 2.0, 3.0]));
        assert_eq!(play.clip, "x.wav");
        assert_eq!(play.volume, 0.3);
        assert_eq!(play.pitch, 2.0);
        assert!(play.looping);
        assert_eq!(play.position, Some([1.0, 2.0, 3.0]));
        assert_eq!(play.follow, None, "follow 由 World 补，组件不知道实体");
    }
}
