//! 音频引擎：kira 的最小封装（解码 / 混音 / 空间化），一帧一结算。
//!
//! # 边界
//!
//! 本 crate 是 kira 的**唯一入口**：kira 的类型（含它自带的 glam 0.33）
//! 一律不出这道墙 —— 对上只暴露 [`AudioEngine`] 与 core 的纯数据
//! （`AudioCommand` / `ListenerPose` / `AudioStats`）。升级 kira、换后端
//! 都只动这里。
//!
//! # 结算模型（为什么不是逐命令即时执行）
//!
//! 每帧一次 [`AudioEngine::apply`]：先刷监听者与主音量，再逐条执行
//! 命令束（`AudioPort::drain` 的产物），最后 GC 掉已结束的声音。
//! 音频命令对延迟一帧（≤16 ms）完全不敏感，而"一帧一次"换来：
//!
//! - 上层（World / 脚本）与音频后端之间**零共享状态**；
//! - 无声降级只是"换了执行器"，命令受理 / 计数逻辑一字不差 ——
//!   无头 CI 上照样能测整条胶水。
//!
//! # 无声降级
//!
//! 打不开输出设备（无卡、容器、CI）时进 `Playback::Silent`：照常解码、
//! 照常受理、照常计数，只是没有 kira 句柄、不出声。**不 mock 后端、
//! 不跑 DSP** —— 那是另一份需要维护的音频代码，而它验证不了真后端的任何东西。
//!
//! # GC
//!
//! 活动声音自记开始时刻与时长：设备模式看 `handle.state() == Stopped`，
//! 无声模式按墙钟走到时长（`pitch` 会改速率，时长按速率折算）。
//! 循环声永不 GC，只能被 `Stop*` 命令停掉 —— 与"循环声是资源声明"
//! 的语义一致。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use kira::listener::ListenerHandle;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::sound::PlaybackState;
use kira::track::{SpatialTrackBuilder, SpatialTrackDistances, SpatialTrackHandle};
use kira::{
    AudioManager, AudioManagerSettings, Capacities, Decibels, DefaultBackend, Tween,
};
use xgame_core::{AudioCommand, AudioStats, EntityId, ListenerPose, PlaySound};

/// 默认后端的管理器类型（别名只为压低签名噪音）。
type Manager = AudioManager<DefaultBackend>;

/// 空间轨道的上限容量。每个空间化声源占一个子轨道，512 同时发声
/// 对 2D/3D 混合的场景足够宽裕；超出时播放失败会被记日志（不 panic）。
const SPATIAL_TRACK_CAPACITY: usize = 512;

/// 输出后端：真设备或无声降级。
enum Playback {
    /// 真设备：管理器 + 监听者句柄（监听者必须常驻 —— Drop 即移除）。
    /// `manager` 装箱：`AudioManager` 有六百多字节，直接内联会让 `Playback`
    /// 的两个变体大小悬殊（clippy `large_enum_variant`），而它在整条热路径上
    /// 只按引用解引用，装箱零成本。
    Device {
        manager: Box<Manager>,
        listener: ListenerHandle,
    },
    /// 无声降级。
    Silent,
}

/// 一个正在跟踪的声音。
struct ActiveSound {
    /// clip 的项目相对路径（StopClip 按它匹配）。
    clip: String,
    /// 关联实体（StopEntity / EmitterPosition 按它匹配；一次性 UI 音为 `None`）。
    entity: Option<EntityId>,
    /// 是否循环（循环声不 GC）。
    looping: bool,
    /// 起播时刻（无声模式 GC 用墙钟）。
    started: Instant,
    /// 折算后的时长（秒，已除以 pitch）。
    duration: f64,
    /// 设备模式的播放句柄（无声模式为 `None`）。
    handle: Option<StaticSoundHandle>,
    /// 空间化发射轨道（Drop 即从混音树移除；非空间化为 `None`）。
    spatial: Option<SpatialTrackHandle>,
}

