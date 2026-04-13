# Integration Guide

How `workspace-state-graph` connects to the pleme-io ecosystem.

## Architecture Overview

```
workspace-state-graph (this crate)
    |
    |-- iac-forge (IacType shared type system)
    |
    +-- consumed by:
        |-- pangea-forge (Ruby IaC generation from typed resources)
        |-- pangea-sim (infrastructure simulation with graph verification)
        |-- ruby-synthesizer (typed resource composition)
        |-- convergence-controller (process model + deployment ordering)
        |-- tameshi/kensa (compliance verification against graph invariants)
```

## iac-forge: The Shared Type System

All port types in `workspace-state-graph` use `iac_forge::ir::IacType`:

```rust
pub enum IacType {
    String,
    Integer,
    Float,
    Boolean,
    List(Box<IacType>),
    Set(Box<IacType>),
    Map(Box<IacType>),
    Object(BTreeMap<String, IacType>),
    Enum(Vec<String>),
    Any,
}
```

This is the same type system used by all six IaC forge backends (terraform-forge,
pulumi-forge, crossplane-forge, ansible-forge, pangea-forge, steampipe-forge).
When `workspace-state-graph` verifies type compatibility between workspaces, it
uses the exact same types that drive code generation.

**Implication:** A verified graph guarantees that generated Terraform code will
have type-compatible remote state references. The verification and generation
share a single source of truth for types.

### Dependency

```toml
[dependencies]
iac-forge = { git = "https://github.com/pleme-io/iac-forge" }
```

Only the `ir` module is used (`iac_forge::ir::IacType`). No backend code is
pulled in.

## pangea-forge: Ruby IaC Generation

`pangea-forge` generates Ruby DSL resource functions for the Pangea IaC
framework. Each generated resource has typed attributes that map to `IacType`.

The connection to `workspace-state-graph`:

1. `pangea-forge` generates typed Pangea resources with input/output attributes
2. `workspace-state-graph` models the cross-workspace references between those resources
3. Verification proves the references are correct before deployment

Example flow:

```
pangea-forge generates:
  seph-vpc workspace -> outputs vpc_id (String), subnet_ids (List(String))
  seph-cluster workspace -> inputs vpc_id (String), subnet_ids (List(String))

workspace-state-graph verifies:
  seph-vpc.vpc_id (String) -> seph-cluster.vpc_id (String)  -- types match
  seph-vpc.subnet_ids (List(String)) -> seph-cluster.subnet_ids (List(String))  -- types match
```

## pangea-sim: Infrastructure Simulation

`pangea-sim` simulates infrastructure deployments without calling cloud APIs.
`workspace-state-graph` provides the topology for simulation:

1. Build the workspace graph (via `pleme_infrastructure_graph()` or `CompositionBuilder`)
2. Verify the graph (`verify_graph()`)
3. Compute deployment order (`deployment_order()`)
4. Simulate each workspace in order, using synthesized outputs as inputs to downstream workspaces

The simulation proves that the deployment would succeed without spending
cloud resources. Combined with pangea-architectures RSpec tests (358 tests),
this creates a zero-cost verification pipeline.

## ruby-synthesizer: Typed Resource Composition

`ruby-synthesizer` composes Pangea resources into reusable architectures.
It is itself a convergence machine with lattice-preserving type homomorphisms.

The workspace graph models the cross-architecture boundaries:

- Each architecture produces outputs (typed ports)
- Other architectures consume those outputs (typed ports)
- `workspace-state-graph` verifies the wiring between architectures

The `OutputPort.source_resource` field (e.g., `aws_route53_zone.zone.zone_id`)
maps directly to the resource path in synthesized Terraform output.

## convergence-controller: Process Model

The `CompositionPlan` maps directly to the convergence controller's CRDs:

| workspace-state-graph | convergence-controller CRD |
|-----------------------|---------------------------|
| `CompositionPlan.parent` | `ConvergenceProcess` (parent) |
| `ChildProcess.pid` | `ConvergenceProcess.status.pid` |
| `ChildProcess.ppid` | `ConvergenceProcess.spec.parentPid` |
| `ChildProcess.state_key` | S3 state path for pangea-operator |
| `deployment_order()` | Process start order |
| `affected_workspaces()` | Signal propagation (which processes to re-converge) |
| `WorkspaceEdge` | Cross-process typed IPC |

