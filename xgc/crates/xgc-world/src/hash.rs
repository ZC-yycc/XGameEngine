//! 世界状态哈希：回放 / 交易回滚 / 确定性测试的最小判据。
//!
//! 只要求“同输入同输出”，不做密码学。遍历顺序固定：archetype 创建序、行序、
//! 组件 ID 升序、字段声明序。

use super::World;

impl World {
    pub fn state_hash(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for arch in &self.archetypes {
            let mut component_ids: Vec<_> = arch.columns.keys().copied().collect();
            component_ids.sort_by_key(|id| id.0);

            for row in 0..arch.entities.len() {
                let key = arch.entities[row];
                let id = self.metas[key.index() as usize].id;
                mix(&mut hash, id.raw());
                mix(&mut hash, 0xff);

                for component in &component_ids {
                    mix(&mut hash, u64::from(component.0));
                    let Some(column) = arch.columns.get(component) else {
                        continue;
                    };
                    for name in column.field_names() {
                        mix_str(&mut hash, name);
                        if let Some(value) = column.get_field(row, name) {
                            value.hash_into(&mut hash);
                        }
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
