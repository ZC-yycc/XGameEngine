# XGameEngine

> Rust 实现的类 Unity 游戏引擎，基于 wgpu + egui + rapier3d。

## 快速开始

```powershell
# 编辑器（egui 面板 + 3D 场景视图）
cargo run -p xgame-editor

# 独立游戏（无编辑器，直接运行场景）
cargo run -p xgame-game
```

## 项目结构

```
XGameEngine/
├── crates/
│   ├── xgame-core/       # 核心类型：Scene, Entity, Component, Transform, GameClock
│   ├── xgame-math/       # 数学库：glam 重导出 + projection 辅助
│   ├── xgame-render/     # 渲染管线：wgpu 上下文、glTF 加载、场景渲染 (ScenePipeline)
│   ├── xgame-physics/    # 物理引擎：rapier3d 刚体、碰撞体、调试绘制
│   ├── xgame-editor/     # 编辑器：egui 面板、SceneRenderer、GameRuntime
│   ├── xgame-derive/     # 过程宏：#[derive(Panel)]
│   └── xgame-game/       # 独立游戏二进制
├── Resources/            # 资源文件（glTF 模型、纹理）
└── Scenes/               # 场景文件（RON 序列化）
```

## 依赖方向

```
core ← math ← {render, physics} ← editor / game
```
