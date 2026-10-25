//! `#[derive(Inspect)]` 的回归测试（双轨架构 P1）。
//!
//! # 这套测试在守什么
//!
//! 手写 `Inspect` 最容易出的错是**三处下标错位** —— 表、`inspect_get`、
//! `inspect_set` 各写一份下标，对不上时表现是"改 A 字段动了 B 字段"，
//! 而两边都是 `f32`/`bool`，编译器一声不吭。derive 存在的意义就是让这种
//! 错误**结构上不可能发生**；本文件则守着"derive 本身有没有坏"：
//! 表的元数据、下标对齐、属性解析、skip 的重编号。
//!
//! 为什么放在 `crates/xgame/tests/`（而不是 xgame-derive 里）：
//! 生成的代码引用 `::xgame::inspect::*` —— 只有"以**外部 crate** 身份使用
//! `xgame`"的地方才解析得到（xgame-derive 自己是 proc-macro crate，xgame 的
//! `src/` 内部也不行）。集成测试恰好就是外部 crate。
//!
//! # 没覆盖的
//!
//! 「不支持的类型不标 `skip` ⇒ 编译期报错」需要 `trybuild` 才能断言，
//! 本仓库暂未引入 —— 靠报错文案本身足够指向性（会点名字段和类型）。

use xgame::prelude::*;

/// 覆盖全部支持类型 + 一个被 `skip` 的字段 + `label` 覆盖。
#[derive(Default, Inspect)]
struct Tunable {
    /// `label` 覆盖显示名，但 `name` **仍是 `speed`** —— 寻址名与显示名分离，
    /// 这样重命名显示名不会破坏已有脚本。
    #[inspect(
        min = -720.0,
        max = 720.0,
        speed = 5.0,
        label = "Speed",
        unit = "°/s",
        tooltip = "角速度"
    )]
    speed: f32,
    /// 只给 `step`：没有 `min/max` ⇒ 编辑器画裸拖拽值而不是滑块。
    #[inspect(step = 0.5)]
    offset: f64,
    /// 不标 `skip` 的话它类型不支持，derive 会**编译期报错**。
    #[inspect(skip)]
    internal: f64,
    name: String,
    alive: bool,
    count: u32,
    seed: i64,
    id: i32,
    pos: glam::Vec3,
    dir: glam::Vec2,
    tint: glam::Vec4,
}

/// 一个没有可调字段的组件（单元结构体）。
#[derive(Default, Inspect)]
struct Bare;

/// 字段全被 `skip` 掉 —— 与单元结构体走同一条空表路径。
///
/// 字段本身**永远不被读**：这个类型的全部作用就是证明"全 skip ⇒ 空表"，
/// 所以字段是刻意无用的（它们若被读，反而说明测试在测别的东西）。
#[allow(dead_code)]
#[derive(Default, Inspect)]
struct AllSkipped {
    #[inspect(skip)]
    a: f32,
    #[inspect(skip)]
    b: String,
}

/// 泛型结构体：`split_for_impl` 生成的 impl 头不能丢 `<>`。
#[derive(Default, Inspect)]
struct Generic<T> {
    speed: f32,
    /// 不支持的类型 + `skip` 同时出现 —— skip 判定必须**先于**类型检查，
    /// 否则 `PhantomData<T>` 会先把编译器顶回错误。
    #[inspect(skip)]
    _marker: std::marker::PhantomData<T>,
}

/// 每种 `FieldKind` 一个**能被认出来**的值。
///
/// 数值刻意不取 0（0 是所有类型的零值，错位时读回来也是 0，测不出来）；
/// 字符串与向量同理。
fn sample(kind: FieldKind) -> FieldValue {
    match kind {
        FieldKind::F32 => FieldValue::F32(1.5),
        FieldKind::F64 => FieldValue::F64(2.5),
        FieldKind::I32 => FieldValue::I32(-7),
        FieldKind::I64 => FieldValue::I64(-900_000_000_000),
        FieldKind::U32 => FieldValue::U32(42),
        FieldKind::Bool => FieldValue::Bool(true),
        FieldKind::String => FieldValue::String("written".into()),
        FieldKind::Vec2 => FieldValue::Vec2(glam::Vec2::new(1.0, 2.0)),
        FieldKind::Vec3 => FieldValue::Vec3(glam::Vec3::new(1.0, 2.0, 3.0)),
        FieldKind::Vec4 => FieldValue::Vec4(glam::Vec4::new(1.0, 2.0, 3.0, 4.0)),
    }
}

