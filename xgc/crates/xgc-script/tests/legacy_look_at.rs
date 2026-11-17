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
    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_look_at_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("aim.lua"),
        "function update(ctx) look_at(ctx, 0.0, 1.0, 1.8) end\n",
    )
    .unwrap();
    let mut world = World::new();
    let camera = world.spawn((
        Transform {
            position: [4.2, 2.8, 5.4],
            ..Transform::default()
        },
        Script {
            path: "aim.lua".to_string(),
        },
    ));
    let mut runtime = xgc_script::ScriptRuntime::new(&dir);
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
    std::fs::remove_dir_all(&dir).unwrap();
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
