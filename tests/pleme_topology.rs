//! Prove the real pleme-io infrastructure topology is valid.

use workspace_state_graph::analysis::{affected_workspaces, deployment_order};
use workspace_state_graph::pleme::{minimal_graph, pleme_infrastructure_graph};
use workspace_state_graph::types::WorkspaceId;
use workspace_state_graph::verify::verify_graph;

#[test]
fn pleme_graph_passes_all_verification() {
    let graph = pleme_infrastructure_graph();
    assert!(
        verify_graph(&graph).is_ok(),
        "Real infrastructure graph must be valid"
    );
}

#[test]
fn pleme_graph_has_correct_workspace_count() {
    let graph = pleme_infrastructure_graph();
    // state-backend, pleme-dns, seph-vpc, seph-cluster, akeyless-dev-config
    assert_eq!(graph.nodes.len(), 5);
}

#[test]
fn pleme_deployment_order_respects_dependencies() {
    let graph = pleme_infrastructure_graph();
    let order = deployment_order(&graph).expect("No cycles");

    let pos = |name: &str| order.iter().position(|id| id.0 == name).unwrap();

    // VPC before cluster
    assert!(pos("seph-vpc") < pos("seph-cluster"));
    // DNS before cluster
    assert!(pos("pleme-dns") < pos("seph-cluster"));
    // Cluster before akeyless
    assert!(pos("seph-cluster") < pos("akeyless-dev-config"));
}

#[test]
fn vpc_change_affects_cluster_and_akeyless() {
    let graph = pleme_infrastructure_graph();
    let affected = affected_workspaces(&graph, &WorkspaceId("seph-vpc".into()));
    let names: Vec<&str> = affected.iter().map(|id| id.0.as_str()).collect();

    assert!(
        names.contains(&"seph-cluster"),
        "VPC change must affect cluster"
    );
    assert!(
        names.contains(&"akeyless-dev-config"),
        "VPC change must transitively affect akeyless"
    );
}

#[test]
fn dns_change_affects_only_cluster_and_downstream() {
    let graph = pleme_infrastructure_graph();
    let affected = affected_workspaces(&graph, &WorkspaceId("pleme-dns".into()));
    let names: Vec<&str> = affected.iter().map(|id| id.0.as_str()).collect();

    assert!(names.contains(&"seph-cluster"));
    assert!(
        !names.contains(&"seph-vpc"),
        "DNS change must NOT affect VPC"
    );
}

#[test]
fn state_backend_is_independent() {
    let graph = pleme_infrastructure_graph();
    let affected = affected_workspaces(&graph, &WorkspaceId("state-backend".into()));
    // State backend has no downstream consumers via typed edges
    // (workspaces configure it via pangea.yml, not remote state)
    assert!(
        affected.is_empty(),
        "State backend should have no typed downstream dependents"
    );
}

#[test]
fn minimal_graph_valid() {
    let graph = minimal_graph();
    assert!(verify_graph(&graph).is_ok());
    let order = deployment_order(&graph).expect("No cycles");
    assert_eq!(order.len(), 3);
}

#[test]
fn pleme_graph_edge_count() {
    let graph = pleme_infrastructure_graph();
    // seph-cluster has 3 inputs (vpc_id, subnet_ids, dns_zone_id)
    // akeyless has 1 input (cluster_endpoint)
    assert_eq!(graph.edges.len(), 4);
}
