//! Exhaustive graph proofs — maximum property coverage for the typed DAG,
//! composition model, topology, serialization, and edge cases.
//!
//! These tests complement graph_proofs.rs, composition_proofs.rs, and
//! pleme_topology.rs by proving additional invariants not covered there.

use iac_forge::ir::IacType;
use proptest::prelude::*;
use std::collections::{BTreeSet, HashSet};
use workspace_state_graph::analysis::{affected_workspaces, deployment_order};
use workspace_state_graph::builder::WorkspaceGraphBuilder;
use workspace_state_graph::composition::{CompositionBuilder, CompositionPlan};
use workspace_state_graph::pleme::{
    builder_fleet_composition, minimal_graph, pleme_infrastructure_graph,
};
use workspace_state_graph::types::{WorkspaceGraph, WorkspaceId};
use workspace_state_graph::verify::{
    verify_compatibility, verify_graph, verify_ordering,
    GraphViolation,
};

// ===========================================================================
// Strategies
// ===========================================================================

fn arb_iac_type() -> impl Strategy<Value = IacType> {
    prop_oneof![
        Just(IacType::String),
        Just(IacType::Integer),
        Just(IacType::Float),
        Just(IacType::Boolean),
    ]
}

/// Generate a valid DAG with `n` workspaces forming a linear chain.
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
            b = b.workspace(
                &id,
                &format!("leaf-{i}"),
                &format!("{id}/terraform.tfstate"),
                "aws",
            );
            b = b.input(&id, "root_in", ty.clone(), "root", "root_out");
        }
        b.build()
    })
}

/// Generate a valid linear composition.
fn arb_linear_composition(
    max_children: usize,
) -> impl Strategy<Value = CompositionPlan> {
    (1..=max_children, arb_iac_type()).prop_map(|(n, ty)| {
        let mut builder = CompositionBuilder::new("parent", "Test Platform", "aws");
        for i in 0..n {
            let child_name = format!("child{i}");
            let out_name = format!("out_{i}");
            let ty_clone = ty.clone();
            if i == 0 {
                builder = builder.sub_workspace(&child_name, move |ws| {
                    ws.output(&out_name, ty_clone, &format!("res_{i}.attr"))
                });
            } else {
                let prev_name = format!("child{}", i - 1);
                let prev_out = format!("out_{}", i - 1);
                builder = builder.sub_workspace(&child_name, move |ws| {
                    ws.input_from(&prev_name, &prev_out, ty_clone.clone())
                        .output(&out_name, ty_clone, &format!("res_{i}.attr"))
                });
            }
        }
        builder.build()
    })
}

