use iac_forge::ir::IacType;
use proptest::prelude::*;
use workspace_state_graph::analysis::{affected_workspaces, deployment_order};
use workspace_state_graph::builder::WorkspaceGraphBuilder;
use workspace_state_graph::types::{WorkspaceGraph, WorkspaceId};
use workspace_state_graph::verify::{
    verify_compatibility, verify_connectivity, verify_graph, verify_ordering, verify_uniqueness,
    GraphViolation,
};

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// Generate a simple `IacType` (scalars only, to keep proptest tractable).
fn arb_iac_type() -> impl Strategy<Value = IacType> {
    prop_oneof![
        Just(IacType::String),
        Just(IacType::Integer),
        Just(IacType::Float),
        Just(IacType::Boolean),
    ]
}

/// Generate a valid DAG with `n` workspaces forming a linear chain.
///
/// Each workspace `w_i` produces an output `out_i` and workspace `w_{i+1}`
/// consumes it. All types match.
fn arb_linear_graph(max_nodes: usize) -> impl Strategy<Value = WorkspaceGraph> {
    (1..=max_nodes, arb_iac_type()).prop_map(|(n, ty)| {
        let mut b = WorkspaceGraphBuilder::new();
        for i in 0..n {
            let id = format!("w{i}");
            let name = format!("workspace-{i}");
            let state_key = format!("{id}/terraform.tfstate");
            b = b.workspace(&id, &name, &state_key, "aws");
            b = b.output(&id, &format!("out_{i}"), ty.clone(), &format!("res_{i}.attr"));
            if i > 0 {
                let prev = format!("w{}", i - 1);
                b = b.input(
                    &id,
                    &format!("in_{i}"),
                    ty.clone(),
                    &prev,
                    &format!("out_{}", i - 1),
                );
            }
        }
        b.build()
    })
}

