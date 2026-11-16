//! 渲染图回归：生产者在后也能排到消费者前、独立 pass 保注册序、环 / 多写者报错。

use xgc_render::{GraphError, Pass, RenderGraph, ResourceDesc, ResourceUsage};

fn declare(graph: &mut RenderGraph, names: &[&str]) {
    for name in names {
        graph
            .add_resource(ResourceDesc::texture2d(
                *name,
                "rgba16float",
                1,
                1,
                vec![ResourceUsage::Sampled, ResourceUsage::ColorAttachment],
            ))
            .unwrap();
    }
}

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
    declare(&mut graph, &["shadow", "color", "backbuffer"]);

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
    declare(&mut graph, &["a", "b", "c"]);

    assert_eq!(graph.order_names().unwrap(), vec!["a", "b", "c"]);
}

#[test]
fn cycle_is_reported() {
    let mut graph = RenderGraph::new();
    graph.add(Pass::new("a", &["y"], &["x"])).unwrap();
    graph.add(Pass::new("b", &["x"], &["y"])).unwrap();
    declare(&mut graph, &["x", "y"]);

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
    declare(&mut graph, &["x"]);
    assert!(matches!(
        graph.order(),
        Err(GraphError::MultipleWriters { .. })
    ));
}

#[test]
fn unknown_and_duplicate_resources_are_rejected() {
    let mut graph = RenderGraph::new();
    graph.add(Pass::new("a", &[], &["x"])).unwrap();
    assert!(matches!(
        graph.order(),
        Err(GraphError::UnknownResource { .. })
    ));

    let resource = ResourceDesc::texture2d(
        "x",
        "rgba8unorm",
        1,
        1,
        vec![ResourceUsage::ColorAttachment],
    );
    graph.add_resource(resource.clone()).unwrap();
    assert!(matches!(
        graph.add_resource(resource),
        Err(GraphError::DuplicateResource(_))
    ));
    graph.order().unwrap();
}

#[test]
fn usage_flags_are_enforced() {
    let mut graph = RenderGraph::new();
    graph.add(Pass::new("read", &["x"], &[])).unwrap();
    graph
        .add_resource(ResourceDesc::texture2d(
            "x",
            "rgba8unorm",
            1,
            1,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    assert!(matches!(
        graph.order(),
        Err(GraphError::UnreadableResource { .. })
    ));

    let mut graph = RenderGraph::new();
    graph.add(Pass::new("write", &[], &["x"])).unwrap();
    graph
        .add_resource(ResourceDesc::texture2d(
            "x",
            "rgba8unorm",
            1,
            1,
            vec![ResourceUsage::Sampled],
        ))
        .unwrap();
    assert!(matches!(
        graph.order(),
        Err(GraphError::UnwritableResource { .. })
    ));
}

#[test]
fn execute_calls_passes_in_dependency_order() {
    let mut graph = RenderGraph::new();
    graph
        .add(Pass::new("opaque", &["shadow"], &["color"]))
        .unwrap();
    graph.add(Pass::new("shadow", &[], &["shadow"])).unwrap();
    declare(&mut graph, &["shadow", "color"]);

    let mut executed = Vec::new();
    graph
        .execute(&mut |pass: &Pass| {
            executed.push(pass.name.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(executed, vec!["shadow", "opaque"]);
}

#[test]
fn execute_failure_stops_and_reports_pass() {
    let mut graph = RenderGraph::new();
    graph.add(Pass::new("a", &[], &["x"])).unwrap();
    graph.add(Pass::new("b", &[], &["y"])).unwrap();
    declare(&mut graph, &["x", "y"]);

    let mut executed = Vec::new();
    let err = graph
        .execute(&mut |pass: &Pass| {
            executed.push(pass.name.clone());
            if pass.name == "b" {
                Err("boom".to_string())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert_eq!(executed, vec!["a", "b"]);
    assert!(matches!(err, GraphError::Execute { .. }));
}
