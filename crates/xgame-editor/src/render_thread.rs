//! 渲染线程——把 3D 视口渲染从 UI 线程剥离。
//!
//! # 为什么要有这条线程
//!
//! 旧架构里，离屏 3D 渲染（MSAA + 阴影 pass + 全场景提交）在 winit 事件循环的
//! **同一个线程**上同步执行：一帧 UI 里嵌着一次完整的 GPU 提交，渲染慢一毫秒，
//! 输入响应就晚一毫秒。拖拽 gizmo、转相机时 p99 帧耗 ~35ms，手感就是"一顿一顿"。
//! 商业引擎（Unity / Godot / Unreal）的编辑器无一例外地把场景渲染放在独立线程，
//! UI 线程只消费"最新渲好的那张图"——这正是本模块复制的结构。
//!
//! # 结构
//!
//! ```text
//! UI 线程                              渲染线程
//! ─────                                ──────
//! 输入处理 / 面板 / gizmo
//!   │ 指纹变了才组装 RenderJob ──▶ 持有 SceneRenderer / wgpu 资源
//!   │ (try_send, 满了就丢)           阻塞等任务；取到后只画**最新**一帧
//!   │                                模型加载、视口重建都在这里
//!   │ ◀── FrameResult(TextureId) ──
//! 拿最新 TextureId 贴到视口
//! ```
//!
//! # 关键取舍
//!
//! - **latest-wins，绝不排队**。两条通道容量都是 1：渲染线程忙不过来时 UI 直接
//!   丢帧（下帧再发），UI 没来得及取的结果被更新的覆盖。排队只会把延迟越积越大。
//! - **指纹在 UI 侧算**（`SceneRenderer::scene_signature`），没变就整帧不发 ——
//!   静态场景时渲染线程完全睡眠，一分钱 GPU 都不花。指纹里也含视口尺寸与相机，
//!   所以飞行/缩放/拖拽时每帧都会发，渲染线程按自己的节奏出帧，UI 不被拖慢。
//! - **实体快照用 `Arc`**：Play 模式下每帧都要发（物理在改变换），快照成本是
//!   一次 `Vec<Entity>` clone，与旧路径里 `world_matrices()` 的逐帧分配同级，
//!   换来的是 UI 线程零 GPU 阻塞。
//! - **拾取用的网格包围盒**由渲染线程加载模型后随 `FrameResult` 回传，UI 侧聚成
//!   一张只增不减的表；还没加载过的模型退回单位盒（与旧行为一致）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;

use glam::{Mat4, Vec3};

use xgame_core::{Entity, Light};
use xgame_render::gltf_loader::Bounds;
use xgame_render::scene_pipeline::SceneLighting;

use crate::scene_renderer::SceneRenderer;

/// UI 线程发给渲染线程的一帧渲染任务。
///
/// 全部 owned：渲染线程不碰 `Scene`，自然不存在"UI 改场景改到一半、渲染读到
/// 中间态"的竞态。
pub struct RenderJob {
    /// 输入指纹（UI 侧已算好）。渲染线程信任它，不再重扫一遍实体。
    pub signature: u64,
    /// 视口尺寸（物理像素）。
    pub size: (u32, u32),
    pub view_proj: Mat4,
    pub camera_pos: Vec3,
    pub ambient: [f32; 3],
    pub lights: Vec<Light>,
    /// 场景实体快照（与世界矩阵同序对应）。
    pub entities: Arc<Vec<Entity>>,
    pub world_matrices: Arc<Vec<Mat4>>,
}

/// 渲染线程画完一帧后回传的结果。
struct FrameResult {
    /// 当前可用的 egui 纹理（视口重建中等情况可能暂时没有）。
    texture_id: Option<egui::TextureId>,
    /// 这一帧新加载的模型的本地包围盒，给 UI 侧拾取用。
    new_bounds: Vec<(String, Bounds)>,
}

enum RenderMsg {
    Job(Box<RenderJob>),
    Shutdown,
}

