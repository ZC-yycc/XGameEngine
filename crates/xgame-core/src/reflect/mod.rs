//! 反射：类型元数据 + 借用式字段访问 + 路径寻址。
//!
//! # 这是什么，不是什么
//!
//! 这是**编辑与查询**用的基础设施：让内置组件（`Entity` 的具名字段）与插件组件
//! （`behaviors` 注册的类型）能用同一套接口枚举字段、读、写。
//!
//! 它**不参与落盘**。磁盘格式仍由 `scene_v2::ComponentData`（内置）与
//! `inspect::FieldValues`（插件参数）定义；反射有自己的值类型，不去扩
//! `FieldValue`（后者的变体名是场景文件的一部分，扩它就是改契约）。
//!
//! 它也**不进运行时热路径**。字符串比较 + 动态分发只允许出现在编辑器、MCP、
//! `--check`、以及脚本的字段访问里。`World::update` 的每帧函数不得调用本模块。
//!
//! # 分层
//!
//! ```text
//! meta    —— 编译期常量：TypeMeta / FieldMeta / TypeRef
//! traits  —— 三条访问契约：Reflect / ReflectList / ReflectEnum（+ ReflectItem）
//! value   —— 借用式访问结果：ValueRef / ValueRefMut
//! path    —— 运行期寻址：get_path / set_scalar
//! impls   —— 标量与容器的实现
//! ```
//!
//! # 与 `inspect` 的关系
//!
//! | | `inspect`（既有） | `reflect`（本模块） |
//! |---|---|---|
//! | 目的 | 插件组件参数**落盘** | 任意组件**编辑与查询** |
//! | 值类型 | `FieldValue`（磁盘契约） | `ValueRef`（借用，不落盘） |
//! | 表达力 | 扁平标量 | 递归 + 集合 + 枚举 |
//!
//! 两者刻意共存：删掉 `inspect` 会破坏已有场景文件里的 `behavior_params`。

mod impls;
mod meta;
mod path;
mod traits;
mod value;

