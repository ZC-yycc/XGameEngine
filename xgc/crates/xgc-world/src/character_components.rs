//! 角色控制器组件：胶囊参数与移动约束。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterController {
    pub height: f32,
    pub radius: f32,
    pub step_offset: f32,
    pub slope_limit: f32,
    pub skin_width: f32,
    pub snap_to_ground: f32,
    pub jump_velocity: f32,
    pub jump: bool,
    pub grounded: bool,
}

impl Default for CharacterController {
    fn default() -> Self {
        Self {
            height: HEIGHT_DEFAULT,
            radius: RADIUS_DEFAULT,
            step_offset: STEP_OFFSET_DEFAULT,
            slope_limit: SLOPE_LIMIT_DEFAULT,
            skin_width: SKIN_WIDTH_DEFAULT,
            snap_to_ground: SNAP_TO_GROUND_DEFAULT,
            jump_velocity: 5.0,
            jump: false,
            grounded: false,
        }
    }
}

/// 总高度缺省（米，含两端半球）。
///
/// 下面这组缺省值**逐项对齐旧引擎** `xgame_core::CharacterController`
/// （`crates/xgame-core/src/character.rs` 的 `default_*`）：割接前两边读同一份
/// 场景数据，缺省值不一致会让"没写这些字段"的角色在两个引擎里走出不同的轨迹。
pub const HEIGHT_DEFAULT: f32 = 1.8;
/// 胶囊半径缺省（米）。
pub const RADIUS_DEFAULT: f32 = 0.3;
/// 自动上台阶高度缺省（米）：0.35 ≈ 一级常见楼梯踏步高。
pub const STEP_OFFSET_DEFAULT: f32 = 0.35;
/// 可站立坡度缺省（度）：45° 是"能走上去"与"会滑下来"的直觉分界。
pub const SLOPE_LIMIT_DEFAULT: f32 = 45.0;
/// 皮肤宽度缺省（米）：0.02 够小到看不见，又不会因浮点抖动贴墙颤动。
pub const SKIN_WIDTH_DEFAULT: f32 = 0.02;
/// 贴地吸附距离缺省（米）：走下小台阶 / 缓坡时向下多探这么多。
pub const SNAP_TO_GROUND_DEFAULT: f32 = 0.3;

/// 非有限值（NaN / ±inf）兜底成 `fallback`。
///
/// 与旧端同一理由：`f32::clamp` 对 NaN 的 self 原样返回 NaN，而 `sanitized`
/// 里后一个字段的 clamp 下限由前一个字段算出，NaN 会一路漏到底限参数上触发
/// clamp 自己的 panic —— 作者手滑写 nan 不能让装配期炸掉。
fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

impl CharacterController {
    /// 胶囊的**半高**（米）：`(height - 2r) / 2`，退化配置压回 0（退化成球）。
    pub fn half_height(&self) -> f32 {
        ((self.height - 2.0 * self.radius) * 0.5).max(0.0)
    }

    /// 胶囊中心相对实体原点（脚底）的高度（米）。
    pub fn center_height(&self) -> f32 {
        self.half_height() + self.radius
    }

