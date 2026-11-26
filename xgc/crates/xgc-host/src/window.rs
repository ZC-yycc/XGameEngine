//! 窗口输入和逻辑视口。两个宿主在同一个固定步边界消费设备边沿。

use egui::{Event, Key, PointerButton, Pos2, Rect, Vec2};
use xgc_sim::{InputState, KeyCode, MouseButton, DEFAULT_UI_VIEWPORT};
use xgc_world::{HostUiInput, UiPointerEdge};

const KEY_CODES: &[(Key, KeyCode)] = &[
    (Key::ArrowUp, KeyCode::ArrowUp),
    (Key::ArrowDown, KeyCode::ArrowDown),
    (Key::ArrowLeft, KeyCode::ArrowLeft),
    (Key::ArrowRight, KeyCode::ArrowRight),
    (Key::Escape, KeyCode::Escape),
    (Key::Tab, KeyCode::Tab),
    (Key::Backspace, KeyCode::Backspace),
    (Key::Enter, KeyCode::Enter),
    (Key::Space, KeyCode::Space),
    (Key::Delete, KeyCode::Delete),
    (Key::Num0, KeyCode::Digit0),
    (Key::Num1, KeyCode::Digit1),
    (Key::Num2, KeyCode::Digit2),
    (Key::Num3, KeyCode::Digit3),
    (Key::Num4, KeyCode::Digit4),
    (Key::Num5, KeyCode::Digit5),
    (Key::Num6, KeyCode::Digit6),
    (Key::Num7, KeyCode::Digit7),
    (Key::Num8, KeyCode::Digit8),
    (Key::Num9, KeyCode::Digit9),
    (Key::A, KeyCode::KeyA),
    (Key::B, KeyCode::KeyB),
    (Key::C, KeyCode::KeyC),
    (Key::D, KeyCode::KeyD),
    (Key::E, KeyCode::KeyE),
    (Key::F, KeyCode::KeyF),
    (Key::G, KeyCode::KeyG),
    (Key::H, KeyCode::KeyH),
    (Key::I, KeyCode::KeyI),
    (Key::J, KeyCode::KeyJ),
    (Key::K, KeyCode::KeyK),
    (Key::L, KeyCode::KeyL),
    (Key::M, KeyCode::KeyM),
    (Key::N, KeyCode::KeyN),
    (Key::O, KeyCode::KeyO),
    (Key::P, KeyCode::KeyP),
    (Key::Q, KeyCode::KeyQ),
    (Key::R, KeyCode::KeyR),
    (Key::S, KeyCode::KeyS),
    (Key::T, KeyCode::KeyT),
    (Key::U, KeyCode::KeyU),
    (Key::V, KeyCode::KeyV),
    (Key::W, KeyCode::KeyW),
    (Key::X, KeyCode::KeyX),
    (Key::Y, KeyCode::KeyY),
    (Key::Z, KeyCode::KeyZ),
    (Key::F1, KeyCode::F1),
    (Key::F2, KeyCode::F2),
    (Key::F3, KeyCode::F3),
    (Key::F4, KeyCode::F4),
    (Key::F5, KeyCode::F5),
    (Key::F6, KeyCode::F6),
    (Key::F7, KeyCode::F7),
    (Key::F8, KeyCode::F8),
    (Key::F9, KeyCode::F9),
    (Key::F10, KeyCode::F10),
    (Key::F11, KeyCode::F11),
    (Key::F12, KeyCode::F12),
];

pub fn fit_viewport(available: Rect, size: [u32; 2]) -> Option<Rect> {
    let size = Vec2::new(size[0] as f32, size[1] as f32);
    let scale = (available.width() / size.x).min(available.height() / size.y);
    (scale > 0.0 && scale.is_finite() && size.x > 0.0 && size.y > 0.0)
        .then(|| Rect::from_center_size(available.center(), size * scale))
}

fn logical_point(viewport: Rect, point: Pos2) -> Option<[f32; 2]> {
    let offset = point - viewport.min;
    viewport.contains(point).then_some([
        offset.x * DEFAULT_UI_VIEWPORT[0] as f32 / viewport.width(),
        offset.y * DEFAULT_UI_VIEWPORT[1] as f32 / viewport.height(),
    ])
}

#[derive(Default)]
pub struct WindowInput {
    gameplay: InputState,
    pointer: Option<[f32; 2]>,
    primary_down: bool,
    cancel_pointer: bool,
    pointer_edges: Vec<UiPointerEdge>,
    navigation: Option<&'static str>,
    confirm: bool,
}

