//! 自动保存的**调度**（什么时候写），落盘细节在 `xgame_core::project`。
//!
//! # 它不是什么
//!
//! 自动保存**不是**"保存"。它写的是 `.autosave/<场景相对路径>` 的副本，
//! **不改 `saved_snapshot`**：标题栏的星号、关窗时的确认框、`scene_dirty()`
//! 一个字都不变。用户没按过 Ctrl+S，编辑器就不能假装文件已经对齐了磁盘。
//!
//! # 为什么"到点才算"而不是"改了立刻写"
//!
//! 拖 gizmo 的时候场景每帧都在变，逐帧写盘等于把磁盘当帧缓冲用（还是同步 IO）。
//! 间隔的意义是给出一个"最多丢多久工作"的上界，同时把写盘压到低频。
//!
//! # 空闲时不会自己出帧
//!
//! 编辑器的帧调度在界面静止后会停帧（见 `frame_policy`）。只在这里判"到点没"
//! 是不够的 —— 用户改完东西、手离开鼠标，编辑器停止出帧，`autosave_tick` 就再也
//! 不会被调用。所以 [`AutoSave::wait`] 还要交给主循环去
//! `request_repaint_after`：让它在**恰好的时刻**醒一次，而不是保持满速空转。
//!
//! # 时间的来源
//!
//! 全部走 `Instant` 参数注入（不在内部读时钟），于是单测可以"把时间拨到
//! 三分钟后"去验证间隔语义，不需要真的等。

use std::time::{Duration, Instant, SystemTime};

/// 打开项目时发现的、可以恢复的自动保存副本。
///
/// 只描述"有这件事"，不做任何 IO —— 读写的入口都在
/// [`EditorState::recover_autosave`](crate::state::EditorState::recover_autosave)，
/// 于是"要不要恢复"是个纯 UI 决策，测试里可以只验证状态跃迁。
#[derive(Debug, Clone)]
pub struct AutosaveNotice {
    /// 场景的项目相对路径（`Scenes/Main.ron`）。
    pub scene: String,
    /// 副本的写入时间；文件系统给不出时 `None`。
    pub written: Option<SystemTime>,
}

impl AutosaveNotice {
    /// 「3 分钟前」这类相对时间文案；时间取不到时返回 `None`。
    pub fn age_label(&self) -> Option<String> {
        let written = self.written?;
        let elapsed = SystemTime::now().duration_since(written).ok()?;
        Some(relative_label(elapsed))
    }
}

/// 默认自动保存间隔（秒）。
pub const DEFAULT_INTERVAL_SECS: u64 = 60;
/// 间隔下限：比这更密就等于逐帧写盘。
pub const MIN_INTERVAL_SECS: u64 = 10;
/// 间隔上限：超过一小时，"最多丢多久工作"就没有意义了。
pub const MAX_INTERVAL_SECS: u64 = 3600;

/// `0` 表示关闭 —— 与 `< MIN` 区分开，否则用户没法关掉它。
pub const OFF: u64 = 0;

/// 把所有来源的间隔值收进合法区间（`0` 原样保留 = 关闭）。
pub fn sanitize_interval(secs: u64) -> u64 {
    if secs == OFF {
        OFF
    } else {
        secs.clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS)
    }
}

/// 自动保存的计时状态。
#[derive(Debug, Clone)]
pub struct AutoSave {
    /// 间隔秒数；`0` = 关闭。
    interval_secs: u64,
    /// 下一次到点的时刻；`None` = 关闭或还没排过。
    next_due: Option<Instant>,
    /// 上一次**成功**写入的时刻（状态栏显示用）。
    last_success: Option<Instant>,
    /// 上一次失败的原因；成功后清空。
    last_error: Option<String>,
}

impl AutoSave {
    /// 以给定间隔起步，第一次到点 = `now + 间隔`。
    ///
    /// **不从"打开项目"立刻写**：刚打开的场景是干净的，写一份和磁盘一模一样的
    /// 副本只会让下次启动多问一句"要不要恢复"。
    pub fn new(interval_secs: u64, now: Instant) -> Self {
        let interval_secs = sanitize_interval(interval_secs);
        let next_due = (interval_secs != OFF).then(|| now + Duration::from_secs(interval_secs));
        Self {
            interval_secs,
            next_due,
            last_success: None,
            last_error: None,
        }
    }

    pub fn interval_secs(&self) -> u64 {
        self.interval_secs
    }

    pub fn is_enabled(&self) -> bool {
        self.interval_secs != OFF
    }

    /// 改间隔并**立刻重排**下一次到点时刻。
    ///
    /// 不重排的话，把 600 秒改成 10 秒后用户还要等 590 秒才看到第一次生效 ——
    /// 那会被当成"改了没用"。
    pub fn set_interval_secs(&mut self, secs: u64, now: Instant) {
        self.interval_secs = sanitize_interval(secs);
        self.next_due =
            (self.interval_secs != OFF).then(|| now + Duration::from_secs(self.interval_secs));
    }

    /// 现在是否到点（关闭时永远 false）。
    pub fn due_at(&self, now: Instant) -> bool {
        self.next_due.is_some_and(|due| now >= due)
    }