    /// 把作者可能写坏的值钳回合法区间（纯函数，返回清洗后的副本）。
    ///
    /// 钳制策略与旧端逐条一致：非有限值落回缺省、负数归零、上界取"物理上仍
    /// 稳定"的值。场景文件是手写的，`height < 2·radius` 会让胶囊线段长度为负、
    /// 直接把 NaN 写进变换，整个模拟再也回不来。
    pub fn sanitized(&self) -> Self {
        let mut c = *self;
        c.radius = finite_or(c.radius, RADIUS_DEFAULT).clamp(RADIUS_MIN, RADIUS_MAX);
        // 上限也取 floor 与 HEIGHT_MAX 的较大者：两个常量之间的隐含关系
        // （`2·RADIUS_MAX + 1e-3 <= HEIGHT_MAX`）由文件顶部的编译期断言钉住，
        // runtime 不能因为有人调大 RADIUS_MAX 就 panic —— 坏数据不该炸装配期。
        let floor = 2.0 * c.radius + 1e-3;
        c.height = finite_or(c.height, HEIGHT_DEFAULT).clamp(floor, HEIGHT_MAX.max(floor));
        c.step_offset = finite_or(c.step_offset, STEP_OFFSET_DEFAULT).clamp(0.0, 1.0);
        // 上限 89° 而不是 90°：90° 的余弦在 f32 上是 ~-6e-8，坡度比较会把
        // 垂直墙也算成可爬。
        c.slope_limit = finite_or(c.slope_limit, SLOPE_LIMIT_DEFAULT).clamp(0.0, SLOPE_LIMIT_MAX);
        c.skin_width =
            finite_or(c.skin_width, SKIN_WIDTH_DEFAULT).clamp(SKIN_WIDTH_MIN, SKIN_WIDTH_MAX);
        c.snap_to_ground =
            finite_or(c.snap_to_ground, SNAP_TO_GROUND_DEFAULT).clamp(0.0, SNAP_TO_GROUND_MAX);
        c
    }
}

/// 半径下限 / 上限（米）。
pub const RADIUS_MIN: f32 = 0.05;
/// 半径上限（米）。
pub const RADIUS_MAX: f32 = 2.0;
/// 总高度上限（米）。
pub const HEIGHT_MAX: f32 = 5.0;
/// 皮肤宽度下限（米）。
pub const SKIN_WIDTH_MIN: f32 = 1e-3;
/// 皮肤宽度上限（米）。
pub const SKIN_WIDTH_MAX: f32 = 0.5;
/// 坡度上限（度）。
pub const SLOPE_LIMIT_MAX: f32 = 89.0;
/// 贴地吸附上限（米）。
pub const SNAP_TO_GROUND_MAX: f32 = 2.0;

// 这几个区间常量互相咬合：`height` 的下限由 radius 算出，一旦
// `2·RADIUS_MAX + 1e-3 > HEIGHT_MAX`，`f32::clamp(min, max)` 会因 min > max panic，
// 而且坏数据反而炸得更狠。用编译期断言把关系钉死（改错常量直接编不过）。
const _: () = {
    assert!(2.0 * RADIUS_MAX + 1e-3 <= HEIGHT_MAX);
    assert!(RADIUS_MIN < RADIUS_MAX);
    assert!(SKIN_WIDTH_MIN < SKIN_WIDTH_MAX);
    // 上界为负会让 `clamp(0.0, 负值)` 的 min > max；90° 那条是语义上限（90° 的
    // 余弦在 f32 上约为 -6e-8，垂直墙会被算成可爬），不是 min / max 关系。
    assert!(SLOPE_LIMIT_MAX >= 0.0 && SLOPE_LIMIT_MAX <= 90.0);
    assert!(SNAP_TO_GROUND_MAX >= 0.0);
};

