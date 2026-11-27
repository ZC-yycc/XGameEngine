//! S6 的 50,000 实体编辑工作流。性能用例默认忽略，使用 Release 复跑。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use xgc_render::RenderStats;
use xgc_store::{
    save_project, save_world, ProjectContent, ProjectManifest, PROJECT_FORMAT_VERSION,
};
use xgc_world::{Camera, GlobalTransform, MeshRenderer, Name, Parent, Transform, World};

use super::{egui, install_fonts, wgpu, EditorApp, GpuContext, Renderer};
use crate::EditorState;
use xgc_base::EntityId;

const ENTITIES: usize = 50_000;
const DISTRICTS: usize = 50;
const BUILDINGS: usize = 999;
const MESHES: usize = 2_000;
const SCENE: &str = "scenes/city.json";
const SCREEN: [f32; 2] = [1280.0, 720.0];
const VIEWPORT: [u32; 2] = [800, 450];
const IDLE_SAMPLES: usize = 32;
const ACTION_SAMPLES: usize = 10;
const VIEWPORT_SAMPLES: usize = 30;

thread_local! {
    static ALLOCATIONS: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn count(pointer: *mut u8, size: usize) -> *mut u8 {
    if !pointer.is_null() {
        let _ = ALLOCATIONS.try_with(|counter| {
            if let Some((calls, bytes)) = counter.get() {
                counter.set(Some((calls + 1, bytes + size as u64)));
            }
        });
    }
    pointer
}

// SAFETY: 指针和布局原样交给 System。计数只访问不会分配的线程本地 Cell。
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

#[derive(Clone, Copy)]
struct Sample {
    elapsed: Duration,
    allocations: Option<(u64, u64)>,
}

fn measure<T>(track: bool, task: impl FnOnce() -> T) -> (T, Sample) {
    ALLOCATIONS.with(|counter| counter.set(track.then_some((0, 0))));
    let start = Instant::now();
    let result = task();
    let elapsed = start.elapsed();
    let allocations = ALLOCATIONS.with(|counter| counter.take());
    (
        result,
        Sample {
            elapsed,
            allocations,
        },
    )
}

fn report(label: &str, samples: &[Duration]) {
    assert!(!samples.is_empty());
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let percentile =
        |p: usize| sorted[(sorted.len() * p).div_ceil(100) - 1].as_secs_f64() * 1_000.0;
    println!(
        "s6 {label} n={} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} raw_us={:?}",
        sorted.len(),
        percentile(50),
        percentile(95),
        percentile(99),
        samples.iter().map(Duration::as_micros).collect::<Vec<_>>()
    );
}

fn report_alloc(label: &str, sample: Sample) {
    let (calls, bytes) = sample.allocations.unwrap();
    println!("s6 {label} rust_test_thread_alloc_calls={calls} rust_requested_bytes={bytes}");
}

struct Fixture {
    root: PathBuf,
    initial_hash: u64,
    targets: [EntityId; 2],
    parent: EntityId,
}

impl Fixture {
    fn write() -> Self {
        let mut world = World::new();
        let mut targets = Vec::new();
        let mut first_parent = None;
        for district in 0..DISTRICTS {
            let root = world.spawn((
                Name {
                    value: format!("district-{district:03}"),
                },
                Transform::default(),
            ));
            let parent = world.entity(root).unwrap();
            first_parent.get_or_insert(parent);
            for building in 1..=BUILDINGS {
                if district == DISTRICTS - 1 && building == BUILDINGS {
                    break; // 最后一个位置留给相机，场景总数仍为 50,000。
                }
                let name = Name {
                    value: format!("building-{district:03}-{building:03}"),
                };
                let parent = Parent { entity: parent };
                if building % 25 == 3 {
                    let column = (building - 1) / 25;
                    world.spawn((
                        name,
                        parent,
                        GlobalTransform {
                            position: [
                                column as f32 * 2.0 - 39.0,
                                district as f32 * 2.0 - 49.0,
                                0.0,
                            ],
                            ..Default::default()
                        },
                        MeshRenderer {
                            model: "builtin://cube".into(),
                        },
                    ));
                } else {
                    let key = world.spawn((name, parent, Transform::default()));
                    if district == 0 && building <= 2 {
                        targets.push(world.entity(key).unwrap());
                    }
                }
            }
        }
        world.spawn((
            Name {
                value: "editor-camera".into(),
            },
            Camera::default(),
            GlobalTransform {
                position: [0.0, 0.0, 105.0],
                ..Default::default()
            },
        ));
        assert_eq!(world.len(), ENTITIES);
        assert_eq!(world.count::<Parent>(), ENTITIES - DISTRICTS - 1);
        assert_eq!(world.count::<MeshRenderer>(), MESHES);

        let root = std::env::temp_dir()
            .join("xgc-s6")
            .join(format!("project-{}", std::process::id()));
        save_world(&root.join(SCENE), &world.world_data()).unwrap();
        save_project(
            &root.join("project.xgc"),
            &ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                name: "S6 Editor City".into(),
                next_entity_id: world.next_entity_id().raw(),
                entry_scene: SCENE.into(),
                scenes: vec![SCENE.into()],
                content: ProjectContent {
                    models: "Resources".into(),
                    textures: "Resources".into(),
                    audio: "Resources".into(),
                    shaders: None,
                    prefabs: "Prefabs".into(),
                    scripts: "Scripts".into(),
                },
                search_paths: Vec::new(),
                input: None,
                render: Default::default(),
                quality: Default::default(),
                layers: Default::default(),
            },
        )
        .unwrap();
        Self {
            root,
            initial_hash: world.state_hash(),
            targets: targets.try_into().unwrap(),
            parent: first_parent.unwrap(),
        }
    }
}

