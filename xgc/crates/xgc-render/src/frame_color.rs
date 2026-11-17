//! 逐 draw 颜色合成：模型材质因子 → `.mat` 资产 → 实体覆盖，最后是选择高亮。
//!
//! 全部在静态 / GPU 蒙皮分流**之前**做完，两条路径共享同一份逐 draw 颜色；
//! 选择高亮放在最后**替换**颜色，不会再被资产里的零通道吞掉。

use xgc_base::EntityId;

use super::assets::MeshCache;
use super::draw::DrawList;
use super::glb::MeshError;

/// 依次乘进 `item.color`，最后按需替换选择高亮。
pub(crate) fn compose_colors(
    meshes: &mut MeshCache,
    list: &mut DrawList,
    selected: Option<EntityId>,
) -> Result<(), MeshError> {
    for item in &mut list.items {
        let asset_tint = if item.material_asset.is_empty() {
            None
        } else {
            meshes
                .material(&item.material_asset)?
                .map(|asset| asset.base_color)
        };
        if let Some(tint) = asset_tint {
            item.color = std::array::from_fn(|index| item.color[index] * tint[index]);
        }
        if let Some(mesh) = meshes.get(&item.model)? {
            item.color = mesh.tint(item.color);
        }
    }
    if let Some(selected) = selected {
        for item in &mut list.items {
            if item.entity == selected {
                item.color = [1.0, 0.82, 0.2, 1.0];
            }
        }
    }
    Ok(())
}
