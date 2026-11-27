use xgc_base::{EntityId, EntityKey};
use xgc_reflect::{Component, ComponentId};

use super::{Archetype, World};

mod hierarchy;

pub(crate) fn apply2_mut_arch<A: Component, B: Component>(
    arch: &mut Archetype,
    f: &mut impl FnMut(EntityKey, &mut A, &mut B),
) {
    if !arch.columns.contains_key(&A::ID) || !arch.columns.contains_key(&B::ID) {
        return;
    }
    let Some(mut a) = arch.columns.remove(&A::ID) else {
        return;
    };
    let Some(mut b) = arch.columns.remove(&B::ID) else {
        arch.columns.insert(A::ID, a);
        return;
    };
    {
        let a_values = a.as_any_mut().downcast_mut::<Vec<A>>();
        let b_values = b.as_any_mut().downcast_mut::<Vec<B>>();
        if let (Some(a_values), Some(b_values)) = (a_values, b_values) {
            for ((key, a), b) in arch
                .entities
                .iter()
                .copied()
                .zip(a_values.iter_mut())
                .zip(b_values.iter_mut())
            {
                f(key, a, b);
            }
        }
    }
    arch.columns.insert(A::ID, a);
    arch.columns.insert(B::ID, b);
}

pub(crate) fn apply2_mut_arch_sync<A: Component, B: Component>(
    arch: &mut Archetype,
    f: &impl Fn(EntityKey, &mut A, &mut B),
) {
    if !arch.columns.contains_key(&A::ID) || !arch.columns.contains_key(&B::ID) {
        return;
    }
    let Some(mut a) = arch.columns.remove(&A::ID) else {
        return;
    };
    let Some(mut b) = arch.columns.remove(&B::ID) else {
        arch.columns.insert(A::ID, a);
        return;
    };
    {
        let a_values = a.as_any_mut().downcast_mut::<Vec<A>>();
        let b_values = b.as_any_mut().downcast_mut::<Vec<B>>();
        if let (Some(a_values), Some(b_values)) = (a_values, b_values) {
            for ((key, a), b) in arch
                .entities
                .iter()
                .copied()
                .zip(a_values.iter_mut())
                .zip(b_values.iter_mut())
            {
                f(key, a, b);
            }
        }
    }
    arch.columns.insert(A::ID, a);
    arch.columns.insert(B::ID, b);
}

pub(crate) fn apply3_mut_arch<A: Component, B: Component, C: Component>(
    arch: &mut Archetype,
    f: &mut impl FnMut(EntityKey, &mut A, &mut B, &mut C),
) {
    if !arch.columns.contains_key(&A::ID)
        || !arch.columns.contains_key(&B::ID)
        || !arch.columns.contains_key(&C::ID)
    {
        return;
    }
    let Some(mut a) = arch.columns.remove(&A::ID) else {
        return;
    };
    let Some(mut b) = arch.columns.remove(&B::ID) else {
        arch.columns.insert(A::ID, a);
        return;
    };
    let Some(mut c) = arch.columns.remove(&C::ID) else {
        arch.columns.insert(A::ID, a);
        arch.columns.insert(B::ID, b);
        return;
    };
    {
        let a_values = a.as_any_mut().downcast_mut::<Vec<A>>();
        let b_values = b.as_any_mut().downcast_mut::<Vec<B>>();
        let c_values = c.as_any_mut().downcast_mut::<Vec<C>>();
        if let (Some(a_values), Some(b_values), Some(c_values)) = (a_values, b_values, c_values) {
            for (((key, a), b), c) in arch
                .entities
                .iter()
                .copied()
                .zip(a_values.iter_mut())
                .zip(b_values.iter_mut())
                .zip(c_values.iter_mut())
            {
                f(key, a, b, c);
            }
        }
    }
    arch.columns.insert(A::ID, a);
    arch.columns.insert(B::ID, b);
    arch.columns.insert(C::ID, c);
}

impl World {
    /// 枚举持久 ID，顺序由 archetype 决定。组件条件可整组跳过不匹配的行。
    pub fn entity_ids(
        &self,
        component: Option<ComponentId>,
    ) -> impl Iterator<Item = EntityId> + '_ {
        self.archetypes
            .iter()
            .filter(move |arch| component.is_none_or(|id| arch.columns.contains_key(&id)))
            .flat_map(|arch| arch.entities.iter().copied())
            .filter_map(|key| self.entity(key))
    }

    /// 双组件不可变遍历；只访问同时含 `A`、`B` 的 archetype。
    pub fn for_each2<A: Component, B: Component>(&self, mut f: impl FnMut(EntityKey, &A, &B)) {
        for arch in &self.archetypes {
            let (Some(a), Some(b)) = (arch.columns.get(&A::ID), arch.columns.get(&B::ID)) else {
                continue;
            };
            let (Some(a), Some(b)) = (
                a.as_any().downcast_ref::<Vec<A>>(),
                b.as_any().downcast_ref::<Vec<B>>(),
            ) else {
                continue;
            };
            for ((key, a), b) in arch.entities.iter().copied().zip(a.iter()).zip(b.iter()) {
                f(key, a, b);
            }
        }
    }

    /// 双组件可变遍历。实现上临时移出两列，遍历后放回；`f` 里不要让 panic
    /// 逃逸，否则列会留在局部变量里。
    pub fn for_each2_mut<A: Component, B: Component>(
        &mut self,
        mut f: impl FnMut(EntityKey, &mut A, &mut B),
    ) {
        assert_ne!(A::ID, B::ID, "for_each2_mut 的两个组件不能相同");
        for arch in &mut self.archetypes {
            apply2_mut_arch(arch, &mut f);
        }
    }

    /// 按 archetype 分片的并行双组件遍历。
    ///
    /// 每个 archetype 只由一个线程处理，实体之间互不重叠，所以线程数不改变结果；
    /// 顺序仍按 archetype / 行序，`state_hash` 与单线程逐位一致。
    /// 三组件可变查询；三个 ComponentId 必须互不相同。
    pub fn for_each3_mut<A: Component, B: Component, C: Component>(
        &mut self,
        mut f: impl FnMut(EntityKey, &mut A, &mut B, &mut C),
    ) {
        assert!(
            A::ID != B::ID && A::ID != C::ID && B::ID != C::ID,
            "for_each3_mut 的三个组件必须互不相同"
        );
        for arch in &mut self.archetypes {
            apply3_mut_arch(arch, &mut f);
        }
    }

    pub fn par_for_each2_mut<A: Component, B: Component>(
        &mut self,
        threads: usize,
        f: impl Fn(EntityKey, &mut A, &mut B) + Sync,
    ) {
        assert_ne!(A::ID, B::ID, "par_for_each2_mut 的两个组件不能相同");
        let threads = threads.max(1);
        if threads == 1 || self.archetypes.len() <= 1 {
            self.for_each2_mut(|key, a, b| f(key, a, b));
            return;
        }

        let chunk_size = self.archetypes.len().div_ceil(threads);
        std::thread::scope(|scope| {
            for chunk in self.archetypes.chunks_mut(chunk_size) {
                let f = &f;
                scope.spawn(move || {
                    for arch in chunk {
                        apply2_mut_arch_sync(arch, f);
                    }
                });
            }
        });
    }
}