#[derive(Clone, Copy)]
struct UiSample {
    layout: Sample,
    tessellation: Sample,
}

impl UiSample {
    fn total(self) -> Duration {
        self.layout.elapsed + self.tessellation.elapsed
    }
}

struct CityUi {
    app: EditorApp,
    ctx: egui::Context,
}

impl CityUi {
    fn new(state: EditorState) -> Self {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        Self {
            app: EditorApp::new(state),
            ctx,
        }
    }

    fn frame(
        &mut self,
        events: Vec<egui::Event>,
        locate: Option<&str>,
        track: bool,
    ) -> (UiSample, Option<egui::Pos2>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN.into())),
            events,
            ..Default::default()
        };
        // 使用生产面板与宽度，视口渲染单独计时。
        let (output, layout) = measure(track, || {
            self.ctx.run(input, |ctx| {
                egui::TopBottomPanel::top("xgc-toolbar").show(ctx, |ui| self.app.toolbar(ui));
                egui::SidePanel::left("xgc-hierarchy")
                    .default_width(260.0)
                    .show(ctx, |ui| self.app.hierarchy(ui));
                egui::SidePanel::right("xgc-inspector")
                    .default_width(360.0)
                    .width_range(280.0..=460.0)
                    .show(ctx, |ui| {
                        ui.set_min_width(ui.available_width());
                        self.app.inspector(ui);
                    });
                egui::CentralPanel::default().show(ctx, |_| {});
            })
        });
        let position = locate.map(|text| text_position(&output, text));
        let (primitives, tessellation) = measure(track, || {
            self.ctx.tessellate(output.shapes, output.pixels_per_point)
        });
        assert!(primitives.iter().any(|item| matches!(&item.primitive,
            egui::epaint::Primitive::Mesh(mesh) if !mesh.indices.is_empty())));
        (
            UiSample {
                layout,
                tessellation,
            },
            position,
        )
    }

    fn locate(&mut self, text: &str) -> egui::Pos2 {
        self.frame(Vec::new(), Some(text), false).1.unwrap()
    }

    fn click(&mut self, pos: egui::Pos2, track: bool) -> UiSample {
        self.frame(pointer_button(pos, true), None, false);
        self.frame(pointer_button(pos, false), None, track).0
    }

    fn world(&self) -> &World {
        self.app.state.world()
    }

    fn round(
        &mut self,
        fixture: &Fixture,
        target: usize,
        undo: egui::Pos2,
        redo: egui::Pos2,
        track: bool,
    ) -> [UiSample; 4] {
        let entity = fixture.targets[target];
        let name = format!("building-000-{:03}", target + 1);
        let row = self.locate(&format!(
            "{}  {name}  (parent {})",
            entity.raw(),
            fixture.parent.raw()
        ));
        assert_ne!(self.app.state.selected(), Some(entity));
        let select = self.click(row, track);
        assert_eq!(self.app.state.selected(), Some(entity));

        let field = self.locate(&name);
        self.click(field, false);
        // 先将光标移到末尾，避免连续点击留下的选词改变输入语义。
        let end = |pressed| egui::Event::Key {
            key: egui::Key::End,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let edit = self
            .frame(
                vec![end(true), end(false), egui::Event::Text("X".into())],
                None,
                track,
            )
            .0;
        let key = self.world().key(entity).unwrap();
        let edited_name = &self.world().get::<Name>(key).unwrap().value;
        assert_eq!(edited_name, &format!("{name}X"));
        assert!(self.app.state.is_dirty() && self.app.state.can_undo());
        let edited_hash = self.world().state_hash();
        assert_ne!(edited_hash, fixture.initial_hash);

        let undo_frame = self.click(undo, track);
        assert_eq!(self.app.status.as_deref(), Some("已撤销"));
        assert_eq!(self.world().state_hash(), fixture.initial_hash);
        assert!(self.app.state.can_redo());
        let redo_frame = self.click(redo, track);
        assert_eq!(self.app.status.as_deref(), Some("已重做"));
        assert_eq!(self.world().state_hash(), edited_hash);
        self.click(undo, false);
        assert_eq!(self.world().state_hash(), fixture.initial_hash);
        assert!(!self.app.state.can_undo());
        assert_eq!(self.world().len(), ENTITIES);
        [select, edit, undo_frame, redo_frame]
    }
}

