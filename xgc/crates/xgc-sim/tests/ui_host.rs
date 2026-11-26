use xgc_sim::Simulation;
use xgc_world::{UiElement, UiInput, UiInteraction, UiLayout, World};

#[test]
fn host_ui_input_keeps_the_entity_table_and_author_input_intact() {
    let mut world = World::new();
    let element = world.spawn((
        UiElement {
            offset_max: [100.0, 50.0],
            ..UiElement::default()
        },
        UiLayout::default(),
        UiInteraction::default(),
    ));
    let element_id = world.entity(element).unwrap();
    let author = UiInput {
        cursor: [120.0, 60.0],
        ..UiInput::default()
    };
    for author in [None, Some(author)] {
        let author_key = author.map(|input| world.spawn((input,)));
        let before = world.world_data();
        let snapshot = world.snapshot();
        let element = world.key(element_id).unwrap();
        let hash = world.state_hash();
        let mut sim = Simulation::new(world);
        let host = UiInput {
            cursor: [10.0, 10.0],
            click: true,
            nav: "tab",
            ..UiInput::default()
        };
        sim.set_ui_input(host);
        assert_eq!(sim.world().world_data(), before);
        assert_eq!(sim.world().state_hash(), hash);
        assert_eq!(sim.ui_input(), Some(host));
        sim.step(Vec::new()).unwrap();
        let interaction = sim.world().get::<UiInteraction>(element).unwrap();
        assert!(interaction.hovered && interaction.clicked && interaction.focused);
        assert_eq!(sim.ui_input().unwrap().nav, "none");
        assert_eq!(sim.world().len(), before.entities.len());
        assert_eq!(sim.world().revision(), before.revision);
        if let Some(key) = author_key {
            assert_eq!(sim.world().get::<UiInput>(key).copied(), author);
        }
        world = sim.into_world();
        world.restore_snapshot(&snapshot);
        assert_eq!(world.ui_input(), author, "恢复快照必须丢弃设备状态");
        world.set_host_ui_input(host);
        world.restore_world_data(&before).unwrap();
        let sim = Simulation::new(world);
        assert_eq!(sim.ui_input(), author, "恢复世界必须丢弃设备状态");
        world = sim.into_world();
    }
}
