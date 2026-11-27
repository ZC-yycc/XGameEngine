use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use xgc_base::EntityId;
use xgc_host::{asset_paths, ScriptedRunner};
use xgc_world::{ComponentData, EntityData, Script, Transform, World, WorldData};

const LOW_ID: u64 = 7;
const HIGH_ID: u64 = 40;
const BATCH_PATH: &str = "batch.lua";
const BATCH_GUID: &str = "guid://f1e2d3c4b5a69788796a5b4c3d2e1f00";

struct ScriptRoot(PathBuf);

impl ScriptRoot {
    fn new(scripts: &[(&str, &str)]) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            Self(std::env::temp_dir().join(format!("xgc_batch_{}_{nonce}", std::process::id())));
        std::fs::create_dir(&root.0).unwrap();
        for &(path, source) in scripts {
            std::fs::write(root.0.join(path), source).unwrap();
        }
        root
    }

    fn runner(&self, high_mounts: &[&str]) -> ScriptedRunner {
        let entity = |id, transform, paths: &[&str]| EntityData {
            id: EntityId::from_raw(id),
            revision: 0,
            components: vec![
                ComponentData::from_component(&transform),
                ComponentData::from_component(&Script {
                    paths: paths.iter().map(|&path| path.to_owned()).collect(),
                }),
            ],
        };
        let mut world = World::new();
        // 同一 archetype 的行按 40、7 装入，Lua 结果要能区分存储顺序和持久 ID 顺序。
        world
            .restore_world_data(&WorldData {
                next_id: HIGH_ID + 1,
                entities: vec![
                    entity(
                        HIGH_ID,
                        Transform {
                            position: [200.0, 0.0, 0.0],
                            rotation: [0.0, 1.0, 0.0, 0.0],
                            ..Transform::default()
                        },
                        high_mounts,
                    ),
                    entity(
                        LOW_ID,
                        Transform {
                            position: [100.0, 0.0, 0.0],
                            ..Transform::default()
                        },
                        &[BATCH_PATH],
                    ),
                ],
                ..WorldData::default()
            })
            .unwrap();
        ScriptedRunner::with_assets(world, asset_paths(Some(&self.0), &[])).0
    }
}

impl Drop for ScriptRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn transform(world: &World, id: u64) -> &Transform {
    let key = world.key(EntityId::from_raw(id)).unwrap();
    world.get::<Transform>(key).unwrap()
}

fn assert_batch_step(world: &World, revision: u64) {
    let tick = revision as f32;
    assert_eq!(
        transform(world, LOW_ID).position,
        [100.0 + tick, tick, 1.0],
        "第 {revision} 步的实体 {LOW_ID}"
    );
    assert_eq!(
        transform(world, HIGH_ID).position,
        [200.0 + tick, tick, 2.0],
        "第 {revision} 步的实体 {HIGH_ID}"
    );
    assert_eq!(world.revision(), revision);
    for id in [LOW_ID, HIGH_ID] {
        assert_eq!(
            world.entity_revision(EntityId::from_raw(id)),
            Some(revision)
        );
    }
}

