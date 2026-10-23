//! 无头运行：不开窗口把项目跑 N 帧，输出机器可读的 JSON 统计。
//!
//! # 为什么（Agent-Native 支柱 5）
//!
//! `--check` 回答"项目能不能加载"，这一块回答"跑起来会怎样"：改完场景 /
//! 脚本，一条命令就能拿到 —— 帧耗时分布、实体 / 刚体计数、脚本日志
//! （含 error 样本）、非有限变换检测。CI 与 AI 智能体的"改完看效果"闭环，
//! 缺的就是这一块（见 `Docs/差距分析.md` 文末战略章）。MCP 的 `run_headless`
//! 工具就是本模块的透传壳。
//!
//! # 为什么放在编辑器二进制
//!
//! 与 `--check` 同一家族 —— 编辑器二进制是"项目工具"：`--check` 管静态
//! 验证，`--headless` 管动态验证。跑的是 [`xgame_runtime::World`]，与编辑器
//! ▶ Play、独立游戏**同一段** `update`，所以无头跑出来的行为与玩家实跑
//! 一致（差别只在没有渲染与音频设备，见下）。
//!
//! # 确定性（刻意的）
//!
//! 每帧推进固定的 `dt`（默认 = 项目的固定步长）、没有输入、没有墙钟依赖 ——
//! 同一个项目跑两遍，**模拟结果**逐位一致（`falling_ball_is_deterministic_across_runs`
//! 钉死：两次运行 `--dump-scene` 的落盘场景逐字节相等）。报告里只有
//! `setup_ms` / `run_ms` / `frame_us` 是墙钟测量，本来就该浮动。物理与脚本都按
//! 同一串时间推进，适合做回归基准与"两次改动对差异"。
//!
//! # 明确不做的事（边界）
//!
//! - **默认不建 GPU / 不碰声卡**：CI 与无显卡机器直接能跑；音频走
//!   [`xgame_runtime::World::new`] 的 silent 引擎 —— 命令、解码、计数照常，
//!   只是不出声（报告里 `audio.available: false` 就是它，不是错误）。
//!   **唯一例外是 `--screenshot`**：截图必须要有图形设备，拿不到适配器
//!   就是一个可读的错误（退出码 1），而不是静默降级。
//! - **不加载资产到 GPU**（截图为唯一例外，见上）：模型 / 纹理是渲染器
//!   的事；引用完整性由 `--check` / MCP `project_check` 管。
//! - **不推进动画播放时钟**：动画采样在渲染宿主侧（`AnimationDriver`），
//!   脚本推的 `set_anim_param` 等参数照常写入场景，只是没人消费 ——
//!   骨骼姿态是渲染专用数据，无头运行里没有观察者。`--screenshot` 例外：
//!   那时候有观察者了，时钟按同一串 dt 逐帧推进，骨骼姿态停在模拟终点
//!   （与编辑器 Play 的行为同构）。
//! - **不重新编译 Rust 插件**：`Plugins/*.dll` 按现状加载（与打包出的游戏
//!   同一条路径）；改了插件源码要先在编辑器里 Play 一次或重跑 PIE 构建。
//!
//! # 退出码契约（与 `--check` 对齐）
//!
//! `0` = 干净跑完；`1` = 加载失败 / 参数非法；`2` = 跑完但观察到问题
//! （脚本 error 日志、非有限变换）。报告 JSON 照常打到 stdout —— 智能体
//! 既拿退出码、也拿结构化数据。
//!
//! ```sh
//! cargo run -p xgame-editor -- --headless --frames 300 projects/SampleScene
//! cargo run -p xgame-editor -- --headless --dump-scene out.ron projects/SampleScene
//! cargo run -p xgame-editor -- --headless --frames 120 --screenshot shot.png projects/SampleScene
//! ```

use std::path::PathBuf;
use std::time::Instant;

use serde_json::json;
use xgame_core::{ProjectHandle, RenderConfig, Scene};
use xgame_math::glam::Vec3;
use xgame_render::animation::AnimationDriver;
use xgame_render::screenshot::{ScreenshotRenderer, ShotCamera};
use xgame_render::scene_pipeline::SceneLighting;
use xgame_runtime::{BehaviorRegistry, LogLevel, ScriptLog, World, WorldConfig};

/// 默认跑多少帧。300 帧 ≈ 5 秒 @ 60Hz —— 够看到落体落定、脚本跑若干循环，
/// 又不至于让"随手跑一下"变成等半分钟。
pub const DEFAULT_FRAMES: u32 = 300;

/// 帧数上限。防"手滑写了个大数挂死一个智能体"：100k 帧 @60Hz 已是 ~28 分钟
/// 的模拟时长，真实的无头验证远用不到；要跑更长应当换方向（压测另有基准）。
pub const MAX_FRAMES: u32 = 100_000;

/// 每帧 dt 上限（秒）。dt > 固定步长时时钟每帧补多个固定步（有 5 步上限），
/// 到 1.0 以上就谈不上"固定步长运行"了 —— 参数写错早点报错，别默默出怪数据。
pub const MAX_DT: f32 = 1.0;

/// error 日志最多带几条样本回来：报告要小，计数要全 —— 条数看
/// `logs.error`，样本只看前几条找原因。
const MAX_ERROR_SAMPLES: usize = 10;

/// 缺省项目（与 `--check` 同一缺省，仓库内 `cargo run` 直接可跑）。
const DEFAULT_PROJECT: &str = "projects/SampleScene";

/// 截图默认尺寸（16:9，1080p 的队友）。
pub const DEFAULT_SHOT_SIZE: (u32, u32) = (1280, 720);

/// 截图边长上限。GPU 纹理不是无限的，4096² 的附件 ×2（MSAA）已在 64 MB 量级；
/// 再大不是"看得更清"而是"吃显存"，早点报错比 OOM 强。
pub const MAX_SHOT_EDGE: u32 = 4096;

// ---------------------------------------------------------------------------
// 参数
// ---------------------------------------------------------------------------

/// 无头运行的参数（已解析、已校验）。
#[derive(Debug, Clone, PartialEq)]
pub struct HeadlessArgs {
    /// 项目根目录（含 `project.ron`）。
    pub project: PathBuf,
    /// 跑多少帧。
    pub frames: u32,
    /// 每帧推进的秒数；`None` = 项目的固定步长（确定性运行的推荐值）。
    pub dt: Option<f32>,
    /// 额外把 JSON 报告写一份到该路径（stdout 照常输出）。
    pub report: Option<PathBuf>,
    /// 把跑完后的场景以 RON 写到该路径 —— 观察运行结果的通用出口
    /// （物理落点、脚本改过的变换都在里面，可 diff、可交给 `scene_write`）。
    pub dump_scene: Option<PathBuf>,
    /// 跑完后离屏渲染一帧、落盘 PNG 到该路径（需要 GPU；未给 = 不截图）。
    pub screenshot: Option<PathBuf>,
    /// 截图尺寸（像素）。
    pub shot_size: (u32, u32),
    /// 截图相机位置（世界坐标）；与 `camera_target` 成对出现，
    /// 都不给 = 自动取景框住整个场景。
    pub camera_pos: Option<[f32; 3]>,
    /// 截图相机看向的目标点（世界坐标）。
    pub camera_target: Option<[f32; 3]>,
}

impl Default for HeadlessArgs {
    fn default() -> Self {
        Self {
            project: PathBuf::from(DEFAULT_PROJECT),
            frames: DEFAULT_FRAMES,
            dt: None,
            report: None,
            dump_scene: None,
            screenshot: None,
            shot_size: DEFAULT_SHOT_SIZE,
            camera_pos: None,
            camera_target: None,
        }
    }
}

/// `--headless` 之后参数的用法说明（`--help` 与参数报错共用）。
pub fn usage() -> String {
    format!(
        "\
用法: xgame-editor --headless [选项] [项目路径]

无头跑项目 N 帧（不开窗口），向 stdout 输出 JSON 统计。
退出码: 0 = 干净跑完 / 1 = 加载失败或参数错误 / 2 = 跑完但观察到问题
（脚本 error 日志、非有限变换）。详见 Docs/无头运行.md。

选项:
  --frames N          跑多少帧（默认 {DEFAULT_FRAMES}，上限 {MAX_FRAMES}）
  --dt 秒             每帧推进的秒数（默认 = 项目的固定步长；要确定性就沿用默认）
  --report 路径       额外把 JSON 报告写一份到该文件
  --dump-scene 路径   把跑完后的场景以 RON 写到该文件
  --screenshot 路径   跑完后离屏渲染一帧存成 PNG（需要 GPU；默认取景框住全场）
  --shot-size WxH     截图尺寸（默认 {}x{}，边长上限 {MAX_SHOT_EDGE}）
  --camera-pos x,y,z  截图相机位置（世界坐标，与 --camera-target 成对；缺省自动取景）
  --camera-target x,y,z  截图相机看向的目标点（世界坐标）
  --help              显示这段帮助

项目路径缺省 = {DEFAULT_PROJECT}（仓库内直接可跑）。
",
        DEFAULT_SHOT_SIZE.0, DEFAULT_SHOT_SIZE.1
    )
}