/// UI 线程持有的渲染线程句柄。
///
/// 关闭项目 / 退出应用时随 `EditorState` 一起 drop：`Drop` 会发 `Shutdown` 并
/// join 线程，不会留下孤儿线程持有 wgpu 设备。
pub struct RenderHandle {
    tx: SyncSender<RenderMsg>,
    rx: Receiver<FrameResult>,
    thread: Option<JoinHandle<()>>,
    /// 渲染线程最近一次交卷的纹理。UI 每帧拿它贴视口。
    latest_texture: Option<egui::TextureId>,
    /// 最近一次**成功发出**的任务指纹。相同输入不重复发（静态场景渲染线程睡眠）。
    last_sent_signature: Option<u64>,
    /// 已加载模型的本地包围盒（渲染线程回传累积）。
    bounds: HashMap<String, Bounds>,
    /// 已发出但还没交卷的任务数。
    ///
    /// 大于 0 时 UI 必须用 `request_repaint_after` 保持轮询 —— 否则 Hybrid 帧调度
    /// 在"无输入"时停帧，渲染好的新纹理到了也没人贴出来（打开项目后视口会一直
    /// 是空白，直到用户动一下鼠标）。
    in_flight: usize,
}

impl RenderHandle {
    /// 启动渲染线程。wgpu 的 `Device`/`Queue` 与 egui-wgpu 渲染器都是可跨线程
    /// 共享的句柄，线程内自己构造 `SceneRenderer`（管线创建放线程里做，不阻塞
    /// UI 第一帧）。
    pub fn spawn(
        device: &Arc<wgpu::Device>,
        queue: &Arc<wgpu::Queue>,
        egui_renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
        surface_format: wgpu::TextureFormat,
        project_root: PathBuf,
        sample_count: u32,
    ) -> Self {
        let (job_tx, job_rx) = sync_channel::<RenderMsg>(1);
        // 结果通道给 4 个槽位：每个任务必然产出一份结果（UI 靠它给 in_flight
        // 计数销账），槽位要是不够、结果被丢，UI 的账就永远销不掉。
        let (result_tx, result_rx) = sync_channel::<FrameResult>(4);

        let device = Arc::clone(device);
        let queue = Arc::clone(queue);
        let thread = std::thread::Builder::new()
            .name("xgame-scene-render".to_owned())
            .spawn(move || {
                Self::render_loop(
                    job_rx,
                    result_tx,
                    device,
                    queue,
                    egui_renderer,
                    surface_format,
                    project_root,
                    sample_count,
                );
            })
            .expect("启动渲染线程失败");

        Self {
            tx: job_tx,
            rx: result_rx,
            thread: Some(thread),
            latest_texture: None,
            last_sent_signature: None,
            bounds: HashMap::new(),
            in_flight: 0,
        }
    }

    /// 渲染线程主循环：阻塞等任务，醒来只画**最新**一帧。
    #[allow(clippy::too_many_arguments)]
    fn render_loop(
        job_rx: Receiver<RenderMsg>,
        result_tx: SyncSender<FrameResult>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        egui_renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
        surface_format: wgpu::TextureFormat,
        project_root: PathBuf,
        sample_count: u32,
    ) {
        let mut renderer = SceneRenderer::new(
            &device,
            &queue,
            egui_renderer,
            surface_format,
            project_root,
            sample_count,
        );

        'outer: while let Ok(msg) = job_rx.recv() {
            let mut job = match msg {
                RenderMsg::Shutdown => break,
                RenderMsg::Job(job) => job,
            };
            // 清掉积压：通道里若已有更新的任务，旧的一帧都不值得画。
            // UI 发得比渲染快时（拖相机撞上一帧慢渲染），这是"最新优先"的关键。
            loop {
                match job_rx.try_recv() {
                    Ok(RenderMsg::Job(newer)) => job = newer,
                    Ok(RenderMsg::Shutdown) => break 'outer,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break 'outer,
                }
            }

            renderer.set_viewport_size(job.size.0, job.size.1);
            let lighting = SceneLighting::new(job.ambient, &job.lights);
            let loaded = renderer.render(
                &job.entities,
                &job.world_matrices,
                &job.view_proj,
                &job.camera_pos,
                lighting,
            );

            let result = FrameResult {
                texture_id: renderer.texture_id(),
                new_bounds: loaded,
            };
            // 每个任务都必须交一份卷（UI 按卷给 in_flight 销账）。UI 在 await
            // 期间每帧都会把通道取空，正常不会满；真满了就自旋等一等 —— 阻塞的
            // 是渲染线程自己，UI 不受影响。（`SyncSender::send_timeout` 还是
            // 不稳定 API，手写这个有界自旋。）
            let mut result = result;
            for _ in 0..100 {
                match result_tx.try_send(result) {
                    Ok(()) => break,
                    Err(TrySendError::Full(returned)) => {
                        result = returned;
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(TrySendError::Disconnected(_)) => break 'outer,
                }
            }
        }
    }

