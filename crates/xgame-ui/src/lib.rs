//! XGameEngine 屏幕空间 UI 运行时（纯 CPU）。
//!
//! 这个 crate 只做四件事，**不碰任何 GPU 类型**：
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`layout`] | 把 `Entity::ui` 的锚点解算成像素矩形（`UiRect`），给出绘制顺序 |
//! | [`input`] | 指针 → 悬停 / 按下 / 点击（写回 `UiElement::state`） |
//! | [`text`] | 字体加载、字形光栅化、R8 图集、换行与对齐 |
//! | [`draw`] | 把布局 + 样式 + 文本合成一份「矩形 + 精灵 + 批」的绘制数据 |
//!
//! 为什么与 GPU 分开：布局与命中测试是**玩法逻辑**（脚本要读点击、无头运行
//! 要能推 UI 状态），没有显卡也必须正确；渲染只是把同一份绘制数据画出来。
//! `xgame-render` 里的 `UiRenderer` 是这里唯一的消费者。
//!
//! # 数据流（每帧）
//!
//! ```text
//! Scene(Entity::ui) ──layout::visit──► [LayoutItem] ──input::update──► UiState 写回 Scene
//!                                     │
//!                                     └─draw::Painter::build──► UiDrawData ──► GPU
//! ```
//!
//! 布局与命中测试都不依赖字体度量：**矩形由锚点唯一决定**。文本只在
//! 生成绘制数据时才需要排版 —— 这让"脚本里判断按钮点没点中"完全不受字体
//! 加载失败影响（找不到中文字体时按钮照样能点，只是看不见字）。

pub mod draw;
pub mod input;
pub mod layout;
pub mod text;

pub use draw::{Painter, SpriteMode, UiCommand, UiDrawData, UiQuad, UiSprite, UiTexture};
pub use input::{update_interaction, UiEvent, UiEventKind, UiInteraction, UiPointer};
pub use layout::{visit, LayoutItem, UiRect, UiTree};
pub use text::{GlyphAtlas, TextEngine};
