//! 根运动的渲染侧：把根节点的位移轴改回**绑定姿态**。
//!
//! 运行时（`xgc-sim/src/root_motion.rs`）已经把根位移交给实体世界位置；如果蒙皮
//! 再保留一次髋骨平移，角色会前进两次。旧端同口径函数是
//! `crates/xgame-core/src/animation_runtime.rs::compute_bones_composed_with_root_motion`：
//! 基础姿势（含交叉淡入）采完后、additive / layers 之前，按节点名找到根节点，
//! 把它 `translation` 的开关轴替换成绑定姿态值；additive / layers 属于骨架姿势，
//! 不动。
//!
//! 节点名不命中时逐位不变——坏配置只该不生效，不该把角色画歪。

use super::animation::{AnimationNode, LocalPose};

/// 把 `locals` 里根节点的开关轴平移改回绑定姿态。
pub(crate) fn strip(
    locals: &mut [LocalPose],
    nodes: &[AnimationNode],
    node_name: &str,
    axes: [bool; 3],
) {
    if node_name.is_empty() {
        return;
    }
    let Some((index, node)) = nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.name == node_name)
    else {
        return;
    };
    let Some(local) = locals.get_mut(index) else {
        return;
    };
    for (axis, enabled) in axes.iter().enumerate() {
        if *enabled {
            local.translation[axis] = node.translation[axis];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, translation: [f32; 3]) -> AnimationNode {
        AnimationNode {
            name: name.to_string(),
            parent: None,
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }

    fn local(translation: [f32; 3]) -> LocalPose {
        LocalPose {
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }

    #[test]
    fn strips_only_the_selected_axes_back_to_bind() {
        let nodes = vec![node("hip", [0.0, 1.0, 0.0])];
        let mut locals = vec![local([2.0, 3.0, 4.0])];
        strip(&mut locals, &nodes, "hip", [true, false, true]);
        assert_eq!(locals[0].translation, [0.0, 3.0, 0.0]);
    }

    #[test]
    fn unknown_node_or_empty_name_is_a_no_op() {
        let nodes = vec![node("hip", [0.0; 3])];
        let mut locals = vec![local([2.0, 3.0, 4.0])];
        strip(&mut locals, &nodes, "missing", [true, true, true]);
        strip(&mut locals, &nodes, "", [true, true, true]);
        assert_eq!(locals[0].translation, [2.0, 3.0, 4.0]);
    }
}