// ===========================================================================
// Graph property proofs (proptest)
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    // ── GP-1: Edge count is bounded by N*(N-1) ─────────────────────────────
    //
    // A valid graph with N nodes has at most N*(N-1) directed edges.
    // This is the theoretical maximum for a simple directed graph.
    #[test]
    fn gp1_edge_count_bounded_by_n_times_n_minus_1(graph in arb_linear_graph(10)) {
        let n = graph.nodes.len();
        let max_edges = n * n.saturating_sub(1);
        prop_assert!(
            graph.edges.len() <= max_edges,
            "Graph with {} nodes has {} edges, exceeding max {}",
            n, graph.edges.len(), max_edges
        );
    }

    // ── GP-2: Deployment order length equals node count (acyclic) ──────────
    //
    // For any acyclic graph, topological sort visits every node exactly once.
    #[test]
    fn gp2_deployment_order_length_equals_node_count(graph in arb_linear_graph(10)) {
        let order = deployment_order(&graph).expect("linear graph has no cycles");
        prop_assert_eq!(
            order.len(),
            graph.nodes.len(),
            "Deployment order must include every node"
        );
    }

    // ── GP-3: affected_workspaces never includes the changed workspace ─────
    //
    // The changed workspace itself is excluded from the affected set.
    #[test]
    fn gp3_affected_never_includes_self(graph in arb_fanout_graph(8)) {
        for id in graph.nodes.keys() {
            let affected = affected_workspaces(&graph, id);
            prop_assert!(
                !affected.contains(id),
                "affected_workspaces must never include the changed workspace {:?}",
                id
            );
        }
    }

    // ── GP-4: affected_workspaces is transitively complete ──────────────────
    //
    // If A -> B -> C, changing A must include both B and C.
    // We verify by checking that every affected node's dependents are also affected.
    #[test]
    fn gp4_affected_is_transitively_complete(graph in arb_linear_graph(8)) {
        if graph.nodes.is_empty() {
            return Ok(());
        }
        let first_key = graph.nodes.keys().next().unwrap().clone();
        let affected = affected_workspaces(&graph, &first_key);
        let affected_set: BTreeSet<_> = affected.iter().collect();

        // For each affected workspace, all of ITS dependents must also be affected.
        for ws_id in &affected {
            let downstream = affected_workspaces(&graph, ws_id);
            for ds in &downstream {
                prop_assert!(
                    affected_set.contains(ds),
                    "Transitive dependent {:?} of {:?} missing from affected set of {:?}",
                    ds, ws_id, first_key
                );
            }
        }
    }

    // ── GP-5: Adding an output does not break existing edges ────────────────
    //
    // Adding a new output to a workspace must not invalidate existing
    // connectivity or type compatibility.
    #[test]
    fn gp5_adding_output_preserves_existing_edges(graph in arb_linear_graph(8)) {
        // First verify the original graph is valid.
        prop_assert!(verify_graph(&graph).is_ok());

        // Add a new output to the first node.
        let mut modified = graph.clone();
        if let Some(first_node) = modified.nodes.values_mut().next() {
            first_node.outputs.push(workspace_state_graph::OutputPort {
                name: "extra_output_xyz".to_owned(),
                field_type: IacType::Boolean,
                source_resource: "res.extra".to_owned(),
            });
        }

        // The graph should still be valid.
        prop_assert!(
            verify_graph(&modified).is_ok(),
            "Adding an output must not break existing edges"
        );
    }

    // ── GP-6: Removing all inputs makes workspace a root ────────────────────
    //
    // A workspace with no inputs has in-degree 0, so it appears first
    // (or at least not after any other node it doesn't depend on) in deployment order.
    #[test]
    fn gp6_removing_inputs_makes_root(graph in arb_linear_graph(8)) {
        if graph.nodes.len() < 2 {
            return Ok(());
        }
        // Remove all inputs from the last node.
        let mut modified = graph.clone();
        let last_key = modified.nodes.keys().rev().next().unwrap().clone();
        if let Some(node) = modified.nodes.get_mut(&last_key) {
            node.inputs.clear();
        }
        // Also remove corresponding edges.
        modified.edges.retain(|e| e.to != last_key);

        // The last node should now have no dependencies and can appear
        // anywhere in the deployment order (it's a root).
        let order = deployment_order(&modified);
        prop_assert!(
            order.is_ok(),
            "Graph with removed inputs should still be acyclic"
        );
    }

    // ── GP-7: Deployment order elements are unique ──────────────────────────
    //
    // No workspace appears twice in the deployment order.
    #[test]
    fn gp7_deployment_order_has_no_duplicates(graph in arb_linear_graph(10)) {
        let order = deployment_order(&graph).expect("linear graph has no cycles");
        let unique: HashSet<_> = order.iter().collect();
        prop_assert_eq!(
            unique.len(),
            order.len(),
            "Deployment order must not contain duplicates"
        );
    }

    // ── GP-8: Fan-out affected_workspaces from root ─────────────────────────
    //
    // In a fan-out graph, changing the root affects all leaves.
    #[test]
    fn gp8_fanout_root_affects_all_leaves(graph in arb_fanout_graph(8)) {
        let root_id = WorkspaceId("root".to_owned());
        let affected = affected_workspaces(&graph, &root_id);

        // All non-root nodes should be affected.
        let expected = graph.nodes.len() - 1;
        prop_assert_eq!(
            affected.len(),
            expected,
            "Changing root in fanout must affect all {} leaves, got {}",
            expected, affected.len()
        );
    }

    // ── GP-9: Fan-out leaf change affects nothing ───────────────────────────
    //
    // In a fan-out graph, changing any leaf affects no other workspace.
    #[test]
    fn gp9_fanout_leaf_change_affects_nothing(graph in arb_fanout_graph(8)) {
        for (id, node) in &graph.nodes {
            // Skip root (which has outputs consumed by others).
            if node.inputs.is_empty() && !node.outputs.is_empty() {
                continue;
            }
            let affected = affected_workspaces(&graph, id);
            prop_assert!(
                affected.is_empty(),
                "Leaf {:?} change must affect nothing, but affected {:?}",
                id, affected
            );
        }
    }

    // ── GP-10: Edge references are consistent ───────────────────────────────
    //
    // Every edge's from/to references must exist in the node set, and the
    // edge's field_type must match the output's type.
    #[test]
    fn gp10_edge_references_are_consistent(graph in arb_linear_graph(10)) {
        for edge in &graph.edges {
            prop_assert!(
                graph.nodes.contains_key(&edge.from),
                "Edge source {:?} not in nodes", edge.from
            );
            prop_assert!(
                graph.nodes.contains_key(&edge.to),
                "Edge target {:?} not in nodes", edge.to
            );
            // Output type matches edge type.
            let from_node = &graph.nodes[&edge.from];
            if let Some(output) = from_node.outputs.iter().find(|o| o.name == edge.output_name) {
                prop_assert_eq!(
                    &output.field_type,
                    &edge.field_type,
                    "Edge field_type must match output field_type"
                );
            }
        }
    }
}