    /// 收渲染线程的回卷：更新"最新纹理"与包围盒表。每帧调用一次。
    pub fn poll(&mut self) {
        while let Ok(result) = self.rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            if result.texture_id.is_some() {
                self.latest_texture = result.texture_id;
            }
            for (model, bounds) in result.new_bounds {
                self.bounds.insert(model, bounds);
            }
        }
    }

    /// 是否还有已发出、未交卷的渲染任务（有就该保持 UI 轮询，见 `in_flight`）。
    pub fn is_awaiting_frame(&self) -> bool {
        self.in_flight > 0
    }

    /// 当前该贴到视口上的纹理（还没有任何一帧交卷时为 `None`）。
    pub fn latest_texture(&self) -> Option<egui::TextureId> {
        self.latest_texture
    }

    /// 某个模型标识的本地包围盒（渲染线程加载过才有）。
    pub fn mesh_bounds(&self, model: &str) -> Option<Bounds> {
        self.bounds.get(model).copied()
    }

    /// 这份指纹要不要发给渲染线程（与上次成功发出的相同就不发）。
    pub fn should_send(&self, signature: u64) -> bool {
        self.last_sent_signature != Some(signature)
    }

    /// 提交一帧渲染任务。
    ///
    /// 渲染线程还在忙（通道满）时**直接丢帧**并返回 `false`：UI 线程绝不阻塞等
    /// GPU，下一帧会带着更新的相机/场景再来试。指纹记账只在真发出去时更新，
    /// 被丢的帧不会挡住下一帧重试。
    pub fn submit(&mut self, job: RenderJob) -> bool {
        let signature = job.signature;
        match self.tx.try_send(RenderMsg::Job(Box::new(job))) {
            Ok(()) => {
                self.last_sent_signature = Some(signature);
                self.in_flight += 1;
                true
            }
            Err(TrySendError::Full(_)) => false,
            Err(TrySendError::Disconnected(_)) => false,
        }
    }
}

impl Drop for RenderHandle {
    fn drop(&mut self) {
        // 任务通道容量是 1，此刻可能正好满着（线程在画一帧慢渲染）——
        // 短暂自旋等它腾位置：通道每个循环迭代都会排干，Shutdown 很快就能进去。
        // 有界重试（~2s）兜底"线程卡死"的极端情况，宁可丢线程也别把 UI 卡死。
        let mut msg = RenderMsg::Shutdown;
        let mut delivered = false;
        for _ in 0..2000 {
            match self.tx.try_send(msg) {
                Ok(()) => {
                    delivered = true;
                    break;
                }
                Err(TrySendError::Full(returned)) => {
                    msg = returned;
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(TrySendError::Disconnected(_)) => {
                    delivered = true; // 线程已经没了，无需再通知
                    break;
                }
            }
        }
        if let Some(thread) = self.thread.take() {
            if delivered {
                let _ = thread.join();
            }
            // 没送达到（线程卡死）：不 join，宁可把线程留活也不卡死 UI 退出。
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// latest-wins 语义：指纹没变不该发；变了才发。
    ///
    /// 这是"静态场景渲染线程完全睡眠"的全部依据 —— 一旦这条退化，空闲时
    /// 渲染线程会被一帧帧同样的任务唤醒空转，GPU 白烧。
    #[test]
    fn should_send_only_when_signature_changes() {
        // 不真起线程：构造一个"假"句柄只测记账逻辑。
        let (tx, _rx) = sync_channel::<RenderMsg>(1);
        let (_result_tx, result_rx) = sync_channel::<FrameResult>(4);
        let mut handle = RenderHandle {
            tx,
            rx: result_rx,
            thread: None,
            latest_texture: None,
            last_sent_signature: None,
            bounds: HashMap::new(),
            in_flight: 0,
        };

        assert!(handle.should_send(42), "第一次任何指纹都该发");
        handle.last_sent_signature = Some(42);
        assert!(!handle.should_send(42), "同样的指纹不该重复发");
        assert!(handle.should_send(43), "指纹变了必须发");
    }
}
