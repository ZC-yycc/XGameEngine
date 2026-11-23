//! 跨系统并行的写视图：把互不相交的组件列临时从世界里提取出来。
//!
//! 执行计划里同一批系统的读写集合互不相交（批次划分见 `xgc-sim`），但 Rust 的
//! 借用检查不允许把 `&mut World` 同时交给几个系统。做法是先按组件把列所有权从
//! archetype 里取出（[`World::extract_writes`]），每个系统拿一份 [`WriteSet`] 在
//! 自己的线程里写（[`WriteSet::get_mut`] / [`WriteSet::for_each_mut`]），最后统一
//! 塞回（[`World::restore_writes`]）。
//!
//! 刻意的取舍：
//!
//! - 提取期间世界读不到这些列（`get` / `for_each` 返回空）。这是硬约束：并行窗口
//!   内读同一列就会看到撕裂状态，所以宁可让「世界读不到」成为可观测事实。
//! - 提取 / 还原只是搬列，不写值，因此不推进 revision、不动实体表；真正的写入沿用
//!   `get_mut` / `for_each_mut` 语义（与串行路径一样不 bump revision）。
//! - 不用 unsafe：列是 `Box<dyn Column>`，提取后世界侧为空、`WriteSet` 侧独占，
//!   所有权清楚，编译器能证明不相交。
//! - 提取期间**不允许增删实体**（spawn / despawn / 换组件都会改行号）：还原时用
//!   archetype 数量与列长度做断言，违约就 panic 而不是静默错位。

use std::collections::{BTreeMap, HashMap};

use xgc_base::EntityKey;
use xgc_reflect::ComponentId;

use crate::{Column, Component, World};

/// 一批组件的写视图：从世界里临时提取出来的列 + 实体行号索引。
pub struct WriteSet {
    /// 组件 ID → 每个 archetype 的列（`None` = 该 archetype 没有这个组件）。
    columns: BTreeMap<ComponentId, Vec<Option<Box<dyn Column>>>>,
    /// 组件 ID → 实体 → (archetype 下标, 行号)，给 [`WriteSet::get_mut`] 用。
    rows: BTreeMap<ComponentId, HashMap<EntityKey, (usize, usize)>>,
    /// 每个 archetype 的实体顺序快照，给遍历配对键用（提取期间实体表不允许变）。
    entities: Vec<Vec<EntityKey>>,
}

impl WriteSet {
    /// 这份视图是否包含某个组件的列。
    pub fn contains(&self, component: ComponentId) -> bool {
        self.columns.contains_key(&component)
    }

    /// 包含的组件数量（不是实体数量）。
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// 按句柄取一行可变引用。缺实体 / 缺组件 / 未提取该组件都返回 None。
    pub fn get_mut<T: Component>(&mut self, key: EntityKey) -> Option<&mut T> {
        let (archetype, row) = *self.rows.get(&T::ID)?.get(&key)?;
        let column = self.columns.get_mut(&T::ID)?.get_mut(archetype)?.as_mut()?;
        column.as_any_mut().downcast_mut::<Vec<T>>()?.get_mut(row)
    }

    /// 单组件可变遍历。顺序与 `World::for_each_mut` 一致（按 archetype、行号升序）。
    pub fn for_each_mut<T: Component>(&mut self, mut f: impl FnMut(EntityKey, &mut T)) {
        let Self {
            columns, entities, ..
        } = self;
        let Some(parts) = columns.get_mut(&T::ID) else {
            return;
        };
        for (archetype, part) in parts.iter_mut().enumerate() {
            let Some(column) = part.as_mut() else {
                continue;
            };
            let Some(values) = column.as_any_mut().downcast_mut::<Vec<T>>() else {
                continue;
            };
            for (key, value) in entities[archetype].iter().copied().zip(values.iter_mut()) {
                f(key, value);
            }
        }
    }
}

impl World {
    /// 把 `components` 的列从世界里取出，装进一份写视图。
    ///
    /// 只允许给执行计划里「写集合互不相交」的系统用；同一实体上的两个组件分属
    /// 两份视图时才能并行写（这正是跨系统并行的前提）。组件 ID 重复会被忽略。
    pub fn extract_writes(&mut self, components: &[ComponentId]) -> WriteSet {
        let entities: Vec<Vec<EntityKey>> = self
            .archetypes
            .iter()
            .map(|archetype| archetype.entities.clone())
            .collect();
        let archetypes = self.archetypes.len();
        let mut columns = BTreeMap::new();
        let mut rows = BTreeMap::new();
        for &component in components {
            if columns.contains_key(&component) {
                continue;
            }
            let mut parts = Vec::with_capacity(archetypes);
            let mut index = HashMap::new();
            for (archetype, keys) in entities.iter().enumerate() {
                let part = self.archetypes[archetype].columns.remove(&component);
                if part.is_some() {
                    for (row, key) in keys.iter().copied().enumerate() {
                        index.insert(key, (archetype, row));
                    }
                }
                parts.push(part);
            }
            columns.insert(component, parts);
            rows.insert(component, index);
        }
        WriteSet {
            columns,
            rows,
            entities,
        }
    }

