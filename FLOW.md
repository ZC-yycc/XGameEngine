# XGameEngine 运行流程

> 从启动到关机，每一帧发生了什么。
>
> 入口：`cargo run -p xgame-editor`

---

## 一、启动阶段

```
main()                           crates/xgame-editor/src/main.rs
  └─ eframe::run_native(...)     egui 原生框架 (winit + wgpu)
       └─ EditorApp::new(cc)     初始化
```

`EditorApp::new` 执行：

```
EditorState::new()
  ├── 尝试加载 Scenes/scene.ron
  │     ├── 成功 → 解析为 Scene { name, entities }
  │     └── 失败 → 创建默认场景 (Main Camera + Directional Light)
  └── 从 eframe 的 wgpu device/queue 创建 SceneRenderer（离屏渲染器）
```

---

## 二、每帧循环

eframe 每帧调用 `EditorApp::update(ctx)`，按序执行 5 个阶段：

```
 1  theme::install(ctx)          应用 Unity 暗色主题
 2  toolbar::show() + 模式切换   播放/暂停/停止，变换工具
 3  physics_step() (仅 Playing)   固定步长物理模拟
 4  CentralPanel (egui_tiles)     4 面板 Dock 布局
 5  StatusBar                     底部状态栏
```

### 阶段 1：主题

```rust
panels::theme::install(ctx);
```

深色背景 + 蓝色强调色，锐利边角，Unity 风格。每帧调用无副作用（`set_style` 覆盖）。

### 阶段 2：Toolbar + 模式切换

```rust
let prev_mode = self.state.play_mode;
panels::toolbar::show(&mut self.state, ctx);
match (prev_mode, self.state.play_mode) {
    (Edit, Playing) | (Paused, Playing) => enter_play_mode(),
    (Playing, Edit) | (Paused, Edit)    => exit_play_mode(),
    _ => {}
}
```

**模式生命周期：**

```
Edit ──▶ [▶ Play] ──▶ Playing ──▶ [⏸ Pause] ──▶ Paused
  ▲                                                   │
  └────────────────── [⏹ Stop] ───────────────────────┘
```

- **Playing**：创建 `GameRuntime` → 保存实体变换 → 启动 `PhysicsWorld`
- **Paused**：暂停物理步进，场景视图保持最后状态
- **Edit**：销毁 `GameRuntime` → 恢复保存的变换

### 阶段 3：物理步进

仅在 `play_mode == Playing` 时执行：

```rust
// crates/xgame-editor/src/game_runtime.rs
pub fn fixed_update(&mut self, dt: f32, entities: &mut [Entity]) {
    self.clock.tick(dt);                              // 累加帧时间
    while self.clock.consume_fixed_update() {          // 消耗固定步长 (1/60s)
        self.physics.step(self.clock.fixed_dt());      // rapier3d 模拟
    }
    self.clock.reset_step_counter();                  // 重置步数限制
    self.sync_transforms(entities);                   // 物理 → 实体同步
}
```

物理与渲染帧率**完全解耦**。渲染 30fps 时物理仍跑 60Hz。
`max_steps = 5` 防止死亡螺旋。

### 阶段 4：四面板 Dock 布局

```
egui_tiles::Tree 布局

┌────────────┬───────────────────────┬──────────────┐
│ Hierarchy  │     Scene View        │  Inspector   │
│   20%      │        75%            │    25%       │
│            ├───────────────────────┤              │
│            │     Console           │              │
│            │        25%            │              │
└────────────┴───────────────────────┴──────────────┘
```

每个面板通过 `#[derive(Panel)]` 派生宏自动分发到 `show_content()`。

#### 4a. Hierarchy 面板

```
hierarchy::show_content(state, ui)
  ├── 搜索过滤器 (TextEdit)
  ├── [+] 创建按钮 (Play 模式下禁用 + 黄字提示)
  ├── 实体列表 (ScrollArea)
  │     ├── 过滤/选择/高亮
  │     └── 右键菜单: Create Empty / Duplicate / Delete
  ├── [Create Empty] 底部按钮
  └── 快捷键: Delete (删除), Ctrl+D (复制), 仅在 Edit 模式有效
```

