# XGameEngine 开发计划

> Rust 实现的类 Unity 游戏引擎

---

## 整体路线图

| 阶段 | 状态 | 内容 |
|---|---|---|
| **一** | ✅ 完成 | Workspace + 窗口 + 三角形 |
| **二** | ✅ 完成 | 基础渲染（glTF、纹理、Blinn-Phong 光照） |
| **三** | ✅ 完成 | Model 矩阵 + Transform 层级 |
| **四** | ✅ 完成 | 场景系统（RON 序列化、多实体） |
| **五** | ✅ 完成 | 物理集成（rapier3d） |
| **六** | ✅ 完成 | 编辑器（egui 面板） |
| **七** | ⬜ 待开始 | 动画系统 |
| **八** | ⬜ 待开始 | 音频系统 |
| **九** | ⬜ 待开始 | 后处理 + 阴影 |
| **十** | ⬜ 待开始 | 脚本系统 + 热重载 |

---

## 第一阶段：骨架 ✅

- [x] Cargo workspace 搭建
- [x] xgame-core（Color, WindowConfig）
- [x] xgame-math（glam re-export, Transform）
- [x] xgame-render（RenderContext, 深度缓冲, triangle example）

## 第二阶段：基础渲染 ✅

- [x] glTF 模型加载（.gltf / .glb）
- [x] 纹理上传 + GPU 采样
- [x] 方向光 + Blinn-Phong 着色
- [x] 轨道相机
- [x] 深度测试 + 背面剔除

## 第三阶段：Model 矩阵 ✅

- [x] ModelUniform + ModelTransform
- [x] 顶点着色器 world_pos 变换
- [x] 法线矩阵（mat3x3）
- [x] WASD 键盘操控 + 旋转/重置

## 第四阶段：场景系统 ✅

- [x] Entity / Scene 类型（RON 序列化）
- [x] 场景文件归类到 Scenes/

## 第五阶段：物理集成 ✅

- [x] rapier3d 依赖引入
- [x] RigidBody 组件（static / dynamic / kinematic）
- [x] Collider 组件（cuboid, ball, capsule, cylinder, trimesh）
- [x] 物理步骤与渲染同步
- [x] 碰撞事件回调
- [x] 物理可视化（debug draw）

## 第六阶段：编辑器 ✅

- [x] egui 集成到 wgpu/winit
- [x] 面板布局（Hierarchy / Inspector / Scene View）
- [x] 场景视图（3D 渲染到纹理 → egui 显示）
- [x] Hierarchy 树形面板（选择/可见性）
- [x] Inspector 属性编辑（名称/Transform/可见性）
- [ ] 资源浏览器
- [ ] Play / Pause / Stop 模式

## 第七阶段：动画系统 ⬜

- [ ] 骨骼蒙皮（GPU skinning）
- [ ] 动画片段加载（glTF animation）
- [ ] 关键帧插值
- [ ] 动画混合（过渡、叠加）
- [ ] 动画状态机（Animator）

## 第八阶段：音频系统 ⬜

- [ ] kira / rodio 集成
- [ ] AudioSource 组件
- [ ] 3D 空间音频
- [ ] 混音器

## 第九阶段：后处理 + 阴影 ⬜

- [ ] RenderGraph 构建
- [ ] 级联阴影贴图（CSM）
- [ ] Bloom 后处理
- [ ] ToneMapping（ACES）
- [ ] FXAA / TAA
- [ ] 天空盒

## 第十阶段：脚本 + 热重载 ⬜

- [ ] 脚本调度系统
- [ ] Rust 脚本热重载（动态库加载）
- [ ] 或嵌入 Lua/Rhai 解释器
- [ ] 生命周期回调（Start / Update / OnDestroy）

---

## 当前项目结构

```
XGameEngine/
├── Cargo.toml                     # workspace
├── Resources/                     # 资源
│   ├── Box.glb
│   ├── Fox.gltf
│   └── Texture.png
├── Scenes/                        # 场景文件
│   └── scene.ron
├── crates/
│   ├── xgame-core/                # 核心类型
│   │   └── src/
│   │       ├── color.rs
│   │       └── config.rs
│   ├── xgame-math/                # 数学库
│   │   └── src/
│   │       ├── projection.rs
│   │       └── transform.rs
│   └── xgame-render/              # 渲染器
│       └── src/
│           ├── lib.rs             # RenderContext
│           ├── camera.rs          # Camera + Light
│           ├── mesh.rs            # Mesh
│           ├── gltf_loader.rs     # glTF 加载器
│           ├── texture.rs         # GPU 纹理
│           ├── model.rs           # Model 矩阵
│           └── scene.rs           # 场景类型
│   └── xgame-editor/              # 编辑器（egui）
│       └── src/
│           └── main.rs             # 编辑器入口
│   └── xgame-physics/             # 物理引擎
│       └── src/
│           ├── lib.rs             # crate root
│           ├── rigid_body.rs      # RigidBody
│           ├── collider.rs        # Collider
│           ├── world.rs           # PhysicsWorld
│           └── debug_draw.rs      # 调试绘制
└── ROADMAP.md
```

---

## 运行命令

```powershell
# 编辑器
cargo run -p xgame-editor
```