/// 解析 `--headless` 之后的参数（不含 `--headless` 本身）。
pub fn parse_args(args: &[String]) -> Result<HeadlessArgs, String> {
    let mut out = HeadlessArgs::default();
    let mut project_set = false;
    // 只在 `--screenshot` 下才有意义的选项被显式给过没有 —— 用于"给了但
    // 没开截图"的报错（静默忽略参数是智能体最容易被坑的失败模式）。
    let mut shot_tuning_set = false;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--frames" => {
                let raw = expect_value(&mut iter, "--frames")?;
                let frames: u32 = raw
                    .parse()
                    .map_err(|_| format!("--frames 需要一个正整数，收到 {raw:?}\n\n{}", usage()))?;
                if frames == 0 || frames > MAX_FRAMES {
                    return Err(format!(
                        "--frames 必须在 1..={MAX_FRAMES} 之间，收到 {frames}\n\n{}",
                        usage()
                    ));
                }
                out.frames = frames;
            }
            "--dt" => {
                let raw = expect_value(&mut iter, "--dt")?;
                let dt: f32 = raw
                    .parse()
                    .map_err(|_| format!("--dt 需要一个秒数，收到 {raw:?}\n\n{}", usage()))?;
                if !dt.is_finite() || dt <= 0.0 || dt > MAX_DT {
                    return Err(format!(
                        "--dt 必须在 (0, {MAX_DT}] 秒之间，收到 {dt}\n\n{}",
                        usage()
                    ));
                }
                out.dt = Some(dt);
            }
            "--report" => {
                out.report = Some(PathBuf::from(expect_value(&mut iter, "--report")?));
            }
            "--dump-scene" => {
                out.dump_scene = Some(PathBuf::from(expect_value(&mut iter, "--dump-scene")?));
            }
            "--screenshot" => {
                out.screenshot = Some(PathBuf::from(expect_value(&mut iter, "--screenshot")?));
            }
            "--shot-size" => {
                let raw = expect_value(&mut iter, "--shot-size")?;
                out.shot_size = parse_shot_size(raw)?;
                shot_tuning_set = true;
            }
            "--camera-pos" => {
                let raw = expect_value(&mut iter, "--camera-pos")?;
                out.camera_pos = Some(parse_vec3(raw, "--camera-pos")?);
                shot_tuning_set = true;
            }
            "--camera-target" => {
                let raw = expect_value(&mut iter, "--camera-target")?;
                out.camera_target = Some(parse_vec3(raw, "--camera-target")?);
                shot_tuning_set = true;
            }
            other if other.starts_with("--") => {
                return Err(format!("未知参数 {other:?}\n\n{}", usage()));
            }
            other => {
                if project_set {
                    return Err(format!("多余的位置参数 {other:?}\n\n{}", usage()));
                }
                out.project = PathBuf::from(other);
                project_set = true;
            }
        }
    }

    // 成对校验放在循环之后：选项可以任意顺序给。
    if out.camera_pos.is_some() != out.camera_target.is_some() {
        return Err(format!(
            "--camera-pos 与 --camera-target 必须成对给出（缺一不可）\n\n{}",
            usage()
        ));
    }
    if out.screenshot.is_none() && shot_tuning_set {
        return Err(format!(
            "--shot-size / --camera-pos / --camera-target 只在配合 --screenshot 时才有意义\n\n{}",
            usage()
        ));
    }

    Ok(out)
}

/// `WxH` 形式的截图尺寸（大小写 x/X 都收）。
fn parse_shot_size(raw: &str) -> Result<(u32, u32), String> {
    let bad = || format!("--shot-size 需要 WxH（如 1280x720），收到 {raw:?}\n\n{}", usage());
    let (w, h) = raw
        .split_once(['x', 'X'])
        .ok_or_else(bad)?;
    let w: u32 = w.trim().parse().map_err(|_| bad())?;
    let h: u32 = h.trim().parse().map_err(|_| bad())?;
    if !(16..=MAX_SHOT_EDGE).contains(&w) || !(16..=MAX_SHOT_EDGE).contains(&h) {
        return Err(format!(
            "--shot-size 的每条边必须在 16..={MAX_SHOT_EDGE} 之间，收到 {w}x{h}\n\n{}",
            usage()
        ));
    }
    Ok((w, h))
}

/// `x,y,z` 形式的三维向量（逗号分隔，必须是有限数）。
fn parse_vec3(raw: &str, flag: &str) -> Result<[f32; 3], String> {
    let bad = || format!("{flag} 需要 x,y,z 三个数（逗号分隔），收到 {raw:?}\n\n{}", usage());
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 3 {
        return Err(bad());
    }
    let mut out = [0.0f32; 3];
    for (i, part) in parts.iter().enumerate() {
        let v: f32 = part.trim().parse().map_err(|_| bad())?;
        if !v.is_finite() {
            return Err(bad());
        }
        out[i] = v;
    }
    Ok(out)
}

/// 取一个选项的值；缺值给可读报错而不是 panic。
fn expect_value<'a>(
    iter: &mut std::slice::Iter<'a, String>,
    flag: &str,
) -> Result<&'a str, String> {
    iter.next()
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} 缺少值\n\n{}", usage()))
}

// ---------------------------------------------------------------------------
// 运行
// ---------------------------------------------------------------------------

/// 帧耗时分布（微秒）。
///
/// 百分位用**最近秩**（nearest-rank）而不是插值 —— 报告里的每个数都是
/// "真实出现过的一帧"，插值出来的 4.7 ms 帧没人真的经历过。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
}

impl FrameStats {
    /// 从逐帧样本算分布。空样本（frames=0）在参数层就被禁止，这里兜底零值。
    fn from_samples(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self {
                mean: 0.0,
                min: 0.0,
                max: 0.0,
                p50: 0.0,
                p95: 0.0,
                p99: 0.0,
            };
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        Self {
            mean,
            min: sorted[0],
            max: sorted[sorted.len() - 1],
            p50: percentile(&sorted, 0.50),
            p95: percentile(&sorted, 0.95),
            p99: percentile(&sorted, 0.99),
        }
    }

    fn to_json(self) -> serde_json::Value {
        json!({
            "mean": self.mean,
            "min": self.min,
            "max": self.max,
            "p50": self.p50,
            "p95": self.p95,
            "p99": self.p99,
        })
    }
}

/// 最近秩百分位：第 ⌈p·n⌉ 个（1 起）样本的值。
fn percentile(sorted: &[f64], p: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    let n = sorted.len();
    let rank = (p * n as f64).ceil() as usize;
    sorted[rank.clamp(1, n) - 1]
}

/// 截图那一帧的绘制读数（与 `xgame_render::scene_pipeline::FrameStats` 同口径）。
///
/// 为什么截图报告要带它：截图可能用显式机位（`--camera-pos/--camera-target`），
/// 此时"相机外的东西被剔掉了"是**画面的解释**而不是 bug。把 draw call 与剔除数
/// 一起写进报告，智能体不必靠猜就能区分"这张图没画那个物体"的两种原因：
/// 场景里本来没有 / 相机没框到。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotRenderStats {
    /// 主 pass 发出的 draw call 数（不透明批次 + 半透明逐项）。
    pub draw_calls: usize,
    pub opaque_batches: usize,
    pub opaque_instances: usize,
    pub blended_draws: usize,
    /// 因整个实体落在相机视锥外而跳过的实体数。
    pub culled_entities: usize,
    /// 被剔除的绘制项数。
    pub culled_instances: usize,
    /// 只在阴影 pass 出现的实例数（相机外、影子在画面里的投影物）。
    pub shadow_only_instances: usize,
    /// 被剔除的绘制项占全部绘制项的比例（0..=1）。
    pub cull_ratio: f32,
    /// 屏幕空间 UI 的绘制读数（没有 UI 元素时全为 0）。
    ///
    /// 单独一段而不是并进 `draw_calls`：3D 的 draw 与 UI 的 draw 是两条
    /// pass、两个数量级，混在一起既看不清也断言不了"UI 只画了 1 次"。
    pub ui: xgame_render::ui_renderer::UiStats,
}