fn pointer_button(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

fn text_position(output: &egui::FullOutput, text: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| {
            let egui::epaint::Shape::Text(text_shape) = &shape.shape else {
                return None;
            };
            let center = text_shape.pos + text_shape.galley.size() * 0.5;
            (text_shape.galley.text() == text && shape.clip_rect.contains(center)).then_some(center)
        })
        .unwrap_or_else(|| panic!("找不到可见 egui 文本 {text:?}"))
}

fn report_ui(label: &str, samples: &[UiSample]) {
    let totals: Vec<_> = samples.iter().map(|sample| sample.total()).collect();
    let layouts: Vec<_> = samples.iter().map(|sample| sample.layout.elapsed).collect();
    let meshes: Vec<_> = samples
        .iter()
        .map(|sample| sample.tessellation.elapsed)
        .collect();
    report(&format!("{label}_cpu"), &totals);
    report(&format!("{label}_layout"), &layouts);
    report(&format!("{label}_tessellation"), &meshes);
}

fn benchmark_ui(ui: &mut CityUi, fixture: &Fixture) {
    let undo = ui.locate("Undo");
    let redo = ui.locate("Redo");
    ui.round(fixture, 0, undo, redo, false);
    let idle: Vec<_> = (0..IDLE_SAMPLES)
        .map(|_| ui.frame(Vec::new(), None, false).0)
        .collect();
    let idle_alloc = ui.frame(Vec::new(), None, true).0;
    report_ui("idle", &idle);
    report_alloc("idle_layout", idle_alloc.layout);
    report_alloc("idle_tessellation", idle_alloc.tessellation);

    let rounds: Vec<_> = (0..ACTION_SAMPLES)
        .map(|i| ui.round(fixture, (i + 1) % 2, undo, redo, false))
        .collect();
    let tracked = ui.round(fixture, (ACTION_SAMPLES + 1) % 2, undo, redo, true);
    for (i, label) in ["select", "inspector_edit", "undo", "redo"]
        .into_iter()
        .enumerate()
    {
        let series: Vec<_> = rounds.iter().map(|round| round[i]).collect();
        report_ui(label, &series);
        report_alloc(&format!("{label}_layout"), tracked[i].layout);
        report_alloc(&format!("{label}_tessellation"), tracked[i].tessellation);
    }
}

