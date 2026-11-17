//! 解析天空 + 漫反射环境照明（L2 球谐，E/π 口径）。
//!
//! 与旧引擎同一条解析函数：天顶 / 地平线 / 地面三段渐变 + 太阳盘 + 光晕，直接对
//! **方向**求值。背景色、物体环境照明都从这同一个函数取 —— "背景是蓝天、物体却按
//! 另一套环境光照明"的分叉从根上不存在。
//!
//! 漫反射环境用 9 个 L2 实球谐系数表示：CPU 侧用**确定性**斐波那契球点
//! （1024 个、等权）数值积分，余弦卷积因子 `[1, 2/3, 1/4]` 已折进系数，存的是
//! **E/π** —— 与常量 `ambient` 同一标定，所以两套环境光能直接相加。
//!
//! 这一刀只做漫反射半球；镜面预过滤（GGX 重要性采样立方体）与天空盒 pass 留给后续。

/// 天空整体强度上限：再亮整幅背景糊成纯白。
pub const MAX_INTENSITY: f32 = 10.0;
/// 太阳强度上限。
pub const MAX_SUN_INTENSITY: f32 = 10.0;
/// 太阳盘半径范围（度）。
pub const MIN_SUN_SIZE: f32 = 0.1;
pub const MAX_SUN_SIZE: f32 = 12.0;
/// L2 实球谐系数个数。
pub const SH_COEFFS: usize = 9;
/// 球谐投影采样点数（斐波那契球，等权）。
const SH_SAMPLES: usize = 1024;
/// 余弦卷积因子（l = 0 / 1 / 2），存 E/π 口径。
const SH_CONVOLUTION: [f32; 3] = [1.0, 2.0 / 3.0, 0.25];
const SUN_DISK_RADIANCE: f32 = 40.0;
const SUN_GLOW: f32 = 0.35;

/// 画面设置里的天空节（对应旧 `project.ron` 的 `render.sky`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkySettings {
    pub enabled: bool,
    /// 整体强度乘子（0 = 天空全黑，等价于只有 ambient）。
    pub intensity: f32,
    pub zenith: [f32; 3],
    /// 天空段与地面段在视线水平处**都**取它 —— 渐变在 `dir.y == 0` 处连续。
    pub horizon: [f32; 3],
    /// 镜头朝下看时的颜色。
    pub ground: [f32; 3],
    /// 太阳盘半径（度）。
    pub sun_size: f32,
    /// 太阳盘与光晕强度乘子；场景没有方向光时太阳整段不出现。
    pub sun_intensity: f32,
}

impl Default for SkySettings {
    /// 与旧 `SkyConfig::default` 一致：默认关、强度 1。关着时环境照明只剩 `ambient`，
    /// 画面与天空上线前逐位相同。
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: 1.0,
            zenith: [0.12, 0.28, 0.58],
            horizon: [0.58, 0.68, 0.82],
            ground: [0.16, 0.15, 0.14],
            sun_size: 1.5,
            sun_intensity: 1.0,
        }
    }
}

impl SkySettings {
    /// 加载边界清洗：NaN / 无穷换缺省值，强度与太阳半径夹到安全范围。
    pub fn sanitized(mut self) -> Self {
        for color in [&mut self.zenith, &mut self.horizon, &mut self.ground] {
            for channel in color.iter_mut() {
                if !channel.is_finite() {
                    *channel = 0.0;
                }
            }
        }
        if !self.intensity.is_finite() {
            self.intensity = 1.0;
        }
        if !self.sun_intensity.is_finite() {
            self.sun_intensity = 1.0;
        }
        if !self.sun_size.is_finite() {
            self.sun_size = 1.5;
        }
        self.intensity = self.intensity.clamp(0.0, MAX_INTENSITY);
        self.sun_intensity = self.sun_intensity.clamp(0.0, MAX_SUN_INTENSITY);
        self.sun_size = self.sun_size.clamp(MIN_SUN_SIZE, MAX_SUN_SIZE);
        self
    }
}

/// 天空 uniform（与 WGSL 逐字节对齐；5 个 vec4 = 80 字节）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SkyUniform {
    /// xyz = 指向太阳的单位向量，w = 天空是否开启。
    pub(crate) sun_dir: [f32; 4],
    /// rgb = 太阳颜色，a = 太阳强度乘子。
    pub(crate) sun_color: [f32; 4],
    /// rgb = 天顶色，a = 整体强度乘子。
    pub(crate) zenith: [f32; 4],
    /// rgb = 地平线色，a = 太阳盘半径（度）。
    pub(crate) horizon: [f32; 4],
    /// rgb = 地面色。
    pub(crate) ground: [f32; 4],
}

