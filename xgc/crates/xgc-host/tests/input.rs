use std::path::PathBuf;

use xgc_base::EntityKey;
use xgc_host::ScriptedRunner;
use xgc_sim::{InputMap, InputState, KeyCode};
use xgc_world::{Script, Transform, Velocity, World};

struct InputScript {
    root: PathBuf,
    runner: ScriptedRunner,
    entity: EntityKey,
}

impl InputScript {
    fn new(name: &str, source: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("xgc_host_input_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("input.lua"), source).unwrap();
        let mut world = World::new();
        let entity = world.spawn((
            Transform::default(),
            Velocity::default(),
            Script {
                path: "input.lua".to_string(),
            },
        ));
        let (runner, _) =
            ScriptedRunner::with_assets(world, xgc_host::asset_paths(Some(&root), &[]));
        Self {
            root,
            runner,
            entity,
        }
    }

    fn velocity(&self) -> [f32; 3] {
        self.runner
            .world()
            .get::<Velocity>(self.entity)
            .unwrap()
            .linear
    }
}

impl Drop for InputScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn action_press_is_visible_for_one_step_while_axis_stays_held() {
    let mut fixture = InputScript::new(
        "headless",
        r#"
        function update(ctx)
            assert(not action_down(ctx, 'Unknown'))
            assert(not action_pressed(ctx, 'Unknown'))
            assert(not action_released(ctx, 'Unknown'))
            local x = axis(ctx, 'MoveX')
            local pressed = action_pressed(ctx, 'Jump')
            local down = action_down(ctx, 'Jump')
            local released = action_released(ctx, 'Jump')
            set_velocity(ctx, self_id(ctx), x, pressed and 9 or 0, down and 1 or released and 2 or 0)
        end
        "#,
    );
    let map = InputMap::parse_ron(
        r#"InputMap(actions: {"Jump": [Key(Space)]}, axes: {"MoveX": (positive: [Key(KeyD)], negative: [Key(KeyA)])})"#,
    )
    .unwrap();
    let mut state = InputState::default();
    state.keys.extend([KeyCode::KeyD, KeyCode::Space]);
    fixture.runner.apply_input(&map, &state);
    fixture.runner.step().unwrap();
    assert_eq!(fixture.velocity(), [1.0, 9.0, 1.0]);

    fixture.runner.apply_input(&map, &state);
    fixture.runner.step().unwrap();
    assert_eq!(fixture.velocity(), [1.0, 0.0, 1.0]);
    fixture.runner.step().unwrap();
    assert_eq!(fixture.velocity(), [1.0, 0.0, 1.0]);

    state.keys.clear();
    fixture.runner.apply_input(&map, &state);
    fixture.runner.step().unwrap();
    assert_eq!(fixture.velocity(), [0.0, 0.0, 2.0]);
    fixture.runner.step().unwrap();
    assert_eq!(fixture.velocity(), [0.0; 3]);
}

#[cfg(feature = "window-input")]
#[test]
fn short_window_press_survives_multiple_repaints_and_runs_once() {
    use egui::{Context, Event, Key, Modifiers, RawInput};
    use xgc_host::window::WindowInput;

    let mut fixture = InputScript::new(
        "window",
        r#"
        local presses, releases = 0, 0
        function update(ctx)
            if action_pressed(ctx, 'Jump') then presses = presses + 1 end
            if action_released(ctx, 'Jump') then releases = releases + 1 end
            set_velocity(ctx, self_id(ctx), presses, releases, action_down(ctx, 'Jump') and 1 or 0)
        end
        "#,
    );
    let map = InputMap::parse_ron(r#"InputMap(actions: {"Jump": [Key(Space)]})"#).unwrap();
    let context = Context::default();
    let mut input = WindowInput::default();
    let sample = |input: &mut WindowInput, events| {
        let _ = context.run(
            RawInput {
                events,
                ..RawInput::default()
            },
            |ctx| input.sample(ctx, None),
        );
    };
    let key = |pressed, repeat| Event::Key {
        key: Key::Space,
        physical_key: Some(Key::Space),
        pressed,
        repeat,
        modifiers: Modifiers::NONE,
    };
    for pressed in [true, false] {
        sample(&mut input, vec![key(pressed, false)]);
    }
    for _ in 0..3 {
        fixture.runner.step_window(Some(&map), &mut input).unwrap();
        assert_eq!(fixture.velocity(), [1.0, 1.0, 0.0]);
    }

    sample(&mut input, vec![key(true, false)]);
    fixture.runner.step_window(Some(&map), &mut input).unwrap();
    assert_eq!(fixture.velocity(), [2.0, 1.0, 1.0]);
    sample(&mut input, vec![key(true, true)]);
    fixture.runner.step_window(Some(&map), &mut input).unwrap();
    assert_eq!(fixture.velocity(), [2.0, 1.0, 1.0]);

    sample(&mut input, vec![key(false, false), key(true, false)]);
    sample(&mut input, vec![Event::WindowFocused(false)]);
    fixture.runner.step_window(Some(&map), &mut input).unwrap();
    assert_eq!(fixture.velocity(), [2.0, 1.0, 0.0]);
    sample(
        &mut input,
        vec![Event::WindowFocused(true), key(false, false)],
    );
    fixture.runner.step_window(Some(&map), &mut input).unwrap();
    assert_eq!(fixture.velocity(), [2.0, 1.0, 0.0]);
}