// ===========================================================================
// Composition property proofs (proptest)
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    // ── CP-1: N children yields N+1 nodes ───────────────────────────────────
    //
    // Any composition with N children has exactly N+1 nodes (parent + children).
    #[test]
    fn cp1_n_children_yields_n_plus_1_nodes(plan in arb_linear_composition(8)) {
        let expected_nodes = plan.children.len() + 1;
        prop_assert_eq!(
            plan.graph.nodes.len(),
            expected_nodes,
            "Composition with {} children must have {} nodes, got {}",
            plan.children.len(), expected_nodes, plan.graph.nodes.len()
        );
    }

    // ── CP-2: PIDs always start from 2 and are sequential ──────────────────
    #[test]
    fn cp2_pids_sequential_from_2(plan in arb_linear_composition(8)) {
        for (i, child) in plan.children.iter().enumerate() {
            let expected = u32::try_from(i).unwrap() + 2;
            prop_assert_eq!(
                child.pid, expected,
                "Child {} PID must be {}, got {}", i, expected, child.pid
            );
        }
    }

    // ── CP-3: State keys always follow pangea/{parent}/{child} ──────────────
    #[test]
    fn cp3_state_keys_follow_convention(plan in arb_linear_composition(8)) {
        let parent = &plan.parent.0;
        for child in &plan.children {
            let expected = format!("pangea/{parent}/{}", child.name);
            prop_assert_eq!(
                &child.state_key, &expected,
                "State key for {:?} must be {}, got {}",
                child.name, expected, child.state_key
            );
        }
    }

    // ── CP-4: All children have ppid == 1 ──────────────────────────────────
    #[test]
    fn cp4_all_children_have_ppid_1(plan in arb_linear_composition(8)) {
        for child in &plan.children {
            prop_assert_eq!(
                child.ppid, 1,
                "Child {:?} ppid must be 1, got {}", child.name, child.ppid
            );
        }
    }

    // ── CP-5: Deployment order within composition respects all edges ────────
    #[test]
    fn cp5_deployment_order_respects_all_edges(plan in arb_linear_composition(8)) {
        let order = plan.deployment_order().expect("valid composition");
        for edge in &plan.graph.edges {
            let from_pos = order.iter().position(|w| *w == edge.from);
            let to_pos = order.iter().position(|w| *w == edge.to);
            if let (Some(fp), Some(tp)) = (from_pos, to_pos) {
                prop_assert!(
                    fp < tp,
                    "Edge {:?} -> {:?} violated order ({} >= {})",
                    edge.from, edge.to, fp, tp
                );
            }
        }
    }

    // ── CP-6: Child IDs follow {parent}-{child} naming ─────────────────────
    #[test]
    fn cp6_child_ids_follow_naming(plan in arb_linear_composition(8)) {
        let parent = &plan.parent.0;
        for child in &plan.children {
            let expected_id = format!("{parent}-{}", child.name);
            prop_assert_eq!(
                &child.id.0, &expected_id,
                "Child ID must be {}, got {}", expected_id, child.id.0
            );
        }
    }
}

// ===========================================================================
// Topology proofs (pleme_infrastructure_graph)
// ===========================================================================

/// The real topology is a DAG (no cycles).
#[test]
fn topology_is_dag() {
    let graph = pleme_infrastructure_graph();
    let violations = verify_ordering(&graph);
    assert!(
        violations.is_empty(),
        "Real topology must be acyclic: {violations:?}"
    );
}

/// Every workspace has a unique state_key.
#[test]
fn topology_unique_state_keys() {
    let graph = pleme_infrastructure_graph();
    let mut seen = HashSet::new();
    for node in graph.nodes.values() {
        assert!(
            seen.insert(&node.state_key),
            "Duplicate state_key: {}",
            node.state_key
        );
    }
}

/// Every input references an existing workspace.
#[test]
fn topology_every_input_references_existing_workspace() {
    let graph = pleme_infrastructure_graph();
    for node in graph.nodes.values() {
        for input in &node.inputs {
            assert!(
                graph.nodes.contains_key(&input.source_workspace),
                "Input {:?} on {:?} references nonexistent workspace {:?}",
                input.name,
                node.id,
                input.source_workspace
            );
        }
    }
}

