//! LOD 层级按从近到远的顺序保存，模型和距离可分别编辑。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct LodLevel {
    pub model: String,
    pub max_distance: f32,
}

const DISTANCE_MIN: f64 = -(f32::MAX as f64);
const DISTANCE_MAX: f64 = f32::MAX as f64;

impl LodLevel {
    fn descriptor() -> FieldDescriptor {
        FieldDescriptor::new(
            "level",
            FieldType::Struct(vec![
                FieldDescriptor::new("model", FieldType::String, Value::String(String::new()))
                    .doc("内建图元、模型 GUID 或项目相对路径"),
                FieldDescriptor::new("max_distance", FieldType::F32, Value::F64(0.0))
                    .unit("m")
                    .range(DISTANCE_MIN, DISTANCE_MAX)
                    .doc("此级的最大距离，零或负值表示无限距离"),
            ]),
            Self::default().to_value(),
        )
    }

    fn to_value(&self) -> Value {
        Value::Struct(
            [
                ("model".into(), Value::String(self.model.clone())),
                (
                    "max_distance".into(),
                    Value::F64(f64::from(self.max_distance)),
                ),
            ]
            .into(),
        )
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        let Value::Struct(fields) = value else {
            return Err("期望 LOD 层级结构".into());
        };
        if fields.len() != 2 {
            return Err("LOD 层级结构必须只包含 model 和 max_distance".into());
        }
        let Some(model) = fields.get("model").and_then(Value::as_string) else {
            return Err("model 必须是字符串".into());
        };
        let max_distance = match fields.get("max_distance") {
            Some(Value::F64(distance)) if (DISTANCE_MIN..=DISTANCE_MAX).contains(distance) => {
                *distance as f32
            }
            _ => return Err("max_distance 必须是 f32 可表示的有限距离".into()),
        };
        Ok(Self {
            model: model.to_string(),
            max_distance,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct LodGroup {
    pub levels: Vec<LodLevel>,
}

impl Component for LodGroup {
    const ID: ComponentId = ComponentId(0x0204);
    const NAME: &'static str = "render.lod";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "由近到远的 LOD 模型与距离配置"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![FieldDescriptor::new(
            "levels",
            FieldType::List(Box::new(LodLevel::descriptor())),
            Value::List(Vec::new()),
        )
        .doc("按从近到远排列，空列表表示未配置 LOD")]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "levels" => Some(Value::List(
                self.levels.iter().map(LodLevel::to_value).collect(),
            )),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        if field != "levels" {
            return Err(format!("render.lod 没有字段 {field}"));
        }
        let Value::List(levels) = value else {
            return Err(format!(
                "render.lod.levels 期望 list，得到 {}",
                value.type_name()
            ));
        };
        self.levels = levels
            .iter()
            .enumerate()
            .map(|(index, level)| {
                LodLevel::from_value(level).map_err(|err| format!("levels[{index}]：{err}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(())
    }
}