    /// 把写视图塞回世界。要求提取期间实体表只被写过值、没有增删。
    pub fn restore_writes(&mut self, set: WriteSet) {
        let WriteSet {
            columns, entities, ..
        } = set;
        for (component, parts) in columns {
            assert_eq!(
                self.archetypes.len(),
                parts.len(),
                "提取期间 archetype 数量变了：并行窗口内不允许增删实体"
            );
            for ((archetype, part), keys) in parts.into_iter().enumerate().zip(entities.iter()) {
                let Some(column) = part else {
                    continue;
                };
                assert_eq!(
                    column.len(),
                    keys.len(),
                    "提取期间实体行数变了：并行窗口内不允许增删实体"
                );
                self.archetypes[archetype].columns.insert(component, column);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::{Transform, Velocity};

    /// 造一个含 `count` 个 (Transform, Velocity) 实体的世界，位置 / 速度带初值。
    fn seeded(count: usize) -> (World, Vec<EntityKey>) {
        let mut world = World::new();
        let keys = (0..count)
            .map(|index| {
                let transform = Transform {
                    position: [index as f32; 3],
                    ..Transform::default()
                };
                let velocity = Velocity {
                    linear: [index as f32 * 0.5, 0.0, 0.0],
                };
                world.spawn((transform, velocity))
            })
            .collect();
        (world, keys)
    }

    #[test]
    fn extract_hides_columns_until_restore() {
        let (mut world, keys) = seeded(4);
        let before = world.state_hash();
        assert!(world.get::<Transform>(keys[0]).is_some());

        let set = world.extract_writes(&[Transform::ID]);
        assert!(set.contains(Transform::ID));
        assert_eq!(set.len(), 1);
        assert!(world.get::<Transform>(keys[0]).is_none());
        assert!(world.get::<Velocity>(keys[0]).is_some());

        world.restore_writes(set);
        assert!(world.get::<Transform>(keys[0]).is_some());
        assert_eq!(world.state_hash(), before);
    }

    #[test]
    fn extract_does_not_bump_the_revision() {
        let (mut world, _) = seeded(2);
        let before = world.revision();
        let set = world.extract_writes(&[Transform::ID]);
        assert_eq!(world.revision(), before);
        world.restore_writes(set);
        assert_eq!(world.revision(), before);
    }

    #[test]
    fn parallel_disjoint_writes_match_the_serial_path() {
        let (mut world, _) = seeded(64);
        let mut transforms = world.extract_writes(&[Transform::ID]);
        let mut velocities = world.extract_writes(&[Velocity::ID]);
        let moved = {
            thread::scope(|scope| {
                scope.spawn(|| {
                    transforms.for_each_mut::<Transform>(|_, transform| {
                        transform.position[0] += 1.0;
                    });
                });
                scope.spawn(|| {
                    velocities.for_each_mut::<Velocity>(|_, velocity| velocity.linear[1] += 2.0);
                });
            });
            (transforms, velocities)
        };
        world.restore_writes(moved.0);
        world.restore_writes(moved.1);

        let (mut expected, _) = seeded(64);
        expected.for_each_mut::<Transform>(|_, transform| transform.position[0] += 1.0);
        expected.for_each_mut::<Velocity>(|_, velocity| velocity.linear[1] += 2.0);
        assert_eq!(world.state_hash(), expected.state_hash());
    }

    #[test]
    fn write_set_rejects_unknown_entities_and_components() {
        let (mut world, keys) = seeded(2);
        let mut set = world.extract_writes(&[Transform::ID]);

        assert!(set.get_mut::<Transform>(keys[0]).is_some());
        assert!(set.get_mut::<Velocity>(keys[0]).is_none());
        assert!(set.get_mut::<Transform>(EntityKey::new(99, 0)).is_none());
        assert!(!set.contains(Velocity::ID));
    }

    #[test]
    fn for_each_mut_keeps_the_world_order() {
        let (mut world, _) = seeded(5);
        let mut expected = Vec::new();
        world.for_each::<Transform>(|key, _| expected.push(key));

        let mut set = world.extract_writes(&[Transform::ID]);
        let mut actual = Vec::new();
        set.for_each_mut::<Transform>(|key, _| actual.push(key));
        assert_eq!(actual, expected);
    }
}
