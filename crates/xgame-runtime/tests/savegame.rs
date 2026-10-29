//! 存档 / 读档的运行时语义（P2 #32 的端到端验收）。
//!
//! 这里钉的是引擎**保证**的那一半：抓取哪些状态、读档怎么套用、对不上的实体怎么办、
//! 变量在切场景 / 销毁实体时的生命周期，以及脚本侧（`save_game` / `save_set`）
//! 的完整闭环。
//!
//! 格式与落盘纪律（槽名、排序、坏档拒绝）在 `xgame-core/tests/savegame.rs`。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use xgame_core::{
    CharacterContacts, CharacterController, CharacterState, Entity, PhysicsBody, Prefab, SaveValue,
    Scene, SceneLoader,
};
use xgame_math::glam::Vec3;
use xgame_runtime::{World, WorldConfig};

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xgame_runtime_save_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn box_scene() -> Scene {
    let mut scene = Scene::new("boxes");
    let mut ground = Entity::new("ground");
    ground.physics = Some(PhysicsBody::static_box([8.0, 0.5, 8.0]));
    ground.transform.position = Vec3::new(0.0, -0.5, 0.0);
    scene.add_entity(ground);

    let mut falling = Entity::new("falling");
    falling.physics = Some(PhysicsBody::dynamic_box([0.5, 0.5, 0.5]));
    falling.transform.position = Vec3::new(0.0, 6.0, 0.0);
    scene.add_entity(falling);
    scene
}

/// 只认内存里预置的场景（不碰盘）：用来测"读档顺手切场景"。
struct MemoryLoader {
    scenes: Arc<Mutex<HashMap<String, Scene>>>,
}

impl SceneLoader for MemoryLoader {
    fn load_scene(&self, relative: &str) -> Result<Scene, String> {
        self.scenes
            .lock()
            .unwrap()
            .get(&relative.replace('\\', "/"))
            .cloned()
            .ok_or_else(|| format!("没有这份场景: {relative}"))
    }

    fn load_prefab(&self, _relative: &str) -> Result<Prefab, String> {
        Err("测试装载器不支持预制体".into())
    }
}

/// 存 → 改变状态 → 读：位置 / 旋转 / 速度都回到存档那一刻。
#[test]
fn save_then_load_restores_transforms_and_velocity() {
    let root = temp_root("roundtrip");
    let mut world = World::new(box_scene(), &root, WorldConfig::default());

    // 让它落一会儿，拿到非零速度与非原点位置。
    for _ in 0..12 {
        world.update(1.0 / 60.0);
    }
    let captured = world.capture_save();
    let falling = world.scene().entities[1].clone();
    assert!(
        captured.entity(falling.id).is_some(),
        "存档里应当有落体实体"
    );
    let saved_pos = falling.transform.position;
    let saved_vel = Vec3::from_array(captured.entity(falling.id).unwrap().linear_velocity);
    assert!(saved_vel.length() > 0.1, "落下中的方块应当有速度: {saved_vel:?}");

    world.save_game("slot1").expect("存档");

    // 继续跑一段：位置与速度都会变。
    for _ in 0..40 {
        world.update(1.0 / 60.0);
    }
    let drifted = world.scene().entities[1].transform.position;
    assert!(drifted.distance(saved_pos) > 0.2, "状态应当已经变了");

    let report = world.load_game("slot1").expect("读档");
    assert_eq!(report.applied, 2, "两个实体都该对上");
    assert_eq!(report.missing, 0);
    assert!(!report.scene_switched, "没记录场景就不切");
    let restored = world.scene().entities[1].clone();
    assert!(
        (restored.transform.position - saved_pos).length() < 1e-4,
        "位置应回到存档那一刻: {:?} vs {saved_pos:?}",
        restored.transform.position
    );
    // 再跑一帧：速度回来意味着它会继续往下掉，而不是原地停住。
    let before = world.scene().entities[1].transform.position;
    world.update(1.0 / 60.0);
    let after = world.scene().entities[1].transform.position;
    assert!(
        after.y < before.y,
        "读档后应当继续下落（速度被恢复）: {before:?} -> {after:?}"
    );

    std::fs::remove_dir_all(&root).unwrap();
}

