use crate::types::{WorkspaceGraph, WorkspaceId};
use iac_forge::ir::IacType;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use thiserror::Error;

/// A violation of graph invariants.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GraphViolation {
    #[error(
        "Disconnected input: {workspace}.{input} requires {source_workspace}.{source_output} which doesn't exist"
    )]
    DisconnectedInput {
        workspace: String,
        input: String,
        source_workspace: String,
        source_output: String,
    },
    #[error(
        "Type mismatch: {from_workspace}.{output} ({output_type:?}) -> {to_workspace}.{input} ({input_type:?})"
    )]
    TypeMismatch {
        from_workspace: String,
        output: String,
        output_type: IacType,
        to_workspace: String,
        input: String,
        input_type: IacType,
    },
    #[error("Cycle detected involving: {workspaces:?}")]
    CyclicDependency { workspaces: Vec<String> },
    #[error("Orphan workspace: {workspace} has no inputs or outputs")]
    OrphanWorkspace { workspace: String },
    #[error("Duplicate output: {workspace}.{output} defined multiple times")]
    DuplicateOutput { workspace: String, output: String },
}

/// Verify all graph invariants.
///
/// Returns `Ok(())` if every invariant holds, or `Err` with all violations.
pub fn verify_graph(graph: &WorkspaceGraph) -> Result<(), Vec<GraphViolation>> {
    let mut violations = Vec::new();
    violations.extend(verify_uniqueness(graph));
    violations.extend(verify_connectivity(graph));
    violations.extend(verify_compatibility(graph));
    violations.extend(verify_ordering(graph));
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// Every required input has a providing output in the source workspace.
pub fn verify_connectivity(graph: &WorkspaceGraph) -> Vec<GraphViolation> {
    let mut violations = Vec::new();
    for node in graph.nodes.values() {
        for input in &node.inputs {
            let source = graph.nodes.get(&input.source_workspace);
            let output_exists = source.is_some_and(|src| {
                src.outputs.iter().any(|o| o.name == input.source_output)
            });
            if !output_exists {
                violations.push(GraphViolation::DisconnectedInput {
                    workspace: node.id.0.clone(),
                    input: input.name.clone(),
                    source_workspace: input.source_workspace.0.clone(),
                    source_output: input.source_output.clone(),
                });
            }
        }
    }
    violations
}

/// No cycles in the dependency graph (topological sort must exist).
pub fn verify_ordering(graph: &WorkspaceGraph) -> Vec<GraphViolation> {
    // Build adjacency from inputs: source_workspace -> current workspace.
    let mut in_degree: BTreeMap<&WorkspaceId, usize> = BTreeMap::new();
    let mut adjacency: BTreeMap<&WorkspaceId, Vec<&WorkspaceId>> = BTreeMap::new();

    for id in graph.nodes.keys() {
        in_degree.entry(id).or_insert(0);
        adjacency.entry(id).or_default();
    }

    for node in graph.nodes.values() {
        // Collect unique source workspaces to avoid counting duplicate edges.
        let sources: BTreeSet<&WorkspaceId> =
            node.inputs.iter().map(|inp| &inp.source_workspace).collect();
        for src in sources {
            if graph.nodes.contains_key(src) {
                adjacency.entry(src).or_default().push(&node.id);
                *in_degree.entry(&node.id).or_insert(0) += 1;
            }
        }
    }

    // Kahn's algorithm.
    let mut queue: VecDeque<&WorkspaceId> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();
    let mut visited = 0usize;

    while let Some(current) = queue.pop_front() {
        visited += 1;
        if let Some(neighbors) = adjacency.get(current) {
            for &neighbor in neighbors {
                let deg = in_degree.get_mut(neighbor).expect("node must exist");
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(neighbor);
                }
            }
        }
    }

    if visited == graph.nodes.len() {
        Vec::new()
    } else {
        // Every node still with nonzero in-degree participates in a cycle.
        let cycle_members: Vec<String> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg > 0)
            .map(|(id, _)| id.0.clone())
            .collect();
        vec![GraphViolation::CyclicDependency {
            workspaces: cycle_members,
        }]
    }
}

/// Output type matches input type for every edge.
pub fn verify_compatibility(graph: &WorkspaceGraph) -> Vec<GraphViolation> {
    let mut violations = Vec::new();
    for node in graph.nodes.values() {
        for input in &node.inputs {
            if let Some(source_node) = graph.nodes.get(&input.source_workspace) {
                if let Some(output) = source_node
                    .outputs
                    .iter()
                    .find(|o| o.name == input.source_output)
                {
                    if output.field_type != input.field_type {
                        violations.push(GraphViolation::TypeMismatch {
                            from_workspace: input.source_workspace.0.clone(),
                            output: input.source_output.clone(),
                            output_type: output.field_type.clone(),
                            to_workspace: node.id.0.clone(),
                            input: input.name.clone(),
                            input_type: input.field_type.clone(),
                        });
                    }
                }
            }
        }
    }
    violations
}

