# Composition Model

How architectures decompose into sub-workspaces with typed guarantees.

## The Problem

A platform like Seph requires multiple Terraform workspaces: VPC, DNS, cluster,
secrets. Each workspace has its own state file, but they reference each other --
the cluster workspace needs the VPC ID from the VPC workspace. These cross-state
references are the source of most infrastructure deployment failures:

- A workspace references an output that does not exist
- A workspace expects a string but gets a list
- Two workspaces form a circular dependency
- An output name is duplicated, creating ambiguous references

`workspace-state-graph` makes these failures impossible by catching them at
verification time, before any infrastructure is deployed.

## Process Model Analogy

The composition model maps directly to the Unix process model. This is not a
loose metaphor -- it is the literal operating model used by the convergence
controller:

| Unix Process | Infrastructure Composition |
|-------------|---------------------------|
| Parent process (PID 1) | Parent workspace -- orchestrator, holds no resources |
| Child process (PID 2+) | Sub-workspace -- owns independent Terraform state |
| Process IPC | Typed ports (OutputPort -> InputPort) |
| `fork()` | `CompositionBuilder::sub_workspace()` |
| `waitpid()` | Deployment order (topological sort) |
| `kill(SIGTERM)` | Impact analysis (which workspaces must re-deploy) |
| `/proc` | `CompositionPlan.children` |

### PID Assignment

PIDs are assigned sequentially starting from 2. PID 1 is always the parent
(orchestrator). All children have `ppid = 1` -- the composition tree is flat
within a single architecture:

```rust
let plan = CompositionBuilder::new("seph", "Seph Platform", "aws")
    .sub_workspace("network", |ws| ws.output("vpc_id", IacType::String, "aws_vpc.seph.id"))
    .sub_workspace("dns", |ws| ws.output("zone_id", IacType::String, "aws_route53_zone.main.zone_id"))
    .sub_workspace("cluster", |ws| {
        ws.input_from("network", "vpc_id", IacType::String)
          .input_from("dns", "zone_id", IacType::String)
    })
    .build();

// PID assignment:
// seph (parent)  -> PID 1 (implicit)
// seph-network   -> PID 2
// seph-dns       -> PID 3
// seph-cluster   -> PID 4
```

## State Key Naming Convention

State keys follow a deterministic pattern: `pangea/{parent}/{child}`. This
ensures every workspace's S3 state file can be derived from the composition
tree without configuration:

```
pangea/seph              -- parent orchestrator state
pangea/seph/network      -- network sub-workspace
pangea/seph/dns          -- DNS sub-workspace
pangea/seph/cluster      -- cluster sub-workspace
pangea/seph/secrets      -- secrets sub-workspace
```

The `CompositionBuilder` generates these automatically:

```rust
let plan = CompositionBuilder::new("seph", "Seph", "aws")
    .sub_workspace("network", |ws| ws.output("id", IacType::String, "r.id"))
    .build();

assert_eq!(plan.children[0].state_key, "pangea/seph/network");
```

## Workspace ID Resolution

Sub-workspace IDs are formed as `{parent}-{child}`. When a sub-workspace
references a sibling via `input_from("network", ...)`, the builder resolves
the reference to the fully qualified ID `{parent}-network`:

```rust
// Within CompositionBuilder for parent "seph":
.sub_workspace("cluster", |ws| {
    ws.input_from("network", "vpc_id", IacType::String)
    // Resolves to: source_workspace = "seph-network"
})
```

This prevents name collisions between compositions while keeping the builder
API concise.

## Deployment Ordering

`CompositionPlan::deployment_order()` returns a topological sort -- workspaces
appear after all their dependencies. For the Seph platform:

```
1. seph (parent -- no deps, deployed first)
2. seph-network (no deps among children)
3. seph-dns (no deps among children)
4. seph-cluster (depends on network + dns)
5. seph-secrets (depends on cluster)
```

The sort is deterministic (BTreeMap iteration order breaks ties). If the graph
contains a cycle, `deployment_order()` returns `Err(GraphViolation::CyclicDependency)`.

## Impact Analysis

`affected_workspaces()` computes the transitive closure of all workspaces
affected by a change:

```rust
use workspace_state_graph::analysis::affected_workspaces;
use workspace_state_graph::types::WorkspaceId;

// If seph-network changes, which workspaces need re-deployment?
let affected = affected_workspaces(&plan.graph, &WorkspaceId("seph-network".into()));
// Returns: [seph-cluster, seph-secrets]
// (cluster depends on network, secrets depends on cluster)

// The changed workspace itself is NOT included
assert!(!affected.contains(&WorkspaceId("seph-network".into())));
```

This enables targeted deployment: instead of re-deploying everything, only
the affected workspaces need to re-converge.

## Diamond Dependencies

Real infrastructure often has diamond patterns: VPC produces outputs consumed
by both subnets and security groups, which are both consumed by the cluster.
The composition model handles this correctly:

```rust
let plan = CompositionBuilder::new("diamond", "Diamond", "aws")
    .sub_workspace("a", |ws| {
        ws.output("x", IacType::String, "res_a.x")
    })
    .sub_workspace("b", |ws| {
        ws.input_from("a", "x", IacType::String)
            .output("y", IacType::String, "res_b.y")
    })
    .sub_workspace("c", |ws| {
        ws.input_from("a", "x", IacType::String)
            .output("z", IacType::String, "res_c.z")
    })
    .sub_workspace("d", |ws| {
        ws.input_from("b", "y", IacType::String)
            .input_from("c", "z", IacType::String)
    })
    .build();

// 5 nodes (parent + 4), 4 edges (a->b, a->c, b->d, c->d)
assert!(plan.verify().is_ok());

// Deployment order: a before b and c, both before d
let order = plan.deployment_order().expect("no cycles");
```

## Input Aliasing

When the local input name differs from the source output name, use `input_as`:

```rust
.sub_workspace("cluster", |ws| {
    ws.input_as(
        "dns_zone_id",       // local name in this workspace
        "dns",               // sibling workspace
        "zone_id",           // output name on the sibling
        IacType::String,
    )
})
```

This is used when a workspace consumes the same type from multiple sources
and needs distinct local names.

## Composition vs Direct Graph Construction

Use `CompositionBuilder` when modeling a single architecture that decomposes
into sub-workspaces with a shared parent. Use `WorkspaceGraphBuilder` when
constructing arbitrary graphs (e.g., the full pleme-io topology that spans
multiple independent architectures).

| Feature | CompositionBuilder | WorkspaceGraphBuilder |
|---------|-------------------|----------------------|
| Auto PID assignment | Yes | No |
| Auto state key generation | Yes (pangea/{parent}/{child}) | Manual |
| Sibling name resolution | Yes ({parent}-{child}) | Manual |
| `verify()` method | On CompositionPlan | Use `verify_graph()` |
| Parent workspace | Auto-created | Manual |
