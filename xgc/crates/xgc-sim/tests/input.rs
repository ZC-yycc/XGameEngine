//! 输入映射：解析项目 input.ron 并折叠动作 / 轴。

use std::path::Path;

use xgc_sim::{InputMap, InputState, KeyCode, MouseButton};

#[test]
fn parses_project_input_ron_and_folds_state() {
    let path = Path::new("../../../projects/XGame/input.ron");
    if !path.exists() {
        eprintln!("skip: 没有旧项目 input.ron");
        return;
    }
    let map = InputMap::parse_ron(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(map.actions.contains_key("Jump"));
    assert!(map.axes.contains_key("MoveX"));

    let mut state = InputState::default();
    state.keys.insert(KeyCode::Space);
    assert!(map.action_pressed("Jump", &state));
    assert!(!map.action_pressed("Fire", &state));
    state.mouse.insert(MouseButton::Left);
    assert!(map.action_pressed("Fire", &state));

    state.keys.insert(KeyCode::KeyD);
    assert_eq!(map.axis_value("MoveX", &state), 1.0);
    state.keys.insert(KeyCode::KeyA);
    assert_eq!(map.axis_value("MoveX", &state), 0.0);
}
