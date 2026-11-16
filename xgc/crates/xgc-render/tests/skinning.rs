//! CPU 蒙皮回归。

use xgc_render::{identity, skin_positions, translation, Mesh, SkinData};

#[test]
fn skin_positions_applies_pose_and_inverse_bind() {
    let mut mesh = Mesh::new(vec![[0.0, 0.0, 0.0]], Vec::new());
    mesh.skin = Some(SkinData {
        joints: vec![[0, 0, 0, 0]],
        weights: vec![[1.0, 0.0, 0.0, 0.0]],
        joint_nodes: vec![0],
        inverse_bind: vec![identity()],
    });
    let positions = skin_positions(&mesh, &[translation(2.0, 3.0, 4.0)]);
    assert_eq!(positions, vec![[2.0, 3.0, 4.0]]);
}

#[test]
fn skin_positions_falls_back_without_skin() {
    let mesh = Mesh::new(vec![[1.0, 2.0, 3.0]], Vec::new());
    assert_eq!(
        skin_positions(&mesh, &[translation(9.0, 9.0, 9.0)]),
        vec![[1.0, 2.0, 3.0]]
    );
}
