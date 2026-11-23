//! `xgc parity <旧图> <新图> [结构性上限%] [内部结构性上限%]`：视觉对等判定。
//!
//! `image-diff` 只报"有多少像素不同"，但两个独立渲染器在同一台机器上也不可能做到
//! 逐字节一致：±1 LSB 是浮点 → 8 位编码的舍入，轮廓附近的 ≥8 差是 MSAA 覆盖差
//! （三角边落在采样点两侧）。拿一个 3% 的总差异当门槛，会把"固有噪声"和"真的画错"
//! 混成同一个数字，于是门槛只能靠调参。
//!
//! 这里把结构性差异拆成两部分：
//! - **轮廓（edge）**：差异像素落在任一图像自身的 1px 轮廓带内 ⇒ 采样覆盖差；
//! - **内部（interior）**：不在轮廓带内的结构性差 ⇒ 着色 / 几何 / 阴影的语义差。
//!
//! 判定门槛：内部结构性 ≤ `DEFAULT_MAX_INTERIOR_PCT`、结构性 ≤ `DEFAULT_MAX_STRUCTURAL_PCT`；
//! `mean_abs` 与 `max` 只打印不判定（它们是水位不是契约）。`PARITY_EDGE_THRESHOLD`
//! 在 `xgc_render` 里定义，与"结构性 ≥8"是两个独立口径。
//!
//! 旧端参考图必须在验收机重新生成（别的 GPU / 驱动环境的舍入不同，即使轮廓带口径
//! 也可能把整幅画面顶出 ±1 噪声），规则写在 `xgc/acceptance/README.md`。

use std::path::Path;
use std::process::ExitCode;

use xgc_render::{parity_report, ImageData};

/// 结构性差异（全部，含轮廓带）的上限：0.1%。
pub const DEFAULT_MAX_STRUCTURAL_PCT: f64 = 0.1;

/// 内部结构性差异（真正的语义差）的上限：0.01%。
pub const DEFAULT_MAX_INTERIOR_PCT: f64 = 0.01;

pub fn parity(
    old: Option<&str>,
    new: Option<&str>,
    max_structural: Option<&str>,
    max_interior: Option<&str>,
) -> ExitCode {
    let (Some(old), Some(new)) = (old, new) else {
        eprintln!("用法：xgc parity <旧图> <新图> [结构性上限%] [内部结构性上限%]");
        return ExitCode::from(2);
    };
    let max_structural = match parse_limit(max_structural, DEFAULT_MAX_STRUCTURAL_PCT) {
        Ok(limit) => limit,
        Err(code) => return code,
    };
    let max_interior = match parse_limit(max_interior, DEFAULT_MAX_INTERIOR_PCT) {
        Ok(limit) => limit,
        Err(code) => return code,
    };
    let left = match ImageData::load(Path::new(old)) {
        Ok(image) => image,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    let right = match ImageData::load(Path::new(new)) {
        Ok(image) => image,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    let report = match parity_report(&left, &right) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    let structural_pct = f64::from(report.structural_ratio) * 100.0;
    let interior_pct = f64::from(report.interior_ratio) * 100.0;
    let passed = structural_pct <= max_structural && interior_pct <= max_interior;
    println!(
        "parity: {}x{} 像素，mean_abs {:.2}、max {}；结构性 {}（{:.3}%，限 {:.3}%）= 轮廓 {} + 内部 {}（{:.4}%，限 {:.4}%） -> {}",
        report.width,
        report.height,
        report.mean_abs,
        report.max_delta,
        report.structural,
        structural_pct,
        max_structural,
        report.edge_structural,
        report.interior_structural,
        interior_pct,
        max_interior,
        if passed { "通过" } else { "未通过" }
    );
    if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// 解析百分比上限：空 = 缺省；负数 / 非数字 / 非有限值 = 用法错误（退出码 2）。
pub fn parse_limit(text: Option<&str>, default: f64) -> Result<f64, ExitCode> {
    let Some(text) = text else {
        return Ok(default);
    };
    match text.parse::<f64>() {
        Ok(limit) if limit.is_finite() && limit >= 0.0 => Ok(limit),
        Ok(limit) => {
            eprintln!("百分比上限必须是非负数：{limit}");
            Err(ExitCode::from(2))
        }
        Err(_) => {
            eprintln!("百分比上限不是数字：{text}");
            Err(ExitCode::from(2))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_default_and_reject_negative_or_garbage() {
        assert_eq!(parse_limit(None, 0.1).expect("缺省"), 0.1);
        assert_eq!(parse_limit(Some("2.5"), 0.1).expect("数字"), 2.5);
        assert!(parse_limit(Some("-1"), 0.1).is_err());
        assert!(parse_limit(Some("abc"), 0.1).is_err());
        assert!(parse_limit(Some("NaN"), 0.1).is_err());
    }
}
