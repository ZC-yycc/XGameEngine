//! 脚本读写路径的帧预算，使用固定世界和每帧变化的目标防止空跑。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use xgc_base::EntityId;
use xgc_script::ScriptRuntime;
use xgc_world::{
    Command, EntityData, GlobalTransform, Name, PhysicsBody, Script, Transform, UiElement,
    UiInteraction, Velocity, World,
};

thread_local! {
    static ALLOCATIONS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

struct CountingAllocator;

fn count(pointer: *mut u8, size: usize) -> *mut u8 {
    if !pointer.is_null() {
        let _ = ALLOCATIONS.try_with(|counter| {
            let (calls, bytes) = counter.get();
            counter.set((calls + 1, bytes + size));
        });
    }
    pointer
}

// SAFETY: 只转发原布局和指针，记账不分配也不变更内存。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(unsafe { System.alloc(layout) }, layout.size())
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(unsafe { System.alloc_zeroed(layout) }, layout.size())
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(unsafe { System.realloc(pointer, layout, size) }, size)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct ScriptDir(PathBuf);

impl ScriptDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("xgc_view_{}_{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for ScriptDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn script_reads_live_pose_and_name_with_bounded_frame_allocations() {
    const BACKGROUND: usize = 4096;
    const WARM: usize = 8;
    const MEASURED: usize = 60;
    let dir = ScriptDir::new();
    let mut world = World::new();
    for index in 0..BACKGROUND {
        world.spawn((
            Name {
                value: format!("background {index}"),
            },
            Transform::default(),
            Velocity::default(),
            PhysicsBody::default(),
        ));
    }
    let hole = world.spawn((Transform::default(),));
    let source = world.spawn((
        Name {
            value: "guide".into(),
        },
        Transform {
            position: [0.0, 2.0, -3.0],
            ..Transform::default()
        },
        GlobalTransform {
            position: [99.0; 3],
            ..GlobalTransform::default()
        },
    ));
    let source_id = world.entity(source).unwrap().raw();
    assert!(world.despawn(hole));
    // 后分配的同名实体占用较早回收的槽，首占必须按持久 ID 判断。
    world.spawn((
        Name {
            value: "guide".into(),
        },
        Transform::default(),
    ));
    let actor = world.spawn((
        Transform::default(),
        Script {
            path: "follow.lua".into(),
        },
    ));
    std::fs::write(
        dir.0.join("follow.lua"),
        format!(
            r#"
function update(ctx)
    local target = entity_id(ctx, "guide")
    assert(target == {source_id} and entity_name(ctx, target) == "guide")
    local source = entity_pos(ctx, target)
    local id = self_id(ctx)
    local old = entity_pos(ctx, id)
    set_pos(ctx, source.x, old.y + source.y, source.z)
    assert(entity_pos(ctx, id).y == old.y)
end
"#
        ),
    )
    .unwrap();
    let mut runtime = ScriptRuntime::new(&dir.0);
    let step = |runtime: &mut ScriptRuntime, world: &mut World, frame: usize| {
        world.get_mut::<Transform>(source).unwrap().position[0] = frame as f32;
        runtime.run_frame(world, 1.0 / 60.0);
        assert!(runtime.take_error().is_none(), "第 {frame} 帧脚本失败");
        assert_eq!(
            world.get::<Transform>(actor).unwrap().position,
            [frame as f32, frame as f32 * 2.0, -3.0]
        );
    };
    for frame in 1..=WARM {
        step(&mut runtime, &mut world, frame);
    }

    ALLOCATIONS.with(|counter| counter.set((0, 0)));
    let start = Instant::now();
    for frame in WARM + 1..=WARM + MEASURED {
        step(&mut runtime, &mut world, frame);
    }
    let elapsed = start.elapsed();
    let (calls, bytes) = ALLOCATIONS.with(Cell::get);
    eprintln!(
        "script view: {BACKGROUND} 个背景实体 / {MEASURED} 帧，{calls} 次分配 / {bytes} 字节，{elapsed:?}"
    );
    // 命令与 Lua 调用仍有自身开销。这里限制帧快照不再按背景实体数逐行分配。
    assert!(
        calls <= MEASURED * 128,
        "脚本帧超预算：{MEASURED} 帧 {calls} 次分配 / {bytes} 字节"
    );
}

#[test]
fn refreshed_rows_clear_removed_components_and_use_the_reserved_cursor() {
    let dir = ScriptDir::new();
    let mut world = World::new();
    let target = world.spawn((
        Name {
            value: "target".into(),
        },
        Transform {
            position: [4.0, 0.0, 0.0],
            ..Transform::default()
        },
        UiElement {
            text: "before".into(),
            ..UiElement::default()
        },
        UiInteraction {
            clicked: true,
            focused: true,
            ..UiInteraction::default()
        },
    ));
    let target_id = world.entity(target).unwrap();
    world.spawn((Script {
        path: "inspect.lua".into(),
    },));
    let next_id = world.reserve_entity_ids(64) + 64;
    std::fs::write(
        dir.0.join("inspect.lua"),
        format!(
            r#"
local updates = 0
function update(ctx)
    updates = updates + 1
    local target = {target_id}
    if updates == 1 then
        assert(entity_pos(ctx, target).x == 4 and entity_name(ctx, target) == "target")
        assert(ui_text(ctx, target) == "before" and ui_clicked(ctx, target) and ui_focused(ctx, target))
    else
        assert(entity_pos(ctx, target).x == 0 and entity_name(ctx, target) == "")
        assert(not ui_has(ctx, target) and not ui_clicked(ctx, target) and not ui_focused(ctx, target))
        local label = ui_spawn_label(ctx, "created", 0, 0, 20)
        assert(label == {next_id} and ui_has(ctx, label))
        ui_set_text(ctx, label, "updated")
        assert(ui_text(ctx, label) == "updated")
    end
end
"#
        ),
    )
    .unwrap();
    let mut runtime = ScriptRuntime::new(&dir.0);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert_eq!(runtime.take_error(), None);

    // 同一个持久 ID 换代，排序位置没变，旧的可选组件投影必须清掉。
    let plan = world.plan(vec![Command::Replace {
        data: EntityData::new(target_id),
    }]);
    world.apply(&plan).unwrap();
    assert!(!world.is_alive(target));
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert_eq!(runtime.take_error(), None);
    let label = world.key(EntityId::from_raw(next_id)).unwrap();
    assert_eq!(world.get::<UiElement>(label).unwrap().text, "updated");
}