impl From<xgame_render::scene_pipeline::FrameStats> for ShotRenderStats {
    fn from(s: xgame_render::scene_pipeline::FrameStats) -> Self {
        Self {
            draw_calls: s.draw_calls(),
            opaque_batches: s.opaque_batches,
            opaque_instances: s.opaque_instances,
            blended_draws: s.blended_draws,
            culled_entities: s.culled_entities,
            culled_instances: s.culled_instances,
            shadow_only_instances: s.shadow_only_instances,
            cull_ratio: s.cull_ratio(),
            ui: xgame_render::ui_renderer::UiStats::default(),
        }
    }
}

/// 截图的落盘信息（`--screenshot` 未给时为 `None`）。
///
/// `eye` / `target` 是**实际使用**的相机（自动取景时是拟合结果）——
/// "这张图从哪拍的"必须可复现：同一个相机参数 + 同一份场景 = 同一张图。
#[derive(Debug, Clone, PartialEq)]
pub struct ShotInfo {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub eye: [f32; 3],
    pub target: [f32; 3],
    /// 机位是怎么定的：`"explicit"`（命令行给）/ `"scene"`（场景相机实体）/
    /// `"auto"`（自动取景）。智能体据此判断"这张图是不是玩家视角"。
    pub camera_mode: String,
    /// 这一帧的绘制读数（见 [`ShotRenderStats`]）。
    pub render_stats: ShotRenderStats,
}

/// 无头运行的统计报告。
///
/// # 字段名即契约
///
/// JSON 是给机器读的（CI 断言 / MCP 工具透传），字段名与嵌套结构是**稳定
/// 契约** —— 改字段要同步改 `json_contract_has_stable_keys` 测试与
/// `Docs/无头运行.md`。新增字段允许（读方按需取），改名 / 删除不行。
#[derive(Debug, Clone)]
pub struct HeadlessReport {
    /// 项目名（清单里的 `name`）。
    pub project: String,
    /// 项目根目录（绝对路径）。
    pub root: String,
    /// 入口场景的**项目相对路径**。
    pub scene: String,
    /// 入口场景的 `Scene.name`。
    pub scene_name: String,
    /// 跑完时所在场景的项目相对路径。
    ///
    /// 运行期 `load_scene()` 切换过场景时，这里会与 `scene`（入口场景）不同 ——
    /// 有了它，"这一跑最后停在哪一关"才是可验证的（否则只能靠日志猜）。
    pub scene_current: String,
    /// 本次运行成功切换场景的次数（0 = 没换过）。
    pub scene_loads: u32,
    /// 引擎版本（报告的"是谁吐的"戳）。
    pub engine_version: String,
    /// 跑了多少帧（= 请求的帧数）。
    pub frames: u32,
    /// 每帧推进的秒数（显式给了就用给的，否则 = `fixed_dt`）。
    pub dt: f32,
    /// 运行时固定步长（项目清单 `runtime.fixed_dt`）。
    pub fixed_dt: f32,
    /// 装载 + 装配耗时（毫秒，不含帧循环）。
    pub setup_ms: f64,
    /// 帧循环总耗时（毫秒，含逐帧统计采样本身）。
    pub run_ms: f64,
    /// 游戏时钟推进到的总时长（秒，≈ frames × dt）。
    pub total_time: f32,
    /// 跑完时的实体数。
    pub entities: usize,
    /// 跑完时的物理刚体数。
    pub bodies: usize,
    /// 跑完时的碰撞体数。
    pub colliders: usize,
    /// 全程执行的固定步长总数（每帧可能 0 或多个，取决于 dt 与固定步长之比）。
    pub fixed_steps: u64,
    /// 跑完时的时间缩放（脚本 `set_time_scale` 写的值；`0` = 暂停）。
    ///
    /// 与 `fixed_steps` 配合就能无脑判断"暂停到底生效没"：缩放归零后
    /// 固定步应当停止增长，`total_time` 也应当停在那一刻。
    pub time_scale: f32,
    /// 跑完时局部变换含 NaN / Infinity 的实体数（物理炸了、脚本算错都会体现在这里）。
    pub non_finite_transforms: usize,
    /// 逐帧 `World::update` 的耗时分布（微秒）。
    pub frame_us: FrameStats,
    /// 脚本 / 组件日志计数（含装载期脚本编译失败这种"帧 0"日志）。
    pub log_info: u64,
    pub log_warn: u64,
    pub log_error: u64,
    /// error 日志的前 [`MAX_ERROR_SAMPLES`] 条样本（`脚本: 消息` 形式）。
    pub error_samples: Vec<String>,
    /// 跑完时的音频子系统状态（无头运行的 `available` 恒为 false = silent 降级，
    /// 但解码 / 缓存 / 计数照常 —— 见模块头"明确不做的事"）。
    pub audio_available: bool,
    pub audio_active: usize,
    /// 跑完时的粒子统计（CPU 模拟在无头运行里照常推进，只是没人看画面）。
    pub particles_alive: usize,
    /// 参与模拟的发射器数。
    pub particles_emitters: usize,
    /// 各发射器 `max_particles` 之和（`alive == capacity` = 撞顶）。
    pub particles_capacity: usize,
    pub audio_cached_clips: usize,
    pub audio_failed_clips: usize,
    /// 画面设置（`project.ron` 的 `render` 节）：`--screenshot` 出的图就是按
    /// 这一份设置渲染的（曝光 / 色调映射）—— 智能体看报告就知道图是怎么调的。
    pub render: RenderConfig,
    /// `--dump-scene` 写出的路径（没给就是 `None`）。
    pub scene_dump: Option<String>,
    /// `--screenshot` 的落盘信息（没给就是 `None`）。
    pub screenshot: Option<ShotInfo>,
}

impl HeadlessReport {
    /// 跑完但观察到问题：脚本 error 日志、非有限变换（退出码 2 的依据）。
    pub fn has_issues(&self) -> bool {
        self.log_error > 0 || self.non_finite_transforms > 0
    }

    /// 报告 → JSON（字段名即契约，见类型文档）。
    pub fn to_json(&self) -> serde_json::Value {
        // 截图这一段单独构造：整个报告是一个巨大的 `json!`，再往里嵌两层
        // 就会顶到宏的递归上限（实测 `recursion limit reached`）。
        let screenshot = match &self.screenshot {
            Some(s) => json!({
                "path": s.path,
                "width": s.width,
                "height": s.height,
                "camera": {
                    "mode": s.camera_mode,
                    "eye": s.eye,
                    "target": s.target,
                },
                "render_stats": {
                    "draw_calls": s.render_stats.draw_calls,
                    "opaque_batches": s.render_stats.opaque_batches,
                    "opaque_instances": s.render_stats.opaque_instances,
                    "blended_draws": s.render_stats.blended_draws,
                    "culled_entities": s.render_stats.culled_entities,
                    "culled_instances": s.render_stats.culled_instances,
                    "shadow_only_instances": s.render_stats.shadow_only_instances,
                    "cull_ratio": s.render_stats.cull_ratio,
                    "ui": {
                        "draws": s.render_stats.ui.draws,
                        "quads": s.render_stats.ui.quads,
                        "sprites": s.render_stats.ui.sprites,
                        "elements": s.render_stats.ui.elements,
                    },
                },
            }),
            None => serde_json::Value::Null,
        };

        json!({
            "ok": !self.has_issues(),
            "engine_version": self.engine_version,
            "project": self.project,
            "root": self.root,
            "scene": self.scene,
            "scene_name": self.scene_name,
            "scene_current": self.scene_current,
            "scene_loads": self.scene_loads,
            "frames": self.frames,
            "dt": self.dt,
            "fixed_dt": self.fixed_dt,
            "setup_ms": self.setup_ms,
            "run_ms": self.run_ms,
            "total_time": self.total_time,
            "entities": self.entities,
            "bodies": self.bodies,
            "colliders": self.colliders,
            "fixed_steps": self.fixed_steps,
            "time_scale": self.time_scale,
            "non_finite_transforms": self.non_finite_transforms,
            "frame_us": self.frame_us.to_json(),
            "logs": {
                "info": self.log_info,
                "warn": self.log_warn,
                "error": self.log_error,
            },
            "error_samples": self.error_samples,
            "audio": {
                "available": self.audio_available,
                "active": self.audio_active,
                "cached_clips": self.audio_cached_clips,
                "failed_clips": self.audio_failed_clips,
            },
            "particles": {
                "alive": self.particles_alive,
                "emitters": self.particles_emitters,
                "capacity": self.particles_capacity,
            },
            "render": {
                "exposure": self.render.exposure,
                "tone_map": self.render.tone_map,
                "bloom": {
                    "intensity": self.render.bloom.intensity,
                    "threshold": self.render.bloom.threshold,
                    "knee": self.render.bloom.knee,
                },
                "sky": {
                    "enabled": self.render.sky.enabled,
                    "intensity": self.render.sky.intensity,
                    "zenith": self.render.sky.zenith,
                    "horizon": self.render.sky.horizon,
                    "ground": self.render.sky.ground,
                    "sun_size": self.render.sky.sun_size,
                    "sun_intensity": self.render.sky.sun_intensity,
                },
                "fxaa": {
                    "enabled": self.render.fxaa.enabled,
                },
                // TAA：`enabled` 决定这张截图是不是"多帧抖动 + 重投影"的结果
                // —— 静态场景下它比单帧干净，但也不是同一个像素值。
                "taa": {
                    "enabled": self.render.taa.enabled,
                    "history_weight": self.render.taa.history_weight,
                },
            },
            "scene_dump": self.scene_dump,
            "screenshot": screenshot,
        })
    }

