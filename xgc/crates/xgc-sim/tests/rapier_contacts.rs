//! 普通刚体的接触边沿必须通过默认固定步送达宿主。

use xgc_base::{
    layer::{CollisionLayers, LayerIndex},
    EntityId,
};
use xgc_sim::{Simulation, FIXED_DT};
use xgc_world::{
    CollisionEvent, CollisionLayer, Command, ComponentData, EntityData, Name, PhysicsBody, Slope,
    Transform, Velocity, World,
};

const SENSOR_LAYER: u8 = 31;
const BALL_LAYER: u8 = 2;
const OVERLAP: [f32; 3] = [0.65, 2.65, 0.0];
const CORNER_GAP: [f32; 3] = [0.70, 2.70, 0.0];

fn transform_at(position: [f32; 3]) -> Transform {
    Transform {
        position,
        ..Transform::default()
    }
}

fn ball_body() -> PhysicsBody {
    PhysicsBody {
        shape: "ball".into(),
        radius: 0.25,
        gravity_scale: 0.0,
        ..PhysicsBody::default()
    }
}

fn layer(index: u8) -> CollisionLayer {
    CollisionLayer {
        index: LayerIndex::new(index).unwrap(),
    }
}

fn pair_world(position: [f32; 3]) -> (World, EntityId, EntityId) {
    let mut world = World::new();
    let sensor = world.spawn((
        transform_at([0.0, 2.0, 0.0]),
        PhysicsBody {
            kind: "static".into(),
            shape: "cuboid".into(),
            half_extents: [0.5; 3],
            is_sensor: true,
            ..PhysicsBody::default()
        },
        layer(SENSOR_LAYER),
    ));
    // 球靠 Rapier 保持位姿，重力和 ECS 速度都不参与这组几何边沿。
    let ball = world.spawn((transform_at(position), ball_body(), layer(BALL_LAYER)));
    let sensor_id = world.entity(sensor).unwrap();
    let ball_id = world.entity(ball).unwrap();
    (world, sensor_id, ball_id)
}

fn advance(sim: &mut Simulation, commands: Vec<Command>) -> Vec<CollisionEvent> {
    let mut observed = None;
    sim.advance_with(commands, |world| {
        assert!(observed
            .replace(world.collision_events().to_vec())
            .is_none());
    })
    .unwrap();
    let observed = observed.expect("默认固定步应执行宿主回调");
    assert_eq!(observed.as_slice(), sim.world().collision_events());
    observed
}

fn directed(low: EntityId, high: EntityId, enter: bool) -> [CollisionEvent; 2] {
    assert!(low < high);
    [
        CollisionEvent {
            entity: low,
            other: high,
            enter,
        },
        CollisionEvent {
            entity: high,
            other: low,
            enter,
        },
    ]
}

#[test]
fn an_aabb_corner_overlap_does_not_enter_a_box_sensor() {
    // 轴向各侵入外包盒 0.05 m，球心到盒角却有 sqrt(0.2² + 0.2²) > 0.25 m。
    let (world, _, _) = pair_world(CORNER_GAP);
    let events = advance(&mut Simulation::new(world), Vec::new());
    assert!(events.is_empty(), "球形与盒角分离，宿主收到 {events:?}");
}

#[test]
fn an_overlapping_sensor_enters_once_and_keeps_the_ball_in_place() {
    // 同一盒角沿两轴各靠近 0.05 m，球形真正进入。冻结姿态可区分重发与再次接触。
    let (world, sensor, ball) = pair_world(OVERLAP);
    let key = world.key(ball).unwrap();
    let mut sim = Simulation::new(world);
    assert_eq!(advance(&mut sim, Vec::new()), directed(sensor, ball, true));
    for _ in 0..3 {
        assert!(advance(&mut sim, Vec::new()).is_empty());
    }
    assert_eq!(sim.world().get::<Transform>(key).unwrap().position, OVERLAP);
}

#[test]
fn either_layer_can_block_an_existing_overlap_without_moving_it() {
    let (world, sensor, ball) = pair_world(OVERLAP);
    let key = world.key(ball).unwrap();
    let mut sim = Simulation::new(world);
    assert_eq!(advance(&mut sim, Vec::new()), directed(sensor, ball, true));

    for (blocker, peer) in [(BALL_LAYER, SENSOR_LAYER), (SENSOR_LAYER, BALL_LAYER)] {
        // 只改一侧，并保留其它位；两边各测一次才能识别把过滤写成单边判据的回归。
        let layers = CollisionLayers {
            masks: [(blocker, u32::MAX ^ layer(peer).index.member())].into(),
            ..CollisionLayers::default()
        };
        sim.world_mut().set_collision_layers(&layers);
        assert_eq!(advance(&mut sim, Vec::new()), directed(sensor, ball, false));
        assert!(advance(&mut sim, Vec::new()).is_empty());

        sim.world_mut()
            .set_collision_layers(&CollisionLayers::default());
        assert_eq!(advance(&mut sim, Vec::new()), directed(sensor, ball, true));
        assert!(advance(&mut sim, Vec::new()).is_empty());
        assert_eq!(sim.world().get::<Transform>(key).unwrap().position, OVERLAP);
    }
}

fn replace_ball(ball: EntityId, position: [f32; 3]) -> Command {
    let mut data = EntityData::new(ball);
    data.components = vec![
        ComponentData::from_component(&transform_at(position)),
        ComponentData::from_component(&ball_body()),
        ComponentData::from_component(&layer(BALL_LAYER)),
    ];
    Command::Replace { data }
}