impl WindowInput {
    pub fn sample(&mut self, ctx: &egui::Context, viewport: Option<Rect>) {
        ctx.input(|input| {
            if !input.focused
                || input
                    .events
                    .iter()
                    .any(|event| matches!(event, Event::WindowFocused(false)))
            {
                self.gameplay.keys.clear();
                self.gameplay.mouse.clear();
                self.pointer = None;
                self.discard_ui_edges();
                self.primary_down = false;
                return;
            }
            self.pointer = input
                .pointer
                .hover_pos()
                .and_then(|pos| logical_point(viewport?, pos));
            self.primary_down = input.pointer.button_down(PointerButton::Primary);
            if viewport.is_none() {
                self.discard_ui_edges();
            }
            for &(key, code) in KEY_CODES {
                self.set_key(code, input.key_down(key));
            }
            // egui 只报告组合键状态，输入映射用左侧键代表这一组修饰键。
            self.set_key(KeyCode::ShiftLeft, input.modifiers.shift);
            self.set_key(KeyCode::ControlLeft, input.modifiers.ctrl);
            self.set_key(KeyCode::AltLeft, input.modifiers.alt);
            for (button, code) in [
                (PointerButton::Primary, MouseButton::Left),
                (PointerButton::Secondary, MouseButton::Right),
                (PointerButton::Middle, MouseButton::Middle),
            ] {
                if self.pointer.is_some() && input.pointer.button_down(button) {
                    self.gameplay.mouse.insert(code);
                } else {
                    self.gameplay.mouse.remove(&code);
                }
            }
            let Some(viewport) = viewport else {
                return;
            };
            for event in &input.events {
                match event {
                    Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed,
                        ..
                    } => {
                        let point = logical_point(viewport, *pos);
                        self.pointer_edges.push(if *pressed {
                            UiPointerEdge::Press(point)
                        } else {
                            UiPointerEdge::Release(point)
                        });
                    }
                    Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } if !modifiers.command && !modifiers.alt => match key {
                        Key::Tab => self.navigation = Some("tab"),
                        Key::ArrowUp => self.navigation = Some("up"),
                        Key::ArrowDown => self.navigation = Some("down"),
                        Key::ArrowLeft => self.navigation = Some("left"),
                        Key::ArrowRight => self.navigation = Some("right"),
                        Key::Enter | Key::Space => self.confirm = true,
                        _ => {}
                    },
                    _ => {}
                }
            }
        });
    }

    fn set_key(&mut self, code: KeyCode, pressed: bool) {
        if pressed {
            self.gameplay.keys.insert(code);
        } else {
            self.gameplay.keys.remove(&code);
        }
    }

    pub fn gameplay(&self) -> &InputState {
        &self.gameplay
    }

    /// 重绘比固定步快时保留边沿，一次补多步时也只取走一次。
    pub fn take_ui_input(&mut self) -> HostUiInput {
        if std::mem::take(&mut self.cancel_pointer) {
            self.pointer_edges.insert(0, UiPointerEdge::Release(None));
        }
        HostUiInput {
            position: self.pointer,
            primary_down: self.primary_down,
            pointer_edges: std::mem::take(&mut self.pointer_edges),
            navigation: self.navigation.take(),
            confirm: std::mem::take(&mut self.confirm),
        }
    }

    pub fn discard_ui_edges(&mut self) {
        self.pointer_edges.clear();
        // Pause、失焦和视口消失时取消已有捕获，不能在原位置合成点击。
        self.cancel_pointer = true;
        self.navigation = None;
        self.confirm = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Modifiers, RawInput};

    #[test]
    fn layout_coordinates_and_keyboard_edges_use_the_displayed_viewport() {
        let area = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(640.0, 420.0));
        let viewport = fit_viewport(area, DEFAULT_UI_VIEWPORT).unwrap();
        assert_eq!(viewport.min, Pos2::new(10.0, 50.0));
        assert_eq!(viewport.size(), Vec2::new(640.0, 360.0));

        let ctx = Context::default();
        let mut state = WindowInput::default();
        let sample = |state: &mut WindowInput, events| {
            let _ = ctx.run(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| state.sample(ctx, Some(viewport)),
            );
        };
        let key = |key, pressed, repeat| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers: Modifiers::NONE,
        };
        sample(
            &mut state,
            vec![
                Event::PointerMoved(viewport.center()),
                key(Key::ArrowRight, true, false),
                key(Key::Enter, true, false),
            ],
        );
        let input = state.take_ui_input();
        assert_eq!(input.position, Some([640.0, 360.0]));
        assert_eq!(input.navigation, Some("right"));
        assert!(input.confirm);
        assert!(state.gameplay().keys.contains(&KeyCode::ArrowRight));

        sample(
            &mut state,
            vec![
                Event::PointerMoved(area.min),
                key(Key::ArrowRight, true, true),
                key(Key::Enter, true, true),
            ],
        );
        assert_eq!(state.take_ui_input(), HostUiInput::default());
        assert!(state.gameplay().keys.contains(&KeyCode::ArrowRight));
        sample(
            &mut state,
            vec![
                key(Key::ArrowRight, false, false),
                key(Key::Enter, false, false),
            ],
        );
        assert!(state.gameplay().keys.is_empty());
    }
}