#[test]
fn returned_module_updates_both_mounts_once_per_fixed_step() {
    let scripts = ScriptRoot::new(&[
        (
            BATCH_PATH,
            r#"
            local M = {}
            local calls = 0
            local previous
            local position = "core.transform.position"

            function M.update(batch)
                if previous then
                    assert(not pcall(function() previous:len() end), "batch borrow escaped")
                end
                previous = batch
                local count = batch:len()
                assert(count == 2, "both entities must be in the same callback exactly once")
                calls = calls + 1
                for i = 1, count do
                    local e = batch:entity(i)
                    local p = e:get(position)
                    assert(p.y == calls - 1 and p.z == (calls == 1 and 0 or i))
                    e:set(position, {x = p.x + 1, y = calls, z = i})
                end
            end

            return M
            "#,
        ),
        (
            "batch.lua.meta",
            r#"(guid: "f1e2d3c4b5a69788796a5b4c3d2e1f00")"#,
        ),
        (
            "legacy.lua",
            r#"
            local id = self_id(nil)
            assert(id == 40)
            set_rot(nil, 0, 0, 0)
            state_set(nil, "ticks", 0)

            function start(ctx)
                state_set(ctx, "starts", (state_get(ctx, "starts") or 0) + 1)
            end

            function update(ctx)
                assert(self_id(ctx) == id and state_get(ctx, "starts") == 1)
                local tick = state_get(ctx, "ticks") + 1
                state_set(ctx, "ticks", tick)
                xgc.set(id, "core.transform", "scale", {x = 2 + tick, y = 3, z = 4})
            end

            return { label = "legacy metadata", update = function() error("wrong hook") end }
            "#,
        ),
    ]);
    let mut runner = scripts.runner(&[BATCH_GUID, BATCH_PATH, "legacy.lua"]);
    for revision in 1..=2 {
        assert_eq!(runner.step().unwrap(), revision - 1);
        let world = runner.world();
        // 旧挂载的顶层写与逐步钩子写必须随本批一起提交，返回的普通表不改变旧派发。
        let mixed = transform(world, HIGH_ID);
        assert_eq!(mixed.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(mixed.scale, [2.0 + revision as f32, 3.0, 4.0]);
        assert_batch_step(world, revision);
        assert_eq!(runner.script_count(), 4);
    }
}

#[test]
fn failed_batch_callback_leaves_all_its_queued_writes_uncommitted() {
    let scripts = ScriptRoot::new(&[(
        BATCH_PATH,
        r#"
        local M = {}
        local calls = 0
        local position = "core.transform.position"

        function M.update(batch)
            local count = batch:len()
            assert(count == 2)
            calls = calls + 1
            for i = 1, count do
                if calls == 2 and i == 2 then
                    error("abort after writing the first batch member")
                end
                local e = batch:entity(i)
                local p = e:get(position)
                e:set(position, {x = p.x + 1, y = calls, z = i})
            end
        end

        return M
        "#,
    )]);
    let mut runner = scripts.runner(&[BATCH_PATH]);
    assert_eq!(runner.step().unwrap(), 0);
    assert_batch_step(runner.world(), 1);
    let hash_before_error = runner.world().state_hash();

    let error = runner.step().unwrap_err();
    assert!(
        error.contains("abort after writing the first batch member"),
        "{error}"
    );
    // 首次成功证明两个成员都可写；第二步的首条写入也不能改变任一实体和修订号。
    assert_batch_step(runner.world(), 1);
    assert_eq!(runner.world().state_hash(), hash_before_error);
}

#[test]
fn batch_field_paths_read_the_current_callback_overlay() {
    let scripts = ScriptRoot::new(&[(
        BATCH_PATH,
        r#"
        return { update = function(batch)
            local paths = 'script.lua.paths'
            local second = paths .. '[1]'
            for i = 1, batch:len() do
                local row = batch:entity(i)
                assert(row:get('core.velocity.linear') == nil)
                row:set(paths, {'first.lua', 'second.lua'})
                assert(row:get(second) == 'second.lua')
                row:set(second, 'tail.lua')
                assert(row:get(paths)[2] == 'tail.lua')
            end
        end }
        "#,
    )]);
    let mut runner = scripts.runner(&[BATCH_PATH]);
    runner.step().unwrap();
    let world = runner.world();
    assert_eq!(world.revision(), 1);
    for id in [LOW_ID, HIGH_ID] {
        let key = world.key(EntityId::from_raw(id)).unwrap();
        assert_eq!(
            world.get::<Script>(key).unwrap().paths,
            ["first.lua", "tail.lua"]
        );
    }
}

#[test]
fn batch_initialization_cannot_keep_a_mounted_entity_binding() {
    let scripts = ScriptRoot::new(&[(
        BATCH_PATH,
        r#"
        local setter = _G.set_pos
        setter(nil, 9, 9, 9)
        return { update = function(batch) end }
        "#,
    )]);
    let mut runner = scripts.runner(&[BATCH_PATH]);
    let hash = runner.world().state_hash();
    let error = runner.step().unwrap_err();
    assert!(error.contains("批模块初始化只使用标准库"), "{error}");
    assert_eq!(runner.world().state_hash(), hash);
    assert_eq!(runner.script_count(), 0);
}

#[test]
fn batch_and_entity_callbacks_reload_as_one_group() {
    let batch = |delta| {
        format!(
            r#"
            return {{ update = function(batch)
                for i = 1, batch:len() do
                    local row = batch:entity(i)
                    local p = row:get('core.transform.position')
                    p.x = p.x + {delta}
                    row:set('core.transform.position', p)
                end
            end }}
            "#
        )
    };
    let legacy_path = "legacy.lua";
    // 旧钩子写 scale，让两组回调各自有一个可以观测的结果。
    let legacy = |scale| {
        format!("function update(ctx) xgc.set(self_id(ctx), 'core.transform', 'scale', {{x={scale}, y=2, z=3}}) end")
    };
    let scripts = ScriptRoot::new(&[(BATCH_PATH, &batch(1)), (legacy_path, &legacy(4))]);
    let legacy_file = scripts.0.join(legacy_path);
    let batch_file = scripts.0.join(BATCH_PATH);
    let mut runner = scripts.runner(&[BATCH_PATH, legacy_path]);
    let assert_step = |runner: &mut ScriptedRunner, expected_x: f32, expected_scale: f32| {
        runner.step().unwrap();
        assert_eq!(transform(runner.world(), LOW_ID).position[0], expected_x);
        assert_eq!(
            transform(runner.world(), HIGH_ID).position[0],
            expected_x + 100.0
        );
        assert_eq!(
            transform(runner.world(), HIGH_ID).scale,
            [expected_scale, 2.0, 3.0]
        );
    };
    assert_step(&mut runner, 101.0, 4.0);

    std::fs::write(batch_file, batch(10)).unwrap();
    std::fs::write(&legacy_file, "error('rejected candidate')").unwrap();
    assert!(runner
        .poll_script_reload()
        .unwrap_err()
        .contains("rejected candidate"));
    assert_step(&mut runner, 102.0, 4.0);

    std::fs::write(legacy_file, legacy(50)).unwrap();
    assert_eq!(runner.poll_script_reload(), Ok(3));
    assert_step(&mut runner, 112.0, 50.0);
}