/// Every input references an existing output on the source workspace.
#[test]
fn topology_every_input_references_existing_output() {
    let graph = pleme_infrastructure_graph();
    for node in graph.nodes.values() {
        for input in &node.inputs {
            let source = &graph.nodes[&input.source_workspace];
            let output_exists = source.outputs.iter().any(|o| o.name == input.source_output);
            assert!(
                output_exists,
                "Input {:?} on {:?} references nonexistent output {:?} on {:?}",
                input.name, node.id, input.source_output, input.source_workspace
            );
        }
    }
}

/// Type compatibility holds for every edge in the real topology.
#[test]
fn topology_type_compatibility_holds() {
    let graph = pleme_infrastructure_graph();
    let violations = verify_compatibility(&graph);
    assert!(
        violations.is_empty(),
        "Real topology must have no type mismatches: {violations:?}"
    );
}

/// Deployment order puts state-backend and pleme-dns before everything that depends on them.
#[test]
fn topology_deployment_order_roots_first() {
    let graph = pleme_infrastructure_graph();
    let order = deployment_order(&graph).expect("no cycles");
    let pos = |name: &str| order.iter().position(|id| id.0 == name).unwrap();

    // state-backend has no deps, must appear before anything that depends on it.
    // pleme-dns has no deps, must appear before nix-builders and seph-cluster.
    assert!(pos("pleme-dns") < pos("nix-builders"));
    assert!(pos("pleme-dns") < pos("seph-cluster"));
    assert!(pos("seph-vpc") < pos("seph-cluster"));
    assert!(pos("seph-cluster") < pos("akeyless-dev-config"));
}

/// Impact analysis from state-backend change is correct.
/// state-backend has no typed downstream dependents in the graph (consumers use
/// pangea.yml config, not remote state inputs).
#[test]
fn topology_state_backend_impact_correct() {
    let graph = pleme_infrastructure_graph();
    let affected = affected_workspaces(&graph, &WorkspaceId("state-backend".into()));
    assert!(
        affected.is_empty(),
        "state-backend has no typed downstream dependents"
    );
}

/// Impact analysis from pleme-dns cascades correctly.
#[test]
fn topology_dns_impact_cascades() {
    let graph = pleme_infrastructure_graph();
    let affected = affected_workspaces(&graph, &WorkspaceId("pleme-dns".into()));
    let names: BTreeSet<_> = affected.iter().map(|id| id.0.as_str()).collect();

    assert!(names.contains("nix-builders"), "dns -> nix-builders");
    assert!(names.contains("seph-cluster"), "dns -> seph-cluster");
    assert!(
        names.contains("akeyless-dev-config"),
        "dns -> seph-cluster -> akeyless (transitive)"
    );
    assert!(!names.contains("seph-vpc"), "dns does not affect vpc");
    assert!(
        !names.contains("state-backend"),
        "dns does not affect state-backend"
    );
}

/// Builder fleet composition state keys follow convention.
#[test]
fn topology_builder_fleet_state_keys() {
    let plan = builder_fleet_composition();
    for child in &plan.children {
        assert!(
            child.state_key.starts_with("pangea/nix-builders/"),
            "Builder fleet child {:?} state_key must start with pangea/nix-builders/, got {}",
            child.name,
            child.state_key
        );
    }
}

/// Builder fleet PIDs are sequential from 2.
#[test]
fn topology_builder_fleet_pids_sequential() {
    let plan = builder_fleet_composition();
    for (i, child) in plan.children.iter().enumerate() {
        let expected = u32::try_from(i).unwrap() + 2;
        assert_eq!(
            child.pid, expected,
            "Builder fleet child {:?} PID must be {}, got {}",
            child.name, expected, child.pid
        );
    }
}

// ===========================================================================
// Serialization proofs
// ===========================================================================

/// WorkspaceGraph serializes/deserializes roundtrip.
#[test]
fn serialization_workspace_graph_roundtrip() {
    let graph = pleme_infrastructure_graph();
    let json = serde_json::to_string_pretty(&graph).expect("serialize");
    let deserialized: WorkspaceGraph = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(
        deserialized.nodes.len(),
        graph.nodes.len(),
        "Node count must survive roundtrip"
    );
    assert_eq!(
        deserialized.edges.len(),
        graph.edges.len(),
        "Edge count must survive roundtrip"
    );

    // Verify all node IDs survived.
    for key in graph.nodes.keys() {
        assert!(
            deserialized.nodes.contains_key(key),
            "Node {:?} lost in roundtrip",
            key
        );
    }

    // Verify deserialized graph is still valid.
    assert!(verify_graph(&deserialized).is_ok());
}

