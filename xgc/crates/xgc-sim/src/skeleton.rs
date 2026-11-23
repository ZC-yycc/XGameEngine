//! 骨架姿态：把关节 Transform 的父子链合成为逐关节世界矩阵。

use std::collections::{BTreeMap, HashMap, HashSet};

use xgc_base::EntityId;
use xgc_world::{Bone, Component, Parent, Skeleton, SkeletonPose, Transform, World};

use super::{System, SystemAccess};

#[derive(Clone, Copy)]
struct Node {
    parent_index: Option<u32>,
    local: [f32; 16],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SkeletonPoseSystem;

impl System for SkeletonPoseSystem {
    fn runs_after_host(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "skeleton_pose"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![Bone::ID, Parent::ID, Skeleton::ID, Transform::ID],
            writes: vec![SkeletonPose::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        let mut roots: HashMap<EntityId, xgc_base::EntityKey> = HashMap::new();
        world.for_each::<Skeleton>(|key, _| {
            if let Some(entity) = world.entity(key) {
                roots.insert(entity, key);
            }
        });
        if roots.is_empty() {
            return;
        }
        let mut bones: HashMap<EntityId, u32> = HashMap::new();
        world.for_each::<Bone>(|key, bone| {
            if let Some(entity) = world.entity(key) {
                bones.insert(entity, bone.index);
            }
        });
        let mut groups: BTreeMap<EntityId, Vec<(u32, Node)>> = BTreeMap::new();
        world.for_each::<Bone>(|key, bone| {
            let Some(entity) = world.entity(key) else {
                return;
            };
            let Some(transform) = world.get::<Transform>(key) else {
                return;
            };
            let local = compose(transform);
            let mut current = entity;
            let mut parent_index = None;
            let mut root = None;
            let mut visited = HashSet::new();
            while visited.insert(current) {
                let Some(current_key) = world.key(current) else {
                    break;
                };
                let Some(parent) = world.get::<Parent>(current_key) else {
                    break;
                };
                let parent_id = parent.entity;
                if roots.contains_key(&parent_id) {
                    root = Some(parent_id);
                    break;
                }
                if let Some(index) = bones.get(&parent_id) {
                    if parent_index.is_none() {
                        parent_index = Some(*index);
                    }
                    current = parent_id;
                } else {
                    break;
                }
            }
            if let Some(root) = root {
                groups.entry(root).or_default().push((
                    bone.index,
                    Node {
                        parent_index,
                        local,
                    },
                ));
            }
        });
        for (root, mut nodes) in groups {
            nodes.sort_by_key(|(index, _)| *index);
            let mut by_index = BTreeMap::new();
            for (index, node) in &nodes {
                by_index.insert(*index, *node);
            }
            let mut memo: HashMap<u32, [f32; 16]> = HashMap::new();
            let mut matrices = Vec::with_capacity(nodes.len());
            for (index, _) in &nodes {
                let matrix = resolve(*index, &by_index, &mut memo, &mut HashSet::new());
                matrices.push(matrix);
            }
            let text = ron::ser::to_string(&matrices).unwrap_or_default();
            if let Some(pose) = roots
                .get(&root)
                .and_then(|key| world.get_mut::<SkeletonPose>(*key))
            {
                pose.matrices = text;
            }
        }
    }
}

fn resolve(
    index: u32,
    nodes: &BTreeMap<u32, Node>,
    memo: &mut HashMap<u32, [f32; 16]>,
    visiting: &mut HashSet<u32>,
) -> [f32; 16] {
    if let Some(matrix) = memo.get(&index) {
        return *matrix;
    }
    let Some(node) = nodes.get(&index) else {
        return identity();
    };
    if !visiting.insert(index) {
        return identity();
    }
    let matrix = match node.parent_index.and_then(|parent| nodes.get(&parent)) {
        Some(_) => mul(
            resolve(node.parent_index.unwrap(), nodes, memo, visiting),
            node.local,
        ),
        None => node.local,
    };
    visiting.remove(&index);
    memo.insert(index, matrix);
    matrix
}

/// 解析姿态 RON；渲染 / 测试共用。
pub fn parse_pose(text: &str) -> Result<Vec<[f32; 16]>, String> {
    ron::from_str(text).map_err(|err| format!("姿态解析失败：{err}"))
}

fn compose(transform: &Transform) -> [f32; 16] {
    let q = transform.rotation;
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let xx = x * x;
    let yy = y * y;
    let zz = z * z;
    let xy = x * y;
    let xz = x * z;
    let yz = y * z;
    let wx = w * x;
    let wy = w * y;
    let wz = w * z;
    let sx = transform.scale[0];
    let sy = transform.scale[1];
    let sz = transform.scale[2];
    [
        (1.0 - 2.0 * (yy + zz)) * sx,
        (2.0 * (xy + wz)) * sx,
        (2.0 * (xz - wy)) * sx,
        0.0,
        (2.0 * (xy - wz)) * sy,
        (1.0 - 2.0 * (xx + zz)) * sy,
        (2.0 * (yz + wx)) * sy,
        0.0,
        (2.0 * (xz + wy)) * sz,
        (2.0 * (yz - wx)) * sz,
        (1.0 - 2.0 * (xx + yy)) * sz,
        0.0,
        transform.position[0],
        transform.position[1],
        transform.position[2],
        1.0,
    ]
}

fn identity() -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
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
