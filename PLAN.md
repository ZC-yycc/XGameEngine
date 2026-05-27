# XGameEngine 开发计划

> 从"编辑器原型"到"可运行的游戏引擎"的演进路线。


## 一、现状：编辑器即一切

当前代码经过第一阶段重构后，架构如下：

```
xgame-core          ← 核心类型：Color, Config, Transform, Scene, Entity
     ↑
xgame-math          ← 数学：glam 重导出 + projection 辅助
     ↑
xgame-render        ← wgpu 渲染：相机、网格、纹理、glTF 加载
xgame-physics       ← rapier3d 物理：刚体、碰撞体、物理世界
     ↑
xgame-editor        ← 唯一可运行的二进制
     ├── panels/    ← egui UI 面板
     ├── scene_renderer.rs  ← 编辑器离屏渲染器
     └── game_runtime.rs   ← 游戏运行时（已提取）
```

### 已解决的问题

| 问题 | 状态 |
|---|---|
| Transform 在 xgame-math，造成循环依赖 | 已修复：下沉到 xgame-core |
| Scene/Entity 在 xgame-render，层级错误 | 已修复：下沉到 xgame-core |
| 物理逻辑混在 EditorState 中 | 已修复：提取为 GameRuntime |
| Play 按钮无效果 | 已修复：物理引擎接入 |
| eframe 无事件时不调用 update() | 已修复：request_repaint() |
| 物理步进与渲染循环耦合 | 已缓解：GameRuntime 提供 clean API |

### 仍存在的问题

| 问题 | 影响 |
|---|---|
| 游戏逻辑与 UI 渲染在同一个 update() 回调中 | 帧率不稳定，无法固定步长 |
| 物理步进使用预测 dt，非固定步长 | 模拟精度随帧率波动 |
| 没有独立的游戏二进制 | 无法脱离编辑器运行 |
| 渲染有双轨（xgame-render RenderContext vs SceneRenderer） | 维护成本 |
| Play 期间不能增删实体 | entity_to_body 映射会失效 |
| collider 没有可视化 | 调试困难 |
| 没有 ECS 或组件系统 | 扩展性差 |


## 二、三阶段路线图

```
阶段一（已完成）        阶段二（当前目标）        阶段三（远期）
┌─────────────┐       ┌─────────────┐       ┌─────────────┐
│ 层重构       │       │ 固定步长循环  │       │ 独立游戏二进制 │
│ GameRuntime  │ ───→  │ 编辑器持续集成 │ ───→  │ IPC/分离架构  │
│ 物理集成     │       │ Physics Debug │       │ 完整游戏管线  │
└─────────────┘       └─────────────┘       └─────────────┘
```


## 三、阶段二：固定步长循环 + 编辑器强化

### 3.1 固定步长物理

**目标**：物理模拟以固定频率（如 60Hz）运行，与渲染帧率解耦。

**方案**：在 `GameRuntime` 内部维护累加器。

```rust
// game_runtime.rs 新增
const FIXED_DT: f32 = 1.0 / 60.0;

pub struct GameRuntime {
    physics: PhysicsWorld,
    entity_to_body: HashMap<usize, u64>,
    saved_states: Vec<Transform>,
    accumulator: f32,          // ← 新增
}

pub fn fixed_update(&mut self, dt: f32, entities: &mut [Entity]) {
    self.accumulator += dt;
    while self.accumulator >= FIXED_DT {
        self.physics.step(FIXED_DT);
        self.accumulator -= FIXED_DT;
    }
    // 可选：插值
    sync_transforms(entities);
}
```

**改动范围**：仅 `game_runtime.rs`。

**验收标准**：
- 60fps 渲染时物理以 60Hz 运行
- 渲染降至 30fps 时物理仍以 60Hz 运行

### 3.2 编辑器期间实体增删保护

**目标**：Play 模式下 Hierarchy 面板禁用增删，或自动同步。

**方案**：
- `EditorState::play_mode == Playing` 时，`create_entity()` 和 `delete_selected()` 返回 early
- 或在 `GameRuntime` 中处理实体变化（增量同步，更复杂）

**改动范围**：
- `state.rs`：`create_entity()` / `delete_selected()` 加守卫
- `hierarchy.rs`：按钮在 Play 模式下禁用

**验收标准**：
- Play 模式无法在 Hierarchy 中增删实体
- 状态栏显示提示