impl SkyUniform {
    /// 从设置 + 场景太阳装配；没有方向光时太阳强度写 0（"没有灯就没有太阳"）。
    ///
    /// `sun` 是 `(指向太阳的单位向量, 灯光颜色)` —— 颜色取首盏方向光的颜色，
    /// 与旧引擎 `sun_from_lights` 同源。
    pub(crate) fn build(settings: &SkySettings, sun: Option<([f32; 3], [f32; 3])>) -> Self {
        let (dir, color, sun_intensity) = match sun {
            Some((toward, color)) => (normalize_or_zero(toward), color, settings.sun_intensity),
            None => ([0.0; 3], [0.0; 3], 0.0),
        };
        Self {
            sun_dir: [
                dir[0],
                dir[1],
                dir[2],
                f32::from(u8::from(settings.enabled)),
            ],
            sun_color: [color[0], color[1], color[2], sun_intensity],
            zenith: [
                settings.zenith[0],
                settings.zenith[1],
                settings.zenith[2],
                settings.intensity,
            ],
            horizon: [
                settings.horizon[0],
                settings.horizon[1],
                settings.horizon[2],
                settings.sun_size,
            ],
            ground: [
                settings.ground[0],
                settings.ground[1],
                settings.ground[2],
                0.0,
            ],
        }
    }
}

impl SkyUniform {
    /// 按 WGSL 字段顺序（5 个 vec4）打包，供天空盒 uniform 使用。
    pub(crate) fn write(&self, bytes: &mut Vec<u8>) {
        for value in [
            self.sun_dir,
            self.sun_color,
            self.zenith,
            self.horizon,
            self.ground,
        ] {
            for channel in value {
                bytes.extend_from_slice(&channel.to_le_bytes());
            }
        }
    }
}

/// 天空辐射亮度（线性 HDR）；与 WGSL `sky_radiance` 逐式对应。
pub(crate) fn sky_radiance(uniform: &SkyUniform, direction: [f32; 3]) -> [f32; 3] {
    let t = direction[1].clamp(-1.0, 1.0);
    let up_t = t.max(0.0).sqrt();
    let down_t = (-t).max(0.0).sqrt();
    let sky = mix3(uniform.horizon, uniform.zenith, up_t);
    let ground = mix3(uniform.horizon, uniform.ground, down_t);
    let base = if t > 0.0 { sky } else { ground };

    let dot = direction[0] * uniform.sun_dir[0]
        + direction[1] * uniform.sun_dir[1]
        + direction[2] * uniform.sun_dir[2];
    let cos_angle = dot.clamp(-1.0, 1.0);
    let angle = cos_angle.acos();
    let sun_radius = (std::f32::consts::PI / 180.0 * uniform.horizon[3]).max(1e-4);
    let disk = 1.0 - smoothstep(sun_radius * 0.6, sun_radius, angle);
    let glow = SUN_GLOW * cos_angle.max(0.0).powf(16.0);
    std::array::from_fn(|channel| {
        (base[channel]
            + uniform.sun_color[channel] * uniform.sun_color[3] * (SUN_DISK_RADIANCE * disk + glow))
            * uniform.zenith[3]
    })
}

/// 确定性斐波那契球点（等权积分，无随机数 ⇒ 重建结果逐位可复现）。
fn fibonacci_dirs(count: usize) -> Vec<[f32; 3]> {
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    (0..count)
        .map(|index| {
            let y = 1.0 - 2.0 * (index as f32 + 0.5) / count as f32;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let phi = golden * index as f32;
            [r * phi.cos(), y, r * phi.sin()]
        })
        .collect()
}

/// L2 实球谐基：`[Y00, Y1-1(y), Y10(z), Y11(x), Y2-2, Y2-1, Y20, Y21, Y22]`。
fn sh_basis(n: [f32; 3]) -> [f32; SH_COEFFS] {
    let (x, y, z) = (n[0], n[1], n[2]);
    [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ]
}

fn convolution_of(index: usize) -> f32 {
    SH_CONVOLUTION[if index == 0 {
        0
    } else if index < 4 {
        1
    } else {
        2
    }]
}

