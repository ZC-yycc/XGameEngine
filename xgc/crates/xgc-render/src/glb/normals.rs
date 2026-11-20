//! glTF `NORMAL`：每顶点法线（模型空间）。
//!
//! 只接受 float VEC3，与 `TEXCOORD_0` 的读法一致；不归一化 —— 归一化留到 GPU
//! （蒙皮 / 缩放后再归一才准，提前归一会被非均匀缩放带偏）。

use super::{accessor_layout, read_f32, MeshError, Root};

pub(super) fn read_normals(
    root: &Root,
    accessor: usize,
    bin: &[u8],
) -> Result<Vec<[f32; 3]>, MeshError> {
    let accessor = root
        .accessors
        .get(accessor)
        .ok_or_else(|| MeshError::Missing("NORMAL accessor".into()))?;
    if accessor.component_type != 5126 || accessor.ty != "VEC3" {
        return Err(MeshError::Unsupported(format!(
            "NORMAL 需要 float VEC3，实际 type {} component {}",
            accessor.ty, accessor.component_type
        )));
    }
    let (base, stride) = accessor_layout(root, accessor, 12)?;
    // 与 WEIGHTS_0 / inverseBindMatrices 同一套越界守卫：`accessor.count` 来自不可信
    // 的 JSON，直接拿去乘 stride 可能在 32 位机器上溢出、`Vec::with_capacity` 还可能
    // 直接 abort。先算出最后一条法线的末尾，超出 BIN chunk 就报错。
    let needed = base
        .checked_add(stride.saturating_mul(accessor.count.saturating_sub(1)))
        .and_then(|offset| offset.checked_add(12))
        .ok_or_else(|| MeshError::Glb("NORMAL 偏移溢出".into()))?;
    if needed > bin.len() {
        return Err(MeshError::Glb("NORMAL 超出 BIN chunk".into()));
    }
    let mut normals = Vec::with_capacity(accessor.count);
    for index in 0..accessor.count {
        let offset = base + index * stride;
        normals.push([
            read_f32(bin, offset)?,
            read_f32(bin, offset + 4)?,
            read_f32(bin, offset + 8)?,
        ]);
    }
    Ok(normals)
}

/// 缺 `NORMAL` 时按几何生成每顶点法线（与旧端 `compute_face_normals` 同式）。
///
/// 旧端 `crates/xgame-render/src/gltf_loader.rs:1335` 在模型没导法线时走这条：
/// 逐三角形求 `cross(p1 - p0, p2 - p0)`（长度即两倍面积），把同一个面法线累加到
/// 三个顶点上，最后逐顶点归一。**不是"平直着色"**——共享顶点会把相邻面的贡献相加，
/// 得到的是面积加权的平滑法线。这一点很重要：引擎内建图元（没有 `NORMAL` 流）走的是
/// 屏幕导数叉积（每像素取所在三角面的面法线），两者在折角处观感不同；要跟旧端对齐，
/// glTF 导入必须走这里。
///
/// 退化三角形贡献零向量；某顶点一条有效面都没沾到时归一化会除零，旧端落 `(0,0,1)`，
/// 这里照抄（片元还会按视角翻号，不会出现背光黑洞）。
pub(super) fn face_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0f32; 3]; positions.len()];
    for triangle in indices.chunks_exact(3) {
        let (i0, i1, i2) = (
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        );
        // 坏索引（超出 POSITION 条数）是坏资产，不是"少一条法线"：跳过这个三角形，
        // 别让加载期越界 panic，也别把别的顶点法线带偏。
        if i0 >= positions.len() || i1 >= positions.len() || i2 >= positions.len() {
            continue;
        }
        let a = [
            positions[i1][0] - positions[i0][0],
            positions[i1][1] - positions[i0][1],
            positions[i1][2] - positions[i0][2],
        ];
        let b = [
            positions[i2][0] - positions[i0][0],
            positions[i2][1] - positions[i0][1],
            positions[i2][2] - positions[i0][2],
        ];
        let normal = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        for index in [i0, i1, i2] {
            normals[index][0] += normal[0];
            normals[index][1] += normal[1];
            normals[index][2] += normal[2];
        }
    }
    for normal in &mut normals {
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        if length > 0.0 {
            normal[0] /= length;
            normal[1] /= length;
            normal[2] /= length;
        } else {
            *normal = [0.0, 0.0, 1.0];
        }
    }
    normals
}

