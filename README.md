# workspace-state-graph

Typed workspace dependency DAG with verification proofs for cross-workspace
infrastructure dependencies.

Infrastructure platforms decompose into multiple Terraform workspaces (state
boundaries), each producing typed outputs and consuming typed inputs from
others. `workspace-state-graph` models these relationships as a directed
acyclic graph with typed ports and provides verification functions that prove
the graph satisfies connectivity, ordering, type compatibility, and uniqueness
invariants -- at compile/verify time, not at deploy time.

## The Convergence Process Model

Each workspace maps to a **convergence process** in the Unix process model analogy:

| Concept | Infrastructure | Process Model |
|---------|---------------|---------------|
| Parent workspace | Architecture orchestrator | Parent process (PID 1) |
| Sub-workspace | Independent Terraform state | Child process (PID 2+) |
| Typed port | Cross-state reference | Process IPC |
| Deployment order | Topological sort | Process start order |
| Impact analysis | Transitive dependents | Signal propagation |

A single architecture declaration produces multiple workspaces, each with
independent state, while maintaining typed guarantees across the entire
composition.

## Quick Start

### Direct Graph Construction

```rust
use workspace_state_graph::{WorkspaceGraphBuilder, verify_graph};
use iac_forge::ir::IacType;

let graph = WorkspaceGraphBuilder::new()
    .workspace("dns", "pleme-dns", "dns/terraform.tfstate", "aws")
    .output("dns", "zone_id", IacType::String, "aws_route53_zone.zone")
    .workspace("cluster", "seph-cluster", "cluster/terraform.tfstate", "aws")
    .input("cluster", "zone_id", IacType::String, "dns", "zone_id")
    .build();

assert!(verify_graph(&graph).is_ok());
```

### Composition Builder

Decompose an architecture into sub-workspaces with automatic PID assignment,
state key generation, and edge resolution:

```rust
use workspace_state_graph::composition::CompositionBuilder;
use iac_forge::ir::IacType;

let plan = CompositionBuilder::new("seph", "Seph Platform", "aws")
    .sub_workspace("network", |ws| {
        ws.output("vpc_id", IacType::String, "aws_vpc.seph.id")
          .output("subnet_ids",
              IacType::List(Box::new(IacType::String)),
              "aws_subnet.*.id")
    })
    .sub_workspace("dns", |ws| {
        ws.output("zone_id", IacType::String, "aws_route53_zone.main.zone_id")
    })
    .sub_workspace("cluster", |ws| {
        ws.input_from("network", "vpc_id", IacType::String)
          .input_from("dns", "zone_id", IacType::String)
          .output("endpoint", IacType::String, "aws_lb.api.dns_name")
    })
    .build();

assert_eq!(plan.graph.nodes.len(), 4); // parent + 3 children
assert!(plan.verify().is_ok());

// Deployment order respects dependencies
let order = plan.deployment_order().expect("no cycles");
// network and dns deploy before cluster

// Process lookup
let cluster = plan.process_by_name("cluster").unwrap();
assert_eq!(cluster.pid, 4);
assert_eq!(cluster.ppid, 1);
assert_eq!(cluster.state_key, "pangea/seph/cluster");
```

## Verification Functions

Four independent verification functions prove graph invariants. `verify_graph`
runs all four and aggregates violations:

