use xgc_base::{EntityId, EntityKey};

use crate::{Component, GlobalTransform, Parent, Transform, World};

impl World {
    /// 只访问带全局变换的 archetype，局部变换和父节点可以缺省。
    #[doc(hidden)]
    pub fn for_each_hierarchy_input(
        &self,
        mut visit: impl FnMut(EntityKey, EntityId, Option<&Parent>, Option<&Transform>),
    ) {
        for archetype in &self.archetypes {
            let Some(globals) = archetype.columns.get(&GlobalTransform::ID) else {
                continue;
            };
            let Some(globals) = globals.as_any().downcast_ref::<Vec<GlobalTransform>>() else {
                continue;
            };
            let parents = archetype
                .columns
                .get(&Parent::ID)
                .and_then(|column| column.as_any().downcast_ref::<Vec<Parent>>());
            let locals = archetype
                .columns
                .get(&Transform::ID)
                .and_then(|column| column.as_any().downcast_ref::<Vec<Transform>>());
            for (row, (key, _)) in archetype.entities.iter().copied().zip(globals).enumerate() {
                if let Some(id) = self.entity(key) {
                    visit(
                        key,
                        id,
                        parents.and_then(|column| column.get(row)),
                        locals.and_then(|column| column.get(row)),
                    );
                }
            }
        }
    }

    /// 按调用方提供的持久 ID 顺序写回全局变换。每份变换仍记为三次字段写入。
    #[doc(hidden)]
    pub fn write_hierarchy_globals<'a>(
        &mut self,
        values: impl IntoIterator<Item = (EntityKey, &'a GlobalTransform)>,
    ) {
        let mut column = None;
        let mut current_archetype = None;
        for (key, value) in values {
            let Some(meta) = self.metas.get_mut(key.index() as usize) else {
                continue;
            };
            if !meta.alive || meta.generation != key.generation() {
                continue;
            }
            if current_archetype != Some(meta.archetype) {
                current_archetype = Some(meta.archetype);
                column = self.archetypes[meta.archetype as usize]
                    .columns
                    .get_mut(&GlobalTransform::ID)
                    .and_then(|column| column.as_any_mut().downcast_mut::<Vec<GlobalTransform>>());
            }
            let Some(global) = column
                .as_deref_mut()
                .and_then(|column| column.get_mut(meta.row as usize))
            else {
                continue;
            };
            // 一次变换包含三次反射字段写入，每个字段都推进修订号。
            global.position = value.position.map(reflected_float);
            self.revision += 1;
            meta.revision = self.revision;
            global.rotation = value.rotation.map(reflected_float);
            self.revision += 1;
            meta.revision = self.revision;
            global.scale = value.scale.map(reflected_float);
            self.revision += 1;
            meta.revision = self.revision;
        }
    }
}

fn reflected_float(value: f32) -> f32 {
    if value.is_nan() {
        narrow_reflected_nan(f64::from(value))
    } else {
        value
    }
}

// 反射经过 Value 的 f64；折叠 NaN 的两次转换会改变 sNaN 的写回位模式。
#[cold]
#[inline(never)]
fn narrow_reflected_nan(value: f64) -> f32 {
    value as f32
}