    /// 距离下一次到点还有多久；关闭时 `None`，已过点时为 `ZERO`。
    ///
    /// 主循环拿它去 `request_repaint_after`，保证空闲停帧时也能准点醒一次。
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        self.next_due.map(|due| due.saturating_duration_since(now))
    }

    /// 记一次尝试并**推进计时器**。
    ///
    /// 无论成功失败都要推进：一次写盘失败（磁盘满 / 文件被占用）不该变成
    /// 每帧重试 —— 那会把一条警告刷成上千条，还会让界面卡在写盘上。
    pub fn mark_attempt(&mut self, now: Instant) {
        self.next_due =
            (self.interval_secs != OFF).then(|| now + Duration::from_secs(self.interval_secs));
    }

    pub fn mark_success(&mut self, now: Instant) {
        self.last_success = Some(now);
        self.last_error = None;
    }

    pub fn mark_failure(&mut self, message: String) {
        self.last_error = Some(message);
    }

    /// 让下一次 tick **立刻**写一次（恢复后使用：恢复出来的内容还没进任何
    /// 自动保存副本，中间再崩一次就白恢复了）。
    pub fn schedule_now(&mut self, now: Instant) {
        if self.is_enabled() {
            self.next_due = Some(now);
        }
    }

    pub fn last_success(&self) -> Option<Instant> {
        self.last_success
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
}

/// 状态栏 / 悬停提示用的相对时间文案。
///
/// 用相对时间而不是钟表时间：`Instant` 拿不到墙钟，而且"3 分钟前"正是
/// 用户此刻想知道的东西。
pub fn relative_label(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 10 {
        "刚刚".to_owned()
    } else if secs < 60 {
        format!("{secs} 秒前")
    } else if secs < 3600 {
        format!("{} 分钟前", secs / 60)
    } else {
        format!("{} 小时前", secs / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_is_clamped_but_zero_stays_off() {
        assert_eq!(sanitize_interval(0), 0);
        assert_eq!(sanitize_interval(1), MIN_INTERVAL_SECS);
        assert_eq!(sanitize_interval(60), 60);
        assert_eq!(sanitize_interval(99_999), MAX_INTERVAL_SECS);
    }

    /// 到点前不写、到点后写一次，然后计时器重新起算。
    ///
    /// 旧实现（不存在的 `AutoSave`）编译都过不去 —— 这就是"红"。
    #[test]
    fn due_only_after_the_interval_then_rearms() {
        let t0 = Instant::now();
        let mut autosave = AutoSave::new(60, t0);

        assert!(!autosave.due_at(t0), "刚创建不该立刻到点");
        assert!(!autosave.due_at(t0 + Duration::from_secs(59)));
        assert!(autosave.due_at(t0 + Duration::from_secs(60)));

        let t1 = t0 + Duration::from_secs(60);
        autosave.mark_attempt(t1);
        assert!(
            !autosave.due_at(t1 + Duration::from_secs(1)),
            "写完之后要重新起算"
        );
        assert!(autosave.due_at(t1 + Duration::from_secs(60)));
    }

    /// 关闭（间隔 0）时永远不到点，也不会给主循环排唤醒。
    #[test]
    fn zero_interval_disables_everything() {
        let t0 = Instant::now();
        let autosave = AutoSave::new(0, t0);

        assert!(!autosave.is_enabled());
        assert!(!autosave.due_at(t0 + Duration::from_secs(86_400)));
        assert_eq!(autosave.wait(t0), None);
    }

    /// `wait` 是"空闲时也能准点醒"的依据：过点后必须是 0 而不是 None，
    /// 否则主循环会以为"不用唤醒"，自动保存就永远不触发。
    #[test]
    fn wait_saturates_to_zero_after_the_deadline() {
        let t0 = Instant::now();
        let autosave = AutoSave::new(60, t0);

        assert_eq!(autosave.wait(t0), Some(Duration::from_secs(60)));
        assert_eq!(
            autosave.wait(t0 + Duration::from_secs(30)),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            autosave.wait(t0 + Duration::from_secs(90)),
            Some(Duration::ZERO)
        );
    }

    /// 改间隔要立刻生效，不能让用户等旧计时器走完。
    #[test]
    fn changing_the_interval_rearms_immediately() {
        let t0 = Instant::now();
        let mut autosave = AutoSave::new(600, t0);
        assert!(!autosave.due_at(t0 + Duration::from_secs(20)));

        autosave.set_interval_secs(10, t0 + Duration::from_secs(20));
        assert!(
            autosave.due_at(t0 + Duration::from_secs(30)),
            "改成 10 秒后应当从改动那一刻起重新计 10 秒"
        );
    }

    /// 失败不该清掉"上次成功"（状态栏要能说明上次成功是什么时候），
    /// 也不该让计时器停滞。
    #[test]
    fn failure_keeps_last_success_and_still_rearms() {
        let t0 = Instant::now();
        let mut autosave = AutoSave::new(60, t0);
        autosave.mark_attempt(t0);
        autosave.mark_success(t0);
        assert_eq!(autosave.last_success(), Some(t0));

        let t1 = t0 + Duration::from_secs(60);
        autosave.mark_attempt(t1);
        autosave.mark_failure("磁盘满".to_owned());

        assert_eq!(autosave.last_success(), Some(t0), "失败不改变上次成功时间");
        assert_eq!(autosave.last_error(), Some("磁盘满"));
        assert!(
            autosave.due_at(t1 + Duration::from_secs(60)),
            "失败后照常重试一次"
        );
    }

    #[test]
    fn relative_label_reads_naturally() {
        assert_eq!(relative_label(Duration::from_secs(3)), "刚刚");
        assert_eq!(relative_label(Duration::from_secs(42)), "42 秒前");
        assert_eq!(relative_label(Duration::from_secs(180)), "3 分钟前");
        assert_eq!(relative_label(Duration::from_secs(7200)), "2 小时前");
    }
}
