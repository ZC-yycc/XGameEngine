//! 渲染图回归：生产者在后也能排到消费者前、独立 pass 保注册序、环 / 多写者报错。

use xgc_render::{GraphError, Pass, RenderGraph};

#[test]
fn producer_orders_before_consumer_even_if_registered_later() {
    let mut graph = RenderGraph::new();
    graph
        .add(Pass::new("opaque", &["shadow"], &["color"]))
        .unwrap();
    graph.add(Pass::new("shadow", &[], &["shadow"])).unwrap();
    graph
        .add(Pass::new("post", &["color"], &["backbuffer"]))
        .unwrap();

    assert_eq!(
        graph.order_names().unwrap(),
        vec!["shadow", "opaque", "post"]
    );
}

#[test]
fn independent_passes_keep_registration_order() {
    let mut graph = RenderGraph::new();
    graph.add(Pass::new("a", &[], &["a"])).unwrap();
    graph.add(Pass::new("b", &[], &["b"])).unwrap();
    graph.add(Pass::new("c", &[], &["c"])).unwrap();

    assert_eq!(graph.order_names().unwrap(), vec!["a", "b", "c"]);
}

#[test]
fn cycle_is_reported() {
    let mut graph = RenderGraph::new();
    graph.add(Pass::new("a", &["y"], &["x"])).unwrap();
    graph.add(Pass::new("b", &["x"], &["y"])).unwrap();

    let err = graph.order().unwrap_err();
    assert!(matches!(err, GraphError::Cycle(_)));
}

#[test]
fn duplicate_pass_and_multiple_writers_are_rejected() {
    let mut graph = RenderGraph::new();
    graph.add(Pass::new("a", &[], &["x"])).unwrap();
    assert!(matches!(
        graph.add(Pass::new("a", &[], &["y"])),
        Err(GraphError::DuplicatePass(_))
    ));

    let mut graph = RenderGraph::new();
    graph.add(Pass::new("a", &[], &["x"])).unwrap();
    graph.add(Pass::new("b", &[], &["x"])).unwrap();
    assert!(matches!(
        graph.order(),
        Err(GraphError::MultipleWriters { .. })
    ));
}
