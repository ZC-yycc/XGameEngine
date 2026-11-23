//! 逐 draw 颜色合成：模型材质因子 → `.mat` 资产 → 实体覆盖，最后是选择高亮。
//!
//! 全部在静态 / GPU 蒙皮分流**之前**做完，两条路径共享同一份逐 draw 颜色；
//! 选择高亮放在最后**替换**颜色，不会再被资产里的零通道吞掉。

use xgc_base::EntityId;

use super::assets::MeshCache;
use super::draw::DrawList;
use super::glb::MeshError;

/// 依次乘进 `item.color`，最后按需替换选择高亮。
///
/// 叠加顺序与旧端 `MaterialParams` 完全一致：**模型材质 → `.mat` 资产 → 实体覆盖**。
/// - 基色：三者都是乘子；
/// - metallic / roughness：模型材质给底值，资产**替换**，实体覆盖再替换
///   （旧端 `with_asset` / `merged_with` 同语义；`metallic` 不在实体覆盖的字段里）。
///
/// 实体没有 `render.material` 组件时必须保留模型材质的 metallic / roughness ——
/// 用 `Material::default()` 顶掉会让 Box.glb（glTF roughness 缺省 1.0）多出镜面高光。
pub(crate) fn compose_colors(
    meshes: &mut MeshCache,
    list: &mut DrawList,
    selected: Option<EntityId>,
) -> Result<(), MeshError> {
    for item in &mut list.items {
        let asset = if item.material_asset.is_empty() {
            None
        } else {
            meshes.material(&item.material_asset)?.cloned()
        };
        let mut metallic = 0.0;
        let mut roughness = 0.45;
        if let Some(mesh) = meshes.get(&item.model)? {
            item.color = mesh.tint(item.color);
            metallic = mesh.metallic;
            roughness = mesh.roughness;
        }
        if let Some(asset) = asset {
            item.color = std::array::from_fn(|index| item.color[index] * asset.base_color[index]);
            metallic = asset.metallic;
            roughness = asset.roughness;
        }
        if item.has_material {
            roughness = item.roughness;
        }
        item.metallic = metallic;
        item.roughness = roughness;
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use xgc_world::{GlobalTransform, MeshRenderer, Transform, World};

    use super::*;
    use crate::draw::build_draw_list;

    /// 模型资源目录相对 crate 根（`cargo test` 的工作目录就是 crate 根），
    /// 与 `tests/assets.rs` 同一套约定。
    fn resources() -> &'static Path {
        Path::new("../../../projects/XGame/Resources")
    }

    /// 实体没有 `render.material` 时，材质参数必须退回**模型自带材质**：
    /// `Box.glb` 的 glTF `roughnessFactor` 缺省 = 1.0、`metallicFactor` = 0.0。
    /// 旧实现把 `Material::default().roughness = 0.5` 当成模型材质，红方块
    /// 因此比旧端多出环境镜面高光。
    #[test]
    fn model_material_fills_in_when_entity_has_no_material_component() {
        let root = resources();
        if !root.join("Box.glb").exists() {
            eprintln!("skip: 没有 Box.glb");
            return;
        }
        let mut world = World::new();
        world.spawn((
            Transform::default(),
            GlobalTransform::default(),
            MeshRenderer {
                model: "Box.glb".to_string(),
            },
        ));
        let mut list = build_draw_list(&world);
        let mut meshes = MeshCache::new(root);
        compose_colors(&mut meshes, &mut list, None).expect("合成材质失败");
        assert_eq!(list.items[0].metallic, 0.0);
        assert_eq!(list.items[0].roughness, 1.0);
    }

    /// 实体带 `render.material` 时覆盖值替换模型材质（旧端 `merged_with` 同语义）。
    #[test]
    fn entity_material_component_overrides_model_roughness() {
        let root = resources();
        if !root.join("Box.glb").exists() {
            eprintln!("skip: 没有 Box.glb");
            return;
        }
        let mut world = World::new();
        world.spawn((
            Transform::default(),
            GlobalTransform::default(),
            MeshRenderer {
                model: "Box.glb".to_string(),
            },
            xgc_world::Material {
                base_color: [1.0; 4],
                roughness: 0.2,
                asset: String::new(),
            },
        ));
        let mut list = build_draw_list(&world);
        let mut meshes = MeshCache::new(root);
        compose_colors(&mut meshes, &mut list, None).expect("合成材质失败");
        assert_eq!(list.items[0].metallic, 0.0);
        assert_eq!(list.items[0].roughness, 0.2);
    }

    /// `.mat` 资产替换模型材质的 metallic / roughness（旧端 `with_asset` 同语义）；
    /// Box.glb 模型 metallic = 0.0，资产 PolishedMetal 是 1.0。
    #[test]
    fn material_asset_replaces_model_metallic() {
        let root = resources();
        if !root.join("Box.glb").exists() || !root.join("Materials/PolishedMetal.mat").exists() {
            eprintln!("skip: 缺少 Box.glb 或 PolishedMetal.mat");
            return;
        }
        let mut world = World::new();
        world.spawn((
            Transform::default(),
            GlobalTransform::default(),
            MeshRenderer {
                model: "Box.glb".to_string(),
            },
            xgc_world::Material {
                base_color: [1.0; 4],
                roughness: 0.9,
                asset: "Materials/PolishedMetal.mat".to_string(),
            },
        ));
        let mut list = build_draw_list(&world);
        let mut meshes = MeshCache::new(root);
        compose_colors(&mut meshes, &mut list, None).expect("合成材质失败");
        assert_eq!(list.items[0].metallic, 1.0);
        // 实体覆盖仍然最后生效（旧端叠加顺序：模型 → 资产 → 覆盖）。
        assert_eq!(list.items[0].roughness, 0.9);
    }
}