#[test]
fn replacing_a_body_with_the_same_id_tracks_the_logical_pair() {
    let (world, sensor, ball) = pair_world(OVERLAP);
    let original_key = world.key(ball).unwrap();
    let mut sim = Simulation::new(world);
    assert_eq!(advance(&mut sim, Vec::new()), directed(sensor, ball, true));

    // 编辑命令会重建刚体。宿主按持久 ID 识别事件，换代且持续重叠不能伪造离开再进入。
    let events = advance(&mut sim, vec![replace_ball(ball, OVERLAP)]);
    let replacement_key = sim.world().key(ball).unwrap();
    assert_eq!(original_key.index(), replacement_key.index());
    assert_ne!(original_key.generation(), replacement_key.generation());
    assert!(events.is_empty(), "同一逻辑对仍重叠，宿主收到 {events:?}");

    let separated = advance(&mut sim, vec![replace_ball(ball, [2.0, 2.65, 0.0])]);
    assert_eq!(separated, directed(sensor, ball, false));
    assert!(advance(&mut sim, Vec::new()).is_empty());
    let reentered = advance(&mut sim, vec![replace_ball(ball, OVERLAP)]);
    assert_eq!(reentered, directed(sensor, ball, true));
}

#[test]
fn deleting_a_sensor_exits_its_pair_even_when_a_new_id_reuses_the_slot() {
    let (world, old_sensor, ball) = pair_world(OVERLAP);
    let old_key = world.key(old_sensor).unwrap();
    let new_sensor = EntityId::from_raw(100);
    let data = world.entity_data(old_sensor).unwrap().with_id(new_sensor);
    let mut sim = Simulation::new(world);
    assert_eq!(
        advance(&mut sim, Vec::new()),
        directed(old_sensor, ball, true)
    );

    let events = advance(
        &mut sim,
        vec![
            Command::Despawn { entity: old_sensor },
            Command::Spawn { data },
        ],
    );
    let new_key = sim.world().key(new_sensor).unwrap();
    assert!(sim.world().key(old_sensor).is_none());
    assert_eq!(old_key.index(), new_key.index());
    assert_ne!(old_key.generation(), new_key.generation());
    let mut expected = directed(old_sensor, ball, false).to_vec();
    expected.extend(directed(ball, new_sensor, true));
    assert_eq!(events, expected);
    assert!(advance(&mut sim, Vec::new()).is_empty());

    let events = advance(&mut sim, vec![Command::Despawn { entity: new_sensor }]);
    assert_eq!(events, directed(ball, new_sensor, false));
    assert!(advance(&mut sim, Vec::new()).is_empty());
}

#[test]
fn a_dynamic_ball_reports_contact_with_a_slope_collider() {
    let mut world = World::new();
    // 坡面是独立的碰撞体生产路径。抬离隐式地面，用零坡度让持续接触容易判定。
    let slope = world.spawn((Slope {
        origin: [0.0, 2.0, 0.0],
        size: [2.0, 2.0],
        rise: 0.0,
    },));
    let ball = world.spawn((
        transform_at([1.0, 3.0, 1.0]),
        PhysicsBody {
            gravity_scale: 1.0,
            ..ball_body()
        },
    ));
    let slope_id = world.entity(slope).unwrap();
    let ball_id = world.entity(ball).unwrap();
    let mut sim = Simulation::new(world);
    let mut events = Vec::new();
    for _ in 0..90 {
        events.extend(advance(&mut sim, Vec::new()));
    }
    let y = sim.world().get::<Transform>(ball).unwrap().position[1];
    assert!((2.22..2.28).contains(&y), "球要被坡面托住，实际 y={y}");
    assert_eq!(events, directed(slope_id, ball_id, true));
}

#[test]
fn overlapping_pairs_have_the_same_persistent_id_order_at_one_and_four_threads() {
    let (mut world, sensor, right) = pair_world(OVERLAP);
    let left = world.spawn((transform_at([-OVERLAP[0], OVERLAP[1], 0.0]), ball_body()));
    let left_id = world.entity(left).unwrap();
    let mut expected = directed(sensor, right, true).to_vec();
    expected.extend(directed(sensor, left_id, true));
    // 补足有位移数据的 archetype，速度系统的四线程档也实际走分片。
    let velocity = Velocity {
        linear: [1.0, 0.0, 0.0],
    };
    let moving = world.spawn((Transform::default(), velocity));
    world.spawn((
        Transform::default(),
        velocity,
        Name {
            value: "另一个速度分片".into(),
        },
    ));
    let moving_id = world.entity(moving).unwrap();
    let data = world.world_data();

    let run = |threads| {
        let mut world = World::new();
        world.restore_world_data(&data).unwrap();
        let mut sim = Simulation::with_threads(world, FIXED_DT, threads);
        // with_threads 配置速度系统的分片；调度器的批内并行有单独的入口。
        sim.set_parallel_threads(threads);
        assert_eq!(sim.parallel_threads(), threads);
        assert_eq!(advance(&mut sim, Vec::new()), expected);
        assert!(advance(&mut sim, Vec::new()).is_empty());
        let moving = sim.world().key(moving_id).unwrap();
        let position = sim.world().get::<Transform>(moving).unwrap().position;
        assert_eq!(position[0], 2.0 * FIXED_DT);
        sim.world().state_hash()
    };
    assert_eq!(run(1), run(4));
}