    /// 报告文本（pretty JSON + 尾换行；stdout 与 `--report` 文件同一份）。
    pub fn render(&self) -> String {
        let mut text =
            serde_json::to_string_pretty(&self.to_json()).expect("报告是纯数据 JSON，必可序列化");
        text.push('\n');
        text
    }
}

/// 无头跑一个项目。
///
/// 这是整个模块的核心入口：装载 → 装配 → 跑 N 帧 → 收统计 → （可选）落盘。
/// `Err` = 根本跑不起来（项目 / 场景加载失败、报告文件写不出去），
/// 对应 CLI 退出码 1；"跑起来了但有问题"记在报告里（[`HeadlessReport::has_issues`]）。
pub fn run(args: &HeadlessArgs) -> Result<HeadlessReport, String> {
    let setup_start = Instant::now();

    let project =
        ProjectHandle::open(&args.project).map_err(|e| format!("打开项目失败: {e}"))?;
    let scene = project
        .load_entry_scene()
        .map_err(|e| format!("加载入口场景失败: {e}"))?;
    let scene_name = scene.name.clone();

    // 输入映射：与编辑器 Play / 独立游戏同一条读法（没有就默认映射）。
    let input_map = crate::game_runtime::load_input_map(project.root());

    // Rust 插件：与独立游戏同一条路径（直接从 `Plugins/` 加载）。无头运行是
    // 一次性进程，不存在"运行中重编译覆盖 DLL"的文件锁问题，所以不走编辑器
    // PIE 的临时副本那套。句柄必须比 World 活得久 —— 注册表里的构造 / 分发
    // 函数指针指向 DLL 代码，用局部变量持有到函数结束（P0.5 铁律之一）。
    let mut behaviors = BehaviorRegistry::new();
    let _plugins = xgame::plugin::load_plugins_dir(
        project.root().join(crate::plugin_build::PLUGINS_DIR),
        &mut behaviors,
    );

    // 重力 / 固定步长走与两个宿主相同的入口（单入口 = 行为一致）。
    let config = WorldConfig::from_project(project.manifest());
    let fixed_dt = config.fixed_dt;
    let dt = args.dt.unwrap_or(fixed_dt);

    // 音频刻意保持 `World::new` 的 silent 引擎：无头运行不抢声卡（CI / 远程
    // 会话里也必须能跑），但解码 / 缓存 / 计数照常 —— 报告里的 audio 节
    // 不是零值。
    //
    // 场景装载器 = 项目句柄：脚本 `load_scene()` 在无头运行里也**照常生效**
    // （否则"多场景流程"就没法被 CI 与智能体验证）。入口场景路径同时交给运行时，
    // 脚本 `current_scene()` 才有值。
    let entry_scene = project.manifest().entry_scene.clone();
    let mut world = World::new(scene, project.root(), config)
        .with_input_map(input_map)
        .with_behavior_registry(behaviors)
        .with_scene_loader(project.clone())
        .with_current_scene(entry_scene)
        .with_scene_order(project.manifest().scene_order());

    let setup_ms = millis(setup_start.elapsed());

    // ── 帧循环：固定 dt、逐帧计时、日志随手收 ──
    //
    // 截图请求会额外挂一个动画时钟（渲染观察者）：按同一串 dt 逐帧 tick，
    // 骨骼姿态与编辑器 Play 的推进方式一致，停在模拟终点。不截图时不建 ——
    // 无截图路径的每帧成本与"动画从未存在"逐位一致。
    let mut shot_anim = args.screenshot.as_ref().map(|_| AnimationDriver::new());
    let mut frame_samples_us: Vec<f64> = Vec::with_capacity(args.frames as usize);
    let mut fixed_steps: u64 = 0;
    let mut log_info: u64 = 0;
    let mut log_warn: u64 = 0;
    let mut log_error: u64 = 0;
    let mut error_samples: Vec<String> = Vec::new();

    let run_start = Instant::now();
    for _ in 0..args.frames {
        let frame_start = Instant::now();
        let frame = world.update(dt);
        if let Some(anim) = shot_anim.as_mut() {
            // 与独立游戏同一份口径：动画吃缩放后的 dt，脚本暂停时截图里的姿势
            // 也停在那一刻（否则同一份脚本在游戏里停、在截图里还在跑）。
            anim.tick(world.frame_dt(), &world.scene().entities);
        }
        frame_samples_us.push(micros(frame_start.elapsed()));

        fixed_steps += u64::from(frame.fixed_steps);
        for log in world.take_logs() {
            match log.level {
                LogLevel::Info => log_info += 1,
                LogLevel::Warn => log_warn += 1,
                LogLevel::Error => {
                    log_error += 1;
                    if error_samples.len() < MAX_ERROR_SAMPLES {
                        error_samples.push(format_log(&log));
                    }
                }
            }
        }
    }
    let run_ms = millis(run_start.elapsed());

    // ── 收尾统计 ──
    let last = world.last_report();
    let non_finite_transforms = count_non_finite(world.scene());
    let audio = world.audio_stats();
    let particles = world.particle_stats();
    let total_time = world.clock().total_time();

    // 场景落盘放在报告之前：agent 的首要观察对象是"跑完的世界长什么样"，
    // 写失败就整体报失败（exit 1），别给出一份"看起来成功"的报告。
    let scene_dump = match &args.dump_scene {
        Some(path) => {
            dump_scene(world.scene(), path)?;
            Some(path.display().to_string())
        }
        None => None,
    };

    // 截图：与 `--dump-scene` 同一语义 —— 观察运行结果的另一只眼睛。
    // 需要 GPU，初始化 / 渲染 / 落盘任何一步失败都整体报失败（exit 1）。
    let screenshot = match &args.screenshot {
        Some(path) => {
            let anim = shot_anim
                .as_ref()
                .expect("截图请求在帧循环前就建好了动画时钟");
            let (width, height) = args.shot_size;
            let mut renderer = ScreenshotRenderer::new(project.root(), width, height)
                .map_err(|e| format!("截图初始化失败: {e:#}"))?;
            // 相机优先级（写进报告的 `screenshot.camera.mode`，别靠猜）：
            //   1. 显式 `--camera-pos/--camera-target` —— 智能体指定机位；
            //   2. 场景里的**相机实体** —— "玩家实际看到的画面"；
            //   3. 自动取景（顺光、包住整个场景）—— 老场景 / 没配相机的场景。
            //
            // 第 2 条是这一轮的要点：截图的用途是"验收玩家会看到什么"，
            // 场景里已经有了相机就不能再拿自动取景顶替它，否则截图会骗人。
            let (camera, camera_mode) = match (args.camera_pos, args.camera_target) {
                (Some(pos), Some(target)) => (
                    Some(ShotCamera::looking_at(
                        Vec3::from(pos),
                        Vec3::from(target),
                        xgame_render::screenshot::DEFAULT_FOV_Y,
                    )),
                    "explicit",
                ),
                _ => match world.camera_view() {
                    Some(view) => (
                        Some(ShotCamera {
                            eye: view.position,
                            target: view.position + view.forward,
                            fov_y: view.fov_deg.to_radians(),
                            // 用实体自己的上方向：带滚转的机位也照原样截出来。
                            up: view.up,
                        }),
                        "scene",
                    ),
                    None => (None, "auto"),
                },
            };
            let scene = world.scene();
            // 粒子必须进截图：无头运行的帧循环已经推进过模拟，玩家看到的
            // 那一团火不能被截图漏掉（否则"智能体看图验收"完全失效）。
            let particles = world.particle_quads();
            let used = renderer
                .render_scene_with_particles(
                    scene,
                    anim,
                    camera,
                    SceneLighting::new(scene.ambient, &scene.lights),
                    // 画面设置与编辑器视口 / 独立游戏同源：截图不是"特调画面"，
                    // 它就是玩家会看到的那张图（见 screenshot 模块文档）。
                    project.manifest().render,
                    particles,
                )
                .map_err(|e| format!("截图渲染失败: {e:#}"))?;
            renderer
                .save_png(path)
                .map_err(|e| format!("截图落盘失败: {e:#}"))?;
            Some(ShotInfo {
                path: path.display().to_string(),
                width,
                height,
                eye: used.eye.to_array(),
                target: used.target.to_array(),
                camera_mode: camera_mode.to_owned(),
                render_stats: {
                    let mut stats = ShotRenderStats::from(renderer.frame_stats());
                    stats.ui = renderer.ui_stats();
                    stats
                },
            })
        }
        None => None,
    };

    let report = HeadlessReport {
        project: project.name().to_owned(),
        root: project.root().display().to_string(),
        scene: project.manifest().entry_scene.clone(),
        scene_name,
        scene_current: world.current_scene().unwrap_or_default().to_owned(),
        scene_loads: world.scene_loads(),
        engine_version: xgame::ENGINE_VERSION.to_owned(),
        frames: args.frames,
        dt,
        fixed_dt,
        setup_ms,
        run_ms,
        total_time,
        entities: last.entities,
        bodies: last.bodies,
        colliders: last.colliders,
        fixed_steps,
        time_scale: world.time_scale(),
        non_finite_transforms,
        frame_us: FrameStats::from_samples(&frame_samples_us),
        log_info,
        log_warn,
        log_error,
        error_samples,
        audio_available: audio.available,
        audio_active: audio.active,
        audio_cached_clips: audio.cached_clips,
        audio_failed_clips: audio.failed_clips,
        particles_alive: particles.alive,
        particles_emitters: particles.emitters,
        particles_capacity: particles.capacity,
        render: project.manifest().render,
        scene_dump,
        screenshot,
    };

    if let Some(path) = &args.report {
        std::fs::write(path, report.render())
            .map_err(|e| format!("写报告文件失败 ({}): {e}", path.display()))?;
    }

    Ok(report)
}

