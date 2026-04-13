//! Composition proofs — Rust types guarantee state decomposition correctness.
//!
//! These tests prove that CompositionBuilder + verify_graph make it
//! impossible to deploy infrastructure with:
//! - Disconnected cross-state references
//! - Type mismatches between workspaces
//! - Cyclic dependencies (infinite deploy loops)
//! - Duplicate output names (ambiguous references)

use proptest::prelude::*;
use std::collections::HashSet;
use workspace_state_graph::analysis::affected_workspaces;
use workspace_state_graph::composition::CompositionBuilder;
use workspace_state_graph::types::WorkspaceId;
use workspace_state_graph::verify::{
    verify_compatibility, verify_connectivity, verify_ordering, GraphViolation,
};

// ── Strategies ──────────────────────────────────────────────────────────

/// Generate a simple `IacType` (scalars only, to keep proptest tractable).
fn arb_iac_type() -> impl Strategy<Value = iac_forge::ir::IacType> {
    prop_oneof![
        Just(iac_forge::ir::IacType::String),
        Just(iac_forge::ir::IacType::Integer),
        Just(iac_forge::ir::IacType::Float),
        Just(iac_forge::ir::IacType::Boolean),
    ]
}

/// Generate a valid composition with a linear chain of sub-workspaces.
///
/// Each sub-workspace `child_i` produces `out_i` and `child_{i+1}` consumes it.
/// Types are consistent throughout the chain.
fn arb_linear_composition(
    max_children: usize,
) -> impl Strategy<Value = workspace_state_graph::composition::CompositionPlan> {
    (1..=max_children, arb_iac_type(), "[a-z]{3,6}").prop_map(|(n, ty, parent_name)| {
        let mut builder = CompositionBuilder::new(&parent_name, "Test Platform", "aws");
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

/// Generate a valid composition with fan-out: one root child, multiple leaf children.
fn arb_fanout_composition(
    max_leaves: usize,
) -> impl Strategy<Value = workspace_state_graph::composition::CompositionPlan> {
    (1..=max_leaves, arb_iac_type()).prop_map(|(n, ty)| {
        let mut builder = CompositionBuilder::new("fanout", "Fanout Platform", "aws");
        let ty_root = ty.clone();
        builder = builder.sub_workspace("root", move |ws| {
            ws.output("root_out", ty_root, "root_res.attr")
        });
        for i in 0..n {
            let leaf_name = format!("leaf{i}");
            let ty_leaf = ty.clone();
            builder = builder.sub_workspace(&leaf_name, move |ws| {
                ws.input_from("root", "root_out", ty_leaf.clone())
                    .output(&format!("leaf_out_{i}"), ty_leaf, &format!("leaf_res_{i}.attr"))
            });
        }
        builder.build()
    })
}

// ── Property Tests ──────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    // ── Proof 1: Valid compositions always pass verification ─────────────
    //
    // For any randomly generated valid composition (linear chain or fan-out),
    // verify_graph returns Ok. This proves the CompositionBuilder produces
    // structurally sound graphs by construction.
    #[test]
    fn proof1_valid_compositions_always_pass_verification(
        plan in arb_linear_composition(6)
    ) {
        prop_assert!(
            plan.verify().is_ok(),
            "Valid linear composition must pass verification"
        );
    }

    #[test]
    fn proof1_valid_fanout_compositions_pass_verification(
        plan in arb_fanout_composition(6)
    ) {
        prop_assert!(
            plan.verify().is_ok(),
            "Valid fanout composition must pass verification"
        );
    }

    // ── Proof 2: Removing an output breaks connectivity ──────────────────
    //
    // If workspace A outputs X and workspace B reads X, removing X from A
    // causes a DisconnectedInput violation. The type system makes it
    // impossible to silently lose a cross-state reference.
    #[test]
    fn proof2_removing_output_breaks_connectivity(
        plan in arb_linear_composition(6)
    ) {
        if plan.children.len() < 2 {
            return Ok(());
        }
        // Remove all outputs from the first child workspace.
        let mut broken = plan.graph.clone();
        let first_child = &plan.children[0];
        if let Some(node) = broken.nodes.get_mut(&first_child.id) {
            if node.outputs.is_empty() {
                return Ok(());
            }
            node.outputs.clear();
        }
        let violations = verify_connectivity(&broken);
        prop_assert!(
            !violations.is_empty(),
            "Removing output must cause disconnected input"
        );
        for v in &violations {
            prop_assert!(
                matches!(v, GraphViolation::DisconnectedInput { .. }),
                "Expected DisconnectedInput, got {:?}", v
            );
        }
    }

    // ── Proof 3: Type mismatch detection across compositions ─────────────
    //
    // If A outputs String and B reads Integer, verify_compatibility catches it.
    // The Rust type system + verification guarantees type-safe cross-workspace
    // references at compile/verify time rather than at deploy time.
    #[test]
    fn proof3_type_mismatch_detected_across_compositions(
        plan in arb_linear_composition(6)
    ) {
        if plan.children.len() < 2 {
            return Ok(());
        }
        let mut broken = plan.graph.clone();
        // Find the second child and flip its input type.
        let second_child = &plan.children[1];
        if let Some(node) = broken.nodes.get_mut(&second_child.id) {
            if let Some(input) = node.inputs.first_mut() {
                input.field_type = if input.field_type == iac_forge::ir::IacType::String {
                    iac_forge::ir::IacType::Integer
                } else {
                    iac_forge::ir::IacType::String
                };
            } else {
                return Ok(());
            }
        }
        let violations = verify_compatibility(&broken);
        prop_assert!(
            !violations.is_empty(),
            "Type mismatch must be detected"
        );
        for v in &violations {
            prop_assert!(
                matches!(v, GraphViolation::TypeMismatch { .. }),
                "Expected TypeMismatch, got {:?}", v
            );
        }
    }

    // ── Proof 4: Deployment order is a valid topological sort ────────────
    //
    // For any valid composition, deployment_order() returns an ordering where
    // every workspace appears AFTER all its dependencies. No workspace is
    // deployed before its inputs are available.
    #[test]
    fn proof4_deployment_order_is_valid_topological_sort(
        plan in arb_linear_composition(6)
    ) {
        let order = plan.deployment_order().expect("valid composition must sort");
        // Verify every edge is respected.
        for edge in &plan.graph.edges {
            let from_pos = order.iter().position(|w| *w == edge.from);
            let to_pos = order.iter().position(|w| *w == edge.to);
            if let (Some(fp), Some(tp)) = (from_pos, to_pos) {
                prop_assert!(
                    fp < tp,
                    "Edge {:?} -> {:?} violated topological order (positions {} >= {})",
                    edge.from, edge.to, fp, tp
                );
            }
        }
        // All nodes are present in the ordering.
        prop_assert_eq!(
            order.len(),
            plan.graph.nodes.len(),
            "Deployment order must include every workspace"
        );
    }

    // ── Proof 5: Impact analysis is transitively complete ────────────────
    //
    // If A -> B -> C, then changing A must list both B and C as affected.
    // Partial impact analysis would leave workspaces stale after a change.
    #[test]
    fn proof5_impact_analysis_is_transitively_complete(
        plan in arb_linear_composition(6)
    ) {
        if plan.children.is_empty() {
            return Ok(());
        }
        // Change the first child; all downstream children must be affected.
        let first_child_id = &plan.children[0].id;
        let affected = affected_workspaces(&plan.graph, first_child_id);

        // In a linear chain, changing child[0] affects child[1..n].
        let expected_affected: HashSet<_> = plan.children[1..]
            .iter()
            .map(|c| &c.id)
            .collect();

        for expected in &expected_affected {
            prop_assert!(
                affected.contains(expected),
                "Child {:?} should be affected by change to {:?}",
                expected, first_child_id
            );
        }

        // The changed workspace itself must NOT be in the affected set.
        prop_assert!(
            !affected.contains(first_child_id),
            "Changed workspace must not be in affected set"
        );
    }

    // ── Proof 6: Composition PIDs are unique and sequential ──────────────
    //
    // Every child in a composition has a unique PID starting from 2.
    // PID 1 is reserved for the parent. This guarantees unambiguous process
    // identity in the convergence tree.
    #[test]
    fn proof6_composition_pids_are_unique_and_sequential(
        plan in arb_linear_composition(8)
    ) {
        let mut seen_pids = HashSet::new();
        for (i, child) in plan.children.iter().enumerate() {
            let expected_pid = u32::try_from(i).unwrap() + 2;
            prop_assert_eq!(
                child.pid, expected_pid,
                "Child {} should have PID {}, got {}",
                i, expected_pid, child.pid
            );
            prop_assert!(
                seen_pids.insert(child.pid),
                "Duplicate PID {} detected",
                child.pid
            );
            prop_assert_eq!(
                child.ppid, 1,
                "All children must have PPID 1 (parent)"
            );
        }
    }

    // ── Proof 7: State keys follow parent/child naming ──────────────────
    //
    // Child state keys are always pangea/{parent}/{child}. This ensures
    // deterministic S3 state key generation from the composition tree.
    #[test]
    fn proof7_state_keys_follow_parent_child_naming(
        plan in arb_linear_composition(6)
    ) {
        let parent_name = &plan.parent.0;
        for child in &plan.children {
            let expected_key = format!("pangea/{}/{}", parent_name, child.name);
            prop_assert_eq!(
                &child.state_key, &expected_key,
                "State key mismatch for child {:?}: expected {}, got {}",
                child.name, expected_key, child.state_key
            );
        }
    }
}

// ── Non-proptest proofs ─────────────────────────────────────────────────

// ── Proof 8: Empty composition is valid ──────────────────────────────────
//
// A composition with no children (just parent) passes verification.
// This is the base case: an orchestrator with nothing to orchestrate.
#[test]
fn proof8_empty_composition_is_valid() {
    let plan = CompositionBuilder::new("empty", "Empty Platform", "aws").build();

    assert!(plan.verify().is_ok(), "Empty composition must pass verification");
    assert!(plan.children.is_empty(), "Empty composition has no children");
    // Parent workspace must exist in the graph.
    assert_eq!(plan.graph.nodes.len(), 1, "Only parent workspace should exist");
    assert!(
        plan.graph.nodes.contains_key(&WorkspaceId("empty".to_owned())),
        "Parent workspace must be in graph"
    );
    assert!(plan.graph.edges.is_empty(), "No edges in empty composition");

    // Deployment order should contain only the parent.
    let order = plan.deployment_order().expect("empty composition must sort");
    assert_eq!(order.len(), 1);
    assert_eq!(order[0], WorkspaceId("empty".to_owned()));
}

// ── Proof 9: Diamond dependencies are handled ───────────────────────────
//
// A -> B, A -> C, B -> D, C -> D (diamond pattern) — verify it's valid and
// deployment order respects ALL edges. Diamond patterns are common in real
// infrastructure (e.g., VPC -> subnets + security groups -> cluster).
#[test]
fn proof9_diamond_dependencies_are_handled() {
    let plan = CompositionBuilder::new("diamond", "Diamond Platform", "aws")
        .sub_workspace("a", |ws| {
            ws.output("x", iac_forge::ir::IacType::String, "res_a.x")
        })
        .sub_workspace("b", |ws| {
            ws.input_from("a", "x", iac_forge::ir::IacType::String)
                .output("y", iac_forge::ir::IacType::String, "res_b.y")
        })
        .sub_workspace("c", |ws| {
            ws.input_from("a", "x", iac_forge::ir::IacType::String)
                .output("z", iac_forge::ir::IacType::String, "res_c.z")
        })
        .sub_workspace("d", |ws| {
            ws.input_from("b", "y", iac_forge::ir::IacType::String)
                .input_from("c", "z", iac_forge::ir::IacType::String)
        })
        .build();

    // Verification passes.
    assert!(plan.verify().is_ok(), "Diamond composition must pass verification");

    // Correct node count: parent + 4 children.
    assert_eq!(plan.graph.nodes.len(), 5);

    // Correct edge count: a->b, a->c, b->d, c->d = 4 edges.
    assert_eq!(plan.graph.edges.len(), 4);

    // Deployment order respects all edges.
    let order = plan.deployment_order().expect("diamond must sort");
    let pos = |suffix: &str| {
        order
            .iter()
            .position(|id| id.0.ends_with(suffix))
            .unwrap_or_else(|| panic!("workspace ending with '{suffix}' not found in order"))
    };
    assert!(pos("-a") < pos("-b"), "a must deploy before b");
    assert!(pos("-a") < pos("-c"), "a must deploy before c");
    assert!(pos("-b") < pos("-d"), "b must deploy before d");
    assert!(pos("-c") < pos("-d"), "c must deploy before d");

    // Impact analysis: changing a affects b, c, d.
    let affected = affected_workspaces(
        &plan.graph,
        &WorkspaceId("diamond-a".to_owned()),
    );
    assert_eq!(affected.len(), 3, "Changing a must affect b, c, d");
    assert!(affected.contains(&WorkspaceId("diamond-b".to_owned())));
    assert!(affected.contains(&WorkspaceId("diamond-c".to_owned())));
    assert!(affected.contains(&WorkspaceId("diamond-d".to_owned())));
}

// ── Proof 10: Self-referencing composition is rejected ──────────────────
//
// A sub-workspace that reads its own output should cause a cycle error
// when the cycle detection runs. The CompositionBuilder resolves names
// using the parent prefix, so a self-reference creates a real cycle in
// the graph that verify_ordering catches.
#[test]
fn proof10_self_referencing_composition_is_rejected() {
    let plan = CompositionBuilder::new("self_ref", "Self Ref", "aws")
        .sub_workspace("loopy", |ws| {
            ws.output("x", iac_forge::ir::IacType::String, "res.x")
                .input_from("loopy", "x", iac_forge::ir::IacType::String)
        })
        .build();

    // The graph should have a cycle: loopy reads from itself.
    let cycle_violations = verify_ordering(&plan.graph);
    assert!(
        !cycle_violations.is_empty(),
        "Self-referencing workspace must create a cycle"
    );
    for v in &cycle_violations {
        assert!(
            matches!(v, GraphViolation::CyclicDependency { .. }),
            "Expected CyclicDependency, got {v:?}"
        );
    }

    // verify_graph should also fail.
    assert!(
        plan.verify().is_err(),
        "Self-referencing composition must fail verification"
    );
}

// ── Additional composition integrity proofs ─────────────────────────────

/// Verify that duplicate output names within a composed sub-workspace are detected.
#[test]
fn composition_detects_duplicate_outputs() {
    let plan = CompositionBuilder::new("dup", "Duplicate", "aws")
        .sub_workspace("net", |ws| {
            ws.output("vpc_id", iac_forge::ir::IacType::String, "aws_vpc.a.id")
                .output("vpc_id", iac_forge::ir::IacType::String, "aws_vpc.b.id")
        })
        .build();

    let result = plan.verify();
    assert!(
        result.is_err(),
        "Duplicate outputs in composition must be rejected"
    );
    let violations = result.unwrap_err();
    assert!(
        violations.iter().any(|v| matches!(v, GraphViolation::DuplicateOutput { .. })),
        "Must contain DuplicateOutput violation"
    );
}

/// Verify that a composition with type mismatch between siblings is caught
/// at the composition level (not just at the raw graph level).
#[test]
fn composition_type_mismatch_between_siblings() {
    let plan = CompositionBuilder::new("mismatch", "Mismatch", "aws")
        .sub_workspace("producer", |ws| {
            ws.output("value", iac_forge::ir::IacType::String, "res.value")
        })
        .sub_workspace("consumer", |ws| {
            // Consumer expects Integer but producer outputs String.
            ws.input_from("producer", "value", iac_forge::ir::IacType::Integer)
        })
        .build();

    let result = plan.verify();
    assert!(
        result.is_err(),
        "Type mismatch between composition siblings must fail"
    );
    let violations = result.unwrap_err();
    assert!(
        violations.iter().any(|v| matches!(v, GraphViolation::TypeMismatch { .. })),
        "Must contain TypeMismatch violation"
    );
}

/// Verify that a composition referencing a nonexistent sibling is caught.
#[test]
fn composition_disconnected_reference_to_nonexistent_sibling() {
    let plan = CompositionBuilder::new("missing", "Missing", "aws")
        .sub_workspace("consumer", |ws| {
            // References "ghost" which does not exist.
            ws.input_from("ghost", "value", iac_forge::ir::IacType::String)
        })
        .build();

    let result = plan.verify();
    assert!(
        result.is_err(),
        "Reference to nonexistent sibling must fail"
    );
    let violations = result.unwrap_err();
    assert!(
        violations.iter().any(|v| matches!(v, GraphViolation::DisconnectedInput { .. })),
        "Must contain DisconnectedInput violation"
    );
}

/// Verify that process lookup works correctly for all children in a composition.
#[test]
fn composition_process_lookup_consistency() {
    let plan = CompositionBuilder::new("lookup", "Lookup", "aws")
        .sub_workspace("alpha", |ws| {
            ws.output("a", iac_forge::ir::IacType::String, "r.a")
        })
        .sub_workspace("beta", |ws| {
            ws.input_from("alpha", "a", iac_forge::ir::IacType::String)
                .output("b", iac_forge::ir::IacType::String, "r.b")
        })
        .sub_workspace("gamma", |ws| {
            ws.input_from("beta", "b", iac_forge::ir::IacType::String)
        })
        .build();

    // Lookup by PID.
    for (i, child) in plan.children.iter().enumerate() {
        let pid = u32::try_from(i).unwrap() + 2;
        let found = plan.process(pid);
        assert!(found.is_some(), "Process with PID {} must exist", pid);
        assert_eq!(found.unwrap().name, child.name);
    }

    // Lookup by name.
    for child in &plan.children {
        let found = plan.process_by_name(&child.name);
        assert!(found.is_some(), "Process with name {} must exist", child.name);
        assert_eq!(found.unwrap().pid, child.pid);
    }

    // Nonexistent lookups return None.
    assert!(plan.process(99).is_none());
    assert!(plan.process_by_name("nonexistent").is_none());
}
