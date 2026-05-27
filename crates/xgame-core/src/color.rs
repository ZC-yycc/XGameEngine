//! RGBA 颜色类型，以线性空间浮点分量存储。
//!
//! 提供系列构造方法：`new(r, g, b, a)`、`rgb(r, g, b)`、`from_rgba8`、`from_hex`，
//! 以及插值、伽马转换等实用方法。

use glam::Vec4;
use serde::{Deserialize, Serialize};

/// RGBA 颜色，以线性浮点分量存储（各分量范围 0.0–1.0）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

// ---------------------------------------------------------------------------
// 命名颜色常量
// ---------------------------------------------------------------------------

#[allow(non_upper_case_globals)]
impl Color {
    // -- 基础色 --
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0, 1.0);
    pub const RED: Self = Self::new(1.0, 0.0, 0.0, 1.0);
    pub const GREEN: Self = Self::new(0.0, 1.0, 0.0, 1.0);
    pub const BLUE: Self = Self::new(0.0, 0.0, 1.0, 1.0);
    pub const TRANSPARENT: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    // -- 灰度系 --
    pub const GRAY: Self = Self::new(0.5, 0.5, 0.5, 1.0);
    pub const DARK_GRAY: Self = Self::new(0.25, 0.25, 0.25, 1.0);
    pub const LIGHT_GRAY: Self = Self::new(0.75, 0.75, 0.75, 1.0);

    // -- 暖色系 --
    pub const YELLOW: Self = Self::new(1.0, 1.0, 0.0, 1.0);
    pub const ORANGE: Self = Self::new(1.0, 0.647, 0.0, 1.0);
    pub const CORAL: Self = Self::new(1.0, 0.498, 0.314, 1.0);

    // -- 冷色系 --
    pub const CYAN: Self = Self::new(0.0, 1.0, 1.0, 1.0);
    pub const MAGENTA: Self = Self::new(1.0, 0.0, 1.0, 1.0);
    pub const PURPLE: Self = Self::new(0.5, 0.0, 0.5, 1.0);
    pub const TEAL: Self = Self::new(0.0, 0.5, 0.5, 1.0);

    // -- 引擎主题色 --
    pub const ACCENT: Self = Self::new(0.258, 0.525, 0.957, 1.0); // #4285F4
    pub const SUCCESS: Self = Self::new(0.2, 0.8, 0.2, 1.0);
    pub const WARNING: Self = Self::new(1.0, 0.8, 0.0, 1.0);
    pub const ERROR: Self = Self::new(1.0, 0.2, 0.2, 1.0);
}

// ---------------------------------------------------------------------------
// 构造
// ---------------------------------------------------------------------------

impl Color {
    /// 从线性空间浮点分量创建颜色。
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// 从线性空间浮点分量创建不透明颜色（alpha = 1.0）。
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// 从 0–255 范围的整数分量创建颜色（sRGB → 线性）。
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: srgb_to_linear(r as f32 / 255.0),
            g: srgb_to_linear(g as f32 / 255.0),
            b: srgb_to_linear(b as f32 / 255.0),
            a: a as f32 / 255.0,
        }
    }

    /// 从 0–255 范围的整数分量创建不透明颜色。
    pub fn from_rgb8(r: u8, g: u8, b: u8) -> Self {
        Self::from_rgba8(r, g, b, 255)
    }

    /// 从十六进制字符串解析颜色。
    ///
    /// 支持格式：`#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`。
    /// 前缀 `#` 可选。
    ///
    /// 如果字符串不合法，返回 `None`。
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');
        let (r, g, b, a) = match hex.len() {
            3 => (
                u8::from_str_radix(&hex[0..1], 16).ok()? * 17,
                u8::from_str_radix(&hex[1..2], 16).ok()? * 17,
                u8::from_str_radix(&hex[2..3], 16).ok()? * 17,
                255,
            ),
            4 => (
                u8::from_str_radix(&hex[0..1], 16).ok()? * 17,
                u8::from_str_radix(&hex[1..2], 16).ok()? * 17,
                u8::from_str_radix(&hex[2..3], 16).ok()? * 17,
                u8::from_str_radix(&hex[3..4], 16).ok()? * 17,
            ),
            6 => (
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                255,
            ),
            8 => (
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
                u8::from_str_radix(&hex[6..8], 16).ok()?,
            ),
            _ => return None,
        };
        Some(Self::from_rgba8(r, g, b, a))
    }
}