| Function | Invariant | Catches |
|----------|-----------|---------|
| `verify_connectivity` | Every input has a providing output in its source workspace | Dangling cross-state references |
| `verify_compatibility` | Output type matches input type for every edge | `String` wired to `Integer` across workspaces |
| `verify_ordering` | No cycles in the dependency graph (Kahn's algorithm) | Infinite deploy loops |
| `verify_uniqueness` | No duplicate output names within a workspace | Ambiguous remote state lookups |

Each returns a `Vec<GraphViolation>` with detailed error context:

```rust
use workspace_state_graph::verify::{verify_graph, GraphViolation};

match verify_graph(&graph) {
    Ok(()) => println!("All invariants hold"),
    Err(violations) => {
        for v in &violations {
            match v {
                GraphViolation::DisconnectedInput { workspace, input, .. } =>
                    println!("{workspace}.{input} has no source"),
                GraphViolation::TypeMismatch { from_workspace, output, to_workspace, input, .. } =>
                    println!("{from_workspace}.{output} type != {to_workspace}.{input} type"),
                GraphViolation::CyclicDependency { workspaces } =>
                    println!("Cycle involving: {workspaces:?}"),
                GraphViolation::DuplicateOutput { workspace, output } =>
                    println!("{workspace}.{output} defined multiple times"),
                GraphViolation::OrphanWorkspace { workspace } =>
                    println!("{workspace} has no inputs or outputs"),
            }
        }
    }
}
```

## Analysis Functions

### `deployment_order`

Topological sort via Kahn's algorithm. Returns workspaces ordered so that every
workspace appears after all its dependencies:

```rust
use workspace_state_graph::deployment_order;

let order = deployment_order(&graph).expect("no cycles");
// Workspaces appear after their dependencies
```

### `affected_workspaces`

Transitive closure of all workspaces affected by a change. Uses BFS over the
forward dependency graph. The changed workspace itself is not included:

```rust
use workspace_state_graph::affected_workspaces;
use workspace_state_graph::WorkspaceId;

let affected = affected_workspaces(&graph, &WorkspaceId("seph-vpc".into()));
// Returns all workspaces that directly or transitively depend on seph-vpc
```

## Real Infrastructure Topology

The `pleme` module defines the actual pleme-io infrastructure graph:

```text
state-backend (no deps)
     |
seph-vpc (VPC infrastructure)
     |
pleme-dns (Route53 + Porkbun -- independent)
     |  |
seph-cluster (reads seph-vpc + pleme-dns)
     |
akeyless-dev-config (reads seph-cluster)
```

```rust
use workspace_state_graph::pleme::pleme_infrastructure_graph;
use workspace_state_graph::verify::verify_graph;

let graph = pleme_infrastructure_graph();
assert!(verify_graph(&graph).is_ok());
assert_eq!(graph.nodes.len(), 5);
assert_eq!(graph.edges.len(), 4);
```

## Testing

```bash
cargo test
```

56 tests across 5 test suites:

| Suite | Tests | What it proves |
|-------|-------|----------------|
| Unit tests (`src/`) | 21 | Builder, verification, analysis, composition basics |
| `composition_proofs` | 15 | 10 numbered proofs via proptest + deterministic tests |
| `graph_proofs` | 11 | Property tests for graph invariants (valid graphs pass, mutations caught) |
| `pleme_topology` | 8 | Real infrastructure graph validity, deployment order, impact analysis |
| Doc tests | 1 | Code examples in documentation compile and run |

Property tests (proptest) generate random valid graphs (linear chains, fan-out
topologies) and verify that:

1. Valid graphs always pass verification
2. Removing outputs creates disconnected input violations
3. Type mismatches are detected
4. Adding back-edges creates cycle violations
5. Topological sort respects all edges
6. Impact analysis returns the complete transitive closure
7. Composition PIDs are unique and sequential
8. State keys follow `pangea/{parent}/{child}` naming

## Types

```rust
WorkspaceId(String)       // Content-addressable workspace identity
WorkspaceNode             // Metadata: id, name, state_key, provider, outputs, inputs
OutputPort                // name, field_type: IacType, source_resource
InputPort                 // name, field_type: IacType, source_workspace, source_output
WorkspaceEdge             // from, to, output_name, input_name, field_type
WorkspaceGraph            // nodes: BTreeMap<WorkspaceId, WorkspaceNode>, edges: Vec<WorkspaceEdge>
CompositionPlan           // parent, children: Vec<ChildProcess>, graph: WorkspaceGraph
ChildProcess              // id, name, pid, ppid, state_key
GraphViolation            // DisconnectedInput | TypeMismatch | CyclicDependency | OrphanWorkspace | DuplicateOutput
```

Port types use `iac_forge::ir::IacType` -- the same type system used across all
IaC forge backends (Terraform, Pulumi, Crossplane, Ansible, Pangea, Steampipe).

## License

MIT
