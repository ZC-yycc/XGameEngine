use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use xgc_base::EntityId;
use xgc_host::{asset_paths, ScriptedRunner};
use xgc_world::{GlobalTransform, Parent, Script, Transform, World};

struct ScriptRoot(PathBuf);

impl ScriptRoot {
    fn new(sources: &[(&str, &str)]) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("xgc_script_pose_{}_{nonce}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        for &(path, source) in sources {
            std::fs::write(root.join(path), source).unwrap();
        }
        Self(root)
    }

    fn runner(&self, world: World) -> ScriptedRunner {
        ScriptedRunner::with_assets(world, asset_paths(Some(&self.0), &[])).0
    }
}

impl Drop for ScriptRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_vector(actual: [f32; 3], expected: [f32; 3]) {
    assert!(
        actual
            .into_iter()
            .zip(expected)
            .all(|(a, b)| (a - b).abs() < 2e-4),
        "实际 {actual:?}，预期 {expected:?}"
    );
}

#[test]
fn mounted_camera_aims_from_its_latest_world_position() {
    let scripts = ScriptRoot::new(&[
        (
            "pivot.lua",
            "function update(ctx) set_rot(ctx, 0, 0, 90) end",
        ),
        (
            "camera.lua",
            r#"
local function check(label, p, x, y, z)
    assert(math.abs(p.x-x) < 0.0002 and math.abs(p.y-y) < 0.0002 and math.abs(p.z-z) < 0.0002,
        string.format('%s: (%.6f, %.6f, %.6f) 应为 (%d, %d, %d)', label, p.x, p.y, p.z, x, y, z))
end

function start(ctx)
    check('父节点先转', pos(ctx), 14, 18, -2)
    xgc.set(0, 'core.transform', 'position', {x = 13, y = 1, z = -5})
    check('祖先平移', pos(ctx), 17, 17, -3)
    xgc.set(1, 'core.transform', 'scale', {x = 4, y = 2, z = 3})
    check('直接父节点缩放', pos(ctx), 13, 21, -3)
    check('读取父节点的世界位置', entity_pos(ctx, 1), 19, 5, -7)
    local id = self_id(ctx)
    xgc.set(id, 'core.transform', 'position', {x = 3, y = -1, z = 2})
    check('局部字段写入', pos(ctx), 31, 29, -11)
    set_pos(ctx, 25, 13, 1)
    check('世界位置写入', pos(ctx), 25, 13, 1)
    check('用 ID 读取', entity_pos(ctx, id), 25, 13, 1)
    look_at(ctx, 29, 16, 13)
end

function fixed_update(ctx)
    check('同一步后续钩子', pos(ctx), 25, 13, 1)
end
"#,
        ),
    ]);
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let mut world = World::new();
    // 首个实体 ID 为零，父关系由组件是否存在决定。
    let root = world.spawn((
        Transform {
            position: [10.0, 2.0, -4.0],
            rotation: [0.0, half, 0.0, half],
            scale: [2.0; 3],
        },
        GlobalTransform::default(),
    ));
    let root = world.entity(root).unwrap();
    assert_eq!(root, EntityId::from_raw(0));
    let pivot = world.spawn((
        Transform {
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, -half, half],
            scale: [3.0, 2.0, 1.0],
        },
        GlobalTransform::default(),
        Parent { entity: root },
        Script {
            paths: vec!["pivot.lua".into()],
        },
    ));
    let eye = world.spawn((
        Transform {
            position: [2.0, 1.0, -1.0],
            ..Transform::default()
        },
        GlobalTransform::default(),
        Parent {
            entity: world.entity(pivot).unwrap(),
        },
        Script {
            paths: vec!["camera.lua".into()],
        },
    ));
    // 世界前向与上方向通过已传播的实际四元数验证，避免依赖四元数的符号。
    let mut runner = scripts.runner(world);
    runner.step().unwrap();
    let world = runner.world();
    let key = eye;
    assert_vector(
        world.get::<Transform>(key).unwrap().position,
        [1.0, 2.0, 1.0],
    );
    let pose = world.get::<GlobalTransform>(key).unwrap();
    assert_vector(pose.position, [25.0, 13.0, 1.0]);
    let rotation = GlobalTransform {
        scale: [1.0; 3],
        ..*pose
    };
    let forward = xgc_world::to_world_delta(&rotation, [0.0, 0.0, -1.0]);
    let up = xgc_world::to_world_delta(&rotation, [0.0, 1.0, 0.0]);
    assert_vector(forward, [4.0, 3.0, 12.0].map(|axis| axis / 13.0));
    assert_vector(
        up,
        [-3.0, 40.0, -9.0].map(|axis| axis / (13.0 * 10.0_f32.sqrt())),
    );
}