/// 把解析天空投影到 L2 球谐并把余弦卷积折进系数（存 E/π）。
///
/// 结果直接就是漫反射环境项 `E(n)/π`，与常量 `ambient` 同标定。
pub(crate) fn sh_project(uniform: &SkyUniform) -> [[f32; 4]; SH_COEFFS] {
    let weight = 4.0 * std::f32::consts::PI / SH_SAMPLES as f32;
    let mut coeffs = [[0.0f32; 4]; SH_COEFFS];
    for direction in fibonacci_dirs(SH_SAMPLES) {
        let radiance = sky_radiance(uniform, direction);
        for (index, basis) in sh_basis(direction).into_iter().enumerate() {
            let factor = basis * weight * convolution_of(index);
            for channel in 0..3 {
                coeffs[index][channel] += radiance[channel] * factor;
            }
        }
    }
    coeffs
}

/// E/π 球谐求值；与 WGSL `sky_sh_eval` 逐项一致。
pub fn sh_eval(coeffs: &[[f32; 4]; SH_COEFFS], n: [f32; 3]) -> [f32; 3] {
    let basis = sh_basis(n);
    std::array::from_fn(|channel| {
        (0..SH_COEFFS)
            .map(|index| coeffs[index][channel] * basis[index])
            .sum()
    })
}

/// 某个方向上的天空辐射亮度（线性 HDR）：测试与证据脚本对表用。
///
/// 与 GPU 天空盒片元走同一条 `sky_radiance`；`sun = (指向太阳, 光色)`。
pub fn sky_color(
    settings: &SkySettings,
    sun: Option<([f32; 3], [f32; 3])>,
    direction: [f32; 3],
) -> [f32; 3] {
    let uniform = SkyUniform::build(settings, sun);
    sky_radiance(&uniform, normalize_or_zero(direction))
}

/// 天空环境照明：把设置投成 SH，供渲染器一次性装进帧 uniform。
pub fn sky_irradiance(
    settings: &SkySettings,
    sun: Option<([f32; 3], [f32; 3])>,
) -> Option<[[f32; 4]; SH_COEFFS]> {
    if !settings.enabled {
        return None;
    }
    let uniform = SkyUniform::build(settings, sun);
    Some(sh_project(&uniform))
}

fn mix3(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 3] {
    std::array::from_fn(|channel| a[channel] * (1.0 - t) + b[channel] * t)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn normalize_or_zero(value: [f32; 3]) -> [f32; 3] {
    let length = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
    if length > 1e-6 {
        [value[0] / length, value[1] / length, value[2] / length]
    } else {
        [0.0; 3]
    }
}

#[cfg(test)]
mod tests {
    use super::{sh_eval, sh_project, sky_radiance, SkySettings, SkyUniform};

    #[test]
    fn sh_projection_of_a_constant_sky_reproduces_the_constant() {
        let settings = SkySettings {
            enabled: true,
            intensity: 1.0,
            zenith: [0.3; 3],
            horizon: [0.3; 3],
            ground: [0.3; 3],
            sun_size: 0.5,
            sun_intensity: 0.0,
        };
        let coeffs = sh_project(&SkyUniform::build(&settings, None));
        for normal in [
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
        ] {
            let value = sh_eval(&coeffs, normal)[0];
            assert!(
                (value - 0.3).abs() < 2e-3,
                "常量天空在 {normal:?} 上得到 {value}"
            );
        }
    }

    #[test]
    fn horizon_is_continuous_across_the_sky_ground_seam() {
        let settings = SkySettings {
            enabled: true,
            sun_intensity: 0.0,
            ..SkySettings::default()
        };
        let uniform = SkyUniform::build(&settings, None);
        // 两侧都收敛到同一个地平线色：|t| = 1e-6 时差值上界是
        // sqrt(|t|) × |天顶 - 地面| ≈ 1e-3 × 0.4，取 2e-3 容差。
        let above = sky_radiance(&uniform, [0.0, 1e-6, 0.0]);
        let below = sky_radiance(&uniform, [0.0, -1e-6, 0.0]);
        for channel in 0..3 {
            assert!(
                (above[channel] - below[channel]).abs() < 2e-3,
                "地平线处不连续：{above:?} vs {below:?}"
            );
        }
    }

    #[test]
    fn sun_only_appears_when_a_directional_light_exists() {
        let settings = SkySettings {
            enabled: true,
            sun_intensity: 1.0,
            ..SkySettings::default()
        };
        let no_sun = SkyUniform::build(&settings, None);
        let with_sun = SkyUniform::build(&settings, Some(([0.0, 1.0, 0.0], [1.0, 1.0, 1.0])));
        let up = [0.0, 1.0, 0.0];
        assert!(sky_radiance(&with_sun, up)[0] > sky_radiance(&no_sun, up)[0]);
    }
}