pub use meta::{
    FieldFlags, FieldMeta, MetaKind, ReflectMeta, ScalarKind, TypeMeta, TypeRef, VariantMeta,
};
pub use path::{get_display, get_path, get_path_mut, set_scalar};
pub use traits::{Reflect, ReflectEnum, ReflectItem, ReflectList};
pub use value::{ReflectError, ScalarValue, ValueRef, ValueRefMut};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioSource;
    use crate::camera::Camera;

    fn camera() -> Camera {
        Camera {
            fov: 60.0,
            near: 0.1,
            far: 100.0,
            active: false,
        }
    }

    #[test]
    fn metadata_lists_fields_in_declaration_order() {
        let meta = Camera::meta();
        let names: Vec<&str> = meta.fields.iter().map(|f| f.name).collect();
        assert_eq!(names, ["fov", "near", "far", "active"]);
        assert_eq!(meta.name, "Camera");
    }

    #[test]
    fn attributes_reach_the_metadata() {
        let fov = Camera::meta().field("fov").expect("fov 应在元数据里");
        assert_eq!(fov.unit, Some("°"));
        // `unit`（机器可读）与 `note`（排版文案）是两件事，可以同时存在。
        assert_eq!(fov.note, Some("度（垂直）"));
        assert_eq!(fov.decimals, Some(1));
        assert_eq!(fov.label, Some("FOV"));
        assert_eq!(fov.range, Some((1.0, 179.0)));
        assert_eq!(fov.speed, Some(0.5));
        // 文档注释被拼进 doc，编辑器可以直接当提示用。
        assert!(fov.doc.contains("视场角"), "实际 doc = {:?}", fov.doc);
    }

    /// 元数据与手写 Inspector 的取值必须一致 —— 这一条是"S1 逐像素一致"的
    /// 全部前提。漂移了这里先红，不用等截图。
    #[test]
    fn editor_metadata_matches_the_handwritten_ranges() {
        let cam = Camera::meta();
        assert_eq!(cam.field("fov").unwrap().range, Some((1.0, 179.0)));
        assert_eq!(cam.field("near").unwrap().range, Some((0.001, 10000.0)));
        assert_eq!(cam.field("far").unwrap().range, Some((0.002, 100000.0)));
        assert_eq!(cam.field("near").unwrap().decimals, Some(3));

        let audio = AudioSource::meta();
        assert_eq!(audio.field("volume").unwrap().range, Some((0.0, 1.0)));
        assert_eq!(audio.field("pitch").unwrap().range, Some((0.05, 4.0)));
        assert_eq!(audio.field("pitch").unwrap().note, Some("1.0 = 原速"));
        // 成对的距离区间：同组 + 同一个显示开关 + 说明挂在组尾。
        let min = audio.field("min_distance").unwrap();
        let max = audio.field("max_distance").unwrap();
        assert_eq!(min.row_group, Some("distance"));
        assert_eq!(max.row_group, Some("distance"));
        assert_eq!(min.visible_when, Some("spatial"));
        assert_eq!(max.hint, None);
        assert_eq!(max.note, Some("min → max：衰减区间（米）"));
        // 组标签取组内第一个字段的显示名。
        assert_eq!(min.display_name(), "Distance");
    }

    #[test]
    fn read_then_write_scalar_round_trips() {
        let mut cam = camera();
        assert_eq!(get_path(&cam, "fov").unwrap().as_f32(), Some(60.0));

        set_scalar(&mut cam, "fov", &ScalarValue::F32(45.0)).unwrap();
        assert_eq!(cam.fov, 45.0);

        set_scalar(&mut cam, "active", &ScalarValue::Bool(true)).unwrap();
        assert!(cam.active);
    }

    #[test]
    fn write_rejects_type_mismatch() {
        let mut cam = camera();
        let err = set_scalar(&mut cam, "fov", &ScalarValue::Bool(true)).unwrap_err();
        assert!(
            matches!(err, ReflectError::TypeMismatch { .. }),
            "实际错误 = {err:?}"
        );
        // 被拒绝的写入不能改到数据。
        assert_eq!(cam.fov, 60.0);
    }

    #[test]
    fn unknown_field_names_the_type() {
        let cam = camera();
        match get_path(&cam, "nope") {
            Err(ReflectError::NoSuchField { type_name, field }) => {
                assert_eq!(type_name, "Camera");
                assert_eq!(field, "nope");
            }
            other => panic!("期望 NoSuchField，实际 = {other:?}"),
        }
    }

    #[test]
    fn scalars_are_not_traversable() {
        let cam = camera();
        assert!(matches!(
            get_path(&cam, "fov.inner"),
            Err(ReflectError::NotTraversable(_))
        ));
    }

    #[test]
    fn enum_field_is_readable_and_switchable() {
        let mut src = AudioSource::default();
        // 只读侧：是个 `Enum`，不是标量。
        assert!(matches!(
            get_path(&src, "bus").unwrap(),
            ValueRef::Enum(_)
        ));

        // `set_scalar` 不能切变体 —— 变体不是标量，这条边界要守住。
        assert!(set_scalar(&mut src, "bus", &ScalarValue::Str("Bgm".into())).is_err());

        // 走 `ReflectEnum` 才能切。
        {
            let mut slot = get_path_mut(&mut src, "bus").unwrap();
            let ValueRefMut::Enum(e) = &mut slot else {
                panic!("bus 应该是 Enum");
            };
            assert_eq!(e.variant(), "Sfx");
            assert_eq!(e.variant_names(), &["Sfx", "Bgm", "Voice"]);
            assert!(e.set_variant("Bgm"));
            assert!(!e.set_variant("不存在"));
            assert_eq!(e.variant(), "Bgm");
        }
        assert_eq!(src.bus, crate::audio::AudioBus::Bgm);
    }

    #[test]
    fn audio_source_fields_are_covered() {
        let meta = AudioSource::meta();
        let names: Vec<&str> = meta.fields.iter().map(|f| f.name).collect();
        // 声明顺序 = Inspector 顺序（反射通道按声明顺序排版，没有额外的排序号）。
        assert_eq!(
            names,
            [
                "clip",
                "volume",
                "bus",
                "pitch",
                "looping",
                "autoplay",
                "spatial",
                "min_distance",
                "max_distance",
            ]
        );
    }

    /// 无载荷枚举（`BodyKind` / `AudioBus`）由 derive 生成：变体顺序、显示名、
    /// 切变体、以及"没有载荷"这条边界。
    #[test]
    fn unit_enum_derive_covers_variants_and_labels() {
        use crate::physics::BodyKind;
        let meta = BodyKind::meta();
        assert_eq!(meta.kind, MetaKind::Enum);
        let names: Vec<&str> = meta.variants.iter().map(|v| v.name).collect();
        assert_eq!(names, ["Static", "Dynamic", "Kinematic"]);
        // 显示名没写 label 时退回寻址名。
        assert_eq!(meta.variant("Dynamic").unwrap().display_name(), "Dynamic");
        // 变体说明来自文档注释，编辑器拿它当悬浮提示。
        assert!(meta.variant("Static").unwrap().doc.contains("地面"));

        assert_eq!(BodyKind::default().variant(), "Dynamic");
        assert_eq!(BodyKind::default().variant_names().len(), 3);
        assert!(BodyKind::default().payload_meta().is_none());
    }

    /// 带载荷枚举：五个变体、`label` 覆盖显示名、载荷字段表按变体给。
    #[test]
    fn payload_enum_derive_describes_each_variant() {
        use crate::physics::CollisionShape;
        let meta = CollisionShape::meta();
        let names: Vec<&str> = meta.variants.iter().map(|v| v.name).collect();
        assert_eq!(names, ["Cuboid", "Ball", "Capsule", "Cylinder", "Auto"]);
        // 寻址名是 Rust 变体名，界面显示的是 label。
        assert_eq!(meta.variant("Cuboid").unwrap().display_name(), "Box");
        assert_eq!(meta.variant("Auto").unwrap().display_name(), "Auto");
        // 无载荷变体不给载荷类型。
        assert!(meta.variant("Auto").unwrap().payload.is_none());
        assert!(meta.variant("Cuboid").unwrap().payload.is_some());

        let cuboid = CollisionShape::Cuboid {
            half_extents: [1.0, 2.0, 3.0],
        };
        let payload = cuboid.payload_meta().expect("Cuboid 有载荷");
        let fields: Vec<&str> = payload.fields.iter().map(|f| f.name).collect();
        assert_eq!(fields, ["half_extents"]);
        assert_eq!(payload.field("half_extents").unwrap().display_name(), "Extents");
        assert!(matches!(
            cuboid.payload_get("half_extents"),
            Some(ValueRef::List(_))
        ));
        assert!(cuboid.payload_get("radius").is_none());
        assert!(CollisionShape::Auto.payload_meta().is_none());
    }

    /// 切变体重置载荷（不同变体的载荷类型不同，保留旧值没有意义）。
    #[test]
    fn switching_variant_resets_the_payload() {
        use crate::physics::CollisionShape;
        let mut shape = CollisionShape::Cuboid {
            half_extents: [9.0, 9.0, 9.0],
        };
        assert!(shape.set_variant("Ball"));
        assert_eq!(shape, CollisionShape::Ball { radius: 0.0 });
        assert!(!shape.set_variant("Nope"));
        assert_eq!(shape, CollisionShape::Ball { radius: 0.0 });
    }

    /// 载荷字段在路径里**直接按名字**寻址：枚举体内没有额外层级。
    #[test]
    fn payload_fields_are_addressable_by_name() {
        use crate::physics::{CollisionShape, PhysicsBody};
        let mut body = PhysicsBody {
            shape: CollisionShape::Ball { radius: 2.0 },
            ..PhysicsBody::default()
        };

        assert_eq!(get_path(&body, "shape.radius").unwrap().as_f32(), Some(2.0));
        set_scalar(&mut body, "shape.radius", &ScalarValue::F32(4.0)).unwrap();
        assert_eq!(body.shape, CollisionShape::Ball { radius: 4.0 });

        // 载荷里没有的名字 → NoSuchField，而不是 panic。
        assert!(matches!(
            get_path(&body, "shape.nope"),
            Err(ReflectError::NoSuchField { .. })
        ));
        // 无载荷变体再往下走 → 明说"没有值"。
        let auto = PhysicsBody {
            shape: CollisionShape::Auto,
            ..PhysicsBody::default()
        };
        assert!(matches!(
            get_path(&auto, "shape.radius"),
            Err(ReflectError::NoneValue(_))
        ));
    }

    #[test]
    fn nested_type_reference_points_at_target_meta() {
        let bus = AudioSource::meta().field("bus").expect("bus");
        match bus.ty {
            // `Named` 只指向目标类型，结构体 / 枚举的区别要看目标的 `kind`。
            TypeRef::Named(meta) => {
                assert_eq!(meta().name, "AudioBus");
                assert_eq!(meta().kind, MetaKind::Enum);
            }
            other => panic!("bus 应是 Named，实际 = {other:?}"),
        }
    }
}
