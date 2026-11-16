//! 播放器：cooked 世界 + 相对路径脚本可跑帧。

use xgc_world::{Script, Transform, World};

#[test]
fn runs_cooked_world_and_scripts() {
    let dir = std::env::temp_dir().join(format!(
        "xgc_player_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("move.lua"),
        "function update(ctx)\n  local p = entity_pos(ctx, self_id(ctx))\n  set_pos(ctx, p.x + 1.0, p.y, p.z)\nend\n",
    )
    .unwrap();
    let mut world = World::new();
    let key = world.spawn((
        Transform::default(),
        Script {
            path: "move.lua".to_string(),
        },
    ));
    let id = world.entity(key).unwrap();
    xgc_store::save_world(&dir.join("world.json"), &world.world_data()).unwrap();
    let stats = xgc_player::run_cooked(&dir, 3).unwrap();
    assert_eq!(stats.frames, 3);
    assert_eq!(stats.entities, 1);
    assert_eq!(stats.scripts, 1);
    let reloaded = xgc_store::load_world(&dir.join("world.json")).unwrap();
    let mut check = World::new();
    check.restore_world_data(&reloaded).unwrap();
    assert!(check.entity_data(id).is_some());
    std::fs::remove_dir_all(&dir).unwrap();
}
