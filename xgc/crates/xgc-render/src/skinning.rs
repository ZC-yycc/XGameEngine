//! CPU 蒙皮：`pose[joint] * inverse_bind[joint]` 加权变换顶点。

use super::mesh::Mesh;

/// 没有蒙皮 / 逆绑定 / 姿态时返回原顶点副本。
pub fn skin_positions(mesh: &Mesh, pose: &[[f32; 16]]) -> Vec<[f32; 3]> {
    let Some(skin) = &mesh.skin else {
        return mesh.positions.clone();
    };
    if skin.inverse_bind.is_empty() || pose.is_empty() {
        return mesh.positions.clone();
    }
    let mut positions = Vec::with_capacity(mesh.positions.len());
    for (index, position) in mesh.positions.iter().enumerate() {
        let Some(joints) = skin.joints.get(index) else {
            positions.push(*position);
            continue;
        };
        let Some(weights) = skin.weights.get(index) else {
            positions.push(*position);
            continue;
        };
        let mut out = [0.0_f32; 3];
        let mut total = 0.0_f32;
        for slot in 0..4 {
            let weight = weights[slot];
            if weight <= 0.0 {
                continue;
            }
            let joint = joints[slot] as usize;
            let (Some(inverse), Some(pose)) = (skin.inverse_bind.get(joint), pose.get(joint))
            else {
                continue;
            };
            let matrix = mul(*pose, *inverse);
            let point = transform_point(matrix, *position);
            out[0] += point[0] * weight;
            out[1] += point[1] * weight;
            out[2] += point[2] * weight;
            total += weight;
        }
        if total > 0.0 {
            positions.push([out[0] / total, out[1] / total, out[2] / total]);
        } else {
            positions.push(*position);
        }
    }
    positions
}

fn transform_point(matrix: [f32; 16], point: [f32; 3]) -> [f32; 3] {
    [
        matrix[0] * point[0] + matrix[4] * point[1] + matrix[8] * point[2] + matrix[12],
        matrix[1] * point[0] + matrix[5] * point[1] + matrix[9] * point[2] + matrix[13],
        matrix[2] * point[0] + matrix[6] * point[1] + matrix[10] * point[2] + matrix[14],
    ]
}

fn mul(a: [f32; 16], b: [f32; 16]) -> [f32; 16] {
    let mut out = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[column * 4 + k];
            }
            out[column * 4 + row] = sum;
        }
    }
    out
}

/// 单位矩阵，测试 / 默认姿态共用。
pub fn identity() -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

/// 平移矩阵（列主序）。
pub fn translation(x: f32, y: f32, z: f32) -> [f32; 16] {
    let mut matrix = identity();
    matrix[12] = x;
    matrix[13] = y;
    matrix[14] = z;
    matrix
}
