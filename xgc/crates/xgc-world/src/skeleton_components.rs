//! 骨骼：骨架根 / 关节 / 逐帧姿态矩阵。渲染蒙皮后续消费 `SkeletonPose`。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Skeleton {
    pub joint_count: u32,
}

impl Component for Skeleton {
    const ID: ComponentId = ComponentId(0x0502);
    const NAME: &'static str = "animation.skeleton";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "骨架根：关节数量；关节实体通过 Parent 挂到根下"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![FieldDescriptor::new(
            "joint_count",
            FieldType::U32,
            Value::I64(0),
        )]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        (field == "joint_count").then_some(Value::I64(i64::from(self.joint_count)))
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        if field != "joint_count" {
            return Err(format!("animation.skeleton 没有字段 {field}"));
        }
        let value = match value {
            Value::I64(value) => *value,
            other => {
                return Err(format!(
                    "animation.skeleton.joint_count 期望 i64，得到 {}",
                    other.type_name()
                ));
            }
        };
        self.joint_count = u32::try_from(value)
            .map_err(|_| "animation.skeleton.joint_count 不能为负".to_string())?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bone {
    pub index: u32,
}

impl Component for Bone {
    const ID: ComponentId = ComponentId(0x0503);
    const NAME: &'static str = "animation.bone";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "关节：在骨架中的索引；父子关系用 Parent 表达"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![FieldDescriptor::new("index", FieldType::U32, Value::I64(0))]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        (field == "index").then_some(Value::I64(i64::from(self.index)))
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        if field != "index" {
            return Err(format!("animation.bone 没有字段 {field}"));
        }
        let value = match value {
            Value::I64(value) => *value,
            other => {
                return Err(format!(
                    "animation.bone.index 期望 i64，得到 {}",
                    other.type_name()
                ));
            }
        };
        self.index =
            u32::try_from(value).map_err(|_| "animation.bone.index 不能为负".to_string())?;
        Ok(())
    }
}

/// 运行时姿态：`matrices` 是每个关节的列主序 4x4 RON 数组。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkeletonPose {
    pub matrices: String,
}

impl Component for SkeletonPose {
    const ID: ComponentId = ComponentId(0x0504);
    const NAME: &'static str = "animation.pose";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "运行时骨骼姿态：关节世界矩阵 RON 数组"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![FieldDescriptor::new(
            "matrices",
            FieldType::String,
            Value::String(String::new()),
        )]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        (field == "matrices").then_some(Value::String(self.matrices.clone()))
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        if field != "matrices" {
            return Err(format!("animation.pose 没有字段 {field}"));
        }
        self.matrices = value
            .as_string()
            .ok_or_else(|| {
                format!(
                    "animation.pose.matrices 期望 string，得到 {}",
                    value.type_name()
                )
            })?
            .to_string();
        Ok(())
    }
}
