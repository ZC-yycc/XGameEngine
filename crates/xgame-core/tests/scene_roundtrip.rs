//! 写读稳定性门禁（属性测试）。
//!
//! **被测的不变量只有一条**：*保存一次之后，再保存不该产生 diff*。
//!
//! 为什么这一条值得一个独立门禁：场景 / 预制体 / 清单是**用户数据的唯一载体**，
//! 全 RON 纯文本，靠着"改了什么 diff 里就有什么"吃饭。只要加载边界悄悄改写了
//! 一个字段（夹取、归一化、缺省补齐、GUID 重新分配），下次保存就会产生一整片
//! 与用户操作无关的 diff —— 脏检查误报、评审噪音、冲突解决时改错行。
//!
//! 做法：用确定性 xorshift 生成**覆盖全部组件字段**的随机场景 / 预制体，
//! 然后
//!
//! ```text
//! t1 = write(scene)     l1 = load(t1)
//! t2 = write(l1)        l2 = load(t2)
//! t3 = write(l2)
//! ```
//!
//! 断言 `t1 == t2 == t3`（文本逐字节）且 `l1 == l2`（字段逐个）。
//! 生成器只产出**已经合法**的值（有限的浮点、单位四元数、锚点在 0..1），
//! 所以加载边界**不该**有任何东西要夹 —— 一旦有，就是这条测试要抓的东西。
//!
//! 失败可复现：种子写在断言信息里，把 `SEEDS` 改成那一个就能复跑。

use std::collections::BTreeMap;
use std::path::PathBuf;

use glam::{Quat, Vec3};
use xgame_core::prefab::PrefabInstanceRef;
use xgame_core::project::ProjectHandle;
use xgame_core::{
    Animator, AudioSource, BodyKind, Camera, CharacterController, CollisionShape, Entity,
    FieldValue, Light, LightKind, MaterialOverride, ParticleBlend, ParticleEmitter, PhysicsBody,
    Prefab, Scene, Transform, UiElement, UiKind,
};

// ---------------------------------------------------------------------------
// 确定性随机源
// ---------------------------------------------------------------------------

