//! Prove the real pleme-io infrastructure topology is valid.

use workspace_state_graph::analysis::{affected_workspaces, deployment_order};
use workspace_state_graph::pleme::{builder_fleet_composition, minimal_graph, pleme_infrastructure_graph};
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
    // state-backend, pleme-dns, nix-builders, seph-vpc, seph-cluster, akeyless-dev-config
    assert_eq!(graph.nodes.len(), 6);
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
        names.contains(&"nix-builders"),
        "DNS change must affect nix-builders"
    );
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
    // nix-builders has 1 input (zone_id from pleme-dns)
    assert_eq!(graph.edges.len(), 5);
}

#[test]
fn pleme_dns_before_nix_builders() {
    let graph = pleme_infrastructure_graph();
    let order = deployment_order(&graph).expect("No cycles");
    let pos = |name: &str| order.iter().position(|id| id.0 == name).unwrap();

    assert!(
        pos("pleme-dns") < pos("nix-builders"),
        "pleme-dns must deploy before nix-builders"
    );
}

#[test]
fn nix_builders_is_leaf() {
    let graph = pleme_infrastructure_graph();
    let affected = affected_workspaces(&graph, &WorkspaceId("nix-builders".into()));
    assert!(
        affected.is_empty(),
        "nix-builders should have no downstream dependents"
    );
}

#[test]
fn builder_fleet_composition_is_valid() {
    let plan = builder_fleet_composition();
    assert!(
        plan.verify().is_ok(),
        "Builder fleet composition must pass all verification"
    );
}

#[test]
fn builder_fleet_has_4_children() {
    let plan = builder_fleet_composition();
    // parent (nix-builders) + 4 children (network, security, compute, dns)
    assert_eq!(plan.children.len(), 4);
    assert_eq!(plan.graph.nodes.len(), 5);

    let names: Vec<&str> = plan.children.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"network"));
    assert!(names.contains(&"security"));
    assert!(names.contains(&"compute"));
    assert!(names.contains(&"dns"));
}

#[test]
fn builder_fleet_deployment_order() {
    let plan = builder_fleet_composition();
    let order = plan.deployment_order().expect("No cycles");
    let pos = |suffix: &str| {
        order
            .iter()
            .position(|id| id.0.ends_with(suffix))
            .unwrap()
    };

    // network must deploy before security and compute
    assert!(pos("network") < pos("security"));
    assert!(pos("network") < pos("compute"));
    // security must deploy before compute
    assert!(pos("security") < pos("compute"));
    // compute must deploy before dns
    assert!(pos("compute") < pos("dns"));
}

#[test]
fn builder_fleet_network_change_cascades() {
    let plan = builder_fleet_composition();
    let affected = affected_workspaces(
        &plan.graph,
        &WorkspaceId("nix-builders-network".into()),
    );
    let names: Vec<&str> = affected.iter().map(|id| id.0.as_str()).collect();

    assert!(
        names.contains(&"nix-builders-security"),
        "Network change must affect security"
    );
    assert!(
        names.contains(&"nix-builders-compute"),
        "Network change must affect compute"
    );
    assert!(
        names.contains(&"nix-builders-dns"),
        "Network change must affect dns"
    );
}
