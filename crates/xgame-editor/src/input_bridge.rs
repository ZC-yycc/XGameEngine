//! 把 egui 的键鼠状态灌进运行时的 [`Input`](xgame_input::Input)。
//!
//! # 为什么需要这一层
//!
//! 编辑器自己**不**处理游戏输入：视口的鼠标、飞行的 WASD 都是 egui 直接读的。
//! 但进入 Play 之后，脚本里写的 `action_pressed(ctx, "Jump")` 必须真的能按下 ——
//! 否则"在编辑器里试玩"这件事就只剩物理在跑，凡是跟输入沾边的逻辑全部失灵。
//!
//! 于是每帧（仅 Play 时）把 egui 的键盘状态翻译成本引擎的 `KeyCode`，
//! 灌进运行时的 `InputState`。
//!
//! # 两个必须留意的点
//!
//! 1. **修饰键不在 egui 的 `Key` 里**。egui 把 Shift/Ctrl/Alt 放在
//!    `Modifiers` 结构里，`Key` 枚举里根本没有它们；所以修饰键要单独读
//!    `modifiers`，其余键先转发事件，再用 `key_down` 对齐最终电平。
//! 2. **按住会被反复写入 `true`**。这不是问题：`InputState::handle_key` 用
//!    `HashSet::insert` 的返回值区分"首次按下"和"系统按键重复"，
//!    所以 `action_pressed` 不会被按住刷屏。

use egui::Key as EguiKey;
use xgame_input::{KeyCode, MouseButton};
use xgame_math::glam::Vec2;

/// 本引擎键码 → egui 键码。
///
/// 返回 `None` 表示 egui 没有对应的键（小键盘、锁定键、ContextMenu 等）——
/// 那些键在编辑器里试玩时不可用，但在打包出去的游戏里完全正常
/// （游戏走的是 winit 的路径，与 egui 无关）。
fn to_egui(code: KeyCode) -> Option<EguiKey> {
    use EguiKey as E;
    Some(match code {
        KeyCode::KeyA => E::A,
        KeyCode::KeyB => E::B,
        KeyCode::KeyC => E::C,
        KeyCode::KeyD => E::D,
        KeyCode::KeyE => E::E,
        KeyCode::KeyF => E::F,
        KeyCode::KeyG => E::G,
        KeyCode::KeyH => E::H,
        KeyCode::KeyI => E::I,
        KeyCode::KeyJ => E::J,
        KeyCode::KeyK => E::K,
        KeyCode::KeyL => E::L,
        KeyCode::KeyM => E::M,
        KeyCode::KeyN => E::N,
        KeyCode::KeyO => E::O,
        KeyCode::KeyP => E::P,
        KeyCode::KeyQ => E::Q,
        KeyCode::KeyR => E::R,
        KeyCode::KeyS => E::S,
        KeyCode::KeyT => E::T,
        KeyCode::KeyU => E::U,
        KeyCode::KeyV => E::V,
        KeyCode::KeyW => E::W,
        KeyCode::KeyX => E::X,
        KeyCode::KeyY => E::Y,
        KeyCode::KeyZ => E::Z,

        KeyCode::Digit0 => E::Num0,
        KeyCode::Digit1 => E::Num1,
        KeyCode::Digit2 => E::Num2,
        KeyCode::Digit3 => E::Num3,
        KeyCode::Digit4 => E::Num4,
        KeyCode::Digit5 => E::Num5,
        KeyCode::Digit6 => E::Num6,
        KeyCode::Digit7 => E::Num7,
        KeyCode::Digit8 => E::Num8,
        KeyCode::Digit9 => E::Num9,

        KeyCode::F1 => E::F1,
        KeyCode::F2 => E::F2,
        KeyCode::F3 => E::F3,
        KeyCode::F4 => E::F4,
        KeyCode::F5 => E::F5,
        KeyCode::F6 => E::F6,
        KeyCode::F7 => E::F7,
        KeyCode::F8 => E::F8,
        KeyCode::F9 => E::F9,
        KeyCode::F10 => E::F10,
        KeyCode::F11 => E::F11,
        KeyCode::F12 => E::F12,
        KeyCode::F13 => E::F13,
        KeyCode::F14 => E::F14,
        KeyCode::F15 => E::F15,
        KeyCode::F16 => E::F16,
        KeyCode::F17 => E::F17,
        KeyCode::F18 => E::F18,
        KeyCode::F19 => E::F19,
        KeyCode::F20 => E::F20,

        KeyCode::Escape => E::Escape,
        KeyCode::Enter => E::Enter,
        KeyCode::Tab => E::Tab,
        KeyCode::Space => E::Space,
        KeyCode::Backspace => E::Backspace,
        KeyCode::Delete => E::Delete,
        KeyCode::Insert => E::Insert,

        KeyCode::ArrowUp => E::ArrowUp,
        KeyCode::ArrowDown => E::ArrowDown,
        KeyCode::ArrowLeft => E::ArrowLeft,
        KeyCode::ArrowRight => E::ArrowRight,
        KeyCode::Home => E::Home,
        KeyCode::End => E::End,
        KeyCode::PageUp => E::PageUp,
        KeyCode::PageDown => E::PageDown,

        KeyCode::Backquote => E::Backtick,
        KeyCode::Minus => E::Minus,
        KeyCode::Equal => E::Equals,
        KeyCode::BracketLeft => E::OpenBracket,
        KeyCode::BracketRight => E::CloseBracket,
        KeyCode::Backslash => E::Backslash,
        KeyCode::Semicolon => E::Semicolon,
        KeyCode::Comma => E::Comma,
        KeyCode::Period => E::Period,
        KeyCode::Slash => E::Slash,

        // 修饰键 / 小键盘 / 锁定键 / ContextMenu：egui 的 `Key` 里没有。
        _ => return None,
    })
}