### 3.3 Physics Debug 绘制

**目标**：Physics 复选框开启后，场景视图中显示碰撞体线框。

**方案**：
- `GameRuntime` 添加 `debug_lines()` 方法，返回 `Vec<DebugLine>`
- `PhysicsWorld` 遍历 collider_set，获取每个 collider 的 AABB 或形状
- `SceneRenderer` 添加线框渲染管线（或简单在 egui 层绘制 overlay）

**改动范围**：
- `world.rs`：添加 debug draw 方法
- `game_runtime.rs`：暴露 debug_lines
- `scene_view.rs`：叠加绘制
- `scene_renderer.rs`：可选线框 pipeline

**验收标准**：
- 勾选 Physics 后每个实体周围显示白色线框
- 接触点显示十字标记

### 3.4 状态栏 Play 模式信息

**目标**：状态栏显示物理运行状态（实体数/碰撞事件数/FPS）。

**方案**：扩展状态栏格式化。

**验收标准**：状态栏显示 `Playing | 3 entities | 60.0 FPS | Bodies: 3 | Colliders: 3`


## 四、阶段三：独立游戏二进制

### 4.1 创建 xgame-game crate

```
crates/xgame-game/
  ├── Cargo.toml    ← 仅依赖 xgame-core, xgame-math, xgame-render, xgame-physics
  └── src/
      └── main.rs   ← 独立 winit + wgpu 循环
```

**核心循环**：

```rust
fn main() {
    let scene = load_scene("Scenes/scene.ron");
    let mut runtime = GameRuntime::start(&scene.entities);
    let mut renderer = StandaloneRenderer::new();

    loop {
        let dt = timer.tick();
        process_input();
        runtime.fixed_update(dt, &mut scene.entities);
        renderer.render(&scene.entities);
    }
}
```

### 4.2 收拢渲染路径

**问题**：当前有两个平行渲染实现：
- `xgame-render::lib::RenderContext`（窗口 + 交换链，未使用）
- `crate::scene_renderer::SceneRenderer`（编辑器离屏渲染，实际在用）

**方案**：
- 从 `SceneRenderer` 中提取共享渲染逻辑到 `xgame-render`
- `SceneRenderer` 仅保留离屏纹理注册到 egui 的部分
- 创建 `StandaloneRenderer` 直接使用 `xgame-render` 的 `RenderContext`

### 4.3 编辑器 x 运行时交互

**方案 A——子进程**：
- 编辑器启动 `xgame-game` 并传入场景路径
- 通过 stdin/stdout IPC 或共享文件传递状态
- 编辑器停止子进程，读取最终状态

**方案 B——预览模式**：
- 编辑器内嵌 GameRuntime，原地运行
- 无进程隔离，但更简单

推荐：**先实现方案 B**（当前模式），阶段三后期再考虑方案 A。


## 五、架构原则

1. **层间单向依赖**：core ← math ← {render, physics} ← editor
2. **GameRuntime 不依赖编辑器**：核心业务逻辑与 UI 解耦
3. **固定步长优先**：物理 / 未来动画系统使用固定时间步长
4. **功能先于性能**：先跑通，再优化


## 六、文件清单

### 已创建/修改

| 文件 | 说明 |
|---|---|
| `crates/xgame-core/src/transform.rs` | Transform 从 xgame-math 移入 |
| `crates/xgame-core/src/scene.rs` | Entity/Scene 从 xgame-render 移入 |
| `crates/xgame-editor/src/game_runtime.rs` | 新增，编辑器无关的游戏运行时 |

### 后续需创建

| 文件 | 阶段 | 说明 |
|---|---|---|
| `crates/xgame-game/Cargo.toml` | 三 | 独立游戏二进制 |
| `crates/xgame-game/src/main.rs` | 三 | 独立游戏循环 |

### 后续需修改

| 文件 | 阶段 | 说明 |
|---|---|---|
| `crates/xgame-editor/src/game_runtime.rs` | 二 | 固定步长累加器 |
| `crates/xgame-editor/src/panels/state.rs` | 二 | Play 模式增删守卫 |
| `crates/xgame-editor/src/panels/hierarchy.rs` | 二 | Play 模式禁用按钮 |
| `crates/xgame-render/src/lib.rs` | 三 | 共享渲染逻辑 |
| `crates/xgame-physics/src/world.rs` | 二 | debug draw 方法 |
