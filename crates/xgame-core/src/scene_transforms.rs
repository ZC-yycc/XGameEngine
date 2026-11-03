//! 层级变换的复用工作区。只保留容量，每次调用都重建父索引，避免增删 / 重挂后用到旧关系。

use std::collections::HashMap;

use glam::Mat4;

use crate::{Entity, EntityId, Scene};

/// 同步解算世界矩阵的临时缓冲；不缓存场景状态，不参与序列化。
#[derive(Default)]
pub struct SceneTransforms {
    parents: Vec<Option<usize>>,
    matrices: Vec<Mat4>,
    index_of: HashMap<EntityId, usize>,
    state: Vec<u8>,
    chain: Vec<usize>,
}

impl SceneTransforms {
    /// 解算当前场景；结果与实体表同序同长，容量在下次调用时复用。
    pub fn update(&mut self, scene: &Scene) {
        self.prepare(&scene.entities);
        self.resolve(|index, _| scene.entities[index].transform.local_matrix());
    }

    /// 父节点先于子节点更新局部变换，回调拿到的是**已经更新过**的父世界矩阵。
    ///
    /// 动态刚体回写必须走这个顺序：先用旧父矩阵算好所有局部坐标，再统一写入，会把
    /// 父刚体本步的位移重复叠到子刚体上。普通中间节点同样参与传播，不能只遍历刚体。
    /// 回调只能改实体，不能增删实体表；结构变化在下一次调用重新解算。
    pub fn update_with(
        &mut self,
        scene: &mut Scene,
        mut update: impl FnMut(&mut Entity, Option<Mat4>),
    ) {
        self.prepare(&scene.entities);
        self.resolve(|index, parent| {
            let entity = &mut scene.entities[index];
            update(entity, parent);
            entity.transform.local_matrix()
        });
    }

    pub fn matrices(&self) -> &[Mat4] {
        &self.matrices
    }

    pub(crate) fn into_parts(self) -> (Vec<Option<usize>>, Vec<Mat4>) {
        (self.parents, self.matrices)
    }

    fn prepare(&mut self, entities: &[Entity]) {
        parent_indices_into(entities, &mut self.parents, &mut self.index_of);
        let n = entities.len();
        self.matrices.resize(n, Mat4::IDENTITY);
        self.state.clear();
        self.state.resize(n, 0);
    }

    fn resolve(&mut self, mut local: impl FnMut(usize, Option<Mat4>) -> Mat4) {
        // 迭代收链，避免深树递归爆栈；0=未访问、1=本链上、2=已解算。
        // 环与旧批量解算同语义：按实体顺序遇到的第一条回边以单位阵为基底。
        for i in 0..self.parents.len() {
            if self.state[i] == 2 {
                continue;
            }
            self.chain.clear();
            let mut current = i;
            while self.state[current] == 0 {
                self.chain.push(current);
                self.state[current] = 1;
                match self.parents[current] {
                    Some(parent) => current = parent,
                    None => break,
                }
            }
            let mut parent = self
                .chain
                .last()
                .and_then(|&last| self.parents[last])
                .filter(|&p| self.state[p] == 2)
                .map(|p| self.matrices[p]);
            for &index in self.chain.iter().rev() {
                let world = parent.unwrap_or(Mat4::IDENTITY) * local(index, parent);
                self.matrices[index] = world;
                self.state[index] = 2;
                parent = Some(world);
            }
        }
    }
}

/// 单次与复用路径共用身份规则：重复 ID 首占位，悬空父 / 自指父视为根。
pub(crate) fn parent_indices_into(
    entities: &[Entity],
    parents: &mut Vec<Option<usize>>,
    index_of: &mut HashMap<EntityId, usize>,
) {
    parents.clear();
    index_of.clear();
    if entities.iter().all(|e| e.parent.is_none()) {
        parents.resize(entities.len(), None);
        return;
    }
    index_of.reserve(entities.len());
    for (i, entity) in entities.iter().enumerate() {
        index_of.entry(entity.id).or_insert(i);
    }
    parents.extend(entities.iter().enumerate().map(|(i, entity)| {
        entity
            .parent
            .and_then(|id| index_of.get(&id).copied())
            .filter(|&p| p != i)
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};

    fn assert_matches_single(scene: &Scene, transforms: &mut SceneTransforms) {
        transforms.update(scene);
        assert_eq!(transforms.matrices().len(), scene.entities.len());
        for (index, matrix) in transforms.matrices().iter().enumerate() {
            assert!(matrix.abs_diff_eq(scene.world_matrix(index), 1e-5));
        }
    }

    #[test]
    fn workspace_rebuilds_after_reorder_reparent_replacement_and_shrink() {
        let mut scene = Scene::new("workspace");
        let mut root = Entity::new("root");
        root.transform.position = Vec3::new(1.0, 2.0, 3.0);
        root.transform.rotation = Quat::from_rotation_y(0.4);
        let root_id = scene.add_entity(root);
        let mut child = Entity::new("child");
        child.parent = Some(root_id);
        child.transform.position = Vec3::X;
        scene.add_entity(child);
        let mut transforms = SceneTransforms::default();
        assert_matches_single(&scene, &mut transforms);
        scene.entities.reverse();
        assert_matches_single(&scene, &mut transforms);
        scene.entities[0].parent = None;
        scene.entities[1].parent = Some(scene.entities[0].id);
        assert_matches_single(&scene, &mut transforms);
        scene.entities[0] = Entity::new("replacement");
        assert_matches_single(&scene, &mut transforms);
        scene.entities.truncate(1);
        assert_matches_single(&scene, &mut transforms);
        scene.entities.clear();
        assert_matches_single(&scene, &mut transforms);
    }

    #[test]
    fn repeated_ids_and_invalid_parents_follow_single_lookup_rules() {
        let mut scene = Scene::new("invalid-parents");
        let mut first = Entity::new("first");
        first.transform.position = Vec3::X;
        let mut duplicate = first.clone();
        duplicate.transform.position = Vec3::X * 10.0;
        let mut child = Entity::new("child");
        child.parent = Some(first.id);
        scene.entities = vec![first, duplicate, child];
        let mut transforms = SceneTransforms::default();
        assert_matches_single(&scene, &mut transforms);
        assert_eq!(transforms.matrices()[2].w_axis.x, 1.0);
        scene.entities[0].parent = Some(scene.entities[0].id);
        scene.entities[2].parent = Some(EntityId::new());
        assert_matches_single(&scene, &mut transforms);
    }

    #[test]
    fn cycles_do_not_reuse_a_previous_calls_world_matrix() {
        let mut scene = Scene::new("cycle");
        let mut a = Entity::new("a");
        let mut b = Entity::new("b");
        a.parent = Some(b.id);
        b.parent = Some(a.id);
        a.transform.position = Vec3::X;
        b.transform.position = Vec3::Y;
        scene.entities = vec![a, b];
        let mut transforms = SceneTransforms::default();
        for _ in 0..3 {
            transforms.update(&scene);
            assert_eq!(
                transforms.matrices()[0].w_axis.truncate(),
                Vec3::X + Vec3::Y
            );
            assert_eq!(transforms.matrices()[1].w_axis.truncate(), Vec3::Y);
        }
    }
}
