//! 旧脚本 API `look_at` 的朝向回归：必须把 -Z 对准目标，且不引入滚转。
//!
//! 之前的实现把俯仰塞进欧拉角的 Z 分量，结果变成绕自身视轴的滚转：相机只转
//! yaw、俯仰为 0，画面整体歪斜（showcase 第 150 帧截图因此整帧偏移）。

use xgc_world::{Script, Transform, World};

/// `look_at` 必须真的把 -Z 朝向对准目标，且不引入滚转（up 仍朝 +Y）。
///
/// 之前的实现把俯仰塞进欧拉角的 Z 分量，结果变成绕自身视轴的滚转：
/// 相机只转 yaw、俯仰为 0，画面整体歪斜（showcase 截图因此整帧偏移）。
#[test]
fn legacy_look_at_aims_forward_without_roll() {
    let dir = ScriptDir::with_source("function update(ctx) look_at(ctx, 0.0, 1.0, 1.8) end");
    let mut world = World::new();
    let camera = world.spawn((
        Transform {
            position: [4.2, 2.8, 5.4],
            ..Transform::default()
        },
        Script {
            paths: vec!["aim.lua".into()],
        },
    ));
    let mut runtime = xgc_script::ScriptRuntime::new(&dir.0);
    runtime.run_frame(&mut world, 1.0 / 60.0);

    let rotation = world.get::<Transform>(camera).unwrap().rotation;
    let forward = rotate(rotation, [0.0, 0.0, -1.0]);
    let up = rotate(rotation, [0.0, 1.0, 0.0]);
    let expected = [-4.2f32, -1.8, -3.6];
    let length =
        (expected[0] * expected[0] + expected[1] * expected[1] + expected[2] * expected[2]).sqrt();
    for axis in 0..3 {
        let want = expected[axis] / length;
        assert!(
            (forward[axis] - want).abs() < 1e-4,
            "朝向没有对准目标：forward={forward:?}，期望方向 {want}"
        );
    }
    assert!(up[1] > 0.9, "不应出现滚转（up 应仍接近 +Y）：up={up:?}");
    assert!(
        up[0].abs() < 0.3 && up[2].abs() < 0.4,
        "up 被滚转了：{up:?}"
    );
    assert_eq!(runtime.take_error(), None);
}

#[test]
fn look_at_reads_the_position_written_earlier_in_the_hook() {
    let dir = ScriptDir::with_source(
        r#"
local frame = 0
function update(ctx)
    frame = frame + 1
    local id = self_id(ctx)
    if frame == 1 then
        set_pos(ctx, 2, 3, 4)
        look_at(ctx, 2, 3, 3)
        assert(pos(ctx).x == 2 and entity_pos(ctx, id).z == 4)
    else
        set_pos(ctx, 0, 0, 0)
        xgc.set(id, "core.transform", "position", {x = 5, y = 6, z = 7})
        look_at(ctx, 6, 6, 7)
        assert(pos(ctx).z == 7 and entity_pos(ctx, id).x == 5)
    end
end
"#,
    );
    let mut world = World::new();
    let camera = world.spawn((
        Transform {
            position: [7.0, 10.0, 0.0],
            ..Transform::default()
        },
        Script {
            paths: vec!["aim.lua".into()],
        },
    ));
    let mut runtime = xgc_script::ScriptRuntime::new(&dir.0);
    for (position, expected_forward) in [
        ([2.0, 3.0, 4.0], [0.0, 0.0, -1.0]),
        ([5.0, 6.0, 7.0], [1.0, 0.0, 0.0]),
    ] {
        runtime.run_frame(&mut world, 1.0 / 60.0);
        let transform = world.get::<Transform>(camera).unwrap();
        assert_eq!(transform.position, position);
        let forward = rotate(transform.rotation, [0.0, 0.0, -1.0]);
        for axis in 0..3 {
            assert!(
                (forward[axis] - expected_forward[axis]).abs() < 1e-5,
                "look_at 的朝向 {forward:?} 应为 {expected_forward:?}"
            );
        }
        assert_eq!(runtime.take_error(), None);
    }
}

struct ScriptDir(std::path::PathBuf);

impl ScriptDir {
    fn with_source(source: &str) -> Self {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("xgc_lua_look_at_{}_{time}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("aim.lua"), source).unwrap();
        Self(dir)
    }
}

impl Drop for ScriptDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rotate(rotation: [f32; 4], vector: [f32; 3]) -> [f32; 3] {
    let [x, y, z, w] = rotation;
    let qv = [x, y, z];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(qv, vector);
    let t = [t[0] * 2.0, t[1] * 2.0, t[2] * 2.0];
    let c = cross(qv, t);
    [
        vector[0] + w * t[0] + c[0],
        vector[1] + w * t[1] + c[1],
        vector[2] + w * t[2] + c[2],
    ]
}