/// CLI 壳：解析 → 跑 → 打印 → 返回退出码（测试也走这条，不重复逻辑）。
pub fn cli(args: &[String]) -> i32 {
    let opts = match parse_args(args) {
        Ok(opts) => opts,
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    };
    match run(&opts) {
        Ok(report) => {
            print!("{}", report.render());
            if report.has_issues() {
                2
            } else {
                0
            }
        }
        Err(message) => {
            eprintln!("headless FAILED: {message}");
            1
        }
    }
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

fn millis(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

fn micros(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1_000_000.0
}

/// 局部变换含 NaN / Infinity 的实体数。
///
/// 只查局部变换：世界变换由它沿父链解算，局部有限的世界变换必然有限，
/// 反过来局部烂了世界也没救 —— 查一遍本地数据就够。
fn count_non_finite(scene: &Scene) -> usize {
    scene
        .entities
        .iter()
        .filter(|e| {
            let t = &e.transform;
            !(t.position.is_finite() && t.rotation.is_finite() && t.scale.is_finite())
        })
        .count()
}

/// 一条日志转样本文本（与独立游戏的 `emit_script_log` 同款格式）。
fn format_log(log: &ScriptLog) -> String {
    match log.entity {
        Some(id) => format!("{} [{id}]: {}", log.script, log.message),
        None => format!("{}: {}", log.script, log.message),
    }
}

/// 场景写 RON（与 `ProjectHandle::write_scene` 同一序列化配置 —— 输出与
/// 项目内场景读写同构，可 diff、可原样喂给 `scene_write`）。
fn dump_scene(scene: &Scene, path: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建场景输出目录失败 ({}): {e}", parent.display()))?;
        }
    }
    let text = ron::ser::to_string_pretty(scene, ron::ser::PrettyConfig::default())
        .map_err(|e| format!("场景序列化失败: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("写场景文件失败 ({}): {e}", path.display()))
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use xgame_core::{Entity, PhysicsBody, Transform};
    use xgame_math::glam::Vec3;

    /// 临时项目（用完记得删）。默认场景 = 1 个 "Main Camera" 实体。
    fn temp_project(tag: &str) -> ProjectHandle {
        let base = std::env::temp_dir().join(format!(
            "xgame_headless_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        ProjectHandle::create(&base, &format!("Headless{tag}")).expect("创建临时项目")
    }

    fn cleanup(project: &ProjectHandle) {
        let _ = std::fs::remove_dir_all(project.root());
    }

    /// 给项目写一份"加料的"入口场景：往默认场景里追加实体。
    fn patch_scene(project: &ProjectHandle, add: impl FnOnce(&mut Scene)) {
        let mut scene = project.load_entry_scene().expect("加载默认场景");
        add(&mut scene);
        let entry = project.manifest().entry_scene.clone();
        project.write_scene(&entry, &scene).expect("写场景");
    }

    /// 往项目里放一个 `.lua` 脚本。
    fn write_script(project: &ProjectHandle, name: &str, source: &str) {
        let dir = project.scripts_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), source).unwrap();
    }

    fn opts(project: &ProjectHandle, frames: u32) -> HeadlessArgs {
        HeadlessArgs {
            project: project.root().to_path_buf(),
            frames,
            ..HeadlessArgs::default()
        }
    }

    // -- 参数解析 ----------------------------------------------------------

    /// 缺省值：项目 = 示例项目、帧数 = 默认、dt = 项目固定步长、不落盘。
    #[test]
    fn parse_args_defaults_to_bundled_sample() {
        let args = parse_args(&[]).expect("空参数应合法");
        assert_eq!(args, HeadlessArgs::default());
        assert_eq!(args.project, PathBuf::from("projects/SampleScene"));
        assert_eq!(args.frames, DEFAULT_FRAMES);
        assert!(args.dt.is_none());
    }

    /// 各选项都能解析；位置参数 = 项目路径。
    #[test]
    fn parse_args_reads_all_flags() {
        let raw: Vec<String> = [
            "--frames",
            "7",
            "--dt",
            "0.02",
            "--report",
            "out/report.json",
            "--dump-scene",
            "out/final.ron",
            "--screenshot",
            "out/shot.png",
            "--shot-size",
            "640x360",
            "--camera-pos",
            "1,2,3",
            "--camera-target",
            "0,-1,0.5",
            "myproj",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let args = parse_args(&raw).expect("合法参数");
        assert_eq!(args.frames, 7);
        assert_eq!(args.dt, Some(0.02));
        assert_eq!(args.report, Some(PathBuf::from("out/report.json")));
        assert_eq!(args.dump_scene, Some(PathBuf::from("out/final.ron")));
        assert_eq!(args.screenshot, Some(PathBuf::from("out/shot.png")));
        assert_eq!(args.shot_size, (640, 360));
        assert_eq!(args.camera_pos, Some([1.0, 2.0, 3.0]));
        assert_eq!(args.camera_target, Some([0.0, -1.0, 0.5]));
        assert_eq!(args.project, PathBuf::from("myproj"));
    }

    /// 截图相关参数的缺省与成对约束：`--shot-size` 单独给但不截图 = 报错
    /// （静默忽略参数是智能体最容易被坑的失败模式）。
    #[test]
    fn parse_args_validates_screenshot_option_pairings() {
        let shot_only = ["--screenshot", "a.png"];
        assert!(parse_args(&to_strings(&shot_only)).is_ok());

        // 相机参数成对：缺一不可。
        assert!(parse_args(&to_strings(&["--screenshot", "a.png", "--camera-pos", "1,2,3"])).is_err());
        assert!(
            parse_args(&to_strings(&["--screenshot", "a.png", "--camera-target", "0,0,0"]))
                .is_err()
        );
        assert!(parse_args(&to_strings(&[
            "--screenshot",
            "a.png",
            "--camera-pos",
            "1,2,3",
            "--camera-target",
            "0,0,0",
        ]))
        .is_ok());

        // 没开截图就调截图参数 = 报错，而不是默默忽略。
        for extra in [
            vec!["--shot-size", "640x360"],
            vec!["--camera-pos", "1,2,3", "--camera-target", "0,0,0"],
        ] {
            let mut raw = vec!["--frames".to_string(), "3".to_string()];
            raw.extend(to_strings(&extra));
            assert!(
                parse_args(&raw).is_err(),
                "没 --screenshot 时给 {extra:?} 应当报错"
            );
        }

        // 缺省尺寸 = 默认值。
        let args = parse_args(&to_strings(&["--screenshot", "a.png"])).unwrap();
        assert_eq!(args.shot_size, DEFAULT_SHOT_SIZE);
    }

    /// 垃圾参数必须当场报错（退出码 1），而不是默默跑出怪数据。
    #[test]
    fn parse_args_rejects_bad_values() {
        let bad: &[&[&str]] = &[
            &["--frames", "0"],
            &["--frames", "abc"],
            &["--frames", "-1"],
            &["--frames"],
            &["--dt", "0"],
            &["--dt", "-0.5"],
            &["--dt", "2.0"],
            &["--dt"],
            &["--report"],
            &["--dump-scene"],
            &["--screenshot"],
            &["--shot-size", "0x100"],
            &["--shot-size", "1280"],
            &["--shot-size", "1280x99999"],
            &["--shot-size", "axb"],
            &["--camera-pos", "1,2"],
            &["--camera-pos", "1,2,x"],
            &["--camera-pos", "1,2,NaN"],
            &["--camera-target"],
            &["--wat"],
            &["a", "b"],
        ];
        for case in bad {
            let raw = to_strings(case);
            assert!(
                parse_args(&raw).is_err(),
                "参数 {case:?} 应当被拒绝"
            );
        }
        // 上限边界：正好 MAX_FRAMES 合法、+1 非法。
        let ok = vec!["--frames".to_string(), MAX_FRAMES.to_string()];
        assert!(parse_args(&ok).is_ok());
        let over = vec!["--frames".to_string(), (MAX_FRAMES + 1).to_string()];
        assert!(parse_args(&over).is_err());

        // 截图边长边界：正好 MAX_SHOT_EDGE 合法、+1 非法。
        let edge = vec![
            "--screenshot".to_string(),
            "a.png".to_string(),
            "--shot-size".to_string(),
            format!("{MAX_SHOT_EDGE}x{MAX_SHOT_EDGE}"),
        ];
        assert!(parse_args(&edge).is_ok());
        let over = vec![
            "--screenshot".to_string(),
            "a.png".to_string(),
            "--shot-size".to_string(),
            format!("{}x720", MAX_SHOT_EDGE + 1),
        ];
        assert!(parse_args(&over).is_err());
    }

    fn to_strings(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    // -- 逐帧统计 ----------------------------------------------------------

    /// 跑请求的帧数；dt 缺省 = 固定步长时，每帧恰好一个固定步。
    #[test]
    fn headless_runs_requested_frame_count() {
        let project = temp_project("frames");
        let report = run(&opts(&project, 25)).expect("无头运行应成功");
        assert_eq!(report.frames, 25);
        assert_eq!(report.fixed_steps, 25, "dt = 固定步长 ⇒ 每帧恰好一步");
        assert_eq!(report.entities, 1, "默认场景就一个 Main Camera");
        assert_eq!(report.dt, report.fixed_dt);
        assert_eq!(report.log_info, 0, "没脚本没日志");
        assert!(!report.has_issues());
        assert!((report.total_time - 25.0 * report.dt).abs() < 1e-3);
        assert!(report.frame_us.mean > 0.0);
        assert!(report.frame_us.max >= report.frame_us.min);
        assert!(report.frame_us.p50 >= report.frame_us.min && report.frame_us.p50 <= report.frame_us.max);
        cleanup(&project);
    }

    /// `--dt` 小于固定步长时按累计器补步：固定步总数 = ⌊frames·dt/fixed_dt⌋。
    /// 证明 dt 真的进了时钟，而不是被当成"每帧一固定步"糊弄过去。
    #[test]
    fn explicit_dt_drives_the_fixed_step_accumulator() {
        let project = temp_project("dt");
        let mut args = opts(&project, 25);
        args.dt = Some(1.0 / 120.0);
        let report = run(&args).expect("无头运行应成功");
        assert_eq!(report.fixed_steps, 12, "25 帧 × 半速 = 12 个整步 + 半个余量");
        assert_ne!(report.dt, report.fixed_dt);
        cleanup(&project);
    }

    // -- 日志 --------------------------------------------------------------

    /// 脚本每帧打一条 info：计数、报告干净。
    #[test]
    fn script_logs_are_counted() {
        let project = temp_project("logs");
        write_script(
            &project,
            "tick.lua",
            "function update(ctx)\n    log(ctx, 'tick')\nend\n",
        );
        patch_scene(&project, |scene| {
            scene.entities.push(Entity::new("Logger").with_script("Scripts/tick.lua"));
        });

        let report = run(&opts(&project, 10)).expect("无头运行应成功");
        assert_eq!(report.log_info, 10, "10 帧 10 条");
        assert_eq!(report.log_error, 0);
        assert!(report.error_samples.is_empty());
        assert!(!report.has_issues(), "只有 info 日志 = 干净");
        cleanup(&project);
    }

    /// 脚本暂停：无头报告要能直接看出"暂停到底生效没"，判据是三个读数 ——
    /// `time_scale` 归零、`fixed_steps` 停止增长、`total_time` 停在那一刻。
    ///
    /// 这条是给智能体 / CI 用的：没有它，"暂停"只能靠人肉跑游戏看画面。
    #[test]
    fn script_time_scale_pause_is_reported() {
        let project = temp_project("pause");
        write_script(
            &project,
            "pause.lua",
            "local n = 0\nfunction update(ctx)\n    n = n + 1\n    if n == 5 then set_time_scale(ctx, 0) end\nend\n",
        );
        patch_scene(&project, |scene| {
            scene.entities.push(Entity::new("Pauser").with_script("Scripts/pause.lua"));
        });

        let report = run(&opts(&project, 20)).expect("无头运行应成功");
        assert_eq!(report.time_scale, 0.0, "报告要带上跑完时的缩放");
        // 1~5 帧各一步（第 5 帧的固定步在脚本之前就跑完了），之后全冻。
        assert_eq!(report.fixed_steps, 5, "暂停后固定步必须停止增长");
        assert!(
            (report.total_time - 5.0 * report.dt).abs() < 1e-4,
            "time 应停在暂停那一刻，实际 {}",
            report.total_time
        );
        assert!(!report.has_issues());
        cleanup(&project);
    }

    /// 脚本 error 会进计数与样本，报告判定为"有问题"（退出码 2 的依据）。
    #[test]
    fn script_errors_mark_the_run_as_dirty() {
        let project = temp_project("errors");
        write_script(
            &project,
            "boom.lua",
            "function update(ctx)\n    error(ctx, 'boom')\nend\n",
        );
        patch_scene(&project, |scene| {
            scene.entities.push(Entity::new("Boomer").with_script("Scripts/boom.lua"));
        });

        let report = run(&opts(&project, 12)).expect("无头运行仍应跑完");
        assert_eq!(report.log_error, 12);
        assert_eq!(report.error_samples.len(), 12.min(MAX_ERROR_SAMPLES));
        assert!(
            report.error_samples[0].contains("boom"),
            "样本要带原始消息: {:?}",
            report.error_samples
        );
        assert!(report.has_issues(), "error 日志 = 有问题");
        cleanup(&project);
    }

    /// 脚本**编译失败**不是"干净跑完"：宿主把它记成 error 日志，报告必须
    /// 跟着变脏 —— 否则一个语法写错的脚本会被无头运行静默放过。
    #[test]
    fn broken_script_is_reported_as_dirty_not_silently_skipped() {
        let project = temp_project("broken");
        write_script(&project, "broken.lua", "function update(ctx)  -- 未闭合");
        patch_scene(&project, |scene| {
            scene.entities.push(Entity::new("Broken").with_script("Scripts/broken.lua"));
        });

        let report = run(&opts(&project, 5)).expect("无头运行仍应跑完");
        assert!(report.log_error >= 1, "编译失败必须有 error 日志");
        assert!(report.has_issues());
        assert!(
            report.error_samples.iter().any(|s| s.contains("脚本加载失败")),
            "样本要指认编译失败: {:?}",
            report.error_samples
        );
        cleanup(&project);
    }

    /// 日志样本有上限：跑 100 帧 error，样本只带前 MAX_ERROR_SAMPLES 条，
    /// 但计数是全的（报告要小，真相要全）。
    #[test]
    fn error_samples_are_capped_but_count_is_total() {
        let project = temp_project("cap");
        write_script(
            &project,
            "boom.lua",
            "function update(ctx)\n    error(ctx, 'boom')\nend\n",
        );
        patch_scene(&project, |scene| {
            scene.entities.push(Entity::new("Boomer").with_script("Scripts/boom.lua"));
        });

        let report = run(&opts(&project, 30)).expect("无头运行应成功");
        assert_eq!(report.log_error, 30);
        assert_eq!(report.error_samples.len(), MAX_ERROR_SAMPLES);
        cleanup(&project);
    }

    // -- 物理与确定性 ------------------------------------------------------

    /// 落体跑两遍，落点逐位一致（确定性契约）；且真的落了（物理在跑）。
    #[test]
    fn falling_ball_is_deterministic_across_runs() {
        let project = temp_project("determ");
        patch_scene(&project, |scene| {
            scene.entities.push(
                Entity::new("Ball")
                    .with_transform(Transform::from_translation(Vec3::new(0.0, 5.0, 0.0)))
                    .with_physics(PhysicsBody::dynamic_ball(0.5)),
            );
        });

        let dump_dir = project.root().join("dump");
        let mut first = opts(&project, 120);
        first.dump_scene = Some(dump_dir.join("run1.ron"));
        let mut second = opts(&project, 120);
        second.dump_scene = Some(dump_dir.join("run2.ron"));

        let r1 = run(&first).expect("第一次运行");
        let r2 = run(&second).expect("第二次运行");
        assert_eq!(r1.bodies, 1, "球应当建出刚体");
        assert!(!r1.has_issues());
        assert_eq!(r2.entities, r1.entities);
        assert_eq!(
            r2.total_time.to_bits(),
            r1.total_time.to_bits(),
            "两次运行的模拟时长必须逐位一致"
        );

        let text1 = std::fs::read_to_string(dump_dir.join("run1.ron")).unwrap();
        let text2 = std::fs::read_to_string(dump_dir.join("run2.ron")).unwrap();
        assert_eq!(text1, text2, "固定 dt 下两次运行必须逐位一致");

        // 真落了：解析落盘场景，找 Ball 的 y。
        let scene: Scene = ron::from_str(&text1).expect("落盘场景必须能读回");
        let ball = scene.find_by_name("Ball").expect("Ball 应在场景里");
        assert!(
            ball.transform.position.y < 4.0,
            "120 帧自由落体应当明显下落，实际 y = {}",
            ball.transform.position.y
        );
        cleanup(&project);
    }

    /// NaN 变换会被点名（退出码 2 的另一半依据）。
    #[test]
    fn non_finite_transforms_are_detected() {
        let project = temp_project("nan");
        write_script(
            &project,
            "nan.lua",
            "function update(ctx)\n    local nan = 0/0\n    set_pos(ctx, nan, nan, nan)\nend\n",
        );
        patch_scene(&project, |scene| {
            scene.entities.push(Entity::new("NaN").with_script("Scripts/nan.lua"));
        });

        let report = run(&opts(&project, 3)).expect("无头运行应成功");
        assert_eq!(report.non_finite_transforms, 1);
        assert!(report.has_issues());
        cleanup(&project);
    }

    // -- 落盘 --------------------------------------------------------------

    /// `--dump-scene`：写出的场景可读回、实体数一致、报告指认路径；
    /// `--report`：文件内容与 stdout 同一份。
    #[test]
    fn dump_scene_and_report_files_are_written() {
        let project = temp_project("dump");
        let scene_out = project.root().join("out").join("final.ron");
        let report_out = project.root().join("out").join("report.json");
        let mut args = opts(&project, 5);
        args.dump_scene = Some(scene_out.clone());
        args.report = Some(report_out.clone());

        let report = run(&args).expect("无头运行应成功");
        assert_eq!(
            report.scene_dump.as_deref(),
            Some(scene_out.display().to_string().as_str())
        );

        let text = std::fs::read_to_string(&scene_out).expect("场景应当落盘");
        let scene: Scene = ron::from_str(&text).expect("场景必须能读回");
        assert_eq!(scene.entities.len(), report.entities);

        let file_text = std::fs::read_to_string(&report_out).expect("报告应当落盘");
        assert_eq!(file_text, report.render(), "文件与 stdout 必须同一份");
        let parsed: serde_json::Value = serde_json::from_str(&file_text).unwrap();
        assert_eq!(parsed["ok"], true);
        cleanup(&project);
    }

    // -- 截图 ----------------------------------------------------------

    /// 截图落盘 + 报告带实际相机 + 两次运行 PNG 逐字节一致。
    /// 无 GPU 的环境（CI）跳过 —— 见 `Docs/无头运行.md`。
    #[test]
    fn screenshot_is_written_with_deterministic_bytes() {
        let project = temp_project("shot");
        patch_scene(&project, |scene| {
            // 去掉默认场景里那台相机（`default_scene` 自带 `camera` 组件），
            // 这条测试要盯的是**自动取景**这一路。
            for entity in &mut scene.entities {
                entity.camera = None;
            }
            let mut cube = Entity::new("Cube");
            cube.model = "builtin://cube".to_owned();
            scene.entities.push(cube);
        });

        let dir = project.root().join("shots");
        let mut first = opts(&project, 10);
        first.screenshot = Some(dir.join("a.png"));
        first.shot_size = (320, 240);

        let report = match run(&first) {
            Ok(r) => r,
            Err(e) if e.contains("GPU 适配器") => {
                eprintln!("跳过截图测试：{e}");
                cleanup(&project);
                return;
            }
            Err(e) => panic!("截图运行失败: {e}"),
        };

        let shot = report.screenshot.as_ref().expect("报告必须带 screenshot 节");
        assert_eq!((shot.width, shot.height), (320, 240));
        assert_eq!(
            shot.camera_mode, "auto",
            "场景里没有相机实体 → 自动取景，报告必须说清楚是哪种机位"
        );
        assert!(shot.eye.iter().all(|v| v.is_finite()));
        // 自动取景把整个场景框进视锥 ⇒ 剔除读数必须是 0（不是"碰巧没剔"，
        // 而是"没有可剔的东西"）。这也钉死了截图走的是生产剔除路径。
        assert!(shot.render_stats.draw_calls >= 1, "立方体至少占一个 draw call");
        assert_eq!(
            shot.render_stats.culled_instances, 0,
            "自动取景框住全场，不该有实例被剔除"
        );
        assert_eq!(shot.render_stats.cull_ratio, 0.0);
        // render_stats 的字段名是报告契约的一部分（消费方：CI / MCP）。
        let value = report.to_json();
        for key in [
            "draw_calls",
            "opaque_batches",
            "opaque_instances",
            "blended_draws",
            "culled_entities",
            "culled_instances",
            "shadow_only_instances",
            "cull_ratio",
        ] {
            assert!(
                value["screenshot"]["render_stats"].get(key).is_some(),
                "screenshot.render_stats 缺字段 {key}"
            );
        }
        let path_a = std::path::PathBuf::from(&shot.path);
        assert!(path_a.is_file(), "截图文件应当落盘");

        // 像素层面：画面里有东西（不是纯背景色）—— PNG 可无损读回。
        let (w, h, pixels) = xgame_render::screenshot::load_png_rgba(&path_a).expect("PNG 读回");
        assert_eq!((w, h), (320, 240));
        assert_eq!(pixels.len(), 320 * 240 * 4);
        let (pixel_chunks, _) = pixels.as_chunks::<4>();
        let first_pixel = &pixel_chunks[0];
        assert!(
            pixel_chunks.iter().any(|p| p != first_pixel),
            "截图整张一个颜色 = 立方体没画出来"
        );

        // 第二次运行（同参数、不同输出路径）：PNG 逐字节一致。
        let mut second = opts(&project, 10);
        second.screenshot = Some(dir.join("b.png"));
        second.shot_size = (320, 240);
        let report_b = run(&second).expect("第二次截图运行");
        assert_eq!(
            report_b.screenshot.as_ref().unwrap().eye,
            shot.eye,
            "自动取景是确定性的：两次运行的相机必须一样"
        );
        let bytes_a = std::fs::read(&path_a).unwrap();
        let bytes_b = std::fs::read(dir.join("b.png")).unwrap();
        assert_eq!(bytes_a, bytes_b, "两次截图的 PNG 必须逐字节一致");
        cleanup(&project);
    }

    /// 显式相机：报告里回写的机位就是给的那个（"这张图从哪拍的"契约）。
    #[test]
    fn explicit_camera_is_reported_back() {
        let project = temp_project("shotcam");
        let mut args = opts(&project, 3);
        let dir = project.root().join("shots");
        args.screenshot = Some(dir.join("cam.png"));
        args.shot_size = (160, 120);
        args.camera_pos = Some([0.0, 0.0, 4.0]);
        args.camera_target = Some([0.0, 0.0, 0.0]);

        let report = match run(&args) {
            Ok(r) => r,
            Err(e) if e.contains("GPU 适配器") => {
                eprintln!("跳过截图测试：{e}");
                cleanup(&project);
                return;
            }
            Err(e) => panic!("截图运行失败: {e}"),
        };
        let shot = report.screenshot.as_ref().expect("报告必须带 screenshot 节");
        assert_eq!(shot.eye, [0.0, 0.0, 4.0]);
        assert_eq!(shot.target, [0.0, 0.0, 0.0]);
        assert_eq!(shot.camera_mode, "explicit", "命令行机位优先级最高");
        cleanup(&project);
    }

    /// 场景相机：截图必须用**玩家视角**，且显式机位优先于它。
    ///
    /// 这条守的是"截图能当验收证据"：场景里已经有了相机，截图却拿自动取景
    /// 顶替它，那智能体和人都看不出玩家实际看到什么。
    #[test]
    fn screenshot_prefers_scene_camera_over_auto_framing() {
        let project = temp_project("shotscene");
        patch_scene(&project, |scene| {
            // 默认场景自带一台 (0,1,-5) 的相机，这里把它挪到好认的位置。
            let cam = scene
                .entities
                .iter_mut()
                .find(|e| e.camera.is_some())
                .expect("默认场景应当自带相机组件");
            cam.transform.position = Vec3::new(7.0, 3.0, -2.0);
        });

        let dir = project.root().join("shots");
        let mut args = opts(&project, 3);
        args.screenshot = Some(dir.join("scene.png"));
        args.shot_size = (160, 120);
        let report = match run(&args) {
            Ok(r) => r,
            Err(e) if e.contains("GPU 适配器") => {
                eprintln!("跳过截图测试：{e}");
                cleanup(&project);
                return;
            }
            Err(e) => panic!("截图运行失败: {e}"),
        };
        let shot = report.screenshot.as_ref().expect("报告必须带 screenshot 节");
        assert_eq!(shot.camera_mode, "scene");
        assert_eq!(shot.eye, [7.0, 3.0, -2.0], "机位必须来自场景相机实体");

        // 同一场景 + 显式机位 → 显式赢（否则智能体无法覆盖作者机位）。
        let mut explicit = opts(&project, 3);
        explicit.screenshot = Some(dir.join("explicit.png"));
        explicit.shot_size = (160, 120);
        explicit.camera_pos = Some([1.0, 2.0, 3.0]);
        explicit.camera_target = Some([0.0, 0.0, 0.0]);
        let report = run(&explicit).expect("显式机位截图运行");
        let shot = report.screenshot.as_ref().unwrap();
        assert_eq!(shot.camera_mode, "explicit");
        assert_eq!(shot.eye, [1.0, 2.0, 3.0]);
        cleanup(&project);
    }

    // -- JSON 契约 ---------------------------------------------------------

    /// 顶层与嵌套字段名是**钉死的契约**：改字段就必须来改这条测试
    /// （配套文档：Docs/无头运行.md；消费方：CI、MCP `run_headless`）。
    #[test]
    fn json_contract_has_stable_keys() {
        let project = temp_project("contract");
        let report = run(&opts(&project, 3)).expect("无头运行应成功");
        let value = report.to_json();
        let obj = value.as_object().expect("顶层必须是对象");

        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected = vec![
            "ok",
            "engine_version",
            "project",
            "root",
            "scene",
            "scene_name",
            "scene_current",
            "scene_loads",
            "frames",
            "dt",
            "fixed_dt",
            "setup_ms",
            "run_ms",
            "total_time",
            "entities",
            "bodies",
            "colliders",
            "fixed_steps",
            "time_scale",
            "non_finite_transforms",
            "frame_us",
            "logs",
            "error_samples",
            "audio",
            "particles",
            "render",
            "scene_dump",
            "screenshot",
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected, "顶层字段名是契约，改它要同步文档与消费方");

        for key in ["mean", "min", "max", "p50", "p95", "p99"] {
            assert!(
                value["frame_us"].get(key).is_some(),
                "frame_us 缺字段 {key}"
            );
        }
        for key in ["info", "warn", "error"] {
            assert!(value["logs"].get(key).is_some(), "logs 缺字段 {key}");
        }
        for key in ["available", "active", "cached_clips", "failed_clips"] {
            assert!(value["audio"].get(key).is_some(), "audio 缺字段 {key}");
        }
        for key in ["alive", "emitters", "capacity"] {
            assert!(value["particles"].get(key).is_some(), "particles 缺字段 {key}");
        }
        for key in ["exposure", "tone_map", "bloom", "sky", "fxaa", "taa"] {
            assert!(value["render"].get(key).is_some(), "render 缺字段 {key}");
        }
        assert_eq!(
            value["render"]["tone_map"], "Aces",
            "清单默认就是 ACES（新项目的画面基线）"
        );
        assert_eq!(value["render"]["exposure"], 1.0);
        // 泛光默认关（intensity 0），三件套照常出现在报告里 —— 报告是完整
        // 快照，不跟着 `project.ron` 的 skip_serializing_if 省略字段。
        assert_eq!(value["render"]["bloom"]["intensity"], 0.0);
        assert_eq!(value["render"]["bloom"]["threshold"], 1.0);
        // `knee` 是 f32：`json!` 把它**精确加宽**成 f64（0.6000000238…），
        // 与十进制字面量 `0.6`（另一个 f64）不相等 —— 按 f32 的真值比。
        assert_eq!(value["render"]["bloom"]["knee"], f64::from(0.6f32));
        // 天空同样默认关，但字段是完整快照（智能体靠它判断"这张图有没有天空"）。
        assert_eq!(value["render"]["sky"]["enabled"], false);
        assert_eq!(value["render"]["sky"]["intensity"], 1.0);
        assert_eq!(value["render"]["sky"]["sun_size"], f64::from(1.5f32));
        for key in ["zenith", "horizon", "ground"] {
            assert!(
                value["render"]["sky"][key].as_array().is_some_and(|a| a.len() == 3),
                "sky.{key} 应当是三通道数组"
            );
        }
        // FXAA 同样默认关；报告里保留完整开关，截图是否走过第二趟 pass
        // 不用靠猜（低采样 / 细长几何时这决定了边缘像素的差异）。
        assert_eq!(value["render"]["fxaa"]["enabled"], false);
        // TAA 默认关，权重是 0.9（报告里是完整快照，智能体据此判断截图口径）。
        assert_eq!(value["render"]["taa"]["enabled"], false);
        assert_eq!(
            value["render"]["taa"]["history_weight"],
            f64::from(0.9f32),
            "0.9 是 f32 真值（0.89999997…），别按十进制字面量比"
        );
        assert!(value["scene_dump"].is_null(), "没给 --dump-scene 时是 null");
        assert!(value["screenshot"].is_null(), "没给 --screenshot 时是 null");
        cleanup(&project);
    }

    // -- CLI 壳 ------------------------------------------------------------

    /// `cli()` 是 main 的全部逻辑：干净项目 0、跑出问题 2、加载失败 1。
    #[test]
    fn cli_exit_codes_follow_the_contract() {
        let project = temp_project("cli");
        let clean: Vec<String> = vec![
            "--frames".into(),
            "3".into(),
            project.root().display().to_string(),
        ];
        assert_eq!(cli(&clean), 0, "干净跑完 = 0");

        write_script(
            &project,
            "boom.lua",
            "function update(ctx)\n    error(ctx, 'boom')\nend\n",
        );
        patch_scene(&project, |scene| {
            scene.entities.push(Entity::new("Boomer").with_script("Scripts/boom.lua"));
        });
        assert_eq!(cli(&clean), 2, "跑完但有 error 日志 = 2");

        let missing: Vec<String> = vec![
            "--frames".into(),
            "1".into(),
            project.root().join("nope").display().to_string(),
        ];
        assert_eq!(cli(&missing), 1, "项目打不开 = 1");
        assert_eq!(cli(&["--wat".into()]), 1, "参数错误 = 1");
        cleanup(&project);
    }

    // -- 仓库示例项目 ------------------------------------------------------

    /// 仓库自带的示例项目必须能"干净跑完"：脚本无 error、物理在跑、
    /// 音频剪辑能解码（silent 引擎照常解）。示例脚本改了行为但写坏了
    /// 日志 / 物理，会在 CI 里被这条测试抓出来。
    #[test]
    fn bundled_sample_project_runs_clean_headless() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../projects/SampleScene");
        if !root.join("project.ron").is_file() {
            return; // 脱离仓库单独分发时跳过
        }

        let report = run(&HeadlessArgs {
            project: root,
            frames: 60,
            ..HeadlessArgs::default()
        })
        .expect("示例项目应当能无头跑");

        assert!(!report.has_issues(), "示例项目必须干净: {:?}", report.error_samples);
        assert!(report.entities >= 5, "示例场景实体数: {}", report.entities);
        assert!(report.bodies >= 1, "示例场景有物理实体");
        assert_eq!(report.fixed_steps, 60);
        assert!(
            report.audio_cached_clips >= 1,
            "autoplay 的 Beep.wav 应当被 silent 引擎解码进缓存: {}",
            report.audio_cached_clips
        );
        assert_eq!(report.audio_failed_clips, 0);
    }
}