/// xorshift64\*：够随机、零依赖、**同种子同序列**（失败必须能复跑）。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // 0 会让 xorshift 永远停在 0。
        Self(seed.wrapping_mul(2_654_435_761) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// `[lo, hi]` 区间内的 f32（**有限的**：不产生 NaN / ∞）。
    fn f32(&mut self, lo: f32, hi: f32) -> f32 {
        let unit = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32;
        lo + (hi - lo) * unit
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    fn chance(&mut self, p: f32) -> bool {
        self.f32(0.0, 1.0) < p
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }

    /// 单位四元数：**先随机轴角再转四元数**，天然归一化。
    fn quat(&mut self) -> Quat {
        let axis = Vec3::new(self.f32(-1.0, 1.0), self.f32(-1.0, 1.0), self.f32(-1.0, 1.0));
        let axis = if axis.length_squared() < 1e-6 {
            Vec3::Y
        } else {
            axis.normalize()
        };
        Quat::from_axis_angle(axis, self.f32(-std::f32::consts::PI, std::f32::consts::PI))
    }

    /// 名字池里挑一个：**故意包含**引号、反斜杠、换行、中文与 emoji。
    ///
    /// 这些字符在 RON 里都要转义。转义不对称（写进去能读出来、读出来写不回去）
    /// 是最隐蔽的一类数据损伤 —— 表现是"改个名字，diff 里多出十行"。
    fn name(&mut self) -> String {
        const POOL: &[&str] = &[
            "Ground",
            "Wall 2",
            "木箱 A",
            "带\"引号\"的名字",
            "反\\斜杠",
            "换\n行",
            "emoji 🚀",
            "tab\there",
            "",
            " 前后空格 ",
            "很长很长很长很长很长很长很长很长很长很长很长很长很长很长很长的名字",
        ];
        POOL[self.below(POOL.len())].to_string()
    }
}

// ---------------------------------------------------------------------------
// 生成器
// ---------------------------------------------------------------------------

/// 项目里真实存在的资产（写盘时会被编成 GUID、读回时再解回路径）。
const MODELS: &[&str] = &["Resources/Models/Box.glb", "Resources/Models/Two.glb"];
const SCRIPTS: &[&str] = &["Scripts/Player.lua"];
const AUDIO: &[&str] = &["Resources/Audio/Beep.wav"];
const MATERIALS: &[&str] = &["Resources/Materials/Wood.mat"];
const PREFABS: &[&str] = &["Prefabs/Enemy.prefab.ron"];

fn random_transform(rng: &mut Rng) -> Transform {
    Transform {
        position: Vec3::new(rng.f32(-50.0, 50.0), rng.f32(-50.0, 50.0), rng.f32(-50.0, 50.0)),
        rotation: rng.quat(),
        scale: Vec3::new(rng.f32(0.01, 4.0), rng.f32(0.01, 4.0), rng.f32(0.01, 4.0)),
    }
}

fn random_physics(rng: &mut Rng) -> PhysicsBody {
    let shape = match rng.below(5) {
        0 => CollisionShape::Auto,
        1 => CollisionShape::Cuboid {
            half_extents: [rng.f32(0.1, 2.0), rng.f32(0.1, 2.0), rng.f32(0.1, 2.0)],
        },
        2 => CollisionShape::Ball {
            radius: rng.f32(0.05, 1.5),
        },
        3 => CollisionShape::Capsule {
            half_height: rng.f32(0.1, 1.0),
            radius: rng.f32(0.05, 0.5),
        },
        _ => CollisionShape::Cylinder {
            half_height: rng.f32(0.1, 1.0),
            radius: rng.f32(0.05, 0.5),
        },
    };
    PhysicsBody {
        kind: rng.pick(&[BodyKind::Static, BodyKind::Dynamic, BodyKind::Kinematic]),
        shape,
        mass: rng.f32(0.0, 20.0),
        friction: rng.f32(0.0, 1.0),
        restitution: rng.f32(0.0, 1.0),
        gravity_scale: rng.f32(-2.0, 2.0),
        is_sensor: rng.chance(0.2),
    }
}

fn random_ui(rng: &mut Rng) -> UiElement {
    let kind = rng.pick(&[
        UiKind::Panel,
        UiKind::Text,
        UiKind::Image,
        UiKind::Button,
    ]);
    let mut ui = UiElement {
        kind,
        // 锚点严格落在 0..1：越界的锚点是**合法**的（溢出用），但加载边界会夹取，
        // 那是"修数据"而不是"丢数据"，不属于本门禁要盯的 diff。
        anchor_min: [rng.f32(0.0, 0.9), rng.f32(0.0, 0.9)],
        anchor_max: [rng.f32(0.1, 1.0), rng.f32(0.1, 1.0)],
        offset_min: [rng.f32(-100.0, 100.0), rng.f32(-100.0, 100.0)],
        offset_max: [rng.f32(-100.0, 100.0), rng.f32(-100.0, 100.0)],
        ..Default::default()
    };
    if rng.chance(0.5) {
        ui.image = Some(xgame_core::UiImageStyle {
            path: "Resources/Textures/Icon.png".to_string(),
            ..Default::default()
        });
    }
    ui
}

fn random_particle(rng: &mut Rng) -> ParticleEmitter {
    ParticleEmitter {
        emitting: rng.chance(0.8),
        rate: rng.f32(0.0, 200.0),
        max_particles: rng.below(2000) as u32,
        duration: rng.f32(0.0, 10.0),
        looping: rng.chance(0.5),
        lifetime: [rng.f32(0.1, 1.0), rng.f32(1.0, 5.0)],
        speed: [rng.f32(0.0, 2.0), rng.f32(2.0, 10.0)],
        direction: [rng.f32(-1.0, 1.0), rng.f32(-1.0, 1.0), rng.f32(-1.0, 1.0)],
        spread: rng.f32(0.0, 180.0),
        gravity: [0.0, rng.f32(-9.81, 0.0), 0.0],
        drag: rng.f32(0.0, 1.0),
        start_size: rng.f32(0.01, 0.5),
        end_size: rng.f32(0.0, 0.5),
        start_color: [1.0, rng.f32(0.0, 1.0), rng.f32(0.0, 1.0), 1.0],
        end_color: [1.0, 0.0, 0.0, rng.f32(0.0, 1.0)],
        blend: rng.pick(&[ParticleBlend::Alpha, ParticleBlend::Additive]),
        seed: rng.next_u64(),
        ..Default::default()
    }
}

fn random_entity(rng: &mut Rng) -> Entity {
    let mut entity = Entity::new(rng.name());
    entity.transform = random_transform(rng);
    entity.layer = rng.below(32) as u8;
    if rng.chance(0.6) {
        entity.model = rng.pick(MODELS).to_string();
    } else if rng.chance(0.3) {
        // 内置几何不是文件，落盘要原样保留（不能变成 guid://）。
        entity.model = rng
            .pick(&["builtin://cube", "builtin://sphere", "builtin://plane"])
            .to_string();
    }
    if rng.chance(0.3) {
        entity.material_asset = Some(rng.pick(MATERIALS).to_string());
    }
    if rng.chance(0.3) {
        // LOD：各级模型也是资产引用，必须与 `model` 走同一条 GUID 管线；
        // 无限级（0）与有限级都要覆盖，否则"最后一级写 0"的写法没进过盘。
        let mut levels = Vec::new();
        for index in 0..rng.below(3) + 1 {
            levels.push(xgame_core::LodLevel::new(
                rng.pick(MODELS).to_string(),
                if index == 0 && rng.chance(0.5) {
                    0.0
                } else {
                    rng.f32(1.0, 200.0)
                },
            ));
        }
        entity.lod = xgame_core::LodGroup { levels };
    }
    if rng.chance(0.3) {
        entity.scripts.push(rng.pick(SCRIPTS).to_string());
    }
    if rng.chance(0.2) {
        entity.behaviors.push("Spin".to_string());
        let mut params: BTreeMap<String, FieldValue> = BTreeMap::new();
        params.insert("speed".into(), FieldValue::F32(rng.f32(-5.0, 5.0)));
        params.insert("enabled".into(), FieldValue::Bool(rng.chance(0.5)));
        params.insert("label".into(), FieldValue::String(rng.name()));
        entity.behavior_params.insert("Spin".into(), params);
    }
    if rng.chance(0.4) {
        entity.physics = Some(random_physics(rng));
    }
    if rng.chance(0.3) {
        entity.animator = Some(Animator {
            clip: "Walk".into(),
            speed: rng.f32(0.1, 3.0),
            looping: rng.chance(0.5),
            autoplay: rng.chance(0.5),
            fade_time: rng.f32(0.0, 1.0),
            ..Default::default()
        });
    }
    if rng.chance(0.2) {
        entity.morph_weights = vec![rng.f32(0.0, 1.0), rng.f32(0.0, 1.0)];
    }
    if rng.chance(0.3) {
        entity.audio = Some(AudioSource {
            clip: rng.pick(AUDIO).to_string(),
            volume: rng.f32(0.0, 1.0),
            pitch: rng.f32(0.5, 2.0),
            looping: rng.chance(0.5),
            autoplay: rng.chance(0.5),
            spatial: rng.chance(0.5),
            min_distance: rng.f32(0.1, 3.0),
            max_distance: rng.f32(3.0, 60.0),
            // 总线三种都要进过盘（默认组不落盘、非默认组落盘）。
            bus: rng.pick(&[
                xgame_core::AudioBus::Sfx,
                xgame_core::AudioBus::Bgm,
                xgame_core::AudioBus::Voice,
            ]),
        });
    }
    if rng.chance(0.3) {
        entity.material = Some(MaterialOverride {
            base_color: Some([rng.f32(0.0, 1.0), rng.f32(0.0, 1.0), rng.f32(0.0, 1.0), 1.0]),
            metallic: Some(rng.f32(0.0, 1.0)),
            roughness: Some(rng.f32(0.0, 1.0)),
        });
    }
    if rng.chance(0.2) {
        entity.controller = Some(CharacterController {
            radius: rng.f32(0.1, 1.0),
            height: rng.f32(0.5, 3.0),
            ..Default::default()
        });
    }
    if rng.chance(0.25) {
        entity.ui = Some(random_ui(rng));
    }
    if rng.chance(0.2) {
        entity.particle = Some(random_particle(rng));
    }
    if rng.chance(0.2) {
        entity.camera = Some(Camera {
            fov: rng.f32(30.0, 110.0),
            near: rng.f32(0.01, 0.5),
            far: rng.f32(100.0, 2000.0),
            active: rng.chance(0.3),
        });
    }
    if rng.chance(0.2) {
        entity.prefab = Some(PrefabInstanceRef {
            source: rng.pick(PREFABS).to_string(),
            source_id: xgame_core::EntityId::new(),
            is_root: rng.chance(0.5),
        });
    }
    entity
}

fn random_scene(rng: &mut Rng, name: &str) -> Scene {
    let count = 1 + rng.below(12);
    let mut entities: Vec<Entity> = (0..count).map(|_| random_entity(rng)).collect();
    // 随机挂父节点：**只挂到更靠前的实体上**，天然无环（自指 / 成环是坏数据）。
    for index in 1..entities.len() {
        if rng.chance(0.4) {
            let parent = entities[rng.below(index)].id;
            entities[index].parent = Some(parent);
        }
    }
    let mut lights = Vec::new();
    for i in 0..rng.below(4) {
        lights.push(Light {
            name: format!("Light {i}"),
            kind: rng.pick(&[LightKind::Directional, LightKind::Point]),
            direction: [rng.f32(-1.0, 1.0), rng.f32(-1.0, 0.0), rng.f32(-1.0, 1.0)],
            position: [rng.f32(-10.0, 10.0), rng.f32(0.0, 10.0), rng.f32(-10.0, 10.0)],
            color: [rng.f32(0.5, 1.0), rng.f32(0.5, 1.0), rng.f32(0.5, 1.0)],
            intensity: rng.f32(0.1, 8.0),
            range: rng.f32(1.0, 50.0),
            casts_shadow: rng.chance(0.5),
            shadow_resolution: 0,
        });
    }
    Scene {
        name: name.to_string(),
        entities,
        lights,
        ambient: [rng.f32(0.0, 0.3), rng.f32(0.0, 0.3), rng.f32(0.0, 0.3)],
    }
}

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn temp_project(tag: &str) -> (ProjectHandle, PathBuf) {
    let base = std::env::temp_dir().join(format!(
        "xgame-roundtrip-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let project = ProjectHandle::create(&base, "RoundTrip").expect("创建临时项目");

    // 真实资产：引用只有存进资产库才会被编成 GUID，那是本轮最想覆盖的一段。
    for (relative, bytes) in [
        ("Resources/Models/Box.glb", b"glb".as_slice()),
        ("Resources/Models/Two.glb", b"glb".as_slice()),
        ("Resources/Textures/Icon.png", b"png".as_slice()),
        ("Resources/Audio/Beep.wav", b"wav".as_slice()),
        ("Scripts/Player.lua", b"-- lua".as_slice()),
        ("Resources/Materials/Wood.mat", b"(base_color:(1,1,1,1))".as_slice()),
        ("Prefabs/Enemy.prefab.ron", b"(name:\"Enemy\",entities:[])".as_slice()),
    ] {
        let path = project.root().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    (project, base)
}

fn read(project: &ProjectHandle, relative: &str) -> String {
    std::fs::read_to_string(project.path(relative)).expect("读盘")
}

const SEEDS: u64 = 40;

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

/// 随机场景：写 → 读 → 写 → 读 → 写，文本必须逐字节稳定，字段必须逐个相等。
///
/// 三个时刻复用**同一个文件**：`write_scene` 每次都会重扫资产库，写一堆新文件
/// 会让扫描量随用例数平方增长（实测 40 个种子 40 秒，全花在扫描上）。
#[test]
fn random_scenes_survive_save_load_save() {
    let (project, base) = temp_project("scene");
    let relative = "Scenes/roundtrip.ron";
    for seed in 1..=SEEDS {
        let mut rng = Rng::new(seed);
        let scene = random_scene(&mut rng, &format!("S{seed}"));

        project.write_scene(relative, &scene).expect("写场景");
        let t1 = read(&project, relative);
        let l1 = project.load_scene(relative).expect("读回场景");

        project.write_scene(relative, &l1).expect("再写");
        let t2 = read(&project, relative);
        let l2 = project.load_scene(relative).expect("再读回场景");

        project.write_scene(relative, &l2).expect("第三次写");
        let t3 = read(&project, relative);

        assert_eq!(t1, t2, "seed={seed}: 读回再写改动了文本");
        assert_eq!(t2, t3, "seed={seed}: 第二轮仍在变");
        assert!(l1 == l2, "seed={seed}: 读回再写再读，字段变了");
        // GUID 编码必须真的发生了（否则这条测试覆盖不到资产引用）。
        if scene
            .entities
            .iter()
            .any(|e| !e.model.is_empty() && !e.model.starts_with("builtin://"))
        {
            assert!(
                t1.contains("guid://"),
                "seed={seed}: 引用真实资产的场景落盘必须是 GUID:\n{t1}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// 随机预制体：同一套往返断言（预制体与场景共用引用 walk，但序列化路径不同）。
#[test]
fn random_prefabs_survive_save_load_save() {
    let (project, base) = temp_project("prefab");
    let relative = "Prefabs/roundtrip.prefab.ron";
    for seed in 1..=SEEDS {
        let mut rng = Rng::new(seed ^ 0xA5A5);
        let scene = random_scene(&mut rng, &format!("P{seed}"));
        let prefab = Prefab {
            name: scene.name.clone(),
            entities: scene.entities,
        };

        project.write_prefab(relative, &prefab).expect("写预制体");
        let t1 = read(&project, relative);
        let l1 = project.load_prefab(relative).expect("读回预制体");

        project.write_prefab(relative, &l1).expect("再写预制体");
        let t2 = read(&project, relative);

        assert_eq!(t1, t2, "seed={seed}: 预制体读回再写改动了文本");
        assert!(
            l1.entities == prefab.entities,
            "seed={seed}: 预制体字段往返不一致"
        );
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// **坏数据**也要能装进来，而且"修一次之后就稳定"。
///
/// 这一条盯的不是"数据对不对"（那是 `--check` 的活），而是"加载边界会不会
/// 卡死 / 崩 / 越修越乱"：
///
/// - 重复实体 ID：`ensure_unique_ids` 会重分配（同文件引用，损失有界），
///   所以 `t1 != t2` 是**允许**的；
/// - 悬空父节点 / 自指 / 成环：只报告，不修 —— `hierarchy()` 不许因此死循环；
/// - 越界 UI / 粒子数值：`sanitize_*` 夹一次；
/// - 空名字、超长名字、极端浮点：原样保留。
///
/// 断言只有两条：**装得进来**（不 panic、不死循环），**第二轮不再变**。
#[test]
fn hostile_scenes_load_and_settle() {
    let (project, base) = temp_project("hostile");
    for seed in 1..=SEEDS {
        let mut rng = Rng::new(seed ^ 0xDEAD_BEEF);
        let mut scene = random_scene(&mut rng, &format!("H{seed}"));

        // 重复 ID：把后面一半实体全部改成第一个的 ID。
        if scene.entities.len() > 2 {
            let first = scene.entities[0].id;
            for entity in scene.entities.iter_mut().skip(1) {
                if rng.chance(0.5) {
                    entity.id = first;
                }
            }
        }
        // 悬空父节点 / 自指 / 成环（三者都不是"能自动修好"的东西）。
        for index in 0..scene.entities.len() {
            match rng.below(4) {
                0 => scene.entities[index].parent = Some(xgame_core::EntityId::new()),
                1 => scene.entities[index].parent = Some(scene.entities[index].id),
                2 if scene.entities.len() > 2 => {
                    let next = (index + 1) % scene.entities.len();
                    scene.entities[index].parent = Some(scene.entities[next].id);
                }
                _ => {}
            }
        }
        // 越界的 UI / 粒子 / morph：加载边界夹一次，之后必须稳定。
        for entity in scene.entities.iter_mut() {
            if let Some(ui) = entity.ui.as_mut() {
                ui.anchor_min = [rng.f32(-10.0, 10.0), rng.f32(-10.0, 10.0)];
                ui.offset_max = [rng.f32(-1e6, 1e6), rng.f32(-1e6, 1e6)];
            }
            if let Some(particle) = entity.particle.as_mut() {
                particle.max_particles = u32::MAX;
                particle.spread = rng.f32(-720.0, 720.0);
            }
            if rng.chance(0.3) {
                entity.morph_weights = vec![rng.f32(-4.0, 4.0), rng.f32(-4.0, 4.0)];
            }
        }

        let first = "Scenes/hostile.ron".to_string();
        project.write_scene(&first, &scene).expect("坏数据也要写得下去");
        let loaded = project
            .load_scene(&first)
            .expect("坏数据必须装得进来（不 panic）");

        project.write_scene(&first, &loaded).expect("写第二份");
        let settled = project.load_scene(&first).expect("再读回来");
        let second = read(&project, &first);
        project.write_scene(&first, &settled).expect("写第三份");
        let third = read(&project, &first);

        assert_eq!(
            second, third,
            "seed={seed}: 夹取之后仍在变 —— 加载边界的修正是**非幂等**的"
        );

        // 修完之后的数据必须能过一遍"层级遍历"而不死循环（成环时靠 visited 集兜住）。
        let _ = settled.hierarchy();
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// **真实内容**也要满足同一条不变量：仓库里每个示例工程的场景，
/// 打开 → 保存一次 → 再保存一次，**第二次起文本必须完全不变**。
///
/// 与随机用例的分工：随机用例覆盖字段组合，这一条覆盖**已入库的样例资产**
/// （手写注释、GUID 引用、预制体实例）。
///
/// **第一次保存会掉注释，这是刻意的**，所以这里断言的不是"第一次逐字节相同"：
/// 场景是 RON，保存走的是"内存模型重新序列化"，注释不在模型里 —— 想保住注释
/// 只有 `--migrate-assets` 那条**词法替换**的路（见决策记录 §十四）。第一次保存
/// 之后场景变成纯数据，此后必须是不动点；否则就是"打开别人的工程再存一次"
/// 会凭空产生 diff，那是协作里最招骂的一种提交。
#[test]
fn example_projects_stabilise_after_one_save() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("仓库根")
        .to_path_buf();
    let projects = repo.join("projects");
    if !projects.is_dir() {
        return; // 打包成独立 crate 时没有示例工程，跳过。
    }

    let base = std::env::temp_dir().join(format!("xgame-example-roundtrip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    let mut checked = 0usize;
    for entry in std::fs::read_dir(&projects).unwrap() {
        let source = entry.unwrap().path();
        if !source.join("project.ron").is_file() {
            continue;
        }
        let name = source.file_name().unwrap().to_string_lossy().into_owned();
        let target = base.join(&name);
        copy_dir(&source, &target);
        let project = ProjectHandle::open(&target).expect("拷贝出来的工程必须能打开");

        for relative in project.list_scenes() {
            let scene = project.load_scene(&relative).expect("示例场景必须能装");

            project.write_scene(&relative, &scene).expect("示例场景必须能写");
            let first = std::fs::read_to_string(project.path(&relative)).unwrap();

            let reloaded = project.load_scene(&relative).expect("写完必须还能装");
            project.write_scene(&relative, &reloaded).expect("再写一次");
            let second = std::fs::read_to_string(project.path(&relative)).unwrap();

            assert_same_text(&first, &second, &format!("{name}/{relative}"));
            // 掉注释可以（RON 重新序列化的必然），**掉数据不行**。
            assert!(
                reloaded == scene,
                "{name}/{relative}: 保存一次之后字段变了（不是同一份数据）"
            );
            checked += 1;
        }
    }
    assert!(checked >= 4, "示例工程至少要覆盖 4 张场景，实际 {checked}");

    let _ = std::fs::remove_dir_all(&base);
}

/// 逐行指出第一处不同（**只报一处**）：整段 `left/right` 在几十行的场景上没法看，
/// 而"哪一行、原来什么、现在什么"足够定位。
fn assert_same_text(before: &str, after: &str, label: &str) {
    if before == after {
        return;
    }
    let mut detail = String::from("(长度一致但内容不同)");
    for (index, (a, b)) in before.lines().zip(after.lines()).enumerate() {
        if a != b {
            detail = format!("第 {} 行:\n  保存前: {a}\n  保存后: {b}", index + 1);
            break;
        }
    }
    if before.lines().count() != after.lines().count() {
        detail = format!(
            "(行数 {} → {}) {detail}",
            before.lines().count(),
            after.lines().count()
        );
    }
    panic!("{label}: 打开再保存产生了 diff —— 用户什么都没改\n{detail}");
}

/// 递归复制目录（只复制文件，链接按文件读 —— 示例工程里没有链接）。
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        // `Build/` 是产物、可能有几百 MB；`.git` 不是工程内容。
        let name = entry.file_name();
        if name == "Build" || name == ".git" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// 内容一个字节都没变时**不动盘**。
///
/// 为什么值得一条测试：编辑器的热重载扫描按 `mtime` 判断"有没有外部改动"，
/// 一次没有实际变化的保存如果重写了文件，就等于凭空触发一次重载；顺带还省掉
/// 暂存文件 + rename 那一套（在被占用的文件上，这一步会直接失败）。
#[test]
fn unchanged_save_does_not_touch_the_file() {
    let (project, base) = temp_project("nochange");
    let relative = "Scenes/stable.ron";
    let scene = xgame_core::project::default_scene();
    project.write_scene(relative, &scene).expect("首次写盘");
    let path = project.path(relative);
    let first = std::fs::metadata(&path).unwrap().modified().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    project
        .write_scene(relative, &scene)
        .expect("内容没变也必须保存成功");
    let second = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(first, second, "内容没变却重写了文件（mtime 变了）");

    // 负对照：真的改了内容就必须写下去（否则"保存"变成了空操作）。
    let mut changed = scene;
    changed.name = "Changed".into();
    std::thread::sleep(std::time::Duration::from_millis(20));
    project.write_scene(relative, &changed).expect("改过的内容要写盘");
    let third = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_ne!(second, third, "内容变了却没写盘");

    let _ = std::fs::remove_dir_all(&base);
}

/// 清单：`save()` 两次必须逐字节相同（清单里全是手改字段，抖动最招人烦）。
#[test]
fn manifest_save_is_byte_stable() {
    let (project, base) = temp_project("manifest");
    let manifest_path = project.root().join("project.ron");

    // 先写一次"默认 + 一堆非默认值"，再走一遍 open → save。
    let mut handle = project;
    {
        let manifest = handle.manifest_mut();
        manifest.name = "带\"引号\"的项目\\名".into();
        manifest.entry_scene = "Scenes/scene.ron".into();
        manifest.build.scenes = vec!["Scenes/scene.ron".into(), "Scenes/b.ron".into()];
        handle.save().expect("写清单");
    }
    let t1 = std::fs::read_to_string(&manifest_path).unwrap();
    let reopened = ProjectHandle::open(handle.root()).expect("重开项目");
    reopened.save().expect("再写清单");
    let t2 = std::fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(t1, t2, "清单保存两次必须逐字节相同");

    let _ = std::fs::remove_dir_all(&base);
}
