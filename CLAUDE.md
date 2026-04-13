# workspace-state-graph

Typed workspace dependency DAG with verification proofs for cross-workspace
infrastructure dependencies.

## Purpose

The pleme-io ecosystem has 14+ Pangea workspaces (seph-vpc, seph-cluster,
pleme-dns, etc.) that reference each other via Terraform remote state. This
crate provides typed Rust representations of workspace inputs/outputs and
proves graph-level invariants: connectivity, ordering, type compatibility,
and uniqueness.

## Key Types

- `WorkspaceGraph` -- the complete dependency graph (nodes + edges)
- `WorkspaceNode` -- a workspace with typed input/output ports
- `WorkspaceEdge` -- a typed dependency between workspaces
- `InputPort` / `OutputPort` -- typed ports using `iac_forge::ir::IacType`

## Verification

```rust
use workspace_state_graph::{verify_graph, WorkspaceGraphBuilder};
use iac_forge::ir::IacType;

let graph = WorkspaceGraphBuilder::new()
    .workspace("dns", "pleme-dns", "dns/terraform.tfstate", "aws")
    .output("dns", "zone_id", IacType::String, "aws_route53_zone.zone")
    .workspace("cluster", "seph-cluster", "cluster/terraform.tfstate", "aws")
    .input("cluster", "zone_id", IacType::String, "dns", "zone_id")
    .build();

assert!(verify_graph(&graph).is_ok());
```

## Analysis

- `deployment_order(graph)` -- topological sort for deploy sequencing
- `affected_workspaces(graph, changed)` -- transitive impact analysis

## Testing

```bash
cargo test
```

Property tests (proptest) verify that valid graphs always pass, mutations
(removing outputs, changing types, adding back-edges) are always caught,
and topological sort respects every edge.
