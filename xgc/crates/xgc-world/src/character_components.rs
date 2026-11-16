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
}

impl Default for CharacterController {
    fn default() -> Self {
        Self {
            height: 1.8,
            radius: 0.3,
            step_offset: 0.3,
            slope_limit: 45.0,
            skin_width: 0.01,
            snap_to_ground: 0.2,
        }
    }
}

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
            FieldDescriptor::new("step_offset", FieldType::F32, Value::F64(0.3))
                .doc("自动上台阶最大高度（米）"),
            FieldDescriptor::new("slope_limit", FieldType::F32, Value::F64(45.0))
                .doc("可站立最大坡度（度）"),
            FieldDescriptor::new("skin_width", FieldType::F32, Value::F64(0.01))
                .doc("与障碍物保持的皮肤宽度（米）"),
            FieldDescriptor::new("snap_to_ground", FieldType::F32, Value::F64(0.2))
                .doc("贴地吸附距离（米）"),
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
            _ => Err(format!("physics.character 没有字段 {field}")),
        }
    }
}
