//! 输入映射：解析项目 input.ron 并折叠动作 / 轴。

use std::path::Path;

use xgc_sim::{ActionInput, InputMap, InputState, KeyCode, MouseButton};

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
    assert!(map.action_down("Jump", &state));
    assert!(!map.action_down("Fire", &state));
    state.mouse.insert(MouseButton::Left);
    assert!(map.action_down("Fire", &state));

    state.keys.insert(KeyCode::KeyD);
    assert_eq!(map.axis_value("MoveX", &state), 1.0);
    state.keys.insert(KeyCode::KeyA);
    assert_eq!(map.axis_value("MoveX", &state), 0.0);
}

#[test]
fn action_edges_fold_each_binding_before_combining() {
    let map = InputMap::parse_ron(
        r#"InputMap(actions: {
            "Go": [Key(Space), Mouse(Left)],
            "Combo": [Combo((key: KeyG, control: true))],
        })"#,
    )
    .unwrap();
    let mut before = InputState::default();
    before.keys.insert(KeyCode::Space);
    let mut state = before.clone();
    let mut edges = Vec::new();
    state.mouse.insert(MouseButton::Left);
    state.keys.extend([KeyCode::KeyG, KeyCode::ControlRight]);
    state.append_edges_since(&before, &mut edges);
    let down_and_pressed = ActionInput {
        down: true,
        pressed: true,
        released: false,
    };
    assert_eq!(map.action_input("Go", &state, &edges), down_and_pressed);
    assert_eq!(map.action_input("Combo", &state, &edges), down_and_pressed);

    before.clone_from(&state);
    state.keys.remove(&KeyCode::ControlRight);
    state.mouse.clear();
    edges.clear();
    state.append_edges_since(&before, &mut edges);
    assert_eq!(
        map.action_input("Go", &state, &edges),
        ActionInput {
            down: true,
            pressed: false,
            released: true
        },
    );
    assert_eq!(
        map.action_input("Combo", &state, &edges),
        ActionInput::default()
    );

    before.clone_from(&state);
    state.keys.remove(&KeyCode::KeyG);
    edges.clear();
    state.append_edges_since(&before, &mut edges);
    assert_eq!(
        map.action_input("Combo", &state, &edges),
        ActionInput {
            released: true,
            ..ActionInput::default()
        },
    );
}
