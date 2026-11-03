//! 焦点是所有输入设备共同的边界：取消已有操作，后台事件不能重新激活玩法。

use xgame_input::{
    GamepadAxis, GamepadButton, Input, InputState, KeyCode, MouseButton, TouchPhase,
};
use xgame_math::glam::Vec2;

#[test]
fn focus_loss_cancels_edges_and_transient_data() {
    let mut state = InputState::new();
    state.handle_key(KeyCode::KeyW, true);
    state.handle_mouse_button(MouseButton::Left, true);
    state.handle_pad_button(0, GamepadButton::South, true);
    state.handle_cursor_moved(Vec2::new(50.0, 30.0));
    state.handle_scroll(Vec2::Y);
    state.handle_text("输入");
    state.set_focused(false);

    assert!(state.key_released(KeyCode::KeyW), "失焦要给已按下的键收尾");
    assert!(state.mouse_released(MouseButton::Left));
    assert!(state.pad_button_released(GamepadButton::South));
    assert!(!state.key_pressed(KeyCode::KeyW));
    assert!(!state.mouse_pressed(MouseButton::Left));
    assert!(!state.any_key_pressed());
    assert!(!state.has_cursor(), "旧位置不能让后台 UI 保持悬停");
    assert_eq!(state.mouse_delta(), Vec2::ZERO);
    assert_eq!(state.scroll(), Vec2::ZERO);
    assert!(state.text().is_empty());

    // 重复失焦和同帧恢复都不能吞掉取消边沿；边沿只在帧末过期。
    state.set_focused(false);
    state.set_focused(true);
    assert!(state.mouse_released(MouseButton::Left));
    state.end_frame();
    assert!(!state.key_released(KeyCode::KeyW));
    assert!(!state.mouse_released(MouseButton::Left));
    assert!(!state.pad_button_released(GamepadButton::South));
}

#[test]
fn focus_blocks_background_events_but_retains_device_facts() {
    let mut input = Input::with_default_map();
    input.state_mut().set_focused(false);
    input.end_frame();
    let state = input.state_mut();
    state.pad_connected(7, "Background pad");
    state.handle_key(KeyCode::KeyW, true);
    state.handle_mouse_button(MouseButton::Left, true);
    state.handle_cursor_moved(Vec2::new(40.0, 20.0));
    state.handle_mouse_motion(Vec2::ONE);
    state.handle_scroll(Vec2::Y);
    state.handle_text("后台");
    state.handle_touch(1, TouchPhase::Started, Vec2::ONE, 1.0);
    state.handle_pad_button(7, GamepadButton::South, true);
    state.handle_pad_axis(7, GamepadAxis::LeftX, 0.8);
    state.handle_pad_axis(7, GamepadAxis::RightTrigger, 0.9);

    assert!(!input.action_down("Jump"), "后台轮询不能重新按下跳跃");
    assert!(!input.action_down("Fire"));
    assert!(!input.action_pressed("Fire"));
    assert_eq!(input.axis("MoveX"), 0.0);
    assert_eq!(input.axis("MoveY"), 0.0);
    let state = input.state();
    assert_eq!(state.pad_axis_of(7, GamepadAxis::LeftX), 0.0);
    assert!(!state.any_key_pressed());
    assert!(!state.has_cursor());
    assert_eq!(state.touch_count(), 0);
    assert_eq!(state.mouse_delta(), Vec2::ZERO);
    assert_eq!(state.scroll(), Vec2::ZERO);
    assert!(state.text().is_empty());
    assert_eq!(state.pad_name(7), Some("Background pad"));

    // 轴保留最新物理位置，恢复焦点不是一次新的扳机按下。
    input.state_mut().set_focused(true);
    assert_eq!(input.state().pad_axis_of(7, GamepadAxis::LeftX), 0.8);
    assert!(input.action_down("Fire"));
    assert!(!input.action_pressed("Fire"));
    assert!(!input.action_down("Jump"));
    input.state_mut().handle_key(KeyCode::Space, true);
    assert!(input.action_pressed("Jump"), "恢复后新的按下仍要有效");
}

#[test]
fn focus_loss_releases_a_held_trigger_once() {
    let mut input = Input::with_default_map();
    input
        .state_mut()
        .handle_pad_axis(0, GamepadAxis::RightTrigger, 0.8);
    assert!(input.action_down("Fire"));
    input.end_frame();
    input.state_mut().set_focused(false);
    assert!(!input.action_down("Fire"));
    assert!(input.action_released("Fire"));
    input.end_frame();
    assert!(!input.action_released("Fire"));
    input.state_mut().set_focused(true);
    assert!(input.action_down("Fire"));
    assert!(!input.action_pressed("Fire"));
    input.end_frame();
    input.state_mut().set_focused(false);
    input.state_mut().set_focused(true);
    assert!(
        input.action_released("Fire"),
        "同帧恢复不能吞掉扳机取消边沿"
    );
    assert!(!input.action_pressed("Fire"));
}

#[test]
fn focus_regain_rebases_the_first_cursor_position() {
    let mut state = InputState::new();
    state.handle_cursor_moved(Vec2::new(10.0, 20.0));
    state.end_frame();
    state.set_focused(false);
    state.set_focused(true);
    state.handle_cursor_moved(Vec2::new(900.0, 700.0));
    assert_eq!(
        state.mouse_delta(),
        Vec2::ZERO,
        "窗口外位移不能变成恢复后的转头"
    );
    state.handle_cursor_moved(Vec2::new(903.0, 702.0));
    assert_eq!(state.mouse_delta(), Vec2::new(3.0, 2.0));
}

#[test]
fn focus_reset_keeps_focus_and_connections_without_stale_edges() {
    let mut input = Input::with_default_map();
    input.state_mut().pad_connected(0, "Pad");
    input
        .state_mut()
        .handle_pad_button(0, GamepadButton::South, true);
    input.state_mut().set_focused(false);
    input.reset();
    assert!(!input.state().is_focused(), "切场景不能替操作系统恢复焦点");
    assert_eq!(input.state().pad_name(0), Some("Pad"));
    assert!(!input.action_pressed("Jump"));
    assert!(!input.action_released("Jump"));
    input.state_mut().handle_key(KeyCode::Space, true);
    assert!(!input.action_down("Jump"));
}

#[test]
fn focus_default_and_new_both_accept_headless_input() {
    for mut state in [InputState::new(), InputState::default()] {
        assert!(state.is_focused(), "无窗口宿主默认可注入输入");
        state.handle_key(KeyCode::Space, true);
        assert!(state.key_pressed(KeyCode::Space));
    }
}