/// kira 封装的音频引擎。
pub struct AudioEngine {
    playback: Playback,
    /// clip 路径解析基准（项目根目录）。
    root: PathBuf,
    /// 解码缓存：clip 路径 → 共享音频数据（`StaticSoundData` 的帧是 `Arc`，
    /// 克隆是常数成本）。
    clips: HashMap<String, Arc<StaticSoundData>>,
    /// 解码失败过的 clip（记住，避免每帧重试解码刷日志）。
    failed: HashSet<String>,
    /// 正在跟踪的声音。
    sounds: Vec<ActiveSound>,
    /// 累计受理的播放请求（解码前递增）。
    requests: u64,
    /// 累计结束 / 被停的声音。
    stopped: u64,
    /// 上一批命令数。
    last_batch: usize,
}

impl AudioEngine {
    /// 尝试打开默认输出设备；失败时**无声降级**（记警告，不报错）。
    pub fn open(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let settings = AudioManagerSettings {
            capacities: Capacities {
                sub_track_capacity: SPATIAL_TRACK_CAPACITY,
                ..Capacities::default()
            },
            ..AudioManagerSettings::default()
        };
        match Manager::new(settings) {
            Ok(mut manager) => match manager.add_listener(
                // 起始姿态与 `ListenerPose::default()` 同步：第一帧更新前
                // 也不至于朝向离谱。
                [5.0f32, 5.0, 5.0],
                default_orientation_quat(),
            ) {
                Ok(listener) => {
                    log::info!("音频引擎就绪（kira 默认后端）");
                    Self::with_playback(
                        root,
                        Playback::Device {
                            manager: Box::new(manager),
                            listener,
                        },
                    )
                }
                Err(err) => {
                    log::warn!("创建音频监听者失败，进入无声模式: {err:?}");
                    Self::silent(root)
                }
            },
            Err(err) => {
                log::warn!("音频设备不可用，进入无声模式（照常受理命令，只是不出声）: {err:?}");
                Self::silent(root)
            }
        }
    }

    /// 显式无声引擎（无头测试用：不碰设备，行为与设备模式同构）。
    pub fn silent(root: impl Into<PathBuf>) -> Self {
        Self::with_playback(root.into(), Playback::Silent)
    }

    fn with_playback(root: PathBuf, playback: Playback) -> Self {
        Self {
            playback,
            root,
            clips: HashMap::new(),
            failed: HashSet::new(),
            sounds: Vec::new(),
            requests: 0,
            stopped: 0,
            last_batch: 0,
        }
    }

    /// 是否真的连着输出设备。
    pub fn is_available(&self) -> bool {
        matches!(self.playback, Playback::Device { .. })
    }

    /// 结算一批命令：刷监听者与主音量 → 逐条执行 → GC。
    pub fn apply(&mut self, commands: &[AudioCommand], listener: &ListenerPose, master_volume: f32) {
        self.last_batch = commands.len();
        self.update_listener(listener);
        self.update_master_volume(master_volume);
        for command in commands {
            match command {
                AudioCommand::Play(sound) => self.play(sound),
                AudioCommand::EmitterPosition { entity, position } => {
                    self.set_emitter_position(*entity, *position)
                }
                AudioCommand::StopClip(clip) => self.stop_clip(clip),
                AudioCommand::StopEntity(entity) => self.stop_entity(*entity),
                AudioCommand::StopAll => self.stop_all(),
            }
        }
        self.collect_finished();
    }

    /// 当前可观测状态。
    pub fn stats(&self) -> AudioStats {
        AudioStats {
            available: self.is_available(),
            requests: self.requests,
            active: self.sounds.len(),
            stopped: self.stopped,
            cached_clips: self.clips.len(),
            failed_clips: self.failed.len(),
            last_batch: self.last_batch,
        }
    }

    // -----------------------------------------------------------------------
    // 内部：监听者与主音量
    // -----------------------------------------------------------------------

    fn update_listener(&mut self, pose: &ListenerPose) {
        let Playback::Device { listener, .. } = &mut self.playback else {
            return;
        };
        // Tween::default() 是零时长 = 立即生效；监听者每帧跟随相机，
        // 平滑插值反而会造成"声音定位拖影"。
        listener.set_position(pose.position, Tween::default());
        listener.set_orientation(pose.orientation(), Tween::default());
    }

    fn update_master_volume(&mut self, volume: f32) {
        let Playback::Device { manager, .. } = &mut self.playback else {
            return;
        };
        manager
            .main_track()
            .set_volume(linear_to_decibels(volume), Tween::default());
    }