/// CompositionPlan serializes/deserializes roundtrip.
#[test]
fn serialization_composition_plan_roundtrip() {
    let plan = builder_fleet_composition();
    let json = serde_json::to_string_pretty(&plan).expect("serialize");
    let deserialized: CompositionPlan = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(deserialized.parent, plan.parent);
    assert_eq!(deserialized.children.len(), plan.children.len());
    assert_eq!(
        deserialized.graph.nodes.len(),
        plan.graph.nodes.len()
    );
    assert_eq!(
        deserialized.graph.edges.len(),
        plan.graph.edges.len()
    );

    // Verify deserialized plan is still valid.
    assert!(deserialized.verify().is_ok());
}

/// Large graph (50+ nodes) serializes correctly.
#[test]
fn serialization_large_graph_roundtrip() {
    let mut b = WorkspaceGraphBuilder::new();
    let n = 50;
    for i in 0..n {
        let id = format!("ws{i}");
        let name = format!("workspace-{i}");
        let state_key = format!("{id}/terraform.tfstate");
        b = b.workspace(&id, &name, &state_key, "aws");
        b = b.output(&id, &format!("out_{i}"), IacType::String, &format!("res_{i}.attr"));
        if i > 0 {
            let prev = format!("ws{}", i - 1);
            b = b.input(
                &id,
                &format!("in_{i}"),
                IacType::String,
                &prev,
                &format!("out_{}", i - 1),
            );
        }
    }
    let graph = b.build();

    assert_eq!(graph.nodes.len(), n);
    assert_eq!(graph.edges.len(), n - 1);

    let json = serde_json::to_string(&graph).expect("serialize");
    let deserialized: WorkspaceGraph = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(deserialized.nodes.len(), n);
    assert_eq!(deserialized.edges.len(), n - 1);
    assert!(verify_graph(&deserialized).is_ok());
}

/// Minimal graph roundtrip.
#[test]
fn serialization_minimal_graph_roundtrip() {
    let graph = minimal_graph();
    let json = serde_json::to_string(&graph).expect("serialize");
    let deserialized: WorkspaceGraph = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(deserialized.nodes.len(), graph.nodes.len());
    assert!(verify_graph(&deserialized).is_ok());
}

/// Serialization preserves edge field types including List(String).
#[test]
fn serialization_preserves_complex_types() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("a", "A", "a.tfstate", "aws")
        .output(
            "a",
            "subnets",
            IacType::List(Box::new(IacType::String)),
            "aws_subnet.*.id",
        )
        .workspace("b", "B", "b.tfstate", "aws")
        .input(
            "b",
            "subnets",
            IacType::List(Box::new(IacType::String)),
            "a",
            "subnets",
        )
        .build();

    let json = serde_json::to_string(&graph).expect("serialize");
    let deserialized: WorkspaceGraph = serde_json::from_str(&json).expect("deserialize");

    let edge = &deserialized.edges[0];
    assert_eq!(edge.field_type, IacType::List(Box::new(IacType::String)));
}

// ===========================================================================
// Edge cases
// ===========================================================================

/// Single-node graph passes all verification.
#[test]
fn edge_single_node_passes_all() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("solo", "Solo", "solo.tfstate", "aws")
        .output("solo", "id", IacType::String, "res.id")
        .build();

    assert!(verify_graph(&graph).is_ok());
    let order = deployment_order(&graph).expect("no cycles");
    assert_eq!(order.len(), 1);
    assert_eq!(order[0], WorkspaceId("solo".to_owned()));

    let affected = affected_workspaces(&graph, &WorkspaceId("solo".to_owned()));
    assert!(affected.is_empty());
}

/// Two disconnected subgraphs pass verification.
#[test]
fn edge_two_disconnected_subgraphs() {
    // Subgraph 1: a -> b
    // Subgraph 2: c -> d
    // No edges between the subgraphs.
    let graph = WorkspaceGraphBuilder::new()
        .workspace("a", "A", "a.tfstate", "aws")
        .output("a", "x", IacType::String, "res.x")
        .workspace("b", "B", "b.tfstate", "aws")
        .input("b", "x", IacType::String, "a", "x")
        .workspace("c", "C", "c.tfstate", "aws")
        .output("c", "y", IacType::Integer, "res.y")
        .workspace("d", "D", "d.tfstate", "aws")
        .input("d", "y", IacType::Integer, "c", "y")
        .build();

    assert!(verify_graph(&graph).is_ok());

    let order = deployment_order(&graph).expect("no cycles");
    assert_eq!(order.len(), 4);

    // Changing a affects only b, not c or d.
    let affected = affected_workspaces(&graph, &WorkspaceId("a".to_owned()));
    assert_eq!(affected.len(), 1);
    assert!(affected.contains(&WorkspaceId("b".to_owned())));

    // Changing c affects only d, not a or b.
    let affected = affected_workspaces(&graph, &WorkspaceId("c".to_owned()));
    assert_eq!(affected.len(), 1);
    assert!(affected.contains(&WorkspaceId("d".to_owned())));
}

