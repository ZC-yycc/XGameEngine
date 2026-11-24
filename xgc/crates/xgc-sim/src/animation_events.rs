//! 动画事件穿越判定：纯函数，只回答"这一帧跨过了哪几条事件"。
//!
//! 与旧端 `xgame_core::animation_runtime::crossed_events` 逐条对齐：
//! - 时长 / 时间非有限、`duration <= 0`、`to < from` 一律不触发；
//! - 空名、负时间、非有限时间跳过；
//! - 循环剪辑上 `time >= duration` 视为不可达（不猜"下一圈的同一相位"）；
//! - `started` 为真时额外派发 `time ≈ 0` 的事件（首帧 / 换剪辑）；
//! - 命中先按时间排序，保证一帧多事件按时间轴顺序。

use xgc_world::AnimEvent;

/// 时间比较容差：与旧端同一个 `1e-6` 量级，避免浮点累加把"刚好到达"判丢。
const EVENT_EPS: f64 = 1e-6;

/// 把 `(from, to]` 区间内的事件下标追加到 `out`（`out` 由调用方清空）。
pub(crate) fn crossed_events(
    events: &[AnimEvent],
    from: f32,
    to: f32,
    duration: f32,
    looping: bool,
    started: bool,
    out: &mut Vec<usize>,
) {
    if !from.is_finite() || !to.is_finite() || !duration.is_finite() || duration <= 0.0 {
        return;
    }
    let from = from.max(0.0) as f64;
    let to = to.max(0.0) as f64;
    if to < from {
        return;
    }
    let duration = duration as f64;

    let mut hits: Vec<(f64, usize)> = Vec::new();
    for (index, event) in events.iter().enumerate() {
        if event.name.is_empty() || !event.time.is_finite() || event.time < 0.0 {
            continue;
        }
        let time = event.time as f64;
        // 首帧 / 换剪辑：`(from, to]` 天然收不到 from=0 的那条，额外补一次。
        if started && time <= EVENT_EPS {
            hits.push((0.0, index));
        }
        if to <= from {
            continue;
        }
        if looping {
            if time >= duration + EVENT_EPS {
                continue;
            }
            let k_min = ((from + EVENT_EPS - time) / duration).floor() + 1.0;
            let k_max = ((to + EVENT_EPS - time) / duration).floor();
            let mut k = k_min.max(0.0) as i64;
            let k_max = k_max as i64;
            while k <= k_max {
                hits.push((k as f64 * duration + time, index));
                k += 1;
            }
        } else if time <= duration + EVENT_EPS && time > from + EVENT_EPS && time <= to + EVENT_EPS
        {
            hits.push((time, index));
        }
    }
    if hits.is_empty() {
        return;
    }
    hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out.extend(hits.into_iter().map(|(_, index)| index));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events() -> Vec<AnimEvent> {
        vec![
            AnimEvent::new("zero", 0.0),
            AnimEvent::new("half", 0.5),
            AnimEvent::new("done", 1.5),
        ]
    }

    fn crossed(from: f32, to: f32, duration: f32, looping: bool, started: bool) -> Vec<usize> {
        let mut out = Vec::new();
        crossed_events(&events(), from, to, duration, looping, started, &mut out);
        out
    }

    #[test]
    fn fires_each_event_once_on_the_crossing_frame() {
        assert_eq!(crossed(0.0, 0.25, 2.0, false, true), vec![0]);
        assert!(crossed(0.25, 0.4, 2.0, false, false).is_empty());
        assert_eq!(crossed(0.4, 0.6, 2.0, false, false), vec![1]);
        assert_eq!(crossed(1.4, 1.6, 2.0, false, false), vec![2]);
        assert!(crossed(1.6, 1.7, 2.0, false, false).is_empty());
    }

    #[test]
    fn looping_wraps_and_fires_zero_time_each_lap() {
        // 2.9 → 3.1 跨过 3.0：time=0 的事件在下一圈 3.0 处命中。
        assert_eq!(crossed(2.9, 3.1, 3.0, true, false), vec![0]);
        // 0.4 → 0.6 命中 0.5。
        assert_eq!(crossed(0.4, 0.6, 3.0, true, false), vec![1]);
        // 1.4 → 1.6 命中 1.5。
        assert_eq!(crossed(1.4, 1.6, 3.0, true, false), vec![2]);
    }

    #[test]
    fn rejects_bad_data_without_guessing() {
        let mut out = Vec::new();
        crossed_events(&events(), 0.0, 1.0, 0.0, false, true, &mut out);
        assert!(out.is_empty(), "duration = 0 不应触发");
        crossed_events(&events(), 1.0, 0.5, 2.0, false, true, &mut out);
        assert!(out.is_empty(), "to < from 不应触发");
        crossed_events(
            &[AnimEvent::new("", 0.5), AnimEvent::new("bad", f32::NAN)],
            0.0,
            1.0,
            2.0,
            false,
            true,
            &mut out,
        );
        assert!(out.is_empty(), "空名 / 非有限时间应跳过");
    }

    #[test]
    fn started_fires_the_zero_time_event_on_the_first_frame() {
        assert_eq!(crossed(0.0, 0.1, 2.0, true, true), vec![0]);
        assert!(crossed(0.0, 0.1, 2.0, true, false).is_empty());
    }

    #[test]
    fn orders_multiple_hits_by_time_not_author_order() {
        let events = vec![AnimEvent::new("late", 0.9), AnimEvent::new("early", 0.2)];
        let mut out = Vec::new();
        crossed_events(&events, 0.0, 1.0, 2.0, false, false, &mut out);
        assert_eq!(out, vec![1, 0], "0.2 应先于 0.9 派发");
    }
}