**Play 模式守卫**：`create_entity()` / `duplicate_entity()` / `delete_selected()`
在非 Edit 模式返回 early 并输出警告日志。

#### 4b. Scene View 面板（核心）

```
scene_view::show_content(state, ui)
  │
  ├── 顶部栏: 场景名称 + 相机坐标
  │
  ├── 计算 view_proj 矩阵
  │     proj = perspective(60°, aspect, 0.1, 100)
  │     view = look_at(eye, eye + forward, Y)
  │     view_proj = proj * view
  │
  ├── renderer.render(entities, view_proj, camera_pos)
  │   │
  │   ├── Phase 1: 模型加载（惰性）
  │   │     entity.model → GltfMesh::from_file()
  │   │     解析 glTF → 提取 position/normal/color/texcoord
  │   │     上传 GPU → 缓存于 model_cache
  │   │
  │   ├── Phase 2: wgpu 渲染
  │   │     ├── 写入 CameraUniform (view_proj, camera_pos)
  │   │     ├── 开启 RenderPass (背景 #1E1E24)
  │   │     ├── 遍历 entities:
  │   │     │   ├── 跳过无模型 / 未加载实体
  │   │     │   ├── transform → ModelUniform (T*R*S)
  │   │     │   └── 逐 primitive: set_vb / set_ib / draw_indexed
  │   │     └── 提交命令
  │   │
  │   └── 渲染结果 → egui 纹理 (离屏 Texture → TextureId)
  │
  ├── painter.image(tex_id, rect, uv)  显示 3D 场景
  │
  ├── [可选] Physics Debug 叠加
  │   if show_physics_debug && game_runtime.is_some():
  │     state.debug_lines() → Vec<DebugLine>
  │     project_to_screen(start, view_proj) → egui Pos2
  │     project_to_screen(end, view_proj)
  │     painter.add(Shape::line_segment(..., 半透明白/绿/红))
  │
  ├── 飞行相机控制 (右键按住)
  │     rotate(dx, dy)  |  pan_forward(W/S)  |  pan_right(A/D)
  │     move_up(Q/E)    |  zoom(scroll)
  │
  └── 叠加层
        FPS | Tool: Translate
        Speed: X.X (滚轮后短暂显示)
```

**WGSL 着色器**（内联在 `scene_renderer.rs`）：

```wgsl
// 顶点阶段
world_pos = model * position;
normal_mat = transpose(inverse(mat3(model)));
clip_pos = view_proj * world_pos;

// 片元阶段
N = normalize(world_normal);
L = normalize(light_direction);
output = vertex_color * (ambient + max(dot(N, -L), 0) * light_color);
```

**调试线投影**（将 3D 世界坐标映射到 egui 屏幕坐标）：

```rust
fn project_to_screen(point: Vec3, view_proj: &Mat4, screen_size: Vec2) -> Pos2 {
    let clip = *view_proj * Vec4::new(point.x, point.y, point.z, 1.0);
    let ndc = clip.truncate() / clip.w;               // [-1, 1]
    let x = (ndc.x + 1.0) * 0.5 * screen_size.x;
    let y = (1.0 - ndc.y) * 0.5 * screen_size.y;     // 翻转 Y
    egui::pos2(x, y)
}
```

#### 4c. Inspector 面板

```
inspector::show_content(state, ui)
  └── 选中实体时:
        ├── Name: [TextEdit]
        ├── Transform:
        │     ├── Position:  [x] [y] [z]
        │     ├── Rotation:  [x] [y] [z] [w]
        │     └── Scale:     [x] [y] [z]
        └── Model: [TextEdit]
```

#### 4d. Console 面板

```
console::show_content(state, ui)
  ├── [Clear] 按钮 + 消息计数
  ├── 消息列表 (ScrollArea, 自动滚动)
  │     └── 每条: 级别图标 + 文本 + 去重计数 (×N)
  └── 最多保留 500 条
```

### 阶段 5：状态栏