/// 把 egui 的当前键鼠状态同步进运行时输入。
///
/// 每帧调用一次，**只在 Play 模式**下调用。调用时机要在 `physics_step` 之前 ——
/// 否则脚本读到的是上一帧的键。
pub fn sync(ctx: &egui::Context, input: &mut xgame_input::Input) {
    ctx.input(|i| {
        let state = input.state_mut();
        // 只同步最终电平会吞掉两帧之间完成的短按。按原序转发边沿与焦点，
        // 后台事件由 InputState 门控；随后再对齐电平，补启动时已按住的按键。
        for event in &i.events {
            match event {
                egui::Event::WindowFocused(focused) => state.set_focused(*focused),
                egui::Event::Key { key, pressed, .. } => {
                    if let Some(&code) = KeyCode::ALL
                        .iter()
                        .find(|&&code| to_egui(code) == Some(*key))
                    {
                        state.handle_key(code, *pressed);
                    }
                }
                egui::Event::PointerButton {
                    button, pressed, ..
                } => {
                    let button = match button {
                        egui::PointerButton::Primary => MouseButton::Left,
                        egui::PointerButton::Secondary => MouseButton::Right,
                        egui::PointerButton::Middle => MouseButton::Middle,
                        egui::PointerButton::Extra1 => MouseButton::Back,
                        egui::PointerButton::Extra2 => MouseButton::Forward,
                    };
                    state.handle_mouse_button(button, *pressed);
                }
                _ => {}
            }
        }
        state.set_focused(i.focused);
        if !i.focused {
            return;
        }

        // -- 修饰键：从 `modifiers` 读，egui 的 `Key` 里没有它们 --
        state.handle_key(KeyCode::ShiftLeft, i.modifiers.shift);
        state.handle_key(KeyCode::ControlLeft, i.modifiers.ctrl);
        state.handle_key(KeyCode::AltLeft, i.modifiers.alt);
        // egui 的 `command` 在 Windows 上是 Ctrl、在 macOS 上是 Cmd；
        // 统一映射到 Super 会在 Windows 上把 Ctrl 也点亮，所以只在非 ctrl 时才认。
        state.handle_key(KeyCode::SuperLeft, i.modifiers.mac_cmd);

        // -- 普通键 --
        for &code in KeyCode::ALL {
            if code.is_modifier() {
                continue;
            }
            let Some(key) = to_egui(code) else {
                continue;
            };
            state.handle_key(code, i.key_down(key));
        }

        // -- 鼠标 --
        state.handle_mouse_button(MouseButton::Left, i.pointer.primary_down());
        state.handle_mouse_button(MouseButton::Right, i.pointer.secondary_down());
        state.handle_mouse_button(MouseButton::Middle, i.pointer.middle_down());
        state.handle_mouse_button(
            MouseButton::Back,
            i.pointer.button_down(egui::PointerButton::Extra1),
        );
        state.handle_mouse_button(
            MouseButton::Forward,
            i.pointer.button_down(egui::PointerButton::Extra2),
        );

        // egui 用的是自己的 `Vec2`（emath），跟引擎的 glam::Vec2 是两个类型，
        // 这里逐分量搬一下 —— 顺手也让"坐标单位是逻辑点"这件事写在脸上。
        if let Some(pos) = i.pointer.latest_pos() {
            state.handle_cursor_moved(Vec2::new(pos.x, pos.y));
        }
        let delta = i.pointer.delta();
        if delta != egui::Vec2::ZERO {
            state.handle_mouse_motion(Vec2::new(delta.x, delta.y));
        }
        let scroll = i.raw_scroll_delta;
        if scroll != egui::Vec2::ZERO {
            state.handle_scroll(Vec2::new(scroll.x, scroll.y));
        }
    });
}

