//! GPU 蒙皮 routing / palette / 姿态包围的单元回归。
//!
//! 从 `skinned.rs` 按行数预算拆出（`.rs` 文件守 600 行软上限），入口仍由
//! `skinned.rs` 的 `#[cfg(test)] mod tests;` 挂载。

use super::{
    palette_bytes, palette_fits_device, posed_bounds, supports_gpu_skin, MAX_JOINTS,
    MAX_JOINTS_BYTES,
};
use crate::{identity, skin_positions, translation, Mesh, SkinData};

fn mesh(joint: u16) -> Mesh {
    let mut mesh = Mesh::new(vec![[0.0; 3]], Vec::new());
    mesh.skin = Some(SkinData {
        joints: vec![[joint, 0, 0, 0]],
        weights: vec![[1.0, 0.0, 0.0, 0.0]],
        joint_nodes: vec![0],
        inverse_bind: vec![identity()],
    });
    mesh
}

/// 四边形，每顶点 50/50 分摊到两根骨头：用来钉"混合权重也落在保守盒内"。
fn blended_quad() -> Mesh {
    let mut mesh = Mesh::new(
        vec![
            [-0.5, -0.5, 0.0],
            [0.5, -0.5, 0.0],
            [0.5, 0.5, 0.0],
            [-0.5, 0.5, 0.0],
        ],
        vec![0, 1, 2, 0, 2, 3],
    );
    mesh.skin = Some(SkinData {
        joints: vec![[0, 1, 0, 0]; 4],
        weights: vec![[0.5, 0.5, 0.0, 0.0]; 4],
        joint_nodes: vec![0, 1],
        inverse_bind: vec![identity(), identity()],
    });
    mesh
}

#[test]
fn posed_bounds_keep_vertices_that_stay_at_bind_pose() {
    // 顶点 0 权重全零（`skin_positions` 让它留在原地），顶点 1 权重全在会平移的关节上。
    // 只并"动过的关节盒"会把顶点 0 漏在盒外。
    let mut mesh = Mesh::new(vec![[-0.5, 0.0, 0.0], [2.0, 0.0, 0.0]], vec![0, 1, 1]);
    mesh.skin = Some(SkinData {
        joints: vec![[0, 0, 0, 0], [1, 0, 0, 0]],
        weights: vec![[0.0; 4], [1.0, 0.0, 0.0, 0.0]],
        joint_nodes: vec![0, 1],
        inverse_bind: vec![identity(), identity()],
    });
    let pose = vec![identity(), translation(5.0, 0.0, 0.0)];
    let bounds = posed_bounds(&mesh, &pose).expect("有顶点");
    for position in skin_positions(&mesh, &pose) {
        assert!(
            position[0] >= bounds.min[0] - 1e-4 && position[0] <= bounds.max[0] + 1e-4,
            "顶点 {position:?} 落在姿态包围盒 {bounds:?} 之外"
        );
    }
    assert!(
        bounds.min[0] <= -0.5 + 1e-4,
        "保持绑定姿态的顶点不能被漏掉：{bounds:?}"
    );
}

#[test]
fn posed_bounds_contain_every_deformed_vertex() {
    let mesh = blended_quad();
    let pose = vec![translation(-2.0, 0.0, 0.0), translation(3.0, 1.0, 0.0)];
    let bounds = posed_bounds(&mesh, &pose).expect("有顶点");
    for position in skin_positions(&mesh, &pose) {
        for axis in 0..3 {
            assert!(
                position[axis] >= bounds.min[axis] - 1e-4
                    && position[axis] <= bounds.max[axis] + 1e-4,
                "顶点 {position:?} 落在姿态包围盒 {bounds:?} 之外（轴 {axis}）"
            );
        }
    }
}

#[test]
fn posed_bounds_follow_a_translated_palette() {
    let mesh = blended_quad();
    let bind = posed_bounds(&mesh, &[identity(), identity()]).expect("有顶点");
    let moved = posed_bounds(
        &mesh,
        &[translation(5.0, 0.0, 0.0), translation(5.0, 0.0, 0.0)],
    )
    .expect("有顶点");
    assert!(
        (moved.min[0] - (bind.min[0] + 5.0)).abs() < 1e-4,
        "{moved:?}"
    );
    assert!(
        (moved.max[0] - (bind.max[0] + 5.0)).abs() < 1e-4,
        "{moved:?}"
    );
}

#[test]
fn gpu_skin_requires_a_fully_representable_palette() {
    assert!(supports_gpu_skin(&mesh(0), &[identity()]));
    let mut empty = mesh(0);
    empty.positions.clear();
    empty.skin.as_mut().unwrap().joints.clear();
    empty.skin.as_mut().unwrap().weights.clear();
    assert!(!supports_gpu_skin(&empty, &[identity()]));
    assert!(!supports_gpu_skin(&mesh(1), &[identity()]));
    // 256 槽（旧端 BONES_PER_SLOT 同容量）以内仍走 GPU；257 才回退。
    assert!(supports_gpu_skin(&mesh(0), &vec![identity(); MAX_JOINTS]));
    assert!(!supports_gpu_skin(
        &mesh(0),
        &vec![identity(); MAX_JOINTS + 1]
    ));
    let mut missing_inverse = mesh(0);
    missing_inverse.skin.as_mut().unwrap().inverse_bind.clear();
    assert!(!supports_gpu_skin(&missing_inverse, &[identity()]));
}

#[test]
fn palette_uses_the_256_slot_capacity() {
    assert_eq!(MAX_JOINTS_BYTES, 256 * 64, "palette uniform 必须是 16 KiB");
    let bytes = palette_bytes(&[identity(); 256], &[identity(); 256]);
    assert_eq!(bytes.len(), MAX_JOINTS_BYTES);
}

#[test]
fn palette_fits_only_when_the_device_binding_allows_16k() {
    // downlevel GL 的上限正好 16 KiB：等于上限仍然合法。
    assert!(palette_fits_device(MAX_JOINTS_BYTES as u64));
    assert!(palette_fits_device(64 * 1024));
    assert!(!palette_fits_device(MAX_JOINTS_BYTES as u64 - 1));
}
