//! 播放器输入桥：把 egui 的键盘 / 鼠标状态折叠成 `input.ron` 的动作与轴。
//!
//! 窗口只收集设备状态，动作与轴由共用脚本帧宿主结算。

use std::path::Path;

use eframe::egui;
use xgc_host::ScriptedRunner;
use xgc_sim::{InputMap, InputState, KeyCode, MouseButton};

/// `egui::Key` 与项目键位表的对应关系；表里没有的键（标点、Insert 等）不映射。
const KEY_CODES: &[(egui::Key, KeyCode)] = &[
    (egui::Key::ArrowUp, KeyCode::ArrowUp),
    (egui::Key::ArrowDown, KeyCode::ArrowDown),
    (egui::Key::ArrowLeft, KeyCode::ArrowLeft),
    (egui::Key::ArrowRight, KeyCode::ArrowRight),
    (egui::Key::Escape, KeyCode::Escape),
    (egui::Key::Tab, KeyCode::Tab),
    (egui::Key::Backspace, KeyCode::Backspace),
    (egui::Key::Enter, KeyCode::Enter),
    (egui::Key::Space, KeyCode::Space),
    (egui::Key::Delete, KeyCode::Delete),
    (egui::Key::Num0, KeyCode::Digit0),
    (egui::Key::Num1, KeyCode::Digit1),
    (egui::Key::Num2, KeyCode::Digit2),
    (egui::Key::Num3, KeyCode::Digit3),
    (egui::Key::Num4, KeyCode::Digit4),
    (egui::Key::Num5, KeyCode::Digit5),
    (egui::Key::Num6, KeyCode::Digit6),
    (egui::Key::Num7, KeyCode::Digit7),
    (egui::Key::Num8, KeyCode::Digit8),
    (egui::Key::Num9, KeyCode::Digit9),
    (egui::Key::A, KeyCode::KeyA),
    (egui::Key::B, KeyCode::KeyB),
    (egui::Key::C, KeyCode::KeyC),
    (egui::Key::D, KeyCode::KeyD),
    (egui::Key::E, KeyCode::KeyE),
    (egui::Key::F, KeyCode::KeyF),
    (egui::Key::G, KeyCode::KeyG),
    (egui::Key::H, KeyCode::KeyH),
    (egui::Key::I, KeyCode::KeyI),
    (egui::Key::J, KeyCode::KeyJ),
    (egui::Key::K, KeyCode::KeyK),
    (egui::Key::L, KeyCode::KeyL),
    (egui::Key::M, KeyCode::KeyM),
    (egui::Key::N, KeyCode::KeyN),
    (egui::Key::O, KeyCode::KeyO),
    (egui::Key::P, KeyCode::KeyP),
    (egui::Key::Q, KeyCode::KeyQ),
    (egui::Key::R, KeyCode::KeyR),
    (egui::Key::S, KeyCode::KeyS),
    (egui::Key::T, KeyCode::KeyT),
    (egui::Key::U, KeyCode::KeyU),
    (egui::Key::V, KeyCode::KeyV),
    (egui::Key::W, KeyCode::KeyW),
    (egui::Key::X, KeyCode::KeyX),
    (egui::Key::Y, KeyCode::KeyY),
    (egui::Key::Z, KeyCode::KeyZ),
    (egui::Key::F1, KeyCode::F1),
    (egui::Key::F2, KeyCode::F2),
    (egui::Key::F3, KeyCode::F3),
    (egui::Key::F4, KeyCode::F4),
    (egui::Key::F5, KeyCode::F5),
    (egui::Key::F6, KeyCode::F6),
    (egui::Key::F7, KeyCode::F7),
    (egui::Key::F8, KeyCode::F8),
    (egui::Key::F9, KeyCode::F9),
    (egui::Key::F10, KeyCode::F10),
    (egui::Key::F11, KeyCode::F11),
    (egui::Key::F12, KeyCode::F12),
];

#[derive(Default)]
pub(crate) struct PlayerInput {
    map: Option<InputMap>,
}

impl PlayerInput {
    pub(crate) fn load(path: Option<&Path>) -> Result<Self, String> {
        let map = path.map(InputMap::load).transpose()?;
        Ok(Self { map })
    }

    pub(crate) fn apply(&self, ctx: &egui::Context, runner: &mut ScriptedRunner) {
        let Some(map) = &self.map else {
            return;
        };
        let mut state = InputState::default();
        ctx.input(|input| {
            for (key, code) in KEY_CODES {
                if input.key_down(*key) {
                    state.keys.insert(*code);
                }
            }
            if input.pointer.button_down(egui::PointerButton::Primary) {
                state.mouse.insert(MouseButton::Left);
            }
            if input.pointer.button_down(egui::PointerButton::Secondary) {
                state.mouse.insert(MouseButton::Right);
            }
            if input.pointer.button_down(egui::PointerButton::Middle) {
                state.mouse.insert(MouseButton::Middle);
            }
        });
        runner.apply_input(map, &state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgc_world::{Script, Transform, Velocity, World};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xgc_player_input_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn maps_wasd_space_and_mouse() {
        assert!(KEY_CODES.contains(&(egui::Key::W, KeyCode::KeyW)));
        assert!(KEY_CODES.contains(&(egui::Key::Space, KeyCode::Space)));
        assert!(!KEY_CODES.iter().any(|(key, _)| *key == egui::Key::Comma));
    }

    #[test]
    fn distinguishes_a_missing_mapping_from_no_mapping() {
        let dir = temp_dir("load");
        let path = dir.join("input.ron");
        std::fs::write(&path, "InputMap(actions: {\"Jump\": [Key(Space)]})").unwrap();
        assert!(PlayerInput::load(Some(&path)).unwrap().map.is_some());
        std::fs::remove_file(&path).unwrap();
        assert!(PlayerInput::load(Some(&path)).is_err());
        assert!(PlayerInput::load(None).unwrap().map.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn applies_actions_and_axes_to_the_script_view() {
        let dir = temp_dir("apply");
        std::fs::write(
            dir.join("move.lua"),
            "function update(ctx)\n  local x = axis(ctx, 'MoveX')\n  local jump = action_pressed(ctx, 'Jump')\n  set_velocity(ctx, self_id(ctx), x, jump and 9.0 or 0.0, 0.0)\nend\n",
        )
        .unwrap();
        let map = InputMap::parse_ron(
            "InputMap(actions: {\"Jump\": [Key(Space)]}, axes: {\"MoveX\": (positive: [Key(KeyD)], negative: [Key(KeyA)])})",
        )
        .unwrap();
        let mut world = World::new();
        let entity = world.spawn((
            Transform::default(),
            Velocity::default(),
            Script {
                path: "move.lua".to_string(),
            },
        ));
        let (mut runner, _) = ScriptedRunner::with_assets(world, Some(&dir));
        let mut state = InputState::default();
        state.keys.insert(KeyCode::KeyD);
        state.keys.insert(KeyCode::Space);
        runner.apply_input(&map, &state);
        runner.step().unwrap();
        assert_eq!(
            runner.world().get::<Velocity>(entity).unwrap().linear,
            [1.0, 9.0, 0.0]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