fn benchmark_viewport(ui: &CityUi) {
    let context = GpuContext::headless().expect("S6 视口需要实际 GPU 适配器");
    let adapter = context.adapter_info();
    println!(
        "s6 viewport_adapter={:?} backend={:?} device_type={:?} msaa={}",
        adapter.name,
        adapter.backend,
        adapter.device_type,
        context.scene_sample_count()
    );
    let [width, height] = VIEWPORT;
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("s6-editor-viewport"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut renderer = Renderer::new(ui.app.viewport_meshes());
    let manifest = ui.app.state.project.as_ref().unwrap().manifest();
    renderer.apply_project_settings(&context, manifest.render, manifest.quality);
    let render = |renderer: &mut Renderer| {
        renderer
            .render_selected(&context, &texture, ui.world(), ui.app.state.selected())
            .unwrap()
    };
    let check = |stats: &RenderStats| {
        assert_eq!(stats.draws, MESHES);
        assert!(stats.draw_calls > 0 && stats.passes >= 2, "{stats:?}");
    };
    let stats = render(&mut renderer);
    check(&stats);
    let pixels = context.read_texture_rgba8(&texture, width, height).unwrap();
    let background = &pixels[..4];
    let covered_pixels = pixels.chunks_exact(4).filter(|p| *p != background).count();
    assert!(
        covered_pixels >= MESHES,
        "视口几何覆盖像素过少：{covered_pixels}"
    );
    println!(
        "s6 viewport={}x{} covered_pixels={covered_pixels} stats={stats:?}",
        width, height
    );
    drop(pixels);
    for _ in 0..3 {
        check(&render(&mut renderer));
        let _ = context.device().poll(wgpu::Maintain::Wait);
    }
    let mut submit_times = Vec::with_capacity(VIEWPORT_SAMPLES);
    let mut completion_times = Vec::with_capacity(VIEWPORT_SAMPLES);
    for _ in 0..VIEWPORT_SAMPLES {
        let start = Instant::now();
        let stats = render(&mut renderer);
        submit_times.push(start.elapsed());
        let _ = context.device().poll(wgpu::Maintain::Wait);
        completion_times.push(start.elapsed());
        check(&stats);
    }
    report("viewport_cpu_submit", &submit_times);
    report(
        "viewport_submit_through_queue_completion_wall",
        &completion_times,
    );
    let (stats, tracked) = measure(true, || render(&mut renderer));
    check(&stats);
    let _ = context.device().poll(wgpu::Maintain::Wait);
    report_alloc("viewport_cpu_submit", tracked);
    println!("s6 viewport_gpu_timestamp=unavailable egui_gpu_compositor=unmeasured native_surface=unmeasured");
    report_process_memory("after_viewport");
}

#[cfg(windows)]
fn report_process_memory(stage: &str) {
    #[repr(C)]
    struct Counters {
        size: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        working_set: usize,
        peak_paged_pool: usize,
        paged_pool: usize,
        peak_non_paged_pool: usize,
        non_paged_pool: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        #[link_name = "GetCurrentProcess"]
        fn get_current_process() -> *mut std::ffi::c_void;
    }
    #[link(name = "psapi")]
    extern "system" {
        #[link_name = "GetProcessMemoryInfo"]
        fn get_process_memory_info(
            process: *mut std::ffi::c_void,
            counters: *mut Counters,
            size: u32,
        ) -> i32;
    }
    // SAFETY: 结构与 PROCESS_MEMORY_COUNTERS 同布局；伪句柄指向本进程，无需关闭。
    let mut counters: Counters = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<Counters>() as u32;
    counters.size = size;
    let result = unsafe { get_process_memory_info(get_current_process(), &mut counters, size) };
    assert_ne!(
        result,
        0,
        "GetProcessMemoryInfo: {}",
        std::io::Error::last_os_error()
    );
    println!("s6 {stage} process_working_set_bytes={} process_lifetime_peak_working_set_bytes={} process_lifetime_peak_commit_bytes={}",
        counters.working_set, counters.peak_working_set, counters.peak_pagefile_usage);
}

#[cfg(not(windows))]
fn report_process_memory(stage: &str) {
    println!("s6 {stage} process_working_set=unavailable");
}

#[test]
#[ignore = "S6 性能与真实 GPU 视口，使用 Release 和独占窗口运行"]
fn editor_city_50k() {
    if cfg!(debug_assertions) {
        panic!("请用 --release 运行 S6");
    }
    report_process_memory("start");
    let fixture = Fixture::write();
    let fixture_bytes = std::fs::metadata(fixture.root.join(SCENE)).unwrap().len();
    println!("s6 fixture={} entities={ENTITIES} parents={} mesh_entities={MESHES} scene_file_bytes={fixture_bytes} initial_hash={:016x}",
        fixture.root.display(), ENTITIES - DISTRICTS - 1, fixture.initial_hash);
    report_process_memory("after_fixture");

    // 生成和保存已结束。第一轮是刚写盘后的项目打开，不声称已经清空系统缓存。
    let mut opens = Vec::with_capacity(5);
    for _ in 0..5 {
        let (loaded, sample) = measure(false, || EditorState::open(&fixture.root).unwrap());
        assert_eq!(loaded.world().state_hash(), fixture.initial_hash);
        opens.push(sample.elapsed);
    }
    let (state, sample) = measure(true, || EditorState::open(&fixture.root).unwrap());
    assert_eq!(state.project_name(), Some("S6 Editor City"));
    assert_eq!(state.world().len(), ENTITIES);
    assert_eq!(state.world().state_hash(), fixture.initial_hash);
    report("project_open_post_write_and_warm_cache", &opens);
    report_alloc("project_open", sample);
    report_process_memory("after_load");

    let mut ui = CityUi::new(state);
    benchmark_ui(&mut ui, &fixture);
    report_process_memory("after_ui");
    benchmark_viewport(&ui);
    assert_eq!(ui.world().state_hash(), fixture.initial_hash);
    println!("s6 final_hash={:016x}", fixture.initial_hash);
}
