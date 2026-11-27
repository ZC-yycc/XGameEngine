//! 已建碰撞体只更新分组，保留正在求解的刚体、速度和句柄。

use rapier3d::prelude::{Group, InteractionGroups};
use xgc_world::World;

use super::RapierWorld;

impl RapierWorld {
    pub(super) fn sync_collision_groups(&mut self, world: &World) {
        let masks = world.collision_masks();
        let filtered = !masks.all_enabled();
        if !filtered && !self.filtered {
            return;
        }
        self.filtered = filtered;
        let handles = self
            .links
            .iter()
            .map(|link| (link.key, link.collider))
            .chain(self.characters.iter().map(|link| (link.key, link.collider)))
            .chain(self.slopes.iter().map(|link| (link.key, link.collider)));
        for (key, handle) in handles {
            let groups = if filtered {
                let layer = world.collision_layer(key);
                InteractionGroups::new(
                    Group::from_bits_truncate(layer.member()),
                    Group::from_bits_truncate(masks.mask(layer)),
                )
            } else {
                InteractionGroups::all()
            };
            // QueryFilter 在查询时读取 collider flags，分组变化无须重建 BVH。
            if self.colliders[handle].collision_groups() != groups {
                self.colliders[handle].set_collision_groups(groups);
            }
        }
    }
}
