//! UI 布局 / 裁剪 / 命中 / 交互回归。

use xgc_sim::{hit_test, Schedule, UiFocusSystem, UiInteractionSystem, UiLayoutSystem};
use xgc_world::{Parent, UiElement, UiInput, UiInteraction, UiLayout, World};

#[test]
fn ui_layout_resolves_anchor_and_offsets() {
    let mut world = World::new();
    let entity = world.spawn((
        UiElement {
            anchor_min: [0.5, 0.0],
            anchor_max: [0.5, 0.0],
            offset_min: [10.0, 20.0],
            offset_max: [110.0, 60.0],
            visible: true,
            ..UiElement::default()
        },
        UiLayout::default(),
    ));
    let mut schedule = Schedule::new();
    schedule.add(UiLayoutSystem {
        viewport: [200.0, 100.0],
    });
    schedule.run(&mut world, 0.016);
    let layout = world.get::<UiLayout>(entity).unwrap();
    assert_eq!(layout.rect, [110.0, 20.0, 100.0, 40.0]);
    assert!(layout.visible);
}

#[test]
fn ui_layout_is_parent_relative_and_inherits_visibility() {
    let mut world = World::new();
    let parent = world.spawn((
        UiElement {
            offset_min: [100.0, 50.0],
            offset_max: [300.0, 150.0],
            visible: false,
            ..UiElement::default()
        },
        UiLayout::default(),
    ));
    let parent_id = world.entity(parent).unwrap();
    let middle = world.spawn((Parent { entity: parent_id },));
    let child = world.spawn((
        Parent {
            entity: world.entity(middle).unwrap(),
        },
        UiElement {
            offset_min: [10.0, 20.0],
            offset_max: [60.0, 50.0],
            visible: true,
            ..UiElement::default()
        },
        UiLayout::default(),
    ));
    let mut schedule = Schedule::new();
    schedule.add(UiLayoutSystem {
        viewport: [1000.0, 1000.0],
    });
    schedule.run(&mut world, 0.016);
    let layout = world.get::<UiLayout>(child).unwrap();
    assert_eq!(layout.rect, [110.0, 70.0, 50.0, 30.0]);
    assert!(!layout.visible, "隐藏祖先应让子节点不可见");
    assert_eq!(hit_test(&world, [120.0, 80.0]), None);
}

#[test]
fn ui_clip_and_hit_test() {
    let mut world = World::new();
    let parent = world.spawn((
        UiElement {
            offset_min: [0.0, 0.0],
            offset_max: [100.0, 100.0],
            clip_children: true,
            ..UiElement::default()
        },
        UiLayout::default(),
    ));
    let parent_id = world.entity(parent).unwrap();
    let child = world.spawn((
        Parent { entity: parent_id },
        UiElement {
            offset_min: [50.0, 50.0],
            offset_max: [250.0, 250.0],
            ..UiElement::default()
        },
        UiLayout::default(),
    ));
    let mut schedule = Schedule::new();
    schedule.add(UiLayoutSystem {
        viewport: [1000.0, 1000.0],
    });
    schedule.run(&mut world, 0.016);
    let layout = world.get::<UiLayout>(child).unwrap();
    assert_eq!(layout.rect, [50.0, 50.0, 200.0, 200.0]);
    assert_eq!(layout.clip, [0.0, 0.0, 100.0, 100.0]);
    assert_eq!(hit_test(&world, [60.0, 60.0]), Some(child));
    assert_eq!(hit_test(&world, [500.0, 500.0]), None);
}

#[test]
fn ui_interaction_marks_hover_and_click() {
    let mut world = World::new();
    let entity = world.spawn((
        UiElement {
            offset_min: [0.0, 0.0],
            offset_max: [100.0, 50.0],
            ..UiElement::default()
        },
        UiLayout::default(),
        UiInteraction::default(),
    ));
    let mut schedule = Schedule::new();
    schedule.add(UiLayoutSystem {
        viewport: [100.0, 50.0],
    });
    world.spawn((UiInput {
        cursor: [10.0, 10.0],
        click: true,
        ..UiInput::default()
    },));
    schedule.add(UiInteractionSystem);
    schedule.run(&mut world, 0.016);
    let state = world.get::<UiInteraction>(entity).unwrap();
    assert!(state.hovered);
    assert!(state.pressed);
    assert!(state.clicked);
}

