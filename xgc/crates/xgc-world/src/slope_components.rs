//! 斜坡地面：X 轴方向线性抬升，用于角色贴地与台阶之外的连续坡面。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slope {
    pub origin: [f32; 3],
    pub size: [f32; 2],
    pub rise: f32,
}

impl Default for Slope {
    fn default() -> Self {
        Self {
            origin: [0.0; 3],
            size: [10.0, 10.0],
            rise: 0.0,
        }
    }
}

impl Slope {
    /// 斜坡在 (x, z) 处的高度；不在坡面内返回 None。
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        if self.size[0] <= 0.0 || self.size[1] <= 0.0 {
            return None;
        }
        let local_x = x - self.origin[0];
        let local_z = z - self.origin[2];
        if local_x < 0.0 || local_z < 0.0 || local_x > self.size[0] || local_z > self.size[1] {
            return None;
        }
        Some(self.origin[1] + self.rise * (local_x / self.size[0]))
    }

    /// 坡面倾角（度）：沿 +X 每行进 `size[0]` 抬升 `rise`，所以倾角就是 `atan(rise / size[0])`。
    ///
    /// 角色控制器拿它和 `physics.character.slope_limit` 比：超过上限的上升坡按墙处理，
    /// 而不是可站立地面——旧引擎同一档位语义（`KinematicCharacterController.max_slope_climb_angle`），
    /// 但旧端在坡面上会沿坡面下滑，本系统只做到"挡在立面之前"，见 STATUS 的未闭环。
    pub fn degrees(&self) -> f32 {
        if self.size[0] <= 0.0 {
            return 0.0;
        }
        (self.rise.abs() / self.size[0]).atan().to_degrees()
    }

    /// 坡面高度正好等于 `height` 处的 X 坐标（立面落点），用于把角色挡在坡面之前。
    ///
    /// 只有上升坡（`rise > 0`）有能卡住人的立面；下坡一律当地面，返回 `None`。
    /// 结果夹进坡面自己的 X 跨度：不夹的话 `origin[1]` 被抬高时（比如坡面架在 5 m
    /// 高的平台上）外推出来的"墙"会落在坡面之外好几米，把站在平地上的角色也推走。
    /// 夹取只限制墙的位置，**不**取消墙本身：坡架高台时角色即使在坡体下方（垂直
    /// 不相交），只要 X 落在跨度内仍会被挡——这是刻意保留的简化（没有真碰撞体）。
    /// 上界夹取目前不可达（调用点先按 `feet >= height` 早退），留着纯防御。
    pub fn base_x_at(&self, height: f32) -> Option<f32> {
        if self.rise <= 0.0 || self.size[0] <= 0.0 {
            return None;
        }
        let raw = self.origin[0] + (height - self.origin[1]) * self.size[0] / self.rise;
        Some(raw.clamp(self.origin[0], self.origin[0] + self.size[0]))
    }
}

impl Component for Slope {
    const ID: ComponentId = ComponentId(0x0403);
    const NAME: &'static str = "physics.slope";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "斜坡地面：沿 +X 线性抬升 rise"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("origin", FieldType::Vec3, Value::Vec3([0.0; 3])),
            FieldDescriptor::new("size", FieldType::Vec2, Value::Vec2([10.0, 10.0]))
                .doc("X / Z 方向覆盖范围"),
            FieldDescriptor::new("rise", FieldType::F32, Value::F64(0.0)).doc("X 方向总抬升"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "origin" => Some(Value::Vec3([
                f64::from(self.origin[0]),
                f64::from(self.origin[1]),
                f64::from(self.origin[2]),
            ])),
            "size" => Some(Value::Vec2([
                f64::from(self.size[0]),
                f64::from(self.size[1]),
            ])),
            "rise" => Some(Value::F64(f64::from(self.rise))),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "origin" => {
                self.origin = value.as_vec3().ok_or_else(|| {
                    format!("physics.slope.origin 期望 vec3，得到 {}", value.type_name())
                })?;
            }
            "size" => {
                self.size = value.as_vec2().ok_or_else(|| {
                    format!("physics.slope.size 期望 vec2，得到 {}", value.type_name())
                })?;
            }
            "rise" => {
                self.rise = value.as_f32().ok_or_else(|| {
                    format!("physics.slope.rise 期望 f32，得到 {}", value.type_name())
                })?;
            }
            _ => return Err(format!("physics.slope 没有字段 {field}")),
        }
        Ok(())
    }
}
