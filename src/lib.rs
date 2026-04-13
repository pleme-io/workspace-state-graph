//! Typed workspace dependency DAG with verification.
//!
//! Models cross-workspace infrastructure dependencies as a directed acyclic
//! graph with typed ports. Provides verification functions that prove
//! connectivity, ordering, type compatibility, and uniqueness invariants.

/// Impact analysis and deployment ordering.
pub mod analysis;
/// Fluent graph construction helpers.
pub mod builder;
/// Workspace composition — architectures that decompose into sub-workspaces.
pub mod composition;
/// Real pleme-io infrastructure topology.
pub mod pleme;
/// Core types: workspaces, ports, edges, and the graph.
pub mod types;
/// Graph invariant verification.
pub mod verify;

pub use analysis::{affected_workspaces, deployment_order};
pub use builder::WorkspaceGraphBuilder;
pub use types::{InputPort, OutputPort, WorkspaceEdge, WorkspaceGraph, WorkspaceId, WorkspaceNode};
pub use verify::{
    GraphViolation, verify_compatibility, verify_connectivity, verify_graph, verify_ordering,
    verify_uniqueness,
};
