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

mod audio_bus;
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
        assert_eq!(fov.range, Some((1.0, 179.0)));
        assert_eq!(fov.speed, Some(0.5));
        // 文档注释被拼进 doc，编辑器可以直接当提示用。
        assert!(fov.doc.contains("视场角"), "实际 doc = {:?}", fov.doc);
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
        assert_eq!(
            names,
            [
                "clip",
                "volume",
                "pitch",
                "looping",
                "autoplay",
                "spatial",
                "min_distance",
                "max_distance",
                "bus",
            ]
        );
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
