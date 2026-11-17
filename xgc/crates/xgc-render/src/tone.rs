//! 色调映射：与旧引擎同一条 ACES 曲线（曝光后 → 曲线 → 钳制）。
//!
//! 旧引擎渲染到 HDR 目标后在合成 pass 里过曲线；XGC 的实例化 pass 直接写
//! sRGB 目标，所以在**片元输出前**套同一条曲线（随后由目标格式做 sRGB 编码）。
//! 这里的 Rust 版本是 WGSL `aces` 的镜像：改一边必须改另一边，GPU 像素门禁
//! 用它对表（与旧引擎 `postfx::cpu` 的做法一致）。
//!
//! 曲线本身是 ACES 的 RRT+ODT 拟合，`1/0.6` 的亮度基准确认与旧引擎一致：
//! 它把中灰从 ~0.10 拉回 ~0.21，保住"白面 ≈ 1.0"的观感。

const ACES_INPUT_MAT: [[f32; 3]; 3] = [
    [0.59719, 0.07600, 0.02840],
    [0.35458, 0.90834, 0.13383],
    [0.04823, 0.01566, 0.83777],
];
const ACES_OUTPUT_MAT: [[f32; 3]; 3] = [
    [1.60475, -0.10208, -0.00327],
    [-0.53108, 1.10813, -0.07276],
    [-0.07367, -0.00605, 1.07602],
];

fn mat_mul(matrix: &[[f32; 3]; 3], value: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|row| {
        matrix[0][row] * value[0] + matrix[1][row] * value[1] + matrix[2][row] * value[2]
    })
}

fn rrt_and_odt_fit(value: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|index| {
        let x = value[index];
        let a = x * (x + 0.024_578_6) - 0.000_090_537;
        let b = x * (0.983_729 * x + 0.432_951) + 0.238_081;
        a / b
    })
}

/// ACES 曲线：输入线性场景光，输出显示参考线性值（未做 sRGB 编码）。
pub fn aces(linear: [f32; 3]) -> [f32; 3] {
    let scaled = linear.map(|channel| channel.max(0.0) / 0.6);
    let mapped = mat_mul(&ACES_INPUT_MAT, scaled);
    let fitted = rrt_and_odt_fit(mapped);
    let out = mat_mul(&ACES_OUTPUT_MAT, fitted);
    out.map(|channel| channel.clamp(0.0, 1.0))
}

/// 线性 → sRGB 传输函数（仅用于 8 位判据 / 断言，不参与 GPU 写入）。
pub fn srgb_encode(linear: [f32; 3]) -> [f32; 3] {
    linear.map(|x| {
        if x > 0.003_130_8 {
            1.055 * x.powf(1.0 / 2.4) - 0.055
        } else {
            x * 12.92
        }
    })
}

/// 环境反射的 split-sum 第二项 `f0·A + B`（Lazarov 2013 解析拟合）。
///
/// 与 WGSL `env_brdf_approx` 逐式一致；XGC 这一刀还没有逐材质 metallic / roughness，
/// 因此调用方统一按"介电体 + 0.5 粗糙度"传参。
pub fn env_brdf_approx(f0: [f32; 3], roughness: f32, n_dot_v: f32) -> [f32; 3] {
    let c0 = [-1.0f32, -0.0275, -0.572, 0.022];
    let c1 = [1.0f32, 0.0425, 1.04, -0.04];
    let r: [f32; 4] = std::array::from_fn(|index| roughness * c0[index] + c1[index]);
    let a004 = (r[0] * r[0]).min((-9.28 * n_dot_v).exp2()) * r[0] + r[1];
    let ab = [-1.04 * a004 + r[2], 1.04 * a004 + r[3]];
    std::array::from_fn(|channel| f0[channel] * ab[0] + ab[1])
}

/// 各向同性环境反射：`env × brdf(f0=0.04, roughness=0.5)`（与着色器同参）。
pub fn env_specular(env: [f32; 3], n_dot_v: f32) -> [f32; 3] {
    let brdf = env_brdf_approx([0.04; 3], 0.5, n_dot_v);
    std::array::from_fn(|channel| env[channel] * brdf[channel])
}

/// 曲线 + sRGB 编码后的 8 位通道值，供 sRGB 目标 / 证据脚本对表。
pub fn display_u8(linear: [f32; 3]) -> [u8; 3] {
    srgb_encode(aces(linear)).map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// 只过曲线、不做 sRGB 编码的 8 位通道值：`rgba8unorm` 目标（无硬件编码）对表用。
pub fn aces_u8(linear: [f32; 3]) -> [u8; 3] {
    aces(linear).map(|x| (x * 255.0).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::{aces, display_u8, srgb_encode};

    #[test]
    fn aces_maps_black_to_black_and_compresses_white() {
        assert_eq!(aces([0.0; 3]), [0.0; 3]);
        let white = aces([1.0; 3])[0];
        assert!(white < 1.0 && white > 0.5, "白面必须压到 (0.5,1)：{white}");
    }

    #[test]
    fn aces_is_monotonic() {
        let mut previous = 0.0;
        for step in 0..=40 {
            let x = step as f32 * 0.1;
            let y = aces([x; 3])[0];
            assert!(y >= previous - 1e-6, "x={x} 处曲线回退");
            previous = y;
        }
    }

    #[test]
    fn aces_keeps_mid_tone_calibration() {
        // 1/0.6 基准确认：中灰 0.18 进曲线后仍在中间调（旧引擎同一条判据）。
        let mid = aces([0.18; 3])[0];
        assert!((0.2..0.45).contains(&mid), "中灰映射到 {mid}");
    }

    #[test]
    fn srgb_encode_round_trips_known_values() {
        assert!((srgb_encode([0.0; 3])[0]).abs() < 1e-6);
        let white = srgb_encode([1.0; 3])[0];
        assert!((white - 1.0).abs() < 1e-6);
        // 0.5 线性 → ACES 0.5576 → sRGB 0.773 → 8 位 197。
        assert_eq!(display_u8([0.5; 3])[0], 197);
        // 曲线本体（不做 sRGB 编码）在 unorm 目标上就是 8 位 142。
        assert_eq!(super::aces_u8([0.5; 3])[0], 142);
    }
}