/// Generate a valid DAG with fan-out: one root and multiple leaves.
fn arb_fanout_graph(max_leaves: usize) -> impl Strategy<Value = WorkspaceGraph> {
    (1..=max_leaves, arb_iac_type()).prop_map(|(n, ty)| {
        let mut b = WorkspaceGraphBuilder::new();
        b = b.workspace("root", "root", "root/terraform.tfstate", "aws");
        b = b.output("root", "root_out", ty.clone(), "root_res.attr");
        for i in 0..n {
            let id = format!("leaf{i}");
            b = b.workspace(&id, &format!("leaf-{i}"), &format!("{id}/terraform.tfstate"), "aws");
            b = b.input(&id, "root_in", ty.clone(), "root", "root_out");
        }
        b.build()
    })
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

proptest! {
    /// 1. Valid graphs pass all verification.
    #[test]
    fn valid_graphs_pass_verification(graph in arb_linear_graph(8)) {
        prop_assert!(verify_graph(&graph).is_ok());
    }

    /// 2. Removing an output creates a disconnected input violation.
    #[test]
    fn removing_output_creates_disconnected_input(graph in arb_linear_graph(8)) {
        // Only meaningful for graphs with at least 2 nodes (i.e., at least one edge).
        if graph.nodes.len() < 2 {
            return Ok(());
        }
        // Remove the first node's outputs.
        let mut broken = graph.clone();
        let first_key = broken.nodes.keys().next().unwrap().clone();
        if let Some(node) = broken.nodes.get_mut(&first_key) {
            if node.outputs.is_empty() {
                return Ok(());
            }
            node.outputs.clear();
        }
        let violations = verify_connectivity(&broken);
        prop_assert!(!violations.is_empty(), "Expected disconnected input violations");
        for v in &violations {
            let is_disconnected = matches!(v, GraphViolation::DisconnectedInput { .. });
            prop_assert!(is_disconnected, "Expected DisconnectedInput variant");
        }
    }

    /// 3. Type mismatches are detected.
    #[test]
    fn type_mismatches_detected(graph in arb_linear_graph(8)) {
        if graph.nodes.len() < 2 {
            return Ok(());
        }
        // Flip the type on the last node's first input.
        let mut broken = graph.clone();
        let last_key = broken.nodes.keys().rev().next().unwrap().clone();
        if let Some(node) = broken.nodes.get_mut(&last_key) {
            if let Some(input) = node.inputs.first_mut() {
                // Swap to a different type.
                input.field_type = if input.field_type == IacType::String {
                    IacType::Integer
                } else {
                    IacType::String
                };
            } else {
                return Ok(());
            }
        }
        let violations = verify_compatibility(&broken);
        prop_assert!(!violations.is_empty(), "Expected type mismatch violations");
        for v in &violations {
            let is_mismatch = matches!(v, GraphViolation::TypeMismatch { .. });
            prop_assert!(is_mismatch, "Expected TypeMismatch variant");
        }
    }

    /// 4. Cycles are detected: adding a back-edge to a linear chain creates a cycle.
    #[test]
    fn cycles_detected(graph in arb_linear_graph(8)) {
        if graph.nodes.len() < 2 {
            return Ok(());
        }
        // Add a back-edge from the last node to the first.
        let mut broken = graph.clone();
        let first_key = broken.nodes.keys().next().unwrap().clone();
        let last_key = broken.nodes.keys().rev().next().unwrap().clone();
        let last_out_name = {
            let last_node = &broken.nodes[&last_key];
            last_node.outputs.first().map(|o| o.name.clone())
        };
        if let Some(out_name) = last_out_name {
            if let Some(first_node) = broken.nodes.get_mut(&first_key) {
                first_node.inputs.push(workspace_state_graph::InputPort {
                    name: "back_edge_in".to_owned(),
                    field_type: IacType::String,
                    source_workspace: last_key,
                    source_output: out_name,
                });
            }
        } else {
            return Ok(());
        }
        let violations = verify_ordering(&broken);
        prop_assert!(!violations.is_empty(), "Expected cycle detection");
        for v in &violations {
            let is_cycle = matches!(v, GraphViolation::CyclicDependency { .. });
            prop_assert!(is_cycle, "Expected CyclicDependency variant");
        }
    }

    /// 5. Topological sort respects all edges.
    #[test]
    fn topo_sort_respects_edges(graph in arb_linear_graph(8)) {
        let order = deployment_order(&graph).expect("valid graph should sort");
        for edge in &graph.edges {
            let from_pos = order.iter().position(|w| *w == edge.from);
            let to_pos = order.iter().position(|w| *w == edge.to);
            if let (Some(fp), Some(tp)) = (from_pos, to_pos) {
                prop_assert!(fp < tp, "Edge {:?} -> {:?} violated topo order", edge.from, edge.to);
            }
        }
    }

    /// 6. `affected_workspaces` returns the complete transitive closure.
    #[test]
    fn affected_workspaces_transitive_closure(graph in arb_linear_graph(8)) {
        if graph.nodes.is_empty() {
            return Ok(());
        }
        let first_key = graph.nodes.keys().next().unwrap().clone();
        let affected = affected_workspaces(&graph, &first_key);

        // In a linear chain from the first node, all other nodes should be affected.
        let other_count = graph.nodes.len() - 1;
        prop_assert_eq!(
            affected.len(),
            other_count,
            "Expected {} affected, got {}",
            other_count,
            affected.len()
        );

        // Verify the affected set does not contain the source.
        prop_assert!(
            !affected.contains(&first_key),
            "Source should not be in affected set"
        );
    }

    /// 9. Builder auto-generates correct edges.
    #[test]
    fn builder_generates_correct_edges(graph in arb_fanout_graph(6)) {
        // Count expected edges: one per leaf (each has one input).
        let expected_edges: usize = graph
            .nodes
            .values()
            .map(|n| n.inputs.len())
            .sum();
        prop_assert_eq!(
            graph.edges.len(),
            expected_edges,
            "Edge count should match total input count"
        );

        // Each edge should reference existing nodes.
        for edge in &graph.edges {
            prop_assert!(
                graph.nodes.contains_key(&edge.from),
                "Edge source {:?} not in nodes",
                edge.from
            );
            prop_assert!(
                graph.nodes.contains_key(&edge.to),
                "Edge target {:?} not in nodes",
                edge.to
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Non-proptest tests for simpler invariants
// ---------------------------------------------------------------------------

/// 7. Empty graph passes all verification.
#[test]
fn empty_graph_passes_all_verification() {
    let graph = WorkspaceGraphBuilder::new().build();
    assert_eq!(verify_graph(&graph), Ok(()));
    assert!(verify_connectivity(&graph).is_empty());
    assert!(verify_ordering(&graph).is_empty());
    assert!(verify_compatibility(&graph).is_empty());
    assert!(verify_uniqueness(&graph).is_empty());
}

/// 8. Single-node graph passes all verification.
#[test]
fn single_node_passes_all_verification() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("only", "only-ws", "only/terraform.tfstate", "aws")
        .output("only", "vpc_id", IacType::String, "aws_vpc.main")
        .build();
    assert_eq!(verify_graph(&graph), Ok(()));
}

/// Verify that `affected_workspaces` on a diamond DAG returns complete closure.
#[test]
fn affected_workspaces_diamond() {
    // a -> b, a -> c, b -> d, c -> d
    let graph = WorkspaceGraphBuilder::new()
        .workspace("a", "A", "a.tfstate", "aws")
        .output("a", "x", IacType::String, "res.x")
        .workspace("b", "B", "b.tfstate", "aws")
        .input("b", "x", IacType::String, "a", "x")
        .output("b", "y", IacType::String, "res.y")
        .workspace("c", "C", "c.tfstate", "aws")
        .input("c", "x", IacType::String, "a", "x")
        .output("c", "z", IacType::String, "res.z")
        .workspace("d", "D", "d.tfstate", "aws")
        .input("d", "y", IacType::String, "b", "y")
        .input("d", "z", IacType::String, "c", "z")
        .build();

    let affected = affected_workspaces(&graph, &WorkspaceId("a".to_owned()));
    assert_eq!(affected.len(), 3);
    assert!(affected.contains(&WorkspaceId("b".to_owned())));
    assert!(affected.contains(&WorkspaceId("c".to_owned())));
    assert!(affected.contains(&WorkspaceId("d".to_owned())));
}

/// Verify deployment order on a diamond DAG.
#[test]
fn deployment_order_diamond() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("a", "A", "a.tfstate", "aws")
        .output("a", "x", IacType::String, "res.x")
        .workspace("b", "B", "b.tfstate", "aws")
        .input("b", "x", IacType::String, "a", "x")
        .output("b", "y", IacType::String, "res.y")
        .workspace("c", "C", "c.tfstate", "aws")
        .input("c", "x", IacType::String, "a", "x")
        .output("c", "z", IacType::String, "res.z")
        .workspace("d", "D", "d.tfstate", "aws")
        .input("d", "y", IacType::String, "b", "y")
        .input("d", "z", IacType::String, "c", "z")
        .build();

    let order = deployment_order(&graph).expect("should succeed");
    let pos = |id: &str| order.iter().position(|w| w.0 == id).unwrap();
    assert!(pos("a") < pos("b"));
    assert!(pos("a") < pos("c"));
    assert!(pos("b") < pos("d"));
    assert!(pos("c") < pos("d"));
}
