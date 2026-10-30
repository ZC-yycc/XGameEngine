//! XGameEngine 输入层。
//!
//! 把"设备上发生了什么"翻译成"游戏想干什么"，分三级：
//!
//! ```text
//! 平台事件 (winit)                     ← 只在 editor / game 里出现
//!     │  key_from_winit / From<MouseButton>
//!     ▼
//! InputState   谁被按下、指针在哪       ← 与窗口库无关，可测试、可重放
//!     │  InputMap（input.ron）
//!     ▼
//! Input        动作 / 轴                ← 游戏逻辑只认这一层
//! ```
//!
//! 拆成三级而不是一级，是为了让"换窗口库"「做输入回放」「把输入接到 AI 上」
//! 都只需要动一小块代码：游戏逻辑写的是 `input.action_down("Jump")`，
//! 它根本不知道背后是键盘、手柄还是录像。
//!
//! # 快速上手
//!
//! ```ignore
//! use xgame_input::{Input, InputMap};
//!
//! let mut input = Input::with_default_map();
//!
//! // 平台层每帧喂事件：
//! // input.state_mut().handle_key(KeyCode::KeyW, true);
//!
//! if input.action_pressed("Jump") {
//!     // 跳跃
//! }
//! let move_x = input.axis("MoveX");   // -1 .. 1
//!
//! input.end_frame();                  // 帧末清掉边沿信号
//! ```

mod action;
mod key;
mod pad;
mod state;
mod touch;

pub use action::{AnalogBinding, AxisBinding, Binding, Input, InputMap, InputMapError};
pub use key::{KeyCode, MouseButton};
pub use pad::{apply_deadzone, GamepadAxis, GamepadButton, PadState, AXIS_COUNT};
pub use state::InputState;
pub use touch::{TouchPhase, TouchPoint};

#[cfg(feature = "winit")]
pub use key::key_from_winit;

#[cfg(feature = "gilrs")]
mod gamepad_hub;
#[cfg(feature = "gilrs")]
pub use gamepad_hub::GamepadHub;