/// No duplicate output names within a workspace.
pub fn verify_uniqueness(graph: &WorkspaceGraph) -> Vec<GraphViolation> {
    let mut violations = Vec::new();
    for node in graph.nodes.values() {
        let mut seen = HashSet::new();
        for output in &node.outputs {
            if !seen.insert(&output.name) {
                violations.push(GraphViolation::DuplicateOutput {
                    workspace: node.id.0.clone(),
                    output: output.name.clone(),
                });
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::WorkspaceGraphBuilder;

    #[test]
    fn empty_graph_passes_all_verification() {
        let graph = WorkspaceGraphBuilder::new().build();
        assert_eq!(verify_graph(&graph), Ok(()));
    }

    #[test]
    fn single_node_graph_passes_all_verification() {
        let graph = WorkspaceGraphBuilder::new()
            .workspace("dns", "pleme-dns", "dns/terraform.tfstate", "aws")
            .output("dns", "zone_id", IacType::String, "aws_route53_zone.zone")
            .build();
        assert_eq!(verify_graph(&graph), Ok(()));
    }

    #[test]
    fn valid_two_node_graph_passes() {
        let graph = WorkspaceGraphBuilder::new()
            .workspace("dns", "pleme-dns", "dns/terraform.tfstate", "aws")
            .output("dns", "zone_id", IacType::String, "aws_route53_zone.zone")
            .workspace("cluster", "seph-cluster", "cluster/terraform.tfstate", "aws")
            .input("cluster", "zone_id", IacType::String, "dns", "zone_id")
            .build();
        assert_eq!(verify_graph(&graph), Ok(()));
    }

    #[test]
    fn detects_disconnected_input() {
        let graph = WorkspaceGraphBuilder::new()
            .workspace("cluster", "seph-cluster", "cluster/terraform.tfstate", "aws")
            .input(
                "cluster",
                "zone_id",
                IacType::String,
                "nonexistent",
                "zone_id",
            )
            .build();
        let violations = verify_connectivity(&graph);
        assert_eq!(violations.len(), 1);
        assert!(matches!(
            &violations[0],
            GraphViolation::DisconnectedInput { .. }
        ));
    }

    #[test]
    fn detects_type_mismatch() {
        let graph = WorkspaceGraphBuilder::new()
            .workspace("dns", "pleme-dns", "dns/terraform.tfstate", "aws")
            .output("dns", "zone_id", IacType::String, "aws_route53_zone.zone")
            .workspace("cluster", "seph-cluster", "cluster/terraform.tfstate", "aws")
            .input("cluster", "zone_id", IacType::Integer, "dns", "zone_id")
            .build();
        let violations = verify_compatibility(&graph);
        assert_eq!(violations.len(), 1);
        assert!(matches!(
            &violations[0],
            GraphViolation::TypeMismatch { .. }
        ));
    }

    #[test]
    fn detects_cycle() {
        let graph = WorkspaceGraphBuilder::new()
            .workspace("a", "A", "a/terraform.tfstate", "aws")
            .output("a", "out_a", IacType::String, "res_a")
            .input("a", "in_a", IacType::String, "b", "out_b")
            .workspace("b", "B", "b/terraform.tfstate", "aws")
            .output("b", "out_b", IacType::String, "res_b")
            .input("b", "in_b", IacType::String, "a", "out_a")
            .build();
        let violations = verify_ordering(&graph);
        assert_eq!(violations.len(), 1);
        assert!(matches!(
            &violations[0],
            GraphViolation::CyclicDependency { .. }
        ));
    }

    #[test]
    fn detects_duplicate_output() {
        let graph = WorkspaceGraphBuilder::new()
            .workspace("dns", "pleme-dns", "dns/terraform.tfstate", "aws")
            .output("dns", "zone_id", IacType::String, "aws_route53_zone.zone")
            .output("dns", "zone_id", IacType::String, "aws_route53_zone.other")
            .build();
        let violations = verify_uniqueness(&graph);
        assert_eq!(violations.len(), 1);
        assert!(matches!(
            &violations[0],
            GraphViolation::DuplicateOutput { .. }
        ));
    }
}
