use iac_forge::ir::IacType;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Content-addressable workspace identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub String);

/// A typed output port -- what a workspace produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputPort {
    pub name: String,
    pub field_type: IacType,
    /// Resource path producing this output (e.g., `aws_route53_zone.zone.zone_id`).
    pub source_resource: String,
}

/// A typed input port -- what a workspace consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputPort {
    pub name: String,
    pub field_type: IacType,
    pub source_workspace: WorkspaceId,
    pub source_output: String,
}

/// Metadata about a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceNode {
    pub id: WorkspaceId,
    pub name: String,
    /// S3 state file key.
    pub state_key: String,
    /// Cloud provider (e.g., `aws`).
    pub provider: String,
    pub outputs: Vec<OutputPort>,
    pub inputs: Vec<InputPort>,
}

/// A typed edge between workspaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEdge {
    pub from: WorkspaceId,
    pub to: WorkspaceId,
    pub output_name: String,
    pub input_name: String,
    pub field_type: IacType,
}

/// The complete workspace dependency graph.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceGraph {
    pub nodes: BTreeMap<WorkspaceId, WorkspaceNode>,
    pub edges: Vec<WorkspaceEdge>,
}
