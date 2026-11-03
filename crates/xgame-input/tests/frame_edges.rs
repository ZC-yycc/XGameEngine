//! 一帧里的完整短按不能被后到的相反边沿抹掉；电平只描述最终状态。

use xgame_input::{GamepadButton, Input, KeyCode, MouseButton};

fn check_edges(mut inject: impl FnMut(&mut Input, bool), action: &str) {
    let mut input = Input::with_default_map();
    inject(&mut input, true);
    inject(&mut input, false);
    assert!(input.action_pressed(action), "短按的 Press 必须保留");
    assert!(input.action_released(action), "短按的 Release 必须保留");
    assert!(!input.action_down(action));
    input.end_frame();
    assert!(!input.action_pressed(action));
    assert!(!input.action_released(action));

    inject(&mut input, true);
    input.end_frame();
    inject(&mut input, false);
    inject(&mut input, true);
    assert!(input.action_released(action), "再按下不能吞掉此前的 Release");
    assert!(input.action_pressed(action));
    assert!(input.action_down(action));
    input.end_frame();
    inject(&mut input, true);
    assert!(!input.action_pressed(action), "系统重复事件不产生新的边沿");
    assert!(!input.action_released(action));
}

#[test]
fn keyboard_short_taps_keep_both_edges() {
    check_edges(
        |input, down| input.state_mut().handle_key(KeyCode::Space, down),
        "Jump",
    );
}

#[test]
fn mouse_short_taps_keep_both_edges() {
    check_edges(
        |input, down| {
            input.state_mut().handle_mouse_button(MouseButton::Left, down);
        },
        "Fire",
    );
}

#[test]
fn gamepad_short_taps_keep_both_edges() {
    check_edges(
        |input, down| {
            input
                .state_mut()
                .handle_pad_button(0, GamepadButton::South, down);
        },
        "Jump",
    );
}