#[test]
fn ui_focus_cycles_through_interactable_elements() {
    let mut world = World::new();
    let first = world.spawn((
        UiElement {
            offset_min: [0.0, 0.0],
            offset_max: [100.0, 50.0],
            ..UiElement::default()
        },
        UiLayout::default(),
        UiInteraction::default(),
    ));
    let second = world.spawn((
        UiElement {
            offset_min: [0.0, 60.0],
            offset_max: [100.0, 110.0],
            ..UiElement::default()
        },
        UiLayout::default(),
        UiInteraction::default(),
    ));
    let mut schedule = Schedule::new();
    schedule.add(UiLayoutSystem {
        viewport: [100.0, 200.0],
    });
    let input = world.spawn((UiInput::default(),));
    schedule.add(UiFocusSystem);
    world.get_mut::<UiInput>(input).unwrap().nav = "tab";
    schedule.run(&mut world, 0.016);
    assert!(world.get::<UiInteraction>(first).unwrap().focused);
    world.get_mut::<UiInput>(input).unwrap().nav = "tab";
    schedule.run(&mut world, 0.016);
    assert!(world.get::<UiInteraction>(second).unwrap().focused);
    assert!(!world.get::<UiInteraction>(first).unwrap().focused);
}

#[test]
fn ui_focus_direction_picks_geometric_neighbor() {
    let mut world = World::new();
    let center = world.spawn((
        UiElement {
            offset_min: [100.0, 100.0],
            offset_max: [120.0, 120.0],
            ..UiElement::default()
        },
        UiLayout::default(),
        UiInteraction::default(),
    ));
    let right = world.spawn((
        UiElement {
            offset_min: [200.0, 100.0],
            offset_max: [220.0, 120.0],
            ..UiElement::default()
        },
        UiLayout::default(),
        UiInteraction::default(),
    ));
    let down = world.spawn((
        UiElement {
            offset_min: [100.0, 200.0],
            offset_max: [120.0, 220.0],
            ..UiElement::default()
        },
        UiLayout::default(),
        UiInteraction::default(),
    ));
    let mut layout = Schedule::new();
    layout.add(UiLayoutSystem {
        viewport: [400.0, 400.0],
    });
    layout.run(&mut world, 0.016);
    let input = world.spawn((UiInput::default(),));
    let mut focus = Schedule::new();
    focus.add(UiFocusSystem);
    world.get_mut::<UiInput>(input).unwrap().nav = "tab";
    focus.run(&mut world, 0.016);
    assert!(world.get::<UiInteraction>(center).unwrap().focused);
    let mut right_focus = Schedule::new();
    right_focus.add(UiFocusSystem);
    world.get_mut::<UiInput>(input).unwrap().nav = "right";
    right_focus.run(&mut world, 0.016);
    assert!(world.get::<UiInteraction>(right).unwrap().focused);
    assert!(!world.get::<UiInteraction>(down).unwrap().focused);
}

#[test]
fn simulation_host_input_drives_ui() {
    use xgc_sim::Simulation;
    use xgc_world::{HostUiInput, UiPointerEdge};

    let mut world = World::new();
    let element = world.spawn((
        UiElement {
            offset_min: [0.0, 0.0],
            offset_max: [100.0, 50.0],
            ..UiElement::default()
        },
        UiLayout::default(),
        UiInteraction::default(),
    ));
    let upper = world.spawn((
        UiElement {
            offset_max: [100.0, 50.0],
            ..UiElement::default()
        },
        UiLayout::default(),
        UiInteraction::default(),
    ));
    let mut simulation = Simulation::new(world);
    let point = Some([10.0, 10.0]);
    simulation.set_ui_input(HostUiInput {
        position: point,
        pointer_edges: vec![UiPointerEdge::Press(point), UiPointerEdge::Release(point)],
        ..HostUiInput::default()
    });
    simulation.step(Vec::new()).unwrap();
    let lower = simulation.world().get::<UiInteraction>(element).unwrap();
    assert!(!lower.hovered && !lower.clicked, "下层只在上层不接收时命中");
    let upper = simulation.world().get::<UiInteraction>(upper).unwrap();
    assert!(upper.hovered && upper.clicked);
    assert_eq!(simulation.world().host_ui_input().unwrap().position, point);
}
