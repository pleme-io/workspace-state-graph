# Verification Guarantees

What each verification function proves and why it matters.

## The Compiler Is the Verifier

Traditional infrastructure-as-code catches dependency errors at deploy time --
Terraform `plan` or `apply` fails because a remote state reference is missing,
the wrong type, or circular. These failures are expensive: they happen after
CI/CD has already built, pushed, and scheduled the deployment.

`workspace-state-graph` moves verification to the type level. The verification
functions act as a compiler for infrastructure topology -- they catch structural
errors before any cloud API is called. The Rust type system ensures the graph
data structures are well-formed; the verification functions prove the graph
satisfies higher-level invariants.

## Verification Functions

### `verify_connectivity`

**What it proves:** Every input port has a corresponding output port in the
declared source workspace.

**What it catches:**
- A workspace reads from a workspace that does not exist
- A workspace reads an output name that does not exist in the source workspace
- A workspace was renamed but its consumers still reference the old name
- An output was removed without updating consumers

**How it works:** For each node in the graph, for each input on that node,
look up the source workspace and check that it has an output with the matching
name. Any input without a matching output produces a `DisconnectedInput`
violation.

**Why it matters:** Without connectivity verification, a workspace can reference
`remote_state("seph-vpc").vpc_id` and the error only surfaces when Terraform
tries to read the remote state during `plan`. With connectivity verification,
the error is caught immediately during graph construction.

### `verify_compatibility`

**What it proves:** For every connected edge, the output type matches the input
type exactly.

**What it catches:**
- An output declared as `IacType::String` wired to an input declared as `IacType::Integer`
- A `List(String)` output wired to a `String` input
- Type changes in a producer workspace that break downstream consumers

**How it works:** For each input that has a matching output in its source
workspace (connectivity must hold), compare `output.field_type` with
`input.field_type`. Any mismatch produces a `TypeMismatch` violation with
both types for diagnostics.

**Why it matters:** Terraform remote state returns untyped values. A type
mismatch between workspaces causes runtime errors (e.g., passing a list where
a string is expected). Type compatibility verification catches these at the
graph level using the same `IacType` system that drives code generation.

### `verify_ordering`

**What it proves:** The dependency graph is acyclic -- a valid topological
ordering exists.

**What it catches:**
- Circular dependencies (A depends on B, B depends on A)
- Transitive cycles (A -> B -> C -> A)
- Self-referencing workspaces

**How it works:** Kahn's algorithm. Build in-degree counts and adjacency
lists from input declarations. Start with all zero-in-degree nodes, visit
them, decrement neighbors' in-degrees, and repeat. If all nodes are visited,
the graph is acyclic. If not, the remaining nodes with nonzero in-degree
form a cycle, reported as `CyclicDependency` with the workspace names.

**Why it matters:** A cycle in the dependency graph means no valid deployment
order exists -- workspace A needs B's output to deploy, but B needs A's output.
This creates an infinite deployment loop. Cycle detection ensures the
`deployment_order()` function always succeeds on verified graphs.

### `verify_uniqueness`

**What it proves:** No workspace has two outputs with the same name.

**What it catches:**
- Copy-paste errors creating duplicate outputs
- Merge conflicts that duplicate output declarations

**How it works:** For each workspace node, track output names in a `HashSet`.
Any name seen twice produces a `DuplicateOutput` violation.

**Why it matters:** Duplicate outputs create ambiguous remote state lookups.
If workspace A has two outputs named `vpc_id`, consumers cannot determine
which one they will receive. This is especially dangerous because Terraform
would silently pick one.

## `verify_graph`: The Aggregate Check

`verify_graph()` runs all four checks and aggregates violations:

```rust
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
```

Order matters: uniqueness is checked first (cheapest), then connectivity,
compatibility, and finally ordering (most expensive -- Kahn's algorithm).
All violations are collected rather than failing on the first one, so a
single verification pass reports every problem.

## GraphViolation Enum

```rust
pub enum GraphViolation {
    DisconnectedInput {
        workspace: String,
        input: String,
        source_workspace: String,
        source_output: String,
    },
    TypeMismatch {
        from_workspace: String,
        output: String,
        output_type: IacType,
        to_workspace: String,
        input: String,
        input_type: IacType,
    },
    CyclicDependency {
        workspaces: Vec<String>,
    },
    OrphanWorkspace {
        workspace: String,
    },
    DuplicateOutput {
        workspace: String,
        output: String,
    },
}
```

Each variant carries enough context to produce a clear error message.
`GraphViolation` implements `Display` via `thiserror` and `Error`, `Clone`,
`PartialEq`, `Eq`.

Note: `OrphanWorkspace` is defined but not currently emitted by any
verification function. It is reserved for future use (detecting workspaces
that have no inputs and no outputs, indicating dead code in the graph).

## How Proptest Reinforces Guarantees

The verification functions are tested with proptest property-based testing.
Rather than testing specific hand-crafted examples, proptest generates
thousands of random valid graphs and proves:

1. **Positive proof:** Valid graphs always pass verification. This proves
   the verification functions do not produce false positives.

2. **Negative proof (connectivity):** Removing an output from a valid graph
   always produces a `DisconnectedInput` violation. This proves connectivity
   verification has no false negatives for missing outputs.

3. **Negative proof (compatibility):** Changing an input type to a different
   type always produces a `TypeMismatch` violation. This proves type checking
   has no false negatives.

4. **Negative proof (ordering):** Adding a back-edge to a linear chain always
   produces a `CyclicDependency` violation. This proves cycle detection has
   no false negatives.

5. **Structural proof (topology):** Topological sort respects every edge in
   every generated graph. The `from` workspace always appears before the `to`
   workspace in the deployment order.

6. **Structural proof (impact):** The affected set from changing the root of
   a linear chain is exactly all other nodes. Impact analysis is transitively
   complete.

Two graph strategies are used:
- **Linear chains** (w0 -> w1 -> ... -> wN) -- test sequential dependencies
- **Fan-out** (root -> leaf0, leaf1, ..., leafN) -- test parallel dependencies

Composition-specific proptest strategies additionally verify PID uniqueness,
state key naming, and that the CompositionBuilder produces valid graphs by
construction.

## Relationship to Deploy-Time Verification

`workspace-state-graph` verification is pre-deploy (static analysis). It
complements but does not replace deploy-time verification:

| Layer | Tool | When | What |
|-------|------|------|------|
| Graph verification | workspace-state-graph | Before any deploy | Structural invariants |
| Terraform plan | `terraform plan` | Before apply | Resource-level changes |
| Synthesis tests | pangea-architectures (RSpec) | Before deploy | Resource composition |
| InSpec profiles | inspec-secure-vpc | After deploy | Runtime compliance |
| Admission webhooks | sekiban | During deploy | Integrity attestation |
| Continuous verification | kensa | Post-deploy | Compliance drift |

The guarantees compose: graph verification proves the topology is sound,
synthesis tests prove individual resources are correct, and InSpec proves
the deployed state matches expectations.
