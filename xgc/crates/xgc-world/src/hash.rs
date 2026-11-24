//! 世界状态哈希：回放 / 交易回滚 / 确定性测试的最小判据。
//!
//! 只要求“同输入同输出”，不做密码学。实体按 `EntityId` 排序，组件 ID 升序、
//! 字段声明序；因此 `swap_remove` 的行序变化不会影响哈希。

use super::{Archetype, World};

thread_local! {
    /// 按 `EntityId` 排序的实体行号缓存。`state_hash` 每帧都跑，稳态帧要为这点
    /// 排序再申请一条向量的话，热路径分配门禁就永远过不去；复用同一条 scratch
    /// 不改变输出（排序后才消费）。
    static SORTED_ROWS: std::cell::RefCell<Vec<(u64, u32, u32)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

impl Archetype {
    fn hash_layout(&self) -> &[(super::ComponentId, Vec<&'static str>)] {
        let layout = self.hash_layout.get_or_init(|| {
            let mut components: Vec<_> = self.columns.iter().collect();
            components.sort_by_key(|(component, _)| component.0);
            components
                .into_iter()
                .map(|(component, column)| (*component, column.state_field_names()))
                .collect()
        });
        // 布局在第一次哈希时定型：archetype 的列只在 spawn / 快照恢复时整份建好，
        // 查询只是把同一列临时取出再放回（且需要 &mut World），所以缓存与列数必须
        // 一致；漂移会让哈希静默漏掉组件。
        debug_assert_eq!(
            layout.len(),
            self.columns.len(),
            "哈希布局必须覆盖 archetype 的全部列"
        );
        layout
    }
}

impl World {
    pub fn state_hash(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        // 以下持着 `SORTED_ROWS` 的借用，禁止在哈希路径里回调 `state_hash`
        // （树内没有任何组件实现会在 `get_field` / `hash_into` 里碰 `World`）。
        SORTED_ROWS.with(|slot| {
            let mut entities = slot.borrow_mut();
            entities.clear();
            entities.extend(
                self.metas
                    .iter()
                    .filter(|meta| meta.alive)
                    .map(|meta| (meta.id.raw(), meta.archetype, meta.row)),
            );
            entities.sort_by_key(|(id, _, _)| *id);

            for &(id, archetype, row) in entities.iter() {
                mix(&mut hash, id);
                let Some(arch) = self.archetypes.get(archetype as usize) else {
                    continue;
                };
                for (component, names) in arch.hash_layout() {
                    mix(&mut hash, u64::from(component.0));
                    let Some(column) = arch.columns.get(component) else {
                        continue;
                    };
                    for name in names {
                        mix_str(&mut hash, name);
                        if let Some(value) = column.get_field(row as usize, name) {
                            value.hash_into(&mut hash);
                        }
                    }
                }
            }
        });
        hash
    }
}

fn mix(hash: &mut u64, value: u64) {
    *hash ^= value;
    *hash = hash.wrapping_mul(0x100_0000_01b3);
}

fn mix_str(hash: &mut u64, value: &str) {
    mix(hash, value.len() as u64);
    for byte in value.bytes() {
        mix(hash, u64::from(byte));
    }
}
