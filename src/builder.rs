use crate::types::{
    InputPort, OutputPort, WorkspaceEdge, WorkspaceGraph, WorkspaceId, WorkspaceNode,
};
use iac_forge::ir::IacType;
use std::collections::BTreeMap;

/// Pending output to be attached to a workspace.
struct PendingOutput {
    workspace: String,
    name: String,
    field_type: IacType,
    source_resource: String,
}

/// Pending input to be attached to a workspace.
struct PendingInput {
    workspace: String,
    name: String,
    field_type: IacType,
    source_workspace: String,
    source_output: String,
}

/// Fluent builder for constructing a [`WorkspaceGraph`].
///
/// Workspaces, outputs, and inputs are accumulated and assembled into a
/// complete graph (with auto-generated edges) when [`build`](Self::build) is called.
pub struct WorkspaceGraphBuilder {
    nodes: BTreeMap<String, WorkspaceNode>,
    pending_outputs: Vec<PendingOutput>,
    pending_inputs: Vec<PendingInput>,
}

impl WorkspaceGraphBuilder {
    /// Create a new empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            pending_outputs: Vec::new(),
            pending_inputs: Vec::new(),
        }
    }

    /// Add a workspace node.
    #[must_use]
    pub fn workspace(mut self, id: &str, name: &str, state_key: &str, provider: &str) -> Self {
        self.nodes.insert(
            id.to_owned(),
            WorkspaceNode {
                id: WorkspaceId(id.to_owned()),
                name: name.to_owned(),
                state_key: state_key.to_owned(),
                provider: provider.to_owned(),
                outputs: Vec::new(),
                inputs: Vec::new(),
            },
        );
        self
    }

    /// Add an output port to a workspace.
    #[must_use]
    pub fn output(
        mut self,
        workspace: &str,
        name: &str,
        field_type: IacType,
        source: &str,
    ) -> Self {
        self.pending_outputs.push(PendingOutput {
            workspace: workspace.to_owned(),
            name: name.to_owned(),
            field_type,
            source_resource: source.to_owned(),
        });
        self
    }

    /// Add an input port to a workspace.
    #[must_use]
    pub fn input(
        mut self,
        workspace: &str,
        name: &str,
        field_type: IacType,
        source_ws: &str,
        source_output: &str,
    ) -> Self {
        self.pending_inputs.push(PendingInput {
            workspace: workspace.to_owned(),
            name: name.to_owned(),
            field_type,
            source_workspace: source_ws.to_owned(),
            source_output: source_output.to_owned(),
        });
        self
    }

    /// Build the graph, auto-generating edges from input declarations.
    #[must_use]
    pub fn build(mut self) -> WorkspaceGraph {
        // Attach outputs.
        for po in self.pending_outputs {
            if let Some(node) = self.nodes.get_mut(&po.workspace) {
                node.outputs.push(OutputPort {
                    name: po.name,
                    field_type: po.field_type,
                    source_resource: po.source_resource,
                });
            }
        }

        // Attach inputs and collect edges.
        let mut edges = Vec::new();
        for pi in self.pending_inputs {
            let field_type = pi.field_type.clone();
            let input_name = pi.name.clone();
            let source_ws = pi.source_workspace.clone();
            let source_output = pi.source_output.clone();
            let ws_id = pi.workspace.clone();

            if let Some(node) = self.nodes.get_mut(&pi.workspace) {
                node.inputs.push(InputPort {
                    name: pi.name,
                    field_type: pi.field_type,
                    source_workspace: WorkspaceId(pi.source_workspace),
                    source_output: pi.source_output,
                });
            }

            edges.push(WorkspaceEdge {
                from: WorkspaceId(source_ws),
                to: WorkspaceId(ws_id),
                output_name: source_output,
                input_name,
                field_type,
            });
        }

        let nodes = self
            .nodes
            .into_iter()
            .map(|(k, v)| (WorkspaceId(k), v))
            .collect();

        WorkspaceGraph { nodes, edges }
    }
}

impl Default for WorkspaceGraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_auto_generates_edges() {
        let graph = WorkspaceGraphBuilder::new()
            .workspace("dns", "pleme-dns", "dns/terraform.tfstate", "aws")
            .output("dns", "zone_id", IacType::String, "aws_route53_zone.zone")
            .workspace("cluster", "seph-cluster", "cluster/terraform.tfstate", "aws")
            .input("cluster", "zone_id", IacType::String, "dns", "zone_id")
            .build();

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);

        let edge = &graph.edges[0];
        assert_eq!(edge.from, WorkspaceId("dns".to_owned()));
        assert_eq!(edge.to, WorkspaceId("cluster".to_owned()));
        assert_eq!(edge.output_name, "zone_id");
        assert_eq!(edge.input_name, "zone_id");
        assert_eq!(edge.field_type, IacType::String);
    }

    #[test]
    fn builder_attaches_outputs_and_inputs() {
        let graph = WorkspaceGraphBuilder::new()
            .workspace("a", "A", "a.tfstate", "aws")
            .output("a", "x", IacType::Boolean, "res.x")
            .workspace("b", "B", "b.tfstate", "aws")
            .input("b", "y", IacType::Boolean, "a", "x")
            .build();

        let a = &graph.nodes[&WorkspaceId("a".to_owned())];
        assert_eq!(a.outputs.len(), 1);
        assert_eq!(a.outputs[0].name, "x");

        let b = &graph.nodes[&WorkspaceId("b".to_owned())];
        assert_eq!(b.inputs.len(), 1);
        assert_eq!(b.inputs[0].name, "y");
        assert_eq!(b.inputs[0].source_workspace, WorkspaceId("a".to_owned()));
    }
}