#[cfg(test)]
mod tests {
    use super::face_normals;

    /// 单个三角形：三个顶点都拿到同一个面法线（这里是 +Z）。
    #[test]
    fn a_single_triangle_uses_its_face_normal() {
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let normals = face_normals(&positions, &[0, 1, 2]);
        for normal in normals {
            assert_eq!(
                normal,
                [0.0, 0.0, 1.0],
                "共面三角形的顶点法线必须都是面法线"
            );
        }
    }

    /// 共享一条边的两个面：共享顶点拿到两个面法线的和再归一 —— 这正是旧端
    /// "面积加权平滑"的来源，也是它跟屏幕导数叉积在折角处不同的地方。
    ///
    /// 几何：共享边 A=(-1,0,0)..B=(1,0,0)，两块翼分别朝 ±Y 折向 -Z。
    /// 两个面法线为 (0, 1, -1)/√2 与 (0, -1, -1)/√2，各带长度 2√2 的叉积；
    /// A、B 两点的和是 (0, 0, -4) → 归一 (0, 0, -1)；两块翼的尖端只有自己那一面。
    #[test]
    fn a_shared_edge_averages_the_touching_faces() {
        let positions = [
            [-1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, -1.0],
            [0.0, 1.0, -1.0],
        ];
        let normals = face_normals(&positions, &[0, 1, 2, 1, 0, 3]);
        assert_eq!(
            normals[0],
            [0.0, 0.0, -1.0],
            "共享顶点的法线必须取两个面的和"
        );
        assert_eq!(
            normals[1],
            [0.0, 0.0, -1.0],
            "共享顶点的法线必须取两个面的和"
        );
        let tip = 0.5f32.sqrt();
        assert!(
            (normals[2][1] - tip).abs() < 1e-6 && (normals[2][2] + tip).abs() < 1e-6,
            "翼尖只应拿到自己那一面的法线：{:?}",
            normals[2]
        );
        assert!(
            (normals[3][1] + tip).abs() < 1e-6 && (normals[3][2] + tip).abs() < 1e-6,
            "翼尖只应拿到自己那一面的法线：{:?}",
            normals[3]
        );

        // 面积不同的一对三角面：累加的是未归一化叉积（长度 = 两倍面积），
        // 不是「每面先归一化再相加」——后者会把大面和小面当同等权重。
        // 面 1 面积 0.5、法线 +Z；面 2 面积 1.0、法线 +Y，共享边顶点应得
        // (0, 2, 1) 归一化 = (0, 0.8944, 0.4472)；按面同权则会得 (0, 0.7071, 0.7071)。
        let uneven = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 2.0],
        ];
        let weighted = face_normals(&uneven, &[0, 1, 2, 1, 0, 3]);
        assert!(
            (weighted[0][1] - 0.894_427).abs() < 1e-5 && (weighted[0][2] - 0.447_214).abs() < 1e-5,
            "面积加权：共享边顶点必须偏向面积更大的那面：{:?}",
            weighted[0]
        );
        assert_eq!(
            weighted[0], weighted[1],
            "共享边的两个顶点必须拿到同一个面积加权和"
        );
    }

    /// 退化三角形不贡献法线；整块没有有效面时落 (0,0,1)（旧端同款兜底）。
    #[test]
    fn degenerate_geometry_falls_back_to_up() {
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        let normals = face_normals(&positions, &[0, 1, 2]);
        for normal in normals {
            assert_eq!(normal, [0.0, 0.0, 1.0], "退化三角形的法线必须落兜底值");
        }
    }

    /// 坏索引：跳过该三角形，不许越界 panic。
    #[test]
    fn out_of_range_indices_are_skipped() {
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let normals = face_normals(&positions, &[0, 1, 9]);
        for normal in normals {
            assert_eq!(normal, [0.0, 0.0, 1.0], "坏索引不能带出越界或假法线");
        }
    }
}
