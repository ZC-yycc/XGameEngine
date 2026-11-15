//! `AudioBus` 的反射实现。
//!
//! 手写而不是 derive：`#[derive(Reflect)]` 只覆盖具名结构体。枚举的支持不是"以后
//! 补一行"的事 —— 带载荷变体的载荷类型各不相同，切换语义必须由人定：
//! 这里是无载荷枚举，切变体就是整体替换。
//!
//! 放在独立文件而不是 `audio.rs` 末尾，是为了让 `audio.rs` 的测试模块保持
//! 在文件最后（clippy 的 `items_after_test_module`）。

use std::sync::OnceLock;

use crate::audio::AudioBus;
use crate::reflect::{
    MetaKind, ReflectEnum, ReflectItem, ReflectMeta, TypeMeta, ValueRef, ValueRefMut, VariantMeta,
};

impl ReflectMeta for AudioBus {
    fn meta() -> &'static TypeMeta {
        static META: OnceLock<TypeMeta> = OnceLock::new();
        META.get_or_init(|| TypeMeta {
            name: "AudioBus",
            doc: "音量总线（只影响音量归组，不改变空间化 / 停止语义）",
            kind: MetaKind::Enum,
            fields: &[],
            variants: Box::leak(
                vec![
                    VariantMeta {
                        name: "Sfx",
                        // 显示名 = `AudioBus::name()`：界面上一直是小写 ASCII，
                        // 而寻址名必须是 Rust 标识符。两套名字刻意分开。
                        label: Some("sfx"),
                        doc: "音效（脚步、撞击、按钮）。默认组。",
                        payload: None,
                    },
                    VariantMeta {
                        name: "Bgm",
                        label: Some("bgm"),
                        doc: "背景音乐（通常循环）。",
                        payload: None,
                    },
                    VariantMeta {
                        name: "Voice",
                        label: Some("voice"),
                        doc: "对白 / 语音。",
                        payload: None,
                    },
                ]
                .into_boxed_slice(),
            ),
        })
    }
}

impl ReflectEnum for AudioBus {
    fn variant(&self) -> &'static str {
        match self {
            Self::Sfx => "Sfx",
            Self::Bgm => "Bgm",
            Self::Voice => "Voice",
        }
    }

    fn set_variant(&mut self, name: &str) -> bool {
        match name {
            "Sfx" => *self = Self::Sfx,
            "Bgm" => *self = Self::Bgm,
            "Voice" => *self = Self::Voice,
            _ => return false,
        }
        true
    }

    fn payload(&self) -> Option<ValueRef<'_>> {
        None
    }

    fn payload_mut(&mut self) -> Option<ValueRefMut<'_>> {
        None
    }

    fn variant_names(&self) -> &'static [&'static str] {
        &Self::VARIANT_NAMES
    }
}

impl ReflectItem for AudioBus {
    fn as_value(&self) -> ValueRef<'_> {
        ValueRef::Enum(self)
    }
    fn as_value_mut(&mut self) -> ValueRefMut<'_> {
        ValueRefMut::Enum(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_names_match_the_metadata() {
        let bus = AudioBus::Sfx;
        let names = bus.variant_names();
        let meta = AudioBus::meta();
        assert_eq!(names.len(), meta.variants.len());
        for (name, variant) in names.iter().zip(meta.variants) {
            assert_eq!(*name, variant.name);
        }
    }

    #[test]
    fn switching_variant_round_trips() {
        let mut bus = AudioBus::Sfx;
        assert!(bus.set_variant("Voice"));
        assert_eq!(bus.variant(), "Voice");
        assert_eq!(bus, AudioBus::Voice);
        // 未知变体必须原状态不变。
        assert!(!bus.set_variant("Nope"));
        assert_eq!(bus, AudioBus::Voice);
    }
}
