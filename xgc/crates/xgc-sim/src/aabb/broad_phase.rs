use super::BodyBox;

/// 平衡树的叶子按空间划分，查询结果按实体顺序交给窄相。分离后只更新受影响的祖先。
#[derive(Clone, Debug, Default)]
pub(super) struct BroadPhase {
    nodes: Vec<Node>,
    leaves: Vec<usize>,
    order: Vec<usize>,
    pub(super) candidates: Vec<usize>,
}

impl BroadPhase {
    pub(super) fn rebuild(&mut self, bodies: &[BodyBox]) {
        self.nodes.clear();
        self.leaves.resize(bodies.len(), 0);
        self.order.clear();
        self.order.extend(0..bodies.len());
        build(bodies, &mut self.order, &mut self.nodes, &mut self.leaves);
    }

    pub(super) fn find_after(&mut self, body: usize, after: usize, dynamic: bool) {
        self.candidates.clear();
        let bounds = self.nodes[self.leaves[body]].bounds;
        // 根结点最后建成。遍历只产生可能接触的叶子，求解顺序独立于空间划分。
        self.nodes[self.nodes.len() - 1].collect(
            &self.nodes,
            bounds,
            after,
            dynamic,
            &mut self.candidates,
        );
        self.candidates.sort_unstable();
    }

    /// 返回包围范围是否变了；左侧范围不变时，剩余候选仍然有效。
    pub(super) fn update(&mut self, body: usize, data: &BodyBox) -> bool {
        let leaf = self.leaves[body];
        let bounds = Bounds::around(data);
        if self.nodes[leaf].bounds == bounds {
            return false;
        }
        self.nodes[leaf].bounds = bounds;
        let mut parent = self.nodes[leaf].parent;
        while let Some(index) = parent {
            let NodeKind::Branch { left, right } = self.nodes[index].kind else {
                unreachable!("只有分支能作为父结点");
            };
            let bounds = self.nodes[left].bounds.union(self.nodes[right].bounds);
            if self.nodes[index].bounds == bounds {
                break;
            }
            self.nodes[index].bounds = bounds;
            parent = self.nodes[index].parent;
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Bounds {
    min: [f32; 3],
    max: [f32; 3],
}

impl Bounds {
    const UNBOUNDED: Self = Self {
        min: [f32::NEG_INFINITY; 3],
        max: [f32::INFINITY; 3],
    };

    fn around(body: &BodyBox) -> Self {
        // 确保常规范围的 f32 距离平方和不会溢出。异常尺寸或坐标照旧由窄相裁定，
        // 无限范围也只占一个叶子。
        const LIMIT: f32 = 1.0e18;
        let mut bounds = Self::UNBOUNDED;
        for (axis, (center, half)) in body.position.into_iter().zip(body.half).enumerate() {
            if !(-LIMIT..=LIMIT).contains(&center) || !(0.0..=LIMIT).contains(&half) {
                return Self::UNBOUNDED;
            }
            // 同方向舍入不会颠倒区间端点的次序，故重合判定包含等号。球还要涵盖
            // f32 平方、累加和开方的舍入误差，以及极小轴距的平方下溢。
            let reach = if body.sphere.is_some() {
                half * (1.0 + 8.0 * f32::EPSILON) + 1.0e-18
            } else {
                half
            };
            bounds.min[axis] = center - reach;
            bounds.max[axis] = center + reach;
        }
        bounds
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: std::array::from_fn(|axis| self.min[axis].min(other.min[axis])),
            max: std::array::from_fn(|axis| self.max[axis].max(other.max[axis])),
        }
    }

    fn intersects(self, other: Self) -> bool {
        (0..3).all(|axis| self.min[axis] <= other.max[axis] && other.min[axis] <= self.max[axis])
    }
}

#[derive(Clone, Copy, Debug)]
enum NodeKind {
    Leaf(usize),
    Branch { left: usize, right: usize },
}

#[derive(Clone, Debug)]
struct Node {
    bounds: Bounds,
    kind: NodeKind,
    parent: Option<usize>,
    last_body: usize,
    last_dynamic: Option<usize>,
}

impl Node {
    fn collect(
        &self,
        nodes: &[Self],
        bounds: Bounds,
        after: usize,
        dynamic: bool,
        candidates: &mut Vec<usize>,
    ) {
        let last = if dynamic {
            Some(self.last_body)
        } else {
            self.last_dynamic
        };
        if last.is_none_or(|last| last <= after) || !self.bounds.intersects(bounds) {
            return;
        }
        match self.kind {
            NodeKind::Leaf(body) => candidates.push(body),
            NodeKind::Branch { left, right } => {
                nodes[left].collect(nodes, bounds, after, dynamic, candidates);
                nodes[right].collect(nodes, bounds, after, dynamic, candidates);
            }
        }
    }
}

fn build(
    bodies: &[BodyBox],
    order: &mut [usize],
    nodes: &mut Vec<Node>,
    leaves: &mut [usize],
) -> usize {
    let node = if let [body] = *order {
        leaves[body] = nodes.len();
        Node {
            bounds: Bounds::around(&bodies[body]),
            kind: NodeKind::Leaf(body),
            parent: None,
            last_body: body,
            last_dynamic: bodies[body].dynamic.then_some(body),
        }
    } else {
        // 按坐标跨度最大的轴取中位数。即使位置相同，树深也保持对数级。
        let axis = split_axis(bodies, order);
        let middle = order.len() / 2;
        order.select_nth_unstable_by(middle, |&a, &b| {
            bodies[a].position[axis].total_cmp(&bodies[b].position[axis])
        });
        let (first, second) = order.split_at_mut(middle);
        let left = build(bodies, first, nodes, leaves);
        let right = build(bodies, second, nodes, leaves);
        let parent = Some(nodes.len());
        nodes[left].parent = parent;
        nodes[right].parent = parent;
        Node {
            bounds: nodes[left].bounds.union(nodes[right].bounds),
            kind: NodeKind::Branch { left, right },
            parent: None,
            last_body: nodes[left].last_body.max(nodes[right].last_body),
            last_dynamic: nodes[left].last_dynamic.max(nodes[right].last_dynamic),
        }
    };
    let index = nodes.len();
    nodes.push(node);
    index
}

fn split_axis(bodies: &[BodyBox], order: &[usize]) -> usize {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for &body in order {
        for (axis, value) in bodies[body].position.into_iter().enumerate() {
            min[axis] = min[axis].min(value);
            max[axis] = max[axis].max(value);
        }
    }
    let span: [f32; 3] = std::array::from_fn(|axis| max[axis] - min[axis]);
    let mut best = 0;
    for axis in 1..3 {
        if span[axis] > span[best] {
            best = axis;
        }
    }
    best
}
