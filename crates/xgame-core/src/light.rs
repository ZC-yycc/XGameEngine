//! 场景光源。
//!
//! 光源是**场景级数据**，存在 [`Scene::lights`](crate::Scene) 里，随场景一起
//! 序列化到 `.ron`。
//!
//! # 为什么不把光源做成实体组件
//!
//! `Entity::components` 标了 `#[serde(skip)]`——它是纯运行时容器，挂上去的东西
//! 存不进场景文件。而光照属于"环境"，既要在编辑器里统一编辑，又要能持久化。
//! 放在场景级最直接，思路和 Unity 的 Lighting 窗口、Godot 的 WorldEnvironment
//! 一致：环境不是某个物体的一部分。
//!
//! 代价是光源暂时不能被 Transform 层级带动（点光位置要手填），等以后组件系统
//! 支持序列化时再考虑迁移。

use serde::{Deserialize, Serialize};

/// 光源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LightKind {
    /// 方向光：只有方向没有位置，模拟太阳。整片场景均匀受光。
    #[default]
    Directional,
    /// 点光：从一点向外发光，按距离衰减。
    Point,
}

/// 一盏场景光源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Light {
    /// 显示名称，编辑器里用来区分多盏灯。
    pub name: String,
    #[serde(default)]
    pub kind: LightKind,
    /// 方向光的**光线传播方向**（世界空间，从光源射向场景）。
    ///
    /// 注意存的是"光射出去的方向"，不是"指向光源的方向"。所以默认的
    /// `(0.5, -1, 0.5)` 表示光斜着往下方打；着色器里用 `-direction` 才是
    /// 表面指向光源的入射方向。
    #[serde(default = "default_direction")]
    pub direction: [f32; 3],
    /// 点光的世界坐标位置。方向光忽略此字段。
    #[serde(default)]
    pub position: [f32; 3],
    /// 光色（线性 RGB）。
    #[serde(default = "default_color")]
    pub color: [f32; 3],
    /// 强度倍率，直接乘到漫反射与高光项上。
    #[serde(default = "default_intensity")]
    pub intensity: f32,
    /// 点光的影响半径（世界单位），超出距离贡献为 0。方向光忽略。
    #[serde(default = "default_range")]
    pub range: f32,
    /// 这盏灯是否投射阴影。
    ///
    /// 阴影贴图是**每盏灯**一份深度的，开得越多越贵。默认只在主方向光上开，
    /// 点光默认关 —— 点光阴影要 6 面立方体贴图，是这里成本最高的东西。
    #[serde(default = "default_casts_shadow")]
    pub casts_shadow: bool,
    /// 阴影贴图分辨率（正方形边长，像素）。`0` 表示用引擎默认值。
    #[serde(default)]
    pub shadow_resolution: u32,
}

fn default_casts_shadow() -> bool {
    true
}

fn default_direction() -> [f32; 3] {
    [0.5, -1.0, 0.5]
}

fn default_color() -> [f32; 3] {
    [1.0, 0.98, 0.95]
}

fn default_intensity() -> f32 {
    3.0
}

fn default_range() -> f32 {
    10.0
}

impl Default for Light {
    /// 默认是一盏"从右上方打下来的白光"，强度和角度都调成一眼能看出立体感。
    fn default() -> Self {
        Self {
            name: "Directional Light".into(),
            kind: LightKind::Directional,
            direction: default_direction(),
            position: [0.0, 0.0, 0.0],
            color: default_color(),
            intensity: default_intensity(),
            range: default_range(),
            casts_shadow: default_casts_shadow(),
            shadow_resolution: 0,
        }
    }
}

impl Light {
    /// 造一盏方向光。
    pub fn directional(
        name: impl Into<String>,
        direction: [f32; 3],
        color: [f32; 3],
        intensity: f32,
    ) -> Self {
        Self {
            name: name.into(),
            kind: LightKind::Directional,
            direction,
            position: [0.0; 3],
            color,
            intensity,
            range: default_range(),
            casts_shadow: default_casts_shadow(),
            shadow_resolution: 0,
        }
    }

    /// 造一盏点光。
    pub fn point(
        name: impl Into<String>,
        position: [f32; 3],
        color: [f32; 3],
        intensity: f32,
        range: f32,
    ) -> Self {
        Self {
            name: name.into(),
            kind: LightKind::Point,
            direction: [0.0, -1.0, 0.0],
            position,
            color,
            intensity,
            range,
            // 点光阴影要 6 面立方体贴图，成本高，默认不开。
            casts_shadow: false,
            shadow_resolution: 0,
        }
    }

    /// 归一化的光线传播方向。
    ///
    /// 零向量会退化成"垂直向下"——着色器里除以零会得到 NaN，进而让整帧变黑，
    /// 与其在 GPU 侧兜底，不如在 CPU 侧就给一个确定的方向。
    pub fn normalized_direction(&self) -> [f32; 3] {
        let [x, y, z] = self.direction;
        let len = (x * x + y * y + z * z).sqrt();
        if len > 1e-6 {
            [x / len, y / len, z / len]
        } else {
            [0.0, -1.0, 0.0]
        }
    }

    /// 是否参与光照计算。强度为 0 或负数时直接跳过，省一次着色器循环。
    pub fn is_active(&self) -> bool {
        self.intensity > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_light_is_directional_and_lit() {
        let l = Light::default();
        assert_eq!(l.kind, LightKind::Directional);
        assert!(l.is_active());
    }

    #[test]
    fn normalized_direction_handles_zero() {
        let l = Light {
            direction: [0.0, 0.0, 0.0],
            ..Default::default()
        };
        assert_eq!(l.normalized_direction(), [0.0, -1.0, 0.0]);
    }

    #[test]
    fn normalized_direction_has_unit_length() {
        let l = Light::directional("sun", [1.0, -2.0, 3.0], [1.0; 3], 1.0);
        let [x, y, z] = l.normalized_direction();
        let len = (x * x + y * y + z * z).sqrt();
        assert!((len - 1.0).abs() < 1e-5);
    }
}