    // -----------------------------------------------------------------------
    // 内部：播放
    // -----------------------------------------------------------------------

    fn play(&mut self, request: &PlaySound) {
        // 计数在解码**之前**：clip 缺失时上层（World 胶水）的回归测试
        // 依然能断言"请求到达了音频层"。
        self.requests += 1;

        let Some(data) = self.clip_data(&request.clip) else {
            return;
        };

        // 克隆是常数成本（帧数据是 Arc —— `StaticSoundData` 的 clone 只增
        // 引用计数），每次播放的音量 / 速率 / 循环都定制在这份副本上，
        // 缓存里那份保持"出厂设置"。注意不能克隆 `Arc` 本身去播：
        // kira 只认 `StaticSoundData` 自己的 `SoundData` 实现。
        let mut data = (*data).clone();
        data.settings.volume = kira::Value::Fixed(linear_to_decibels(request.volume));
        data.settings.playback_rate = kira::Value::Fixed(kira::PlaybackRate(
            request.pitch.clamp(0.05, 16.0) as f64,
        ));
        if request.looping {
            data.settings.loop_region = Some(kira::sound::Region::from(..));
        }
        let duration = data.duration().as_secs_f64() / (request.pitch.max(0.05) as f64);
        // 空间化只看"有没有位置"：`follow` 只是实体绑定（StopEntity 匹配用），
        // 非空间化的 UI 音也会绑实体 —— 不能因为绑了实体就给它开空间轨道。
        let spatial = request.position.is_some();

        let (handle, spatial_track) = match &mut self.playback {
            Playback::Silent => (None, None),
            Playback::Device { manager, listener } => {
                if spatial {
                    // 空间化：每个声源一条子轨道（Drop 即移除）。
                    let position = request.position.unwrap_or([0.0, 0.0, 0.0]);
                    match manager.add_spatial_sub_track(
                        listener.id(),
                        position,
                        SpatialTrackBuilder::new().distances(SpatialTrackDistances {
                            min_distance: request.min_distance.max(0.001),
                            max_distance: request.max_distance.max(request.min_distance + 0.001),
                        }),
                    ) {
                        Ok(mut track) => match track.play(data) {
                            Ok(handle) => (Some(handle), Some(track)),
                            Err(err) => {
                                log::warn!("空间声启动失败 '{}': {err:?}", request.clip);
                                (None, None)
                            }
                        },
                        Err(err) => {
                            // 容量满了：降级成非空间化播放，好歹听得见。
                            log::warn!(
                                "空间轨道已满（{SPATIAL_TRACK_CAPACITY}），降级非空间化: {}",
                                request.clip
                            );
                            match manager.play(data) {
                                Ok(handle) => (Some(handle), None),
                                Err(_) => {
                                    log::warn!("非空间化降级也失败: {err:?}");
                                    (None, None)
                                }
                            }
                        }
                    }
                } else {
                    match manager.play(data) {
                        Ok(handle) => (Some(handle), None),
                        Err(err) => {
                            log::warn!("声音启动失败 '{}': {err:?}", request.clip);
                            (None, None)
                        }
                    }
                }
            }
        };

        // 播放失败（容量 / 错误）时不跟踪：句柄都没有，GC 无从谈起。
        // 无声模式是唯一"没有句柄却要跟踪"的情况 —— 它靠墙钟 GC，
        // 不跟踪的话测试就看不到"声音在响"。
        if matches!(self.playback, Playback::Device { .. }) && handle.is_none() {
            return;
        }

        self.sounds.push(ActiveSound {
            clip: request.clip.clone(),
            entity: request.follow,
            looping: request.looping,
            started: Instant::now(),
            duration,
            handle,
            spatial: spatial_track,
        });
    }

    /// 解码（带缓存）。失败进 `failed` 表，之后不再重试解码。
    fn clip_data(&mut self, clip: &str) -> Option<Arc<StaticSoundData>> {
        if let Some(data) = self.clips.get(clip) {
            return Some(Arc::clone(data));
        }
        if self.failed.contains(clip) {
            return None;
        }
        let path: PathBuf = self.root.join(clip);
        match StaticSoundData::from_file(&path) {
            Ok(data) => {
                let data = Arc::new(data);
                self.clips.insert(clip.to_owned(), Arc::clone(&data));
                Some(data)
            }
            Err(err) => {
                self.failed.insert(clip.to_owned());
                log::warn!("音频解码失败 '{}': {err:?}", path.display());
                None
            }
        }
    }

