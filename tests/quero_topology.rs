//! Prove the quero.lol infrastructure topology is valid.

use workspace_state_graph::analysis::{affected_workspaces, deployment_order};
use workspace_state_graph::pleme::{
    pleme_infrastructure_graph, quero_infrastructure_graph, quero_platform_composition,
};
use workspace_state_graph::types::WorkspaceId;
use workspace_state_graph::verify::verify_graph;

// ── Graph Verification ──────────────────────────────────────────────

#[test]
fn quero_graph_passes_all_verification() {
    let graph = quero_infrastructure_graph();
    assert!(
        verify_graph(&graph).is_ok(),
        "quero.lol infrastructure graph must be valid"
    );
}

#[test]
fn quero_graph_has_correct_workspace_count() {
    let graph = quero_infrastructure_graph();
    // quero-dns, quero-vpc, quero-builders-aarch64, quero-builders-x86,
    // quero-cache, quero-seph, quero-monitoring
    assert_eq!(graph.nodes.len(), 7);
}

#[test]
fn quero_graph_has_correct_edge_count() {
    let graph = quero_infrastructure_graph();
    // dns→builders-aarch64 (zone_id), dns→builders-x86 (zone_id),
    // dns→cache (zone_id), dns→seph (zone_id), dns→monitoring (zone_id),
    // vpc→builders-aarch64 (vpc_id), vpc→builders-x86 (vpc_id),
    // vpc→seph (vpc_id, subnet_ids = 2 edges),
    // seph→monitoring (cluster_endpoint)
    // Total: 5 (dns) + 2 (vpc→builders) + 2 (vpc→seph) + 1 (seph→monitoring) = 10
    assert_eq!(graph.edges.len(), 10);
}

// ── Deployment Order ────────────────────────────────────────────────

#[test]
fn quero_deployment_order_dns_and_vpc_first() {
    let graph = quero_infrastructure_graph();
    let order = deployment_order(&graph).expect("No cycles");
    let pos = |name: &str| order.iter().position(|id| id.0 == name).unwrap();

    // dns and vpc must deploy before everything else
    assert!(pos("quero-dns") < pos("quero-builders-aarch64"));
    assert!(pos("quero-dns") < pos("quero-builders-x86"));
    assert!(pos("quero-dns") < pos("quero-cache"));
    assert!(pos("quero-dns") < pos("quero-seph"));
    assert!(pos("quero-dns") < pos("quero-monitoring"));

    assert!(pos("quero-vpc") < pos("quero-builders-aarch64"));
    assert!(pos("quero-vpc") < pos("quero-builders-x86"));
    assert!(pos("quero-vpc") < pos("quero-seph"));
}

#[test]
fn quero_deployment_order_monitoring_last() {
    let graph = quero_infrastructure_graph();
    let order = deployment_order(&graph).expect("No cycles");
    let pos = |name: &str| order.iter().position(|id| id.0 == name).unwrap();

    // monitoring depends on seph, which depends on dns+vpc
    assert!(pos("quero-seph") < pos("quero-monitoring"));
}

// ── Impact Analysis ─────────────────────────────────────────────────

#[test]
fn quero_dns_change_affects_builders_cache_seph_monitoring() {
    let graph = quero_infrastructure_graph();
    let affected = affected_workspaces(&graph, &WorkspaceId("quero-dns".into()));
    let names: Vec<&str> = affected.iter().map(|id| id.0.as_str()).collect();

    assert!(
        names.contains(&"quero-builders-aarch64"),
        "DNS change must affect aarch64 builders"
    );
    assert!(
        names.contains(&"quero-builders-x86"),
        "DNS change must affect x86 builders"
    );
    assert!(
        names.contains(&"quero-cache"),
        "DNS change must affect cache"
    );
    assert!(
        names.contains(&"quero-seph"),
        "DNS change must affect seph"
    );
    assert!(
        names.contains(&"quero-monitoring"),
        "DNS change must transitively affect monitoring"
    );
}

#[test]
fn quero_vpc_change_affects_builders_and_seph_not_cache() {
    let graph = quero_infrastructure_graph();
    let affected = affected_workspaces(&graph, &WorkspaceId("quero-vpc".into()));
    let names: Vec<&str> = affected.iter().map(|id| id.0.as_str()).collect();

    assert!(
        names.contains(&"quero-builders-aarch64"),
        "VPC change must affect aarch64 builders"
    );
    assert!(
        names.contains(&"quero-builders-x86"),
        "VPC change must affect x86 builders"
    );
    assert!(
        names.contains(&"quero-seph"),
        "VPC change must affect seph"
    );
    assert!(
        !names.contains(&"quero-cache"),
        "VPC change must NOT affect cache (cache depends on DNS only)"
    );
}

#[test]
fn quero_builders_are_leaves() {
    let graph = quero_infrastructure_graph();
    let affected_aarch64 =
        affected_workspaces(&graph, &WorkspaceId("quero-builders-aarch64".into()));
    let affected_x86 = affected_workspaces(&graph, &WorkspaceId("quero-builders-x86".into()));

    assert!(
        affected_aarch64.is_empty(),
        "aarch64 builders should have no downstream dependents"
    );
    assert!(
        affected_x86.is_empty(),
        "x86 builders should have no downstream dependents"
    );
}