/// 把 egui 的逻辑坐标换算成**视口物理像素**（UI 指针用）。
///
/// 三个坐标系在这一处对齐：
///
/// | 坐标 | 空间 | 谁在用 |
/// |---|---|---|
/// | egui 逻辑点 | 窗口，已乘 DPI 缩放 | 面板布局、鼠标事件 |
/// | 视口逻辑矩形 | 窗口，视口的 `rect` | `response.hover_pos()` |
/// | 视口物理像素 | 离屏纹理像素 | UI 布局、命中测试 |
///
/// 指针不在视口内（或视口还没布局）返回 `None` —— 编辑器里鼠标移到
/// Hierarchy 上时，游戏 UI 不该保持"悬停"。
pub fn viewport_pointer(
    viewport: egui::Rect,
    pixels_per_point: f32,
    cursor: Option<egui::Pos2>,
) -> Option<xgame_math::glam::Vec2> {
    let cursor = cursor?;
    if viewport.width() <= 0.0 || viewport.height() <= 0.0 || !viewport.contains(cursor) {
        return None;
    }
    Some(Vec2::new(
        (cursor.x - viewport.min.x) * pixels_per_point,
        (cursor.y - viewport.min.y) * pixels_per_point,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_frame_key_and_mouse_taps_reach_play_input() {
        let ctx = egui::Context::default();
        ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(1).unwrap());
        let mut input = xgame_input::Input::with_default_map();
        let mut events = Vec::new();
        for pressed in [true, false] {
            events.push(egui::Event::Key {
                key: EguiKey::Space,
                physical_key: Some(EguiKey::Space),
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            });
            events.push(egui::Event::PointerButton {
                pos: egui::pos2(20.0, 30.0),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            });
        }
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| sync(ctx, &mut input),
        );
        for action in ["Jump", "Fire"] {
            assert!(input.action_pressed(action), "Play 漏掉 {action} 的短按");
            assert!(input.action_released(action));
            assert!(!input.action_down(action));
        }
        input.end_frame();
        let _ = ctx.run(egui::RawInput::default(), |ctx| sync(ctx, &mut input));
        assert!(!input.action_pressed("Jump"));
        assert!(!input.action_pressed("Fire"));
    }

    #[test]
    fn focus_is_forwarded_from_egui_before_reading_devices() {
        let ctx = egui::Context::default();
        ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(1).unwrap());
        let mut input = xgame_input::Input::with_default_map();
        input
            .state_mut()
            .handle_pad_button(0, xgame_input::GamepadButton::South, true);
        input
            .state_mut()
            .handle_pad_axis(0, xgame_input::GamepadAxis::LeftX, 0.8);
        let _ = ctx.run(
            egui::RawInput {
                focused: false,
                events: vec![egui::Event::WindowFocused(false)],
                ..Default::default()
            },
            |ctx| sync(ctx, &mut input),
        );
        assert!(!input.state().is_focused(), "Play 必须接到窗口失焦");
        assert!(!input.action_down("Jump"));
        assert_eq!(input.axis("MoveX"), 0.0);
        input.end_frame();

        let _ = ctx.run(
            egui::RawInput {
                focused: true,
                events: vec![egui::Event::WindowFocused(true)],
                ..Default::default()
            },
            |ctx| sync(ctx, &mut input),
        );
        assert!(input.state().is_focused());
        assert!(input.axis("MoveX") > 0.5);
        assert!(!input.action_pressed("Jump"));
        input
            .state_mut()
            .handle_mouse_button(MouseButton::Left, true);
        input.end_frame();
        let _ = ctx.run(
            egui::RawInput {
                focused: true,
                events: vec![
                    egui::Event::WindowFocused(false),
                    egui::Event::WindowFocused(true),
                ],
                ..Default::default()
            },
            |ctx| sync(ctx, &mut input),
        );
        assert!(input.state().is_focused());
        assert!(input.state().focus_lost(), "帧末聚焦不能覆盖中途失焦");
        assert!(input.state().mouse_released(MouseButton::Left));
    }

    #[test]
    fn viewport_pointer_maps_into_texture_pixels() {
        // 视口在窗口里位于 (200, 100)，尺寸 400x300 逻辑点，2 倍 DPI。
        let rect = egui::Rect::from_min_size(egui::pos2(200.0, 100.0), egui::vec2(400.0, 300.0));
        let p = viewport_pointer(rect, 2.0, Some(egui::pos2(250.0, 160.0))).expect("在视口内");
        assert_eq!(p, Vec2::new(100.0, 120.0), "偏移要减掉视口原点，再乘 DPI");
    }

    #[test]
    fn viewport_pointer_rejects_outside_and_degenerate() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 100.0));
        assert!(viewport_pointer(rect, 1.0, Some(egui::pos2(5.0, 5.0))).is_none());
        assert!(viewport_pointer(rect, 1.0, None).is_none());
        // 面板还没布局（egui 的 NOTHING 矩形）
        assert!(viewport_pointer(egui::Rect::NOTHING, 1.0, Some(egui::pos2(1.0, 1.0))).is_none());
    }

    #[test]
    fn letter_and_digit_keys_map() {
        assert_eq!(to_egui(KeyCode::KeyW), Some(EguiKey::W));
        assert_eq!(to_egui(KeyCode::Digit3), Some(EguiKey::Num3));
        assert_eq!(to_egui(KeyCode::Space), Some(EguiKey::Space));
    }

    #[test]
    fn modifiers_and_numpad_are_absent_from_egui() {
        assert_eq!(to_egui(KeyCode::ShiftLeft), None);
        assert_eq!(to_egui(KeyCode::NumpadEnter), None);
    }
}
