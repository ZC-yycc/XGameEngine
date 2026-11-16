//! UI 布局 / 裁剪 / 命中 / 交互回归。

use xgc_sim::{hit_test, Schedule, UiFocusSystem, UiInteractionSystem, UiLayoutSystem};
use xgc_world::{Parent, UiElement, UiInteraction, UiLayout, World};

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
    let child = world.spawn((
        Parent { entity: parent_id },
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
    assert!(!layout.visible, "隐藏父节点应让子节点不可见");
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
    schedule.add(UiInteractionSystem {
        cursor: Some([10.0, 10.0]),
        click: true,
    });
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
    schedule.add(UiFocusSystem {
        next: true,
        confirm: false,
    });
    schedule.run(&mut world, 0.016);
    assert!(world.get::<UiInteraction>(first).unwrap().focused);
    schedule.run(&mut world, 0.016);
    assert!(world.get::<UiInteraction>(second).unwrap().focused);
    assert!(!world.get::<UiInteraction>(first).unwrap().focused);
}