// ── Composition ─────────────────────────────────────────────────────

#[test]
fn quero_composition_is_valid() {
    let plan = quero_platform_composition();
    assert!(
        plan.verify().is_ok(),
        "quero.lol composition must pass all verification"
    );
}

#[test]
fn quero_composition_has_6_children() {
    let plan = quero_platform_composition();
    assert_eq!(plan.children.len(), 6);

    let names: Vec<&str> = plan.children.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"dns"));
    assert!(names.contains(&"vpc"));
    assert!(names.contains(&"builders-aarch64"));
    assert!(names.contains(&"builders-x86"));
    assert!(names.contains(&"cache"));
    assert!(names.contains(&"seph"));
}

#[test]
fn quero_composition_deployment_order() {
    let plan = quero_platform_composition();
    let order = plan.deployment_order().expect("No cycles");
    let pos = |suffix: &str| {
        order
            .iter()
            .position(|id| id.0.ends_with(suffix))
            .unwrap()
    };

    // dns and vpc before builders, cache, seph
    assert!(pos("dns") < pos("builders-aarch64"));
    assert!(pos("dns") < pos("builders-x86"));
    assert!(pos("dns") < pos("cache"));
    assert!(pos("dns") < pos("seph"));
    assert!(pos("vpc") < pos("builders-aarch64"));
    assert!(pos("vpc") < pos("builders-x86"));
    assert!(pos("vpc") < pos("seph"));
}

#[test]
fn quero_composition_pids_2_through_7() {
    let plan = quero_platform_composition();
    let pids: Vec<u32> = plan.children.iter().map(|c| c.pid).collect();

    assert_eq!(pids, vec![2, 3, 4, 5, 6, 7]);
    assert!(plan.children.iter().all(|c| c.ppid == 1));
}

#[test]
fn quero_composition_dns_change_cascades() {
    let plan = quero_platform_composition();
    let affected = affected_workspaces(
        &plan.graph,
        &WorkspaceId("quero-dns".into()),
    );
    let names: Vec<&str> = affected.iter().map(|id| id.0.as_str()).collect();

    assert!(
        names.contains(&"quero-builders-aarch64"),
        "DNS change must cascade to aarch64 builders"
    );
    assert!(
        names.contains(&"quero-builders-x86"),
        "DNS change must cascade to x86 builders"
    );
    assert!(
        names.contains(&"quero-cache"),
        "DNS change must cascade to cache"
    );
    assert!(
        names.contains(&"quero-seph"),
        "DNS change must cascade to seph"
    );
}

// ── Serialization ───────────────────────────────────────────────────

#[test]
fn quero_graph_serialization_roundtrip() {
    let graph = quero_infrastructure_graph();
    let json = serde_json::to_string(&graph).expect("serialize");
    let deserialized: workspace_state_graph::types::WorkspaceGraph =
        serde_json::from_str(&json).expect("deserialize");

    assert_eq!(graph.nodes.len(), deserialized.nodes.len());
    assert_eq!(graph.edges.len(), deserialized.edges.len());
    assert!(verify_graph(&deserialized).is_ok());
}

// ── Independence ────────────────────────────────────────────────────

#[test]
fn quero_and_pleme_graphs_are_independent() {
    let quero = quero_infrastructure_graph();
    let pleme = pleme_infrastructure_graph();

    // No quero workspace appears in pleme graph
    for id in quero.nodes.keys() {
        assert!(
            !pleme.nodes.contains_key(id),
            "quero workspace {:?} should not appear in pleme graph",
            id
        );
    }

    // No pleme workspace appears in quero graph
    for id in pleme.nodes.keys() {
        assert!(
            !quero.nodes.contains_key(id),
            "pleme workspace {:?} should not appear in quero graph",
            id
        );
    }
}

// ── State Keys ──────────────────────────────────────────────────────

#[test]
fn quero_composition_state_keys_follow_pattern() {
    let plan = quero_platform_composition();

    for child in &plan.children {
        assert!(
            child.state_key.starts_with("pangea/quero/"),
            "State key '{}' must follow pangea/quero/{{child}} pattern",
            child.state_key
        );
    }

    // Verify specific keys
    let key_for = |name: &str| {
        plan.children
            .iter()
            .find(|c| c.name == name)
            .unwrap()
            .state_key
            .as_str()
    };

    assert_eq!(key_for("dns"), "pangea/quero/dns");
    assert_eq!(key_for("vpc"), "pangea/quero/vpc");
    assert_eq!(key_for("builders-aarch64"), "pangea/quero/builders-aarch64");
    assert_eq!(key_for("builders-x86"), "pangea/quero/builders-x86");
    assert_eq!(key_for("cache"), "pangea/quero/cache");
    assert_eq!(key_for("seph"), "pangea/quero/seph");
}