```
StatusBar
  "v0.1.0 | Edit Mode | Scene: Test Scene | 3 entities | FPS: 60"

  Playing 时额外显示:
  "▶ Playing | ... | Bodies: 3 | Colliders: 3 | FPS: 60"
```

---

## 三、数据流与状态模型

```
EditorState（中央状态，所有面板共享可变引用）
  │
  ├── scene: Scene
  │     ├── name: String
  │     └── entities: Vec<Entity>
  │           ├── name: String            显示名称
  │           ├── transform: Transform    位置/旋转/缩放
  │           ├── model: String           glTF 路径
  │           ├── id: EntityId            稳定标识符 (u64)
  │           └── components: ComponentMap 运行时组件
  │
  ├── editor_camera: EditorCamera        自由飞行相机 (yaw/pitch/position)
  ├── selected_entity: Option<usize>     场景 entities 索引
  ├── play_mode: PlayMode                { Edit, Playing, Paused }
  │
  ├── show_grid: bool
  ├── show_physics_debug: bool
  │
  ├── scene_renderer: Option<SceneRenderer>  wgpu 离屏渲染器
  └── game_runtime: Option<GameRuntime>      物理运行时 (仅 Playing)
        ├── physics: PhysicsWorld             rapier3d 世界
        ├── entity_to_body                  实体索引 → 刚体 ID
        ├── saved_states                     备份变换 (退出时恢复)
        └── clock: GameClock                  固定步长 60Hz
```

**持久化**：`Scenes/scene.ron`

```ron
Scene(
    name: "Test Scene",
    entities: [
        Entity(
            name: "Box1",
            transform: (translation: (0,0,0), rotation: (0,0,0,1), scale: (1,1,1)),
            model: "Resources/Box.glb",
        ),
    ],
)
```

---

## 四、技术栈

| 层级 | 技术 | 版本 | 用途 |
|---|---|---|---|
| 窗口 + 事件 | winit | 0.30 | 窗口创建、输入事件 |
| GPU | wgpu | 23 | Vulkan/Metal/DX12 抽象 |
| 原生框架 | eframe (egui) | 0.30 | 窗口循环 + 即时模式 GUI |
| 布局 | egui_tiles | 0.11 | 可拖拽面板 dock 布局 |
| 过程宏 | syn / quote / proc-macro2 | 2/1/1 | `#[derive(Panel)]` |
| 数学 | glam | 0.29 | Vec3/Mat4/Quat |
| 物理 | rapier3d | 0.22 | 刚体动力学、碰撞检测 |
| 3D 模型 | gltf (Rust) | 1 | glTF/glb 解析 |
| 纹理 | image | 0.24 | PNG/JPEG 解码 |
| 序列化 | ron + serde | 0.8 / 1 | 场景文件持久化 |

```
xgame-core     核心类型：Color,Config,Transform,Scene,Entity,Component,GameClock,AssetId
    ↑
xgame-math     glam 重导出 + projection 辅助
    ↑
xgame-render   wgpu 管线、glTF 加载、网格、纹理、相机
xgame-physics  rapier3d 刚体、碰撞体、物理世界、调试绘制
    ↑
xgame-editor   egui 面板、SceneRenderer、GameRuntime
```

---

## 五、关键文件速查

| 文件 | 行数 | 核心函数 |
|---|---|---|
| `main.rs` | 150 | `EditorApp::new()`、`update()` |
| `state.rs` | 417 | `enter_play_mode()`、`create_entity()` |
| `scene_renderer.rs` | 511 | `SceneRenderer::render()` |
| `scene_view.rs` | 200 | `show_content()`、`project_to_screen()` |
| `game_runtime.rs` | 100 | `start()`、`fixed_update()` |
| `world.rs` | 573 | `step()`、`add_body()`、`debug_lines()` |
| `hierarchy.rs` | 140 | `show_content()` |
| `inspector.rs` | 137 | `show_content()` |
| `toolbar.rs` | 80 | `show()` |
| `console.rs` | 60 | `show_content()` |
| `dock.rs` | 80 | `create_tile_tree()`、`EditorBehavior` |
| `theme.rs` | 120 | `install()`、`unity_dark_visuals()` |
