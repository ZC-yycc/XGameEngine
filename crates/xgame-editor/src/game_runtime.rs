//! 编辑器内嵌的游戏运行时。
//!
//! 这一层薄得几乎不存在 —— 真正的逻辑在 [`xgame_runtime::World`]，
//! 编辑器按下「▶ Play」跑的**就是**打包出去的游戏那一份实现。
//!
//! # 为什么只留一层"薄壳"
//!
//! 早先这里自带一套物理步进 + 变换回写，与 `xgame-game` 里的另一套**并行存在**。
//! 两份实现迟早会漂移（事实上漂过一次：一处漏了父世界矩阵的逆变换，
//! 于是"父节点一动、子节点被物理拽着跑偏"）。现在只有一份，编辑器只是
//! 多负责两件事：
//!
//! 1. **场景的所有权交换**：`World` 拥有场景，而编辑器的面板全都直接读
//!    `state.scene`。见 [`GameRuntime::update_with_scene`] 的说明。
//! 2. **脚本日志转发**：脚本的 `log()` 要进编辑器的 Console 面板。

use std::path::Path;

use xgame_core::Scene;
use xgame_input::InputMap;
use xgame_physics::DebugLine;
use xgame_runtime::{FrameReport, ScriptLog, World, WorldConfig};

/// 项目根目录下输入映射的文件名。
pub const INPUT_MAP_FILE: &str = "input.ron";

pub struct GameRuntime {
    world: World,
    /// 上一帧的统计（状态栏读）。
    report: FrameReport,
}

impl GameRuntime {
    /// 用一份场景快照启动运行时。
    ///
    /// 收**快照**而不是借用编辑器的场景：Play 跑起来之后编辑器手里那份必须
    /// 保持"进入 Play 之前的样子"，退出时才能原样恢复。
    pub fn start(scene: Scene, project_root: &Path) -> Self {
        let input_map = load_input_map(project_root);
        let world = World::new(scene, project_root, WorldConfig::default())
            .with_input_map(input_map);
        Self {
            world,
            report: FrameReport::default(),
        }
    }

    /// 推进一帧，并把运行时里的场景内容**换回**编辑器持有的那份。
    ///
    /// # 这个"换"是怎么回事
    ///
    /// `World` 拥有场景，而编辑器的面板全都直接读 `state.scene.entities`。
    /// 让面板改读 `runtime.scene()` 需要动几十处调用点；给场景包一层
    /// `Rc<RefCell<_>>` 又会把借用检查挪到运行期。两者都不划算。
    ///
    /// 所以这里用 `std::mem::swap` 交换整个 `Scene`：它内部只有三个 `Vec`/`String`
    /// 和一个小数组，交换是 **O(1)**（换指针，不是逐个元素搬），
    /// 比"每帧克隆一份场景"便宜好几个数量级 —— 而后者正是本项目早就明令禁止的
    /// 每帧副作用。
    ///
    /// 于是每帧的顺序是：
    ///
    /// ```text
    /// swap      运行时拿到编辑器那份（内容与它自己那份一致）→ 编辑器侧暂时是旧内容
    /// update    运行时推进物理 / 脚本，改的是同一批实体
    /// swap      结果交回编辑器 → 面板 / 渲染读到的是最新状态
    /// ```
    ///
    /// 中间那一小段"编辑器侧是旧内容"没有任何读者：`update` 期间没人访问
    /// `state.scene`。这一点是这段代码成立的前提，改动帧序时要留意。
    pub fn update_with_scene(&mut self, dt: f32, editor_scene: &mut Scene) {
        std::mem::swap(editor_scene, self.world.scene_mut());
        self.report = self.world.update(dt);
        std::mem::swap(editor_scene, self.world.scene_mut());
    }

    /// 取走本帧脚本产生的日志。
    pub fn take_logs(&mut self) -> Vec<ScriptLog> {
        self.world.take_logs()
    }

    /// 重新编译所有脚本（脚本热重载）。
    pub fn reload_scripts(&mut self) {
        self.world.reload_scripts();
    }

    pub fn debug_lines(&self) -> Vec<DebugLine> {
        self.world.debug_lines()
    }

    pub fn stats(&self) -> (usize, usize) {
        (self.report.bodies, self.report.colliders)
    }

    /// 运行时输入（Play 模式下由 `input_bridge` 从 egui 灌进来）。
    pub fn input_mut(&mut self) -> &mut xgame_input::Input {
        self.world.input_mut()
    }
}

/// 读项目根目录的 `input.ron`；没有就退回默认映射，坏了就报警再退回默认。
///
/// 「文件不存在」和「文件写坏了」是两件不同的事，前者正常、后者要提示作者。
fn load_input_map(project_root: &Path) -> InputMap {
    let path = project_root.join(INPUT_MAP_FILE);
    match InputMap::load(&path) {
        Ok(Some(map)) => map,
        Ok(None) => InputMap::default_game(),
        Err(e) => {
            log::warn!("输入映射 {} 解析失败，改用默认映射: {e}", path.display());
            InputMap::default_game()
        }
    }
}
