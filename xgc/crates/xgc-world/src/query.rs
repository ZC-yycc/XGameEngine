//! 多组件查询：不可变、可变、按 archetype 分片的并行版本。

use xgc_base::EntityKey;
use xgc_reflect::Component;

use super::{apply2_mut_arch, apply2_mut_arch_sync, World};

impl World {
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