    // -----------------------------------------------------------------------
    // 内部：位置与停止
    // -----------------------------------------------------------------------

    fn set_emitter_position(&mut self, entity: EntityId, position: [f32; 3]) {
        for sound in &mut self.sounds {
            if sound.entity != Some(entity) {
                continue;
            }
            if let Some(track) = &mut sound.spatial {
                track.set_position(position, Tween::default());
            }
        }
    }

    fn stop_clip(&mut self, clip: &str) {
        self.remove_sounds(|sound| sound.clip == clip);
    }

    fn stop_entity(&mut self, entity: EntityId) {
        self.remove_sounds(|sound| sound.entity == Some(entity));
    }

    fn stop_all(&mut self) {
        self.remove_sounds(|_| true);
    }

    /// 摘掉匹配的声音（Drop 句柄 / 空间轨道即停止并释放资源）。
    fn remove_sounds(&mut self, matches: impl Fn(&ActiveSound) -> bool) {
        let before = self.sounds.len();
        self.sounds.retain(|sound| !matches(sound));
        self.stopped += (before - self.sounds.len()) as u64;
    }

    /// GC：非循环声播完即摘（设备模式看句柄状态，无声模式按墙钟）。
    fn collect_finished(&mut self) {
        let mut finished = 0usize;
        self.sounds.retain(|sound| {
            let done = !sound.looping
                && match &sound.handle {
                    Some(handle) => handle.state() == PlaybackState::Stopped,
                    None => sound.started.elapsed().as_secs_f64() >= sound.duration,
                };
            if done {
                finished += 1;
            }
            !done
        });
        self.stopped += finished as u64;
    }
}