/// Self-loop is detected as a cycle.
#[test]
fn edge_self_loop_detected() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("loop", "Loop", "loop.tfstate", "aws")
        .output("loop", "x", IacType::String, "res.x")
        .input("loop", "x", IacType::String, "loop", "x")
        .build();

    let violations = verify_ordering(&graph);
    assert!(
        !violations.is_empty(),
        "Self-loop must be detected as a cycle"
    );
    assert!(matches!(
        &violations[0],
        GraphViolation::CyclicDependency { .. }
    ));
}

/// Duplicate workspace IDs in builder -- last one wins.
#[test]
fn edge_duplicate_workspace_ids() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("dup", "First", "first.tfstate", "aws")
        .workspace("dup", "Second", "second.tfstate", "aws")
        .output("dup", "x", IacType::String, "res.x")
        .build();

    // Only one node with that ID.
    assert_eq!(graph.nodes.len(), 1);
    // The last definition wins.
    let node = &graph.nodes[&WorkspaceId("dup".to_owned())];
    assert_eq!(node.name, "Second");
    assert_eq!(node.state_key, "second.tfstate");
}

/// Graph with only outputs and no inputs passes verification.
#[test]
fn edge_outputs_only_graph() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("a", "A", "a.tfstate", "aws")
        .output("a", "x", IacType::String, "res.x")
        .workspace("b", "B", "b.tfstate", "aws")
        .output("b", "y", IacType::Integer, "res.y")
        .build();

    assert!(verify_graph(&graph).is_ok());
    assert!(graph.edges.is_empty());

    let order = deployment_order(&graph).expect("no cycles");
    assert_eq!(order.len(), 2);
}

/// Deep chain works (A -> B -> C -> D -> E, 5 levels).
#[test]
fn edge_deep_chain_works() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("a", "A", "a.tfstate", "aws")
        .output("a", "out_a", IacType::String, "res.a")
        .workspace("b", "B", "b.tfstate", "aws")
        .input("b", "in_b", IacType::String, "a", "out_a")
        .output("b", "out_b", IacType::String, "res.b")
        .workspace("c", "C", "c.tfstate", "aws")
        .input("c", "in_c", IacType::String, "b", "out_b")
        .output("c", "out_c", IacType::String, "res.c")
        .workspace("d", "D", "d.tfstate", "aws")
        .input("d", "in_d", IacType::String, "c", "out_c")
        .output("d", "out_d", IacType::String, "res.d")
        .workspace("e", "E", "e.tfstate", "aws")
        .input("e", "in_e", IacType::String, "d", "out_d")
        .build();

    assert!(verify_graph(&graph).is_ok());

    let order = deployment_order(&graph).expect("no cycles");
    assert_eq!(order.len(), 5);
    let pos = |id: &str| order.iter().position(|w| w.0 == id).unwrap();
    assert!(pos("a") < pos("b"));
    assert!(pos("b") < pos("c"));
    assert!(pos("c") < pos("d"));
    assert!(pos("d") < pos("e"));

    // Changing a affects all downstream.
    let affected = affected_workspaces(&graph, &WorkspaceId("a".to_owned()));
    assert_eq!(affected.len(), 4);

    // Changing e affects nothing.
    let affected = affected_workspaces(&graph, &WorkspaceId("e".to_owned()));
    assert!(affected.is_empty());
}

/// Diamond dependency composition works.
#[test]
fn edge_diamond_composition_works() {
    let plan = CompositionBuilder::new("dm", "Diamond", "aws")
        .sub_workspace("a", |ws| {
            ws.output("x", IacType::String, "res.x")
        })
        .sub_workspace("b", |ws| {
            ws.input_from("a", "x", IacType::String)
                .output("y", IacType::String, "res.y")
        })
        .sub_workspace("c", |ws| {
            ws.input_from("a", "x", IacType::String)
                .output("z", IacType::String, "res.z")
        })
        .sub_workspace("d", |ws| {
            ws.input_from("b", "y", IacType::String)
                .input_from("c", "z", IacType::String)
        })
        .build();

    assert!(plan.verify().is_ok());
    assert_eq!(plan.graph.nodes.len(), 5); // parent + 4
    assert_eq!(plan.graph.edges.len(), 4); // a->b, a->c, b->d, c->d

    let order = plan.deployment_order().expect("no cycles");
    let pos = |s: &str| order.iter().position(|id| id.0.ends_with(s)).unwrap();
    assert!(pos("-a") < pos("-b"));
    assert!(pos("-a") < pos("-c"));
    assert!(pos("-b") < pos("-d"));
    assert!(pos("-c") < pos("-d"));
}

