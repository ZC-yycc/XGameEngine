//! 场景级环境：光源与环境光。
//!
//! 旧引擎把光源放在场景顶层（实体组件不落盘），XGC 同样把环境放在**世界级数据**里，
//! 而不是伪造出一批光源实体——那会改变实体计数与身份，让 `compare-headless` 与
//! `verify-migration` 失真。字段名与缺省值逐项对齐旧 `Light` / `Scene`，迁移只做
//! 搬运，不做二次解释。

use serde::{Deserialize, Serialize};

/// 光源类型；`Directional` 只看方向，`Point` 只看位置与半径。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightKind {
    #[default]
    Directional,
    Point,
}

/// 一盏场景光源。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneLight {
    /// 显示名称，多盏灯时用来区分。
    pub name: String,
    #[serde(default)]
    pub kind: LightKind,
    /// **光线传播方向**（世界空间，从光源射向场景）。着色器取反才是表面指向
    /// 光源的方向；零向量在 [`Self::normalized_direction`] 里退化成垂直向下。
    #[serde(default = "default_direction")]
    pub direction: [f32; 3],
    /// 点光的世界坐标；方向光忽略。
    #[serde(default)]
    pub position: [f32; 3],
    /// 光色（线性 RGB）。
    #[serde(default = "default_color")]
    pub color: [f32; 3],
    /// 强度倍率，直接乘到漫反射与高光项上；`<= 0` 视为关灯。
    #[serde(default = "default_intensity")]
    pub intensity: f32,
    /// 点光影响半径；方向光忽略。
    #[serde(default = "default_range")]
    pub range: f32,
    #[serde(default = "default_casts_shadow")]
    pub casts_shadow: bool,
    /// 阴影贴图分辨率，`0` 表示用引擎缺省。
    #[serde(default)]
    pub shadow_resolution: u32,
}

impl Default for SceneLight {
    /// 与旧 `Light::default` 同款：右上方向的白光，一眼能看出立体感。
    fn default() -> Self {
        Self {
            name: "Directional Light".to_string(),
            kind: LightKind::Directional,
            direction: default_direction(),
            position: [0.0; 3],
            color: default_color(),
            intensity: default_intensity(),
            range: default_range(),
            casts_shadow: default_casts_shadow(),
            shadow_resolution: 0,
        }
    }
}

impl SceneLight {
    pub fn directional(
        name: impl Into<String>,
        direction: [f32; 3],
        color: [f32; 3],
        intensity: f32,
    ) -> Self {
        Self {
            name: name.into(),
            direction,
            color,
            intensity,
            ..Self::default()
        }
    }

    /// 归一化的光线传播方向；零向量退化成垂直向下，避免着色器里除以零。
    pub fn normalized_direction(&self) -> [f32; 3] {
        let [x, y, z] = self.direction;
        let length = (x * x + y * y + z * z).sqrt();
        if length > 1e-6 {
            [x / length, y / length, z / length]
        } else {
            [0.0, -1.0, 0.0]
        }
    }

    /// 强度非正即视为关灯，光照循环据此跳过这盏。
    pub fn is_active(&self) -> bool {
        self.intensity > 0.0
    }
}

/// 世界级环境：光源表 + 均匀环境光。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneEnvironment {
    #[serde(default = "default_lights")]
    pub lights: Vec<SceneLight>,
    #[serde(default = "default_ambient")]
    pub ambient: [f32; 3],
}

impl Default for SceneEnvironment {
    /// 与旧 `Scene::new` 一致：自带一盏默认方向光和一点环境光。全黑空场景既劝退
    /// 新手，也会让人误判成相机 / 管线坏了。
    fn default() -> Self {
        Self {
            lights: default_lights(),
            ambient: default_ambient(),
        }
    }
}

impl SceneEnvironment {
    /// 缺省环境不落盘：旧世界文件的字节保持不变，读到没有 `environment` 的旧文件
    /// 时也得到与旧引擎相同的缺省光照。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// 第一盏**可用的方向光**：截图顺光取景与后续着色都以它为主光。
    pub fn sun(&self) -> Option<&SceneLight> {
        self.lights
            .iter()
            .find(|light| light.kind == LightKind::Directional && light.is_active())
    }
}

fn default_lights() -> Vec<SceneLight> {
    vec![SceneLight::default()]
}

fn default_ambient() -> [f32; 3] {
    [0.06, 0.06, 0.08]
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

fn default_casts_shadow() -> bool {
    true
}