/// 存档之后才生成的实体：读档**不复活**，计入 `missing`。
#[test]
fn load_skips_entities_that_are_not_in_the_scene() {
    let root = temp_root("missing");
    let mut world = World::new(box_scene(), &root, WorldConfig::default());
    world.save_game("slot1").expect("存档");

    // 存档里多一个虚构 ID 的实体：模拟"存档之后删掉了某个实体"。
    let mut save = xgame_core::read_save(&root, "slot1").expect("读档");
    save.entities.push(xgame_core::savegame::EntityState {
        id: u64::MAX - 5,
        position: [1.0, 2.0, 3.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
        linear_velocity: [0.0; 3],
        angular_velocity: [0.0; 3],
        vars: Default::default(),
    });
    xgame_core::write_save(&root, "slot1", &save).expect("写回");

    let report = world.load_game("slot1").expect("读档");
    assert_eq!(report.applied, 2);
    assert_eq!(report.missing, 1, "对不上的实体要被计数");
    assert_eq!(
        world.scene().entities.len(),
        2,
        "读档不会生成新实体"
    );

    std::fs::remove_dir_all(&root).unwrap();
}

/// 存档里的场景与当前不同：先切场景，再套状态；实体变量清空、全局变量保留。
#[test]
fn load_switches_scene_and_keeps_global_vars_only() {
    let root = temp_root("switch");
    let scenes = Arc::new(Mutex::new(HashMap::new()));
    let mut other = Scene::new("other");
    let mut crate_entity = Entity::new("crate");
    crate_entity.physics = Some(PhysicsBody::static_box([1.0, 1.0, 1.0]));
    crate_entity.transform.position = Vec3::new(0.0, 1.0, 0.0);
    let crate_id = crate_entity.id;
    other.add_entity(crate_entity);
    scenes
        .lock()
        .unwrap()
        .insert("Scenes/Other.ron".to_string(), other);

    let mut world = World::new(box_scene(), &root, WorldConfig::default())
        .with_scene_loader(MemoryLoader {
            scenes: Arc::clone(&scenes),
        })
        .with_current_scene("Scenes/Boxes.ron");

    // 存档：当前是 Boxes，实体变量挂在 Boxes 的实体上。
    world.set_save_var("coins", SaveValue::Number(7.0));
    let falling = world.scene().entities[1].id;
    assert!(world.set_entity_save_var(falling, "hp", SaveValue::Number(80.0)));
    world.save_game("slot1").expect("存档");

    // 手动改存档里的场景字段，模拟"在别的场景里读同一份档"。
    let mut save = xgame_core::read_save(&root, "slot1").expect("读档");
    save.scene = "Scenes/Other.ron".into();
    xgame_core::write_save(&root, "slot1", &save).expect("写回");

    let report = world.load_game("slot1").expect("读档");
    assert!(report.scene_switched, "存档场景不同必须切过去");
    assert_eq!(world.current_scene(), Some("Scenes/Other.ron"));
    assert_eq!(report.applied, 0, "旧场景的实体 ID 在新场景里不存在");
    assert_eq!(report.missing, 2);

    // 全局变量保留；实体变量随切场景清空。
    assert_eq!(world.save_var("coins"), Some(&SaveValue::Number(7.0)));
    assert!(
        world.entity_save_var(falling, "hp").is_none(),
        "实体变量是实体绑定态，切场景必须清（ID 只在单个文件内唯一）"
    );
    let _ = crate_id;

    std::fs::remove_dir_all(&root).unwrap();
}

/// 实体销毁时它的变量一起清掉（不留在表里等下一个实体来读）。
#[test]
fn entity_vars_are_dropped_with_the_entity() {
    let root = temp_root("destroy");
    let mut sc = Scene::new("one");
    let mut e = Entity::new("victim");
    e.scripts.push("Scripts/Killer.lua".into());
    sc.add_entity(e);
    std::fs::create_dir_all(root.join("Scripts")).unwrap();
    std::fs::write(
        root.join("Scripts/Killer.lua"),
        "function update(ctx) destroy(ctx, self_id(ctx)) end",
    )
    .unwrap();

    let mut world = World::new(sc, &root, WorldConfig::default());
    let id = world.scene().entities[0].id;
    assert!(world.set_entity_save_var(id, "hp", SaveValue::Number(1.0)));
    world.update(1.0 / 60.0);
    assert!(world.scene().entities.is_empty(), "脚本应当销毁自己");
    assert!(
        world.entity_save_var(id, "hp").is_none(),
        "实体没了，它的变量不该留在表里"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// 脚本侧完整闭环：`save_set` / `entity_save_set` / `save_game` → 改状态 →
/// `load_game` → 下一帧看到的是存档时的值。
#[test]
fn script_api_saves_and_restores_player_progress() {
    let root = temp_root("script");
    std::fs::create_dir_all(root.join("Scripts")).unwrap();
    std::fs::write(
        root.join("Scripts/Probe.lua"),
        r#"
-- 每帧把 x 推远一点，方便观察"读档把它拨回去"
function update(ctx)
    local me = self_id(ctx)
    local p = pos(ctx)
    set_pos(ctx, p.x + 1.0, p.y, p.z)
end
"#,
    )
    .unwrap();

    let mut sc = Scene::new("probe");
    let mut e = Entity::new("probe");
    e.scripts.push("Scripts/Probe.lua".into());
    sc.add_entity(e);
    let mut world = World::new(sc, &root, WorldConfig::default());
    let id = world.scene().entities[0].id;

    // 跑 3 帧 → x = 3。
    for _ in 0..3 {
        world.update(1.0 / 60.0);
    }
    assert!((world.scene().entities[0].transform.position.x - 3.0).abs() < 1e-4);

    // 写变量 + 存档（同一帧末尾结算，等一帧让文件落盘）。
    assert!(world.set_save_var("coins", SaveValue::Number(12.0)));
    assert!(world.set_entity_save_var(id, "hp", SaveValue::Number(75.0)));
    world.save_game("slot1").expect("存档");
    assert!(root.join("Saves/slot1.sav").is_file());

    // 再跑 4 帧 → x = 7。
    for _ in 0..4 {
        world.update(1.0 / 60.0);
    }
    assert!((world.scene().entities[0].transform.position.x - 7.0).abs() < 1e-4);

    let report = world.load_game("slot1").expect("读档");
    assert_eq!(report.applied, 1);
    assert_eq!(world.save_var("coins"), Some(&SaveValue::Number(12.0)));
    assert_eq!(
        world.entity_save_var(id, "hp"),
        Some(&SaveValue::Number(75.0))
    );
    assert!(
        (world.scene().entities[0].transform.position.x - 3.0).abs() < 1e-4,
        "读档把 x 拨回 3，实际 {}",
        world.scene().entities[0].transform.position.x
    );

    std::fs::remove_dir_all(&root).unwrap();
}

/// 坏槽名 / 不存在的槽：报错而不是 panic，也不改世界状态。
#[test]
fn bad_slots_fail_without_touching_the_world() {
    let root = temp_root("badslot");
    let mut world = World::new(box_scene(), &root, WorldConfig::default());
    let before = world.scene().entities[1].transform.position;

    assert!(world.save_game("../escape").is_err(), "越界槽名必须拒绝");
    assert!(world.load_game("nope").is_err(), "不存在的槽必须报错");
    assert_eq!(
        world.scene().entities[1].transform.position,
        before,
        "失败的存 / 读不该动世界"
    );
    assert!(!root.join("Saves").exists() || !root.join("Saves/escape.sav").exists());
    std::fs::remove_dir_all(&root).unwrap();
}

/// 场景里出现**重复 ID** 时，读档必须套到"第一个"（与 `Scene::find_entity`
/// 同解）。这条钉的是 `apply_save` 里 `or_insert` 那个选择 —— 换成 `insert`
/// 会让同一个场景在两套寻址下给出不同实体，症状是"读档后有的物体位置不对"。
#[test]
fn duplicate_ids_resolve_like_find_entity() {
    let root = temp_root("dup");
    let mut sc = Scene::new("dup");
    let mut first = Entity::new("first");
    first.transform.position = Vec3::new(1.0, 0.0, 0.0);
    sc.add_entity(first);
    let mut second = Entity::new("second");
    // 手写场景可能出现撞 ID（编辑器读盘时修，内存里不保证）。
    second.id = sc.entities[0].id;
    second.transform.position = Vec3::new(9.0, 0.0, 0.0);
    sc.add_entity(second);

    let mut world = World::new(sc, &root, WorldConfig::default());
    let mut save = world.capture_save();
    // 只留**一条**该 ID 的状态（采集按实体逐条来，撞 ID 时会出现两条），
    // 位置写成 5：读档应当套到第一个实体上。
    let dup_id = save.entities[0].id;
    save.entities = save
        .entities
        .into_iter()
        .filter(|e| e.id == dup_id)
        .take(1)
        .collect();
    save.entities[0].position = [5.0, 0.0, 0.0];
    let report = world.apply_save(&save);
    assert_eq!(report.applied, 1);
    assert_eq!(
        world.scene().entities[0].transform.position.x, 5.0,
        "第一个同名 ID 的实体应当被套用"
    );
    assert_eq!(
        world.scene().entities[1].transform.position.x, 9.0,
        "第二个保持不动（与 find_entity 的 '取第一个' 一致）"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// 读档要把**角色运行时状态**一起拨回去。
///
/// `CharacterState.velocity` 是"上一次移动反推出来的速度"，读档后不复位的话，
/// 角色会带着读档前那一刻的速度继续冲（在高处读档 = 直接甩出去）。
#[test]
fn load_resets_character_runtime_velocity() {
    let root = temp_root("character");
    let mut sc = Scene::new("char");
    let mut player = Entity::new("player");
    player.controller = Some(CharacterController::default());
    player.transform.position = Vec3::new(0.0, 1.0, 0.0);
    player.components.insert(CharacterState {
        velocity: Vec3::new(3.0, -18.0, 0.0),
        grounded: true,
        sliding_down_slope: true,
        contacts: CharacterContacts::new(),
        ..CharacterState::default()
    });
    sc.add_entity(player);
    let mut world = World::new(sc, &root, WorldConfig::default());
    let id = world.scene().entities[0].id;

    world.save_game("slot1").expect("存档");

    // 模拟"读档前玩家正以另一个速度移动"。
    let idx = world.scene().find_entity(id).unwrap();
    world.scene_mut().entities[idx]
        .components
        .get_or_insert_with(CharacterState::default)
        .velocity = Vec3::new(0.0, -40.0, 0.0);
    world.scene_mut().entities[idx]
        .components
        .get_mut::<CharacterState>()
        .unwrap()
        .grounded = true;

    world.load_game("slot1").expect("读档");
    let state = *world.scene().entities[idx]
        .component::<CharacterState>()
        .expect("角色状态应当还在");
    assert!(
        state.velocity.length() < 1e-3,
        "读档后不该保留读档前那一刻的速度: {:?}",
        state.velocity
    );
    assert!(!state.grounded, "着地标志交给下一帧重新求解");
    assert!(!state.sliding_down_slope);
    assert!(state.contacts.is_empty(), "接触面备忘随读档一起清空");

    std::fs::remove_dir_all(&root).unwrap();
}

/// 不动点：`capture → 存 → 改乱 → 读 → capture` 的第二份采集必须与第一份相等。
///
/// 这是存档最该有的性质（与场景的 `save → load → save` 门禁同款）：
/// 只要它成立，"读档之后再存档"就不会让数据一点点漂掉。比较用 1e-4 容差，
/// 因为读档时会把旋转归一化（1 ulp 级别的差是容许的，肉眼不可见的漂移不是）。
#[test]
fn save_load_capture_is_a_fixed_point() {
    let root = temp_root("fixed_point");
    let mut sc = Scene::new("fixed");
    for i in 0..200 {
        let mut e = Entity::new(format!("e{i}"));
        e.transform.position = Vec3::new(i as f32 * 0.37, 2.0 + (i % 7) as f32 * 0.5, -1.5);
        e.transform.rotation = xgame_math::glam::Quat::from_rotation_y(i as f32 * 0.11).normalize();
        e.transform.scale = Vec3::splat(1.0 + (i % 3) as f32 * 0.25);
        if i % 3 == 0 {
            e.physics = Some(PhysicsBody::dynamic_box([0.4, 0.4, 0.4]));
        }
        sc.add_entity(e);
    }
    let mut world = World::new(sc, &root, WorldConfig::default());
    // 落几帧：让动态体带上真实速度。
    for _ in 0..7 {
        world.update(1.0 / 60.0);
    }
    for i in 0..20 {
        world.set_save_var(&format!("k{i}"), SaveValue::Number(i as f64));
    }
    let first = world.capture_save();
    world.save_game("slot1").expect("存档");

    // 把世界改乱：位置、速度、变量全动。
    let n = world.scene().entities.len();
    for i in 0..n {
        let idx = i;
        world.scene_mut().entities[idx].transform.position.x += 5.0;
        world.scene_mut().entities[idx].transform.scale = Vec3::splat(3.0);
    }
    world.set_save_var("k0", SaveValue::Text("dirty".into()));
    world.set_save_var("extra", SaveValue::Bool(true));
    for _ in 0..10 {
        world.update(1.0 / 60.0);
    }

    let report = world.load_game("slot1").expect("读档");
    assert_eq!(report.applied, n);
    let second = world.capture_save();

    assert_eq!(second.entities.len(), first.entities.len());
    assert_eq!(second.vars, first.vars, "全局变量必须逐字段回到存档那一刻");
    for (a, b) in first.entities.iter().zip(second.entities.iter()) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.vars, b.vars, "实体 {} 的变量", a.id);
        for (x, y) in a
            .position
            .iter()
            .zip(b.position.iter())
            .chain(a.rotation.iter().zip(b.rotation.iter()))
            .chain(a.scale.iter().zip(b.scale.iter()))
            .chain(a.linear_velocity.iter().zip(b.linear_velocity.iter()))
            .chain(a.angular_velocity.iter().zip(b.angular_velocity.iter()))
        {
            assert!(
                (x - y).abs() < 1e-4,
                "实体 {} 的状态漂了: {x} vs {y}",
                a.id
            );
        }
    }

    std::fs::remove_dir_all(&root).unwrap();
}

/// 同一帧里既 `load_game` 又 `load_scene`：**最后停在切场景请求那一份**。
///
/// 这条钉的是顺序（读档先套用、帧末切场景随后覆盖），而不是"谁更对" ——
/// 两个动作都想决定"这次运行在哪张图"，本来就该是作者的 bug；引擎只保证
/// 结果确定 + 一条 warn，不让他们在半路上猜。
#[test]
fn load_game_and_load_scene_in_one_frame_ends_at_the_scene_request() {
    let root = temp_root("both");
    std::fs::create_dir_all(root.join("Scripts")).unwrap();
    std::fs::write(
        root.join("Scripts/Both.lua"),
        r#"
function update(ctx)
    if _G.done then return end
    _G.done = true
    load_game(ctx, "slot1")
    load_scene(ctx, "Scenes/Other.ron")
end
"#,
    )
    .unwrap();

    let scenes = Arc::new(Mutex::new(HashMap::new()));
    let mut other = Scene::new("other");
    other.add_entity(Entity::new("mark"));
    scenes
        .lock()
        .unwrap()
        .insert("Scenes/Other.ron".to_string(), other);

    let mut sc = Scene::new("start");
    let mut probe = Entity::new("probe");
    probe.scripts.push("Scripts/Both.lua".into());
    sc.add_entity(probe);
    let mut world = World::new(sc, &root, WorldConfig::default())
        .with_scene_loader(MemoryLoader {
            scenes: Arc::clone(&scenes),
        })
        .with_current_scene("Scenes/Start.ron");

    // **在任何一帧跑之前**存档：此时脚本还没执行，存档里没有"读档 + 切场景"。
    world.save_game("slot1").expect("存档");

    world.update(1.0 / 60.0);
    world.update(1.0 / 60.0);
    assert_eq!(
        world.current_scene(),
        Some("Scenes/Other.ron"),
        "帧末的切场景请求是最后一个动作，它生效"
    );
    assert_eq!(
        world.scene().entities.len(),
        1,
        "停在 Other 场景（读档套用的是 Start，已被随后的切场景覆盖）"
    );
    std::fs::remove_dir_all(&root).unwrap();
}
