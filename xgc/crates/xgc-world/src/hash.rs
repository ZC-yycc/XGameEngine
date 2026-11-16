//! 世界状态哈希：回放 / 交易回滚 / 确定性测试的最小判据。
//!
//! 只要求“同输入同输出”，不做密码学。实体按 `EntityId` 排序，组件 ID 升序、
//! 字段声明序；因此 `swap_remove` 的行序变化不会影响哈希。

use super::World;

impl World {
    pub fn state_hash(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut entities: Vec<(u64, u32, u32)> = self
            .metas
            .iter()
            .filter(|meta| meta.alive)
            .map(|meta| (meta.id.raw(), meta.archetype, meta.row))
            .collect();
        entities.sort_by_key(|(id, _, _)| *id);

        for (id, archetype, row) in entities {
            mix(&mut hash, id);
            let Some(arch) = self.archetypes.get(archetype as usize) else {
                continue;
            };
            let mut component_ids: Vec<_> = arch.columns.keys().copied().collect();
            component_ids.sort_by_key(|component| component.0);

            for component in &component_ids {
                mix(&mut hash, u64::from(component.0));
                let Some(column) = arch.columns.get(component) else {
                    continue;
                };
                for name in column.field_names() {
                    mix_str(&mut hash, name);
                    if let Some(value) = column.get_field(row as usize, name) {
                        value.hash_into(&mut hash);
                    }
                }
            }
        }
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
