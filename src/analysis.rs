use crate::types::{WorkspaceGraph, WorkspaceId};
use crate::verify::GraphViolation;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Compute the transitive closure of workspaces affected by a change to `changed`.
///
/// Returns every workspace that directly or transitively depends on the
/// changed workspace. The changed workspace itself is **not** included.
pub fn affected_workspaces(graph: &WorkspaceGraph, changed: &WorkspaceId) -> Vec<WorkspaceId> {
    // Build forward adjacency: source -> dependents.
    let mut forward: BTreeMap<&WorkspaceId, Vec<&WorkspaceId>> = BTreeMap::new();
    for id in graph.nodes.keys() {
        forward.entry(id).or_default();
    }
    for node in graph.nodes.values() {
        for input in &node.inputs {
            if graph.nodes.contains_key(&input.source_workspace) {
                forward
                    .entry(&input.source_workspace)
                    .or_default()
                    .push(&node.id);
            }
        }
    }

    // BFS from `changed`.
    let mut visited: BTreeSet<&WorkspaceId> = BTreeSet::new();
    let mut queue: VecDeque<&WorkspaceId> = VecDeque::new();

    if let Some(neighbors) = forward.get(changed) {
        for &n in neighbors {
            if visited.insert(n) {
                queue.push_back(n);
            }
        }
    }

    while let Some(current) = queue.pop_front() {
        if let Some(neighbors) = forward.get(current) {
            for &n in neighbors {
                if visited.insert(n) {
                    queue.push_back(n);
                }
            }
        }
    }

    visited.into_iter().cloned().collect()
}

/// Compute a valid deployment order via topological sort.
///
/// Returns workspaces ordered so that every workspace appears **after** all of
/// its dependencies. Returns an error if the graph contains a cycle.
pub fn deployment_order(graph: &WorkspaceGraph) -> Result<Vec<WorkspaceId>, GraphViolation> {
    let mut in_degree: BTreeMap<&WorkspaceId, usize> = BTreeMap::new();
    let mut adjacency: BTreeMap<&WorkspaceId, Vec<&WorkspaceId>> = BTreeMap::new();

    for id in graph.nodes.keys() {
        in_degree.entry(id).or_insert(0);
        adjacency.entry(id).or_default();
    }

    for node in graph.nodes.values() {
        let sources: BTreeSet<&WorkspaceId> =
            node.inputs.iter().map(|inp| &inp.source_workspace).collect();
        for src in sources {
            if graph.nodes.contains_key(src) {
                adjacency.entry(src).or_default().push(&node.id);
                *in_degree.entry(&node.id).or_insert(0) += 1;
            }
        }
    }

    let mut queue: VecDeque<&WorkspaceId> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();

    let mut order = Vec::new();

    while let Some(current) = queue.pop_front() {
        order.push(current.clone());
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

    if order.len() == graph.nodes.len() {
        Ok(order)
    } else {
        let cycle_members: Vec<String> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg > 0)
            .map(|(id, _)| id.0.clone())
            .collect();
        Err(GraphViolation::CyclicDependency {
            workspaces: cycle_members,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::WorkspaceGraphBuilder;
    use iac_forge::ir::IacType;

    #[test]
    fn affected_workspaces_transitive() {
        // a -> b -> c
        let graph = WorkspaceGraphBuilder::new()
            .workspace("a", "A", "a.tfstate", "aws")
            .output("a", "x", IacType::String, "res.x")
            .workspace("b", "B", "b.tfstate", "aws")
            .input("b", "x", IacType::String, "a", "x")
            .output("b", "y", IacType::String, "res.y")
            .workspace("c", "C", "c.tfstate", "aws")
            .input("c", "y", IacType::String, "b", "y")
            .build();

        let affected = affected_workspaces(&graph, &WorkspaceId("a".to_owned()));
        assert_eq!(affected.len(), 2);
        assert!(affected.contains(&WorkspaceId("b".to_owned())));
        assert!(affected.contains(&WorkspaceId("c".to_owned())));
    }

    #[test]
    fn affected_workspaces_does_not_include_self() {
        let graph = WorkspaceGraphBuilder::new()
            .workspace("a", "A", "a.tfstate", "aws")
            .output("a", "x", IacType::String, "res.x")
            .workspace("b", "B", "b.tfstate", "aws")
            .input("b", "x", IacType::String, "a", "x")
            .build();

        let affected = affected_workspaces(&graph, &WorkspaceId("a".to_owned()));
        assert!(!affected.contains(&WorkspaceId("a".to_owned())));
    }

    #[test]
    fn affected_workspaces_leaf_has_no_dependents() {
        let graph = WorkspaceGraphBuilder::new()
            .workspace("a", "A", "a.tfstate", "aws")
            .output("a", "x", IacType::String, "res.x")
            .workspace("b", "B", "b.tfstate", "aws")
            .input("b", "x", IacType::String, "a", "x")
            .build();

        let affected = affected_workspaces(&graph, &WorkspaceId("b".to_owned()));
        assert!(affected.is_empty());
    }

    #[test]
    fn deployment_order_respects_edges() {
        // a -> b -> c
        let graph = WorkspaceGraphBuilder::new()
            .workspace("a", "A", "a.tfstate", "aws")
            .output("a", "x", IacType::String, "res.x")
            .workspace("b", "B", "b.tfstate", "aws")
            .input("b", "x", IacType::String, "a", "x")
            .output("b", "y", IacType::String, "res.y")
            .workspace("c", "C", "c.tfstate", "aws")
            .input("c", "y", IacType::String, "b", "y")
            .build();

        let order = deployment_order(&graph).expect("should succeed");
        let pos = |id: &str| {
            order
                .iter()
                .position(|w| w.0 == id)
                .expect("should exist")
        };
        assert!(pos("a") < pos("b"));
        assert!(pos("b") < pos("c"));
    }

    #[test]
    fn deployment_order_cycle_error() {
        let graph = WorkspaceGraphBuilder::new()
            .workspace("a", "A", "a.tfstate", "aws")
            .output("a", "x", IacType::String, "res.x")
            .input("a", "y", IacType::String, "b", "y")
            .workspace("b", "B", "b.tfstate", "aws")
            .output("b", "y", IacType::String, "res.y")
            .input("b", "x", IacType::String, "a", "x")
            .build();

        let result = deployment_order(&graph);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GraphViolation::CyclicDependency { .. }
        ));
    }

    #[test]
    fn deployment_order_empty_graph() {
        let graph = WorkspaceGraphBuilder::new().build();
        let order = deployment_order(&graph).expect("should succeed");
        assert!(order.is_empty());
    }
}