#[test]
fn derive_builds_the_declared_table() {
    let t = Tunable::default();
    let table = t.inspect_fields();

    // 11 个字段，`internal` 被 skip ⇒ 剩 10 个。
    assert_eq!(table.len(), 10, "skip 的字段不该进表");
    assert!(
        table.iter().all(|f| f.name != "internal"),
        "`#[inspect(skip)]` 必须把字段挡在外面 —— 否则用户会改到一个被内部"
    );

    let speed = &table[0];
    assert_eq!(
        speed.name, "speed",
        "寻址名永远是 Rust 字段名，不随 label 变"
    );
    assert_eq!(speed.kind, FieldKind::F32);
    assert_eq!(speed.label, Some("Speed"), "`label` 只改显示名，不改寻址名");
    assert_eq!(
        speed.min,
        Some(-720.0),
        "负号必须被认出来（`syn::Lit` 不含符号）"
    );
    assert_eq!(speed.max, Some(720.0));
    assert_eq!(speed.speed, Some(5.0));
    assert_eq!(speed.unit, "°/s");
    assert_eq!(speed.tooltip, "角速度");
    assert!(speed.has_range(), "min + max 齐全 ⇒ 编辑器画滑块");

    // 没写 `#[inspect(...)]` 的字段：全是默认值。
    let name = table
        .iter()
        .find(|f| f.name == "name")
        .expect("name 应当在表里");
    assert_eq!(name.kind, FieldKind::String);
    assert_eq!(name.label, None);
    assert_eq!(name.min, None);
    assert_eq!(name.max, None);
    assert_eq!(name.speed, None);
    assert_eq!(name.step, None);
    assert_eq!(name.unit, "");
    assert_eq!(name.tooltip, "");
    assert!(
        !name.has_range(),
        "没声明边界 ⇒ 编辑器给裸拖拽值，不强加范围"
    );

    // 每种类型都要出现，否则"支持 N 种类型"这句话是空头支票。
    let kinds: Vec<FieldKind> = table.iter().map(|f| f.kind).collect();
    for k in [
        FieldKind::F32,
        FieldKind::F64,
        FieldKind::I32,
        FieldKind::I64,
        FieldKind::U32,
        FieldKind::Bool,
        FieldKind::String,
        FieldKind::Vec3,
        FieldKind::Vec2,
        FieldKind::Vec4,
    ] {
        assert!(kinds.contains(&k), "表里缺少 {k:?} 类型的字段");
    }
}

/// **本文件的核心断言**：表、`inspect_get`、`inspect_set` 三处下标必须对齐。
///
/// 做法是"把每个字段写成一个可辨认的值，再逐个读回来比对"。错位一格就会读到
/// 邻居的值，而样本值两两不同 ⇒ 必红。（只写不读测不出来，只读不写也测不出来：
/// 得两个方向都走一遍。）
#[test]
fn get_and_set_agree_on_every_index() {
    let mut t = Tunable::default();
    let table = t.inspect_fields().to_vec();

    // 先全写一遍。
    for (i, desc) in table.iter().enumerate() {
        t.inspect_set(i, sample(desc.kind));
    }
    // `internal` 被 skip ⇒ 不在表里 ⇒ 没有任何下标能落到它。上面刚写完 10 个
    // 字段，这一行同时证明"skip 真的把它挡住了"和"写入不会溢到邻居身上"。
    assert_eq!(
        t.internal, 0.0,
        "`#[inspect(skip)]` 的字段不该被任何 inspect_set 写到"
    );
    // 再全读一遍 —— 此时若下标错位，读到的会是别的字段的样本值。
    for (i, desc) in table.iter().enumerate() {
        let got = t.inspect_get(i);
        let want = sample(desc.kind);
        assert_eq!(
            got, want,
            "第 {i} 个字段 `{}` 下标错位：期望 {want:?}，读到 {got:?}",
            desc.name
        );
    }

    // `inspect_field` 是编辑器每帧走的那条路，它也必须给同一批数据。
    for (i, desc) in table.iter().enumerate() {
        let (d, v) = t.inspect_field(i).expect("下标在表内");
        assert_eq!(d.name, desc.name, "inspect_field 的描述与表不一致");
        assert_eq!(
            v,
            sample(desc.kind),
            "inspect_field 的值与 inspect_get 不一致"
        );
    }
}

#[test]
fn mismatched_write_is_silently_ignored() {
    let mut t = Tunable::default();
    // `speed` 是 f32，塞 bool 进去 —— Lua 侧传错类型的常见后果。
    t.inspect_set(0, FieldValue::Bool(true));
    t.inspect_set(0, FieldValue::String("fast".into()));
    assert_eq!(
        t.inspect_get(0),
        FieldValue::F32(0.0),
        "类型不符的写入必须被忽略：静默写进去一个错类型比拒绝更糟"
    );
}

#[test]
fn out_of_range_reads_return_the_type_zero() {
    let t = Tunable::default();
    // 检视是旁路 —— 下标算错不该拖垮宿主进程。
    assert_eq!(t.inspect_get(999), FieldValue::F32(0.0));
    assert!(t.inspect_field(999).is_none());
    assert_eq!(t.inspect_fields().len(), 10, "越界读不能改动表本身");
}

#[test]
fn empty_tables_are_still_well_formed() {
    let mut bare = Bare;
    assert!(bare.inspect_fields().is_empty(), "单元结构体 ⇒ 空表");
    // 空表上的读写一律安静返回，不 panic。
    bare.inspect_set(0, FieldValue::Bool(true));
    assert_eq!(bare.inspect_get(0), FieldValue::F32(0.0));
    assert!(bare.inspect_field(0).is_none());

    let all = AllSkipped::default();
    assert!(
        all.inspect_fields().is_empty(),
        "字段全被 skip ⇒ 与单元结构体同一条空表路径"
    );
    // 编译能过本身就是断言：`PhantomData` 之类的非支持类型必须在 skip 之后才被检查。
    let g = Generic::<f64>::default();
    assert_eq!(g.inspect_fields().len(), 1);
    assert_eq!(g.inspect_fields()[0].name, "speed");
}