// ---------------------------------------------------------------------------
// 转换
// ---------------------------------------------------------------------------

impl Color {
    /// 转换为 `[f32; 4]` 数组。
    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// 将 RGB 分量转换为 sRGB 空间（适用于 UI 显示）。
    pub fn to_srgb(self) -> Self {
        Self {
            r: linear_to_srgb(self.r),
            g: linear_to_srgb(self.g),
            b: linear_to_srgb(self.b),
            a: self.a,
        }
    }

    /// 将 RGB 分量转换回线性空间。
    pub fn to_linear(self) -> Self {
        Self {
            r: srgb_to_linear(self.r),
            g: srgb_to_linear(self.g),
            b: srgb_to_linear(self.b),
            a: self.a,
        }
    }

    /// 设置 alpha 通道并返回新颜色。
    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// 设置 RGB 从另一颜色取色，保留当前 alpha。
    pub fn with_rgb_from(self, other: Color) -> Self {
        Self {
            r: other.r,
            g: other.g,
            b: other.b,
            a: self.a,
        }
    }
}

// ---------------------------------------------------------------------------
// 插值与运算
// ---------------------------------------------------------------------------

impl Color {
    /// 线性插值：`t=0` 返回 `self`，`t=1` 返回 `other`。
    pub fn lerp(self, other: Color, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }

    /// 标量乘法。
    pub fn scale(self, factor: f32) -> Self {
        Self {
            r: self.r * factor,
            g: self.g * factor,
            b: self.b * factor,
            a: self.a,
        }
    }

    /// 逐分量加法（用于混合光）。
    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            r: (self.r + other.r).min(1.0),
            g: (self.g + other.g).min(1.0),
            b: (self.b + other.b).min(1.0),
            a: (self.a + other.a).min(1.0),
        }
    }
}

// ---------------------------------------------------------------------------
// 从 Color 的转换
// ---------------------------------------------------------------------------

impl From<Color> for Vec4 {
    fn from(c: Color) -> Self {
        Vec4::new(c.r, c.g, c.b, c.a)
    }
}

impl From<Color> for [f32; 4] {
    fn from(c: Color) -> Self {
        c.to_array()
    }
}

impl From<[f32; 4]> for Color {
    fn from(arr: [f32; 4]) -> Self {
        Self::new(arr[0], arr[1], arr[2], arr[3])
    }
}

impl From<(f32, f32, f32, f32)> for Color {
    fn from(t: (f32, f32, f32, f32)) -> Self {
        Self::new(t.0, t.1, t.2, t.3)
    }
}

impl From<Vec4> for Color {
    fn from(v: Vec4) -> Self {
        Self::new(v.x, v.y, v.z, v.w)
    }
}

// ---------------------------------------------------------------------------
// 内部：sRGB ↔ 线性转换
// ---------------------------------------------------------------------------

/// sRGB 分量转换为线性空间。
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// 线性分量转换为 sRGB 空间。
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_hex_6digit() {
        let c = Color::from_hex("#FF0000").unwrap();
        assert!((c.r - 1.0).abs() < 0.001);
        assert!(c.g.abs() < 0.001);
        assert!(c.b.abs() < 0.001);
        assert!((c.a - 1.0).abs() < 0.001);
    }

    #[test]
    fn from_hex_3digit() {
        let c = Color::from_hex("0F0").unwrap();
        assert!(c.g > 0.99);
        assert!(c.r.abs() < 0.001);
    }

    #[test]
    fn from_hex_invalid() {
        assert!(Color::from_hex("xyz").is_none());
        assert!(Color::from_hex("#12345").is_none());
    }

    #[test]
    fn lerp_clamping() {
        let a = Color::RED;
        let b = Color::BLUE;
        let mid = a.lerp(b, 0.5);
        assert!((mid.r - 0.5).abs() < 0.001);
        assert!((mid.b - 0.5).abs() < 0.001);
        // 超出 [0,1] 的 t 被 clamp
        let end = a.lerp(b, 2.0);
        assert!((end.b - 1.0).abs() < 0.001);
    }

    #[test]
    fn to_array_roundtrip() {
        let c = Color::new(0.2, 0.4, 0.6, 0.8);
        let arr: [f32; 4] = c.into();
        assert!((arr[0] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn srgb_identity() {
        let c = Color::from_rgba8(128, 128, 128, 255);
        let back = c.to_srgb().to_linear();
        assert!((back.r - c.r).abs() < 0.001);
    }
}