#[test]
fn mounted_positions_preserve_the_affine_parent_chain() {
    let scripts = ScriptRoot::new(&[(
        "point.lua",
        r#"
function update(ctx)
    local before = pos(ctx)
    assert(math.abs(before.x - math.sqrt(2)) < 0.0002 and math.abs(before.z + 1/math.sqrt(2)) < 0.0002,
        string.format('父链上的世界点 (%.6f, %.6f, %.6f)', before.x, before.y, before.z))
    set_pos(ctx, math.sqrt(2), 3, 1/math.sqrt(2))
    local after = entity_pos(ctx, self_id(ctx))
    assert(math.abs(after.x - math.sqrt(2)) < 0.0002 and math.abs(after.y - 3) < 0.0002
        and math.abs(after.z - 1/math.sqrt(2)) < 0.0002)
end
"#,
    )]);
    let mut world = World::new();
    let root = world.spawn((Transform {
        scale: [2.0, 1.0, 1.0],
        ..Transform::default()
    },));
    let root = world.entity(root).unwrap();
    let angle = std::f32::consts::FRAC_PI_8;
    let pivot = world.spawn((
        Transform {
            rotation: [0.0, angle.sin(), 0.0, angle.cos()],
            ..Transform::default()
        },
        Parent { entity: root },
    ));
    let point = world.spawn((
        Transform {
            position: [1.0, 0.0, 0.0],
            ..Transform::default()
        },
        Parent {
            entity: world.entity(pivot).unwrap(),
        },
        Script {
            paths: vec!["point.lua".into()],
        },
    ));
    // 这里刻意不给实体派生的 GlobalTransform，脚本应从当前局部链计算。
    let mut runner = scripts.runner(world);
    runner.step().unwrap();
    let world = runner.world();
    let key = point;
    assert_vector(
        world.get::<Transform>(key).unwrap().position,
        [0.0, 3.0, 1.0],
    );
}

#[test]
fn mounted_position_falls_back_when_the_parent_chain_cannot_be_inverted() {
    let scripts = ScriptRoot::new(&[
        (
            "cycle.lua",
            r#"
function update(ctx)
    local before = pos(ctx)
    assert(before.x == 4 and before.y == 5 and before.z == 6)
    set_pos(ctx, 9, 8, 7)
    local after = pos(ctx)
    assert(after.x == 9 and after.y == 8 and after.z == 7)
end
"#,
        ),
        (
            "singular.lua",
            r#"
function update(ctx)
    set_pos(ctx, 9, 8, 7)
    local after = pos(ctx)
    assert(after.x == 1 and after.y == 8 and after.z == 7)
end
"#,
        ),
    ]);
    let mut world = World::new();
    // 环只存在于祖先之间，访问者自身并不在环上。
    let first = world.spawn((
        Transform {
            position: [99.0, 0.0, 0.0],
            ..Transform::default()
        },
        Parent {
            entity: EntityId::from_raw(1),
        },
    ));
    let first = world.entity(first).unwrap();
    world.spawn((Transform::default(), Parent { entity: first }));
    let singular = world.spawn((Transform {
        position: [1.0, 0.0, 0.0],
        scale: [0.0, 1.0, 1.0],
        ..Transform::default()
    },));
    let singular = world.entity(singular).unwrap();
    let mut children = Vec::new();
    for (parent, path) in [(first, "cycle.lua"), (singular, "singular.lua")] {
        let child = world.spawn((
            Transform {
                position: [4.0, 5.0, 6.0],
                ..Transform::default()
            },
            Parent { entity: parent },
            Script {
                paths: vec![path.into()],
            },
        ));
        children.push(child);
    }
    let mut runner = scripts.runner(world);
    runner.step().unwrap();
    for child in children {
        assert_vector(
            runner.world().get::<Transform>(child).unwrap().position,
            [9.0, 8.0, 7.0],
        );
    }
}