/// 线性音量（`0.0..=1.0`）→ 分贝。
///
/// kira 把 `-60 dB` 及以下视为静音（`Decibels::SILENCE`），`0` 分贝是原音量。
/// 线性 `1.0` 直接映射 `0 dB`；极小值 clamp 进静音档，避免 `log10(0)`。
fn linear_to_decibels(linearity: f32) -> Decibels {
    let v = if linearity.is_finite() {
        linearity.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if v <= 0.001 {
        return Decibels::SILENCE;
    }
    Decibels((20.0 * v.log10()).max(Decibels::SILENCE.0))
}

/// `ListenerPose::default()` 的四元数（[x,y,z,w]）：从 (5,5,5) 看向原点。
fn default_orientation_quat() -> [f32; 4] {
    ListenerPose::default().orientation()
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// 生成一个 16-bit mono PCM WAV（440 Hz 正弦，1 秒）。
    ///
    /// 时长取 1 秒而不是几十毫秒：无声引擎按**墙钟** GC，太短的 WAV 会让
    /// "active 还是 1 吗"这类断言在慢机器上飘。
    /// 自己拼字节而不是引音频测试依赖：RIFF/WAVE 的 PCM 布局几十年没变过。
    fn write_test_wav(path: &Path) {
        const SAMPLE_RATE: u32 = 44100;
        let samples: u32 = SAMPLE_RATE;
        let data_len = samples * 2; // 16-bit mono
        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes()); // fmt 块长度
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // 字节率
        bytes.extend_from_slice(&2u16.to_le_bytes()); // 块对齐
        bytes.extend_from_slice(&16u16.to_le_bytes()); // 位深
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..samples {
            let t = i as f32 / SAMPLE_RATE as f32;
            let sample = (0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin() * i16::MAX as f32) as i16;
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, bytes).expect("写测试 WAV");
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xgame_audio_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 真 WAV：受理 → 解码缓存 → 活动 1 个；停掉 → 归零并计数。
    ///
    /// 走 **silent 引擎**（不碰设备，CI 可跑）；解码用的是 kira 真解码器，
    /// 所以这条测试同时钉住了"路径解析 + WAV 解码 + 状态跟踪"整条链。
    #[test]
    fn plays_and_stops_a_real_wav_in_silent_mode() {
        let root = temp_root("play");
        write_test_wav(&root.join("Beep.wav"));
        let mut engine = AudioEngine::silent(&root);
        let pose = ListenerPose::default();

        engine.apply(
            &[AudioCommand::Play(
                PlaySound::new("Beep.wav").with_volume(0.5),
            )],
            &pose,
            1.0,
        );

        let stats = engine.stats();
        assert_eq!(stats.requests, 1, "请求应在解码前计数");
        assert_eq!(stats.active, 1, "播完之前应当是活动状态");
        assert_eq!(stats.cached_clips, 1, "解码结果应进缓存");
        assert_eq!(stats.failed_clips, 0);
        assert_eq!(stats.last_batch, 1);
        assert!(!stats.available, "silent 引擎不应声称有设备");

        // 再播一次同 clip：缓存命中（cached_clips 仍为 1），活动变 2。
        engine.apply(
            &[AudioCommand::Play(PlaySound::new("Beep.wav"))],
            &pose,
            1.0,
        );
        assert_eq!(engine.stats().active, 2);
        assert_eq!(engine.stats().cached_clips, 1);

        // StopClip：全部实例停止并计数。
        engine.apply(&[AudioCommand::StopClip("Beep.wav".into())], &pose, 1.0);
        let stats = engine.stats();
        assert_eq!(stats.active, 0);
        assert_eq!(stats.stopped, 2);
        assert_eq!(stats.last_batch, 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 缺失 clip：请求照样计数、进 failed 表、不出活动声音；
    /// 再次播放不重试解码（failed_clips 不再增长）。
    #[test]
    fn missing_clip_is_counted_but_not_retried() {
        let root = temp_root("missing");
        let mut engine = AudioEngine::silent(&root);
        let pose = ListenerPose::default();

        engine.apply(
            &[AudioCommand::Play(PlaySound::new("Nope.wav"))],
            &pose,
            1.0,
        );
        engine.apply(
            &[AudioCommand::Play(PlaySound::new("Nope.wav"))],
            &pose,
            1.0,
        );
        let stats = engine.stats();
        assert_eq!(stats.requests, 2, "两次请求都要计数");
        assert_eq!(stats.failed_clips, 1, "同一 clip 只记一次失败");
        assert_eq!(stats.active, 0, "解码失败不产生活动声音");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 实体关联：StopEntity 只停该实体的声音；EmitterPosition 不 panic。
    #[test]
    fn entity_scoped_stops_and_emitter_updates() {
        let root = temp_root("entity");
        write_test_wav(&root.join("Beep.wav"));
        let mut engine = AudioEngine::silent(&root);
        let pose = ListenerPose::default();
        let a = EntityId::new();
        let b = EntityId::new();

        engine.apply(
            &[
                AudioCommand::Play(PlaySound::new("Beep.wav").following(a)),
                AudioCommand::Play(PlaySound::new("Beep.wav").following(b)),
                AudioCommand::Play(PlaySound::new("Beep.wav")), // 无主
            ],
            &pose,
            1.0,
        );
        assert_eq!(engine.stats().active, 3);

        engine.apply(
            &[AudioCommand::EmitterPosition {
                entity: a,
                position: [1.0, 2.0, 3.0],
            }],
            &pose,
            1.0,
        );

        engine.apply(&[AudioCommand::StopEntity(a)], &pose, 1.0);
        let stats = engine.stats();
        assert_eq!(stats.active, 2, "只停 A 的声音");
        assert_eq!(stats.stopped, 1);

        engine.apply(&[AudioCommand::StopAll], &pose, 1.0);
        assert_eq!(engine.stats().active, 0);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 线性音量 → 分贝的边界：1.0 = 0 dB（原音量），0 → 静音档，NaN 不产 NaN。
    #[test]
    fn linear_volume_maps_to_decibels_safely() {
        assert_eq!(linear_to_decibels(1.0), Decibels::IDENTITY);
        assert_eq!(linear_to_decibels(0.0), Decibels::SILENCE);
        assert_eq!(linear_to_decibels(2.0), Decibels::IDENTITY, "超界 clamp 到 1.0");
        assert_eq!(linear_to_decibels(f32::NAN), Decibels::SILENCE);
        let half = linear_to_decibels(0.5);
        assert!((half.0 - (-6.0206)).abs() < 1e-3, "0.5 线性 ≈ -6 dB: {:?}", half.0);
    }
}