### DNS Identity

The workspace graph's naming convention aligns with the convergence process DNS:

```
{service}.{name_or_hash}.{pid}.k8s.quero.lol
```

A `ChildProcess` with `pid: 2` and `name: "network"` in parent `"seph"` maps to:
```
*.seph.1.k8s.quero.lol     -- parent
*.network.2.k8s.quero.lol  -- child (within parent's scope)
```

### Deployment Automation

The convergence controller uses `deployment_order()` to determine the sequence
for bringing up sub-processes:

1. Compute deployment order from the graph
2. For each workspace in order, create the `ConvergenceProcess` CRD
3. Wait for each process to reach `Ready` before starting dependents
4. If a workspace changes, use `affected_workspaces()` to determine which
   downstream processes need re-convergence

## tameshi / kensa: Compliance Verification

The graph itself is a verifiable artifact. `tameshi` can attest the graph's
integrity:

1. Serialize the `WorkspaceGraph` (implements `Serialize`)
2. Compute BLAKE3 hash of the serialized graph
3. Include in the attestation Merkle tree as an infrastructure layer
4. `sekiban` gates deployment on matching graph attestation

`kensa` compliance dimensions map to graph properties:

| Compliance Control | Graph Verification |
|-------------------|-------------------|
| NIST SC-7 (boundary protection) | Connectivity (all references resolved) |
| NIST CM-2 (baseline configuration) | Uniqueness (no ambiguous outputs) |
| NIST SA-10 (developer config mgmt) | Ordering (no cycles) |
| CIS 5.x (network security) | Compatibility (types match) |

## Usage Patterns

### Adding a New Workspace to the Real Topology

Edit `src/pleme.rs` and add the workspace to `pleme_infrastructure_graph()`:

```rust
// Add a new workspace
.workspace("new-ws", "New Workspace", "pangea/new-ws", "aws")
.input("new-ws", "cluster_endpoint", IacType::String, "seph-cluster", "cluster_endpoint")
.output("new-ws", "result", IacType::String, "some_resource.attr")
```

Then run `cargo test` -- the pleme_topology tests will verify the new workspace
integrates correctly.

### Creating a New Architecture Composition

```rust
use workspace_state_graph::composition::CompositionBuilder;
use iac_forge::ir::IacType;

let plan = CompositionBuilder::new("my-arch", "My Architecture", "aws")
    .sub_workspace("base", |ws| {
        ws.output("id", IacType::String, "aws_resource.base.id")
    })
    .sub_workspace("app", |ws| {
        ws.input_from("base", "id", IacType::String)
          .output("endpoint", IacType::String, "aws_lb.app.dns_name")
    })
    .build();

// Verify before using
assert!(plan.verify().is_ok());

// Get deployment order for automation
let order = plan.deployment_order().expect("no cycles");

// Serialize for attestation or storage
let json = serde_json::to_string_pretty(&plan).unwrap();
```

### Checking Impact Before Deployment

```rust
use workspace_state_graph::pleme::pleme_infrastructure_graph;
use workspace_state_graph::analysis::affected_workspaces;
use workspace_state_graph::types::WorkspaceId;

let graph = pleme_infrastructure_graph();

// Before modifying seph-vpc, check what else needs re-deployment
let affected = affected_workspaces(&graph, &WorkspaceId("seph-vpc".into()));
for ws in &affected {
    println!("Must re-deploy: {}", ws.0);
}
// Output: seph-cluster, akeyless-dev-config
```

## Serialization

All core types implement `Serialize`/`Deserialize`. The graph can be:

- Serialized to JSON for storage or API responses
- Deserialized from JSON for verification without rebuilding
- Included in BLAKE3 Merkle trees for tameshi attestation
- Transmitted over MCP for convergence controller integration

```rust
use workspace_state_graph::pleme::pleme_infrastructure_graph;

let graph = pleme_infrastructure_graph();
let json = serde_json::to_string_pretty(&graph).unwrap();
let restored: workspace_state_graph::WorkspaceGraph = serde_json::from_str(&json).unwrap();
assert_eq!(graph.nodes.len(), restored.nodes.len());
```