impl Component for CharacterController {
    const ID: ComponentId = ComponentId(0x0402);
    const NAME: &'static str = "physics.character";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "角色控制器：胶囊高度 / 半径、台阶、坡度、皮肤宽度、贴地吸附"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("height", FieldType::F32, Value::F64(1.8)).doc("总高度（米）"),
            FieldDescriptor::new("radius", FieldType::F32, Value::F64(0.3)).doc("胶囊半径（米）"),
            FieldDescriptor::new("step_offset", FieldType::F32, Value::F64(0.35))
                .doc("自动上台阶最大高度（米）"),
            FieldDescriptor::new("slope_limit", FieldType::F32, Value::F64(45.0))
                .doc("可站立最大坡度（度）"),
            FieldDescriptor::new("skin_width", FieldType::F32, Value::F64(0.02))
                .doc("与障碍物保持的皮肤宽度（米）"),
            FieldDescriptor::new("snap_to_ground", FieldType::F32, Value::F64(0.3))
                .doc("贴地吸附距离（米）"),
            FieldDescriptor::new("jump_velocity", FieldType::F32, Value::F64(5.0))
                .doc("起跳竖直速度（米/秒）"),
            FieldDescriptor::new("jump", FieldType::Bool, Value::Bool(false))
                .doc("起跳请求；系统消费后清零"),
            FieldDescriptor::new("grounded", FieldType::Bool, Value::Bool(false))
                .doc("运行时是否踩地"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "height" => Some(Value::F64(f64::from(self.height))),
            "radius" => Some(Value::F64(f64::from(self.radius))),
            "step_offset" => Some(Value::F64(f64::from(self.step_offset))),
            "slope_limit" => Some(Value::F64(f64::from(self.slope_limit))),
            "skin_width" => Some(Value::F64(f64::from(self.skin_width))),
            "snap_to_ground" => Some(Value::F64(f64::from(self.snap_to_ground))),
            "jump_velocity" => Some(Value::F64(f64::from(self.jump_velocity))),
            "jump" => Some(Value::Bool(self.jump)),
            "grounded" => Some(Value::Bool(self.grounded)),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        let assign = |slot: &mut f32| {
            *slot = value.as_f32().ok_or_else(|| {
                format!(
                    "physics.character.{field} 期望 f32，得到 {}",
                    value.type_name()
                )
            })?;
            Ok(())
        };
        match field {
            "height" => assign(&mut self.height),
            "radius" => assign(&mut self.radius),
            "step_offset" => assign(&mut self.step_offset),
            "slope_limit" => assign(&mut self.slope_limit),
            "skin_width" => assign(&mut self.skin_width),
            "snap_to_ground" => assign(&mut self.snap_to_ground),
            "jump_velocity" => assign(&mut self.jump_velocity),
            "jump" => {
                self.jump = value.as_bool().ok_or_else(|| {
                    format!(
                        "physics.character.jump 期望 bool，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "grounded" => {
                self.grounded = value.as_bool().ok_or_else(|| {
                    format!(
                        "physics.character.grounded 期望 bool，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            _ => Err(format!("physics.character 没有字段 {field}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 缺省值必须逐项等于旧引擎 `CharacterController` 的 `default_*`。
    ///
    /// 割接前两个引擎读同一批场景数据，"没写这些字段"的角色必须走出同样的轨迹；
    /// 缺省值一旦漂移，同一份场景在旧端与 XGC 里就是两个游戏。
    #[test]
    fn defaults_match_the_old_engine() {
        let c = CharacterController::default();
        assert_eq!(c.height, 1.8);
        assert_eq!(c.radius, 0.3);
        assert_eq!(c.step_offset, 0.35);
        assert_eq!(c.slope_limit, 45.0);
        assert_eq!(c.skin_width, 0.02);
        assert_eq!(c.snap_to_ground, 0.3);
    }

    /// 非有限值逐项落回**该字段**的缺省：`f32::clamp` 对 NaN 原样返回，
    /// 不先兜底的话 NaN 会漏到"后一字段下限由前一字段算出"的 clamp 上。
    #[test]
    fn sanitized_falls_back_for_non_finite_values() {
        let messy = CharacterController {
            height: f32::INFINITY,
            radius: f32::NAN,
            step_offset: f32::NEG_INFINITY,
            slope_limit: f32::NAN,
            skin_width: f32::NAN,
            snap_to_ground: f32::INFINITY,
            ..CharacterController::default()
        };
        let c = messy.sanitized();
        assert_eq!(c.radius, RADIUS_DEFAULT);
        assert_eq!(c.height, HEIGHT_DEFAULT);
        assert_eq!(c.step_offset, STEP_OFFSET_DEFAULT);
        assert_eq!(c.slope_limit, SLOPE_LIMIT_DEFAULT);
        assert_eq!(c.skin_width, SKIN_WIDTH_DEFAULT);
        assert_eq!(c.snap_to_ground, SNAP_TO_GROUND_DEFAULT);
    }

    /// 越界值按旧端区间钳制（负数归零、上界取"物理上仍稳定"的值）。
    #[test]
    fn sanitized_clamps_out_of_range_values() {
        let clamped = CharacterController {
            radius: -1.0,
            height: 15.0,
            step_offset: -1.0,
            slope_limit: 1000.0,
            skin_width: 0.0001,
            snap_to_ground: 1000.0,
            ..CharacterController::default()
        }
        .sanitized();
        // 期望值写旧端的区间字面值，不引用实现常量：常量本身被改错时这里也要红。
        assert_eq!(clamped.radius, 0.05);
        assert_eq!(clamped.height, 5.0);
        assert_eq!(clamped.step_offset, 0.0);
        assert_eq!(clamped.slope_limit, 89.0);
        assert_eq!(clamped.skin_width, 1e-3);
        assert_eq!(clamped.snap_to_ground, 2.0);

        let big = CharacterController {
            radius: 15.0,
            height: 1.0,
            step_offset: 15.0,
            slope_limit: -5.0,
            skin_width: 5.0,
            snap_to_ground: -1.0,
            ..CharacterController::default()
        }
        .sanitized();
        assert_eq!(big.radius, 2.0);
        // 高度下限由（钳过的）半径算出：2 * 2.0 + 1e-3。
        assert_eq!(big.height, 2.0 * 2.0 + 1e-3);
        assert_eq!(big.step_offset, 1.0);
        assert_eq!(big.slope_limit, 0.0);
        assert_eq!(big.skin_width, 0.5);
        assert_eq!(big.snap_to_ground, 0.0);
    }

    /// 胶囊几何派生量：缺省 1.8 / 0.3 对应半高 0.6、中心高 0.9（脚底为原点）。
    #[test]
    fn derived_quantities_follow_the_sanitized_definition() {
        let c = CharacterController::default();
        // 1.8 / 0.3 在 f32 里算出来是 0.59999996 / 0.89999998，用容差比较。
        assert!((c.half_height() - 0.6).abs() < 1e-6);
        assert!((c.center_height() - 0.9).abs() < 1e-6);

        // 半径撑满高度区间下界时，轴线长度退化成 0（胶囊变球），不许出现负半高。
        let squashed = CharacterController {
            radius: 2.0,
            height: 1.0,
            ..CharacterController::default()
        }
        .sanitized();
        assert_eq!(squashed.height, 2.0 * squashed.radius + 1e-3);
        assert!(squashed.half_height() >= 0.0);
        assert_eq!(
            squashed.center_height(),
            squashed.half_height() + squashed.radius
        );
    }

    /// schema 导出的缺省值必须与 `CharacterController::default()` 逐字段一致。
    ///
    /// 编辑器「加组件」按 schema 缺省值建 `ComponentData`，运行期工厂走 `Default`：
    /// 两处一旦分叉，编辑器放下的角色与代码里的角色就是两套参数，而且谁都不会报错。
    #[test]
    fn schema_defaults_match_the_component_defaults() {
        let component = CharacterController::default();
        for field in CharacterController::fields() {
            assert_eq!(
                field.default.as_f32(),
                component.get_field(field.name).and_then(|v| v.as_f32()),
                "schema 缺省值与 Default 不一致：{}",
                field.name
            );
        }
    }

    /// 幂等：清洗一次之后字段都在合法区间内，再清洗不会再变。
    #[test]
    fn sanitized_is_idempotent() {
        let messy = CharacterController {
            height: f32::NAN,
            radius: -3.0,
            step_offset: 7.0,
            slope_limit: f32::NEG_INFINITY,
            skin_width: 9.0,
            snap_to_ground: -2.0,
            ..CharacterController::default()
        };
        let once = messy.sanitized();
        assert_eq!(once.sanitized(), once);
    }
}