/// Deep chain composition works (5 levels).
#[test]
fn edge_deep_chain_composition() {
    let plan = CompositionBuilder::new("deep", "Deep Chain", "aws")
        .sub_workspace("l1", |ws| {
            ws.output("o1", IacType::String, "r.1")
        })
        .sub_workspace("l2", |ws| {
            ws.input_from("l1", "o1", IacType::String)
                .output("o2", IacType::String, "r.2")
        })
        .sub_workspace("l3", |ws| {
            ws.input_from("l2", "o2", IacType::String)
                .output("o3", IacType::String, "r.3")
        })
        .sub_workspace("l4", |ws| {
            ws.input_from("l3", "o3", IacType::String)
                .output("o4", IacType::String, "r.4")
        })
        .sub_workspace("l5", |ws| {
            ws.input_from("l4", "o4", IacType::String)
        })
        .build();

    assert!(plan.verify().is_ok());
    assert_eq!(plan.graph.nodes.len(), 6); // parent + 5
    assert_eq!(plan.children.len(), 5);

    let order = plan.deployment_order().expect("no cycles");
    let pos = |s: &str| order.iter().position(|id| id.0.ends_with(s)).unwrap();
    assert!(pos("-l1") < pos("-l2"));
    assert!(pos("-l2") < pos("-l3"));
    assert!(pos("-l3") < pos("-l4"));
    assert!(pos("-l4") < pos("-l5"));
}

/// Workspace with no outputs and no inputs (truly isolated node).
#[test]
fn edge_isolated_node() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("iso", "Isolated", "iso.tfstate", "aws")
        .build();

    assert!(verify_graph(&graph).is_ok());
    let order = deployment_order(&graph).expect("no cycles");
    assert_eq!(order.len(), 1);
}

/// Multiple inputs from the same source workspace.
#[test]
fn edge_multiple_inputs_from_same_source() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("src", "Source", "src.tfstate", "aws")
        .output("src", "out_a", IacType::String, "res.a")
        .output("src", "out_b", IacType::Integer, "res.b")
        .output("src", "out_c", IacType::Boolean, "res.c")
        .workspace("dst", "Destination", "dst.tfstate", "aws")
        .input("dst", "in_a", IacType::String, "src", "out_a")
        .input("dst", "in_b", IacType::Integer, "src", "out_b")
        .input("dst", "in_c", IacType::Boolean, "src", "out_c")
        .build();

    assert!(verify_graph(&graph).is_ok());
    assert_eq!(graph.edges.len(), 3);

    let order = deployment_order(&graph).expect("no cycles");
    let pos = |id: &str| order.iter().position(|w| w.0 == id).unwrap();
    assert!(pos("src") < pos("dst"));
}

/// Workspace with many outputs, only some consumed.
#[test]
fn edge_partial_output_consumption() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("producer", "Producer", "prod.tfstate", "aws")
        .output("producer", "a", IacType::String, "res.a")
        .output("producer", "b", IacType::Integer, "res.b")
        .output("producer", "c", IacType::Boolean, "res.c")
        .output("producer", "d", IacType::Float, "res.d")
        .workspace("consumer", "Consumer", "cons.tfstate", "aws")
        .input("consumer", "x", IacType::String, "producer", "a")
        // b, c, d are not consumed
        .build();

    assert!(verify_graph(&graph).is_ok());
    assert_eq!(graph.edges.len(), 1);

    // Only 1 edge despite 4 outputs.
    let producer = &graph.nodes[&WorkspaceId("producer".to_owned())];
    assert_eq!(producer.outputs.len(), 4);
}

/// Verify input_as works for renamed inputs in composition.
#[test]
fn edge_composition_input_as() {
    let plan = CompositionBuilder::new("rename", "Rename", "aws")
        .sub_workspace("source", |ws| {
            ws.output("original_name", IacType::String, "res.original")
        })
        .sub_workspace("target", |ws| {
            ws.input_as("local_name", "source", "original_name", IacType::String)
        })
        .build();

    assert!(plan.verify().is_ok());
    assert_eq!(plan.graph.edges.len(), 1);

    let edge = &plan.graph.edges[0];
    assert_eq!(edge.input_name, "local_name");
    assert_eq!(edge.output_name, "original_name");
}

/// Composition parent has no PID field in ChildProcess (it is PID 1 conceptually).
#[test]
fn edge_parent_not_in_children_list() {
    let plan = CompositionBuilder::new("test_parent", "Test", "aws")
        .sub_workspace("child", |ws| {
            ws.output("x", IacType::String, "r.x")
        })
        .build();

    // Parent is in graph nodes but NOT in children list.
    assert!(plan.graph.nodes.contains_key(&plan.parent));
    assert!(
        !plan.children.iter().any(|c| c.id == plan.parent),
        "Parent must not appear in children list"
    );
}

/// Large composition (20 children) maintains all invariants.
#[test]
fn edge_large_composition() {
    let mut builder = CompositionBuilder::new("big", "Big Platform", "aws");
    let n = 20;
    for i in 0..n {
        let child_name = format!("c{i}");
        let out_name = format!("out_{i}");
        if i == 0 {
            builder = builder.sub_workspace(&child_name, move |ws| {
                ws.output(&out_name, IacType::String, &format!("r.{i}"))
            });
        } else {
            let prev = format!("c{}", i - 1);
            let prev_out = format!("out_{}", i - 1);
            builder = builder.sub_workspace(&child_name, move |ws| {
                ws.input_from(&prev, &prev_out, IacType::String)
                    .output(&out_name, IacType::String, &format!("r.{i}"))
            });
        }
    }
    let plan = builder.build();

    assert_eq!(plan.children.len(), n);
    assert_eq!(plan.graph.nodes.len(), n + 1);
    assert!(plan.verify().is_ok());

    let order = plan.deployment_order().expect("no cycles");
    assert_eq!(order.len(), n + 1);

    // PIDs: 2 through n+1.
    for (i, child) in plan.children.iter().enumerate() {
        assert_eq!(child.pid, u32::try_from(i).unwrap() + 2);
        assert_eq!(child.ppid, 1);
    }
}

/// Three-node cycle is detected.
#[test]
fn edge_three_node_cycle() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("a", "A", "a.tfstate", "aws")
        .output("a", "out_a", IacType::String, "res.a")
        .input("a", "in_a", IacType::String, "c", "out_c")
        .workspace("b", "B", "b.tfstate", "aws")
        .output("b", "out_b", IacType::String, "res.b")
        .input("b", "in_b", IacType::String, "a", "out_a")
        .workspace("c", "C", "c.tfstate", "aws")
        .output("c", "out_c", IacType::String, "res.c")
        .input("c", "in_c", IacType::String, "b", "out_b")
        .build();

    let violations = verify_ordering(&graph);
    assert!(!violations.is_empty(), "3-node cycle must be detected");
    match &violations[0] {
        GraphViolation::CyclicDependency { workspaces } => {
            assert_eq!(workspaces.len(), 3, "All 3 nodes participate in the cycle");
        }
        other => panic!("Expected CyclicDependency, got {other:?}"),
    }

    // deployment_order should also fail.
    assert!(deployment_order(&graph).is_err());
}

/// Verify that verify_graph aggregates all violation types together.
#[test]
fn edge_verify_graph_aggregates_violations() {
    // Create a graph with both a disconnected input AND a duplicate output.
    let graph = WorkspaceGraphBuilder::new()
        .workspace("ws", "WS", "ws.tfstate", "aws")
        .output("ws", "dup", IacType::String, "res.a")
        .output("ws", "dup", IacType::String, "res.b")
        .input("ws", "phantom", IacType::String, "ghost", "missing")
        .build();

    let result = verify_graph(&graph);
    assert!(result.is_err());
    let violations = result.unwrap_err();

    let has_dup = violations
        .iter()
        .any(|v| matches!(v, GraphViolation::DuplicateOutput { .. }));
    let has_disc = violations
        .iter()
        .any(|v| matches!(v, GraphViolation::DisconnectedInput { .. }));

    assert!(has_dup, "Must detect duplicate output");
    assert!(has_disc, "Must detect disconnected input");
}

/// Nodes with identical names but different IDs are distinct.
#[test]
fn edge_same_name_different_ids() {
    let graph = WorkspaceGraphBuilder::new()
        .workspace("id1", "SharedName", "id1.tfstate", "aws")
        .output("id1", "x", IacType::String, "res.x")
        .workspace("id2", "SharedName", "id2.tfstate", "aws")
        .input("id2", "x", IacType::String, "id1", "x")
        .build();

    assert_eq!(graph.nodes.len(), 2);
    assert!(verify_graph(&graph).is_ok());
}

/// Composition with mixed IacType across different sub-workspace chains.
#[test]
fn edge_mixed_types_across_chains() {
    let plan = CompositionBuilder::new("mixed", "Mixed", "aws")
        .sub_workspace("str_producer", |ws| {
            ws.output("str_val", IacType::String, "res.str")
        })
        .sub_workspace("int_producer", |ws| {
            ws.output("int_val", IacType::Integer, "res.int")
        })
        .sub_workspace("consumer", |ws| {
            ws.input_from("str_producer", "str_val", IacType::String)
                .input_from("int_producer", "int_val", IacType::Integer)
        })
        .build();

    assert!(plan.verify().is_ok());
    assert_eq!(plan.graph.edges.len(), 2);
}
