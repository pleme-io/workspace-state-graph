//! Workspace composition — architectures that decompose into sub-workspaces.
//!
//! Maps the convergence process model to infrastructure state boundaries:
//! - Parent workspace = parent process (orchestrator)
//! - Sub-workspace = child process (owns its own Terraform state)
//! - Typed ports = process IPC (cross-state references)
//!
//! A single architecture declaration can produce multiple workspaces,
//! each with independent state, while maintaining typed guarantees
//! across the entire composition.
//!
//! ```rust
//! use workspace_state_graph::composition::CompositionBuilder;
//! use iac_forge::ir::IacType;
//!
//! let plan = CompositionBuilder::new("seph", "Seph Platform", "aws")
//!     .sub_workspace("network", |ws| {
//!         ws.output("vpc_id", IacType::String, "aws_vpc.seph.id")
//!           .output("subnet_ids",
//!               IacType::List(Box::new(IacType::String)),
//!               "aws_subnet.*.id")
//!     })
//!     .sub_workspace("dns", |ws| {
//!         ws.output("zone_id", IacType::String, "aws_route53_zone.main.zone_id")
//!     })
//!     .sub_workspace("cluster", |ws| {
//!         ws.input_from("network", "vpc_id", IacType::String)
//!           .input_from("dns", "zone_id", IacType::String)
//!           .output("endpoint", IacType::String, "aws_lb.api.dns_name")
//!     })
//!     .build();
//!
//! assert!(plan.graph.nodes.len() == 4); // parent + 3 children
//! assert!(plan.verify().is_ok());
//! ```

use crate::builder::WorkspaceGraphBuilder;
use crate::types::{WorkspaceGraph, WorkspaceId};
use crate::verify;
use iac_forge::ir::IacType;
use serde::{Deserialize, Serialize};

/// A resolved composition plan — an architecture decomposed into sub-workspaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositionPlan {
    /// The parent workspace (orchestrator — holds no resources itself).
    pub parent: WorkspaceId,
    /// Child workspace PIDs and names.
    pub children: Vec<ChildProcess>,
    /// The complete resolved graph (parent + children + edges).
    pub graph: WorkspaceGraph,
}

/// A child workspace in the composition — a sub-process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildProcess {
    /// Workspace identity.
    pub id: WorkspaceId,
    /// Human name.
    pub name: String,
    /// Process ID in the convergence tree.
    pub pid: u32,
    /// Parent PID.
    pub ppid: u32,
    /// S3 state key.
    pub state_key: String,
}

impl CompositionPlan {
    /// Verify the composition graph satisfies all invariants.
    pub fn verify(&self) -> Result<(), Vec<verify::GraphViolation>> {
        verify::verify_graph(&self.graph)
    }

    /// Get deployment order (topological sort respecting dependencies).
    pub fn deployment_order(&self) -> Result<Vec<WorkspaceId>, verify::GraphViolation> {
        crate::analysis::deployment_order(&self.graph)
    }

    /// Get child process by PID.
    #[must_use]
    pub fn process(&self, pid: u32) -> Option<&ChildProcess> {
        self.children.iter().find(|c| c.pid == pid)
    }

    /// Get child process by name.
    #[must_use]
    pub fn process_by_name(&self, name: &str) -> Option<&ChildProcess> {
        self.children.iter().find(|c| c.name == name)
    }
}

// ── Builder ──────────────────────────────────────────────────────────

/// Fluent builder for composing an architecture into sub-workspaces.
pub struct CompositionBuilder {
    parent_name: String,
    parent_label: String,
    provider: String,
    children: Vec<SubWorkspaceSpec>,
    next_pid: u32,
}

struct SubWorkspaceSpec {
    name: String,
    pid: u32,
    outputs: Vec<(String, IacType, String)>,
    inputs: Vec<(String, IacType, String, String)>, // (name, type, source_ws, source_output)
}

impl CompositionBuilder {
    /// Start a new composition. PID 1 is the parent (orchestrator).
    #[must_use]
    pub fn new(name: &str, label: &str, provider: &str) -> Self {
        Self {
            parent_name: name.to_string(),
            parent_label: label.to_string(),
            provider: provider.to_string(),
            children: Vec::new(),
            next_pid: 2, // PID 1 = parent
        }
    }

    /// Add a sub-workspace (child process). The closure configures its ports.
    #[must_use]
    pub fn sub_workspace(
        mut self,
        name: &str,
        f: impl FnOnce(SubWorkspaceBuilder) -> SubWorkspaceBuilder,
    ) -> Self {
        let pid = self.next_pid;
        self.next_pid += 1;
        let builder = f(SubWorkspaceBuilder {
            parent_name: self.parent_name.clone(),
            name: name.to_string(),
            outputs: Vec::new(),
            inputs: Vec::new(),
        });
        self.children.push(SubWorkspaceSpec {
            name: name.to_string(),
            pid,
            outputs: builder.outputs,
            inputs: builder.inputs,
        });
        self
    }

    /// Build the composition plan, generating the workspace graph.
    #[must_use]
    pub fn build(self) -> CompositionPlan {
        let parent_id = self.parent_name.clone();
        let parent_state_key = format!("pangea/{}", self.parent_name);

        let mut graph_builder = WorkspaceGraphBuilder::new()
            .workspace(&parent_id, &self.parent_label, &parent_state_key, &self.provider);

        let mut child_processes = Vec::new();

        for child in &self.children {
            let child_id = format!("{}-{}", self.parent_name, child.name);
            let child_state_key = format!("pangea/{}/{}", self.parent_name, child.name);

            graph_builder = graph_builder.workspace(
                &child_id,
                &child.name,
                &child_state_key,
                &self.provider,
            );

            for (name, ty, source) in &child.outputs {
                graph_builder = graph_builder.output(&child_id, name, ty.clone(), source);
            }

            for (name, ty, source_ws, source_output) in &child.inputs {
                let resolved_source = format!("{}-{}", self.parent_name, source_ws);
                graph_builder = graph_builder.input(
                    &child_id,
                    name,
                    ty.clone(),
                    &resolved_source,
                    source_output,
                );
            }

            child_processes.push(ChildProcess {
                id: WorkspaceId(child_id),
                name: child.name.clone(),
                pid: child.pid,
                ppid: 1,
                state_key: child_state_key,
            });
        }

        CompositionPlan {
            parent: WorkspaceId(parent_id),
            graph: graph_builder.build(),
            children: child_processes,
        }
    }
}

/// Builder for a single sub-workspace within a composition.
pub struct SubWorkspaceBuilder {
    parent_name: String,
    name: String,
    outputs: Vec<(String, IacType, String)>,
    inputs: Vec<(String, IacType, String, String)>,
}

impl SubWorkspaceBuilder {
    /// Declare an output this sub-workspace produces.
    #[must_use]
    pub fn output(mut self, name: &str, field_type: IacType, source: &str) -> Self {
        self.outputs.push((name.to_string(), field_type, source.to_string()));
        self
    }

    /// Declare an input from a sibling sub-workspace.
    #[must_use]
    pub fn input_from(mut self, source_ws: &str, output_name: &str, field_type: IacType) -> Self {
        self.inputs.push((
            output_name.to_string(),
            field_type,
            source_ws.to_string(),
            output_name.to_string(),
        ));
        self
    }

    /// Declare an input with a different local name.
    #[must_use]
    pub fn input_as(
        mut self,
        local_name: &str,
        source_ws: &str,
        source_output: &str,
        field_type: IacType,
    ) -> Self {
        self.inputs.push((
            local_name.to_string(),
            field_type,
            source_ws.to_string(),
            source_output.to_string(),
        ));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_composition() {
        let plan = CompositionBuilder::new("test", "Test Platform", "aws")
            .sub_workspace("vpc", |ws| {
                ws.output("vpc_id", IacType::String, "aws_vpc.main.id")
            })
            .sub_workspace("cluster", |ws| {
                ws.input_from("vpc", "vpc_id", IacType::String)
                    .output("endpoint", IacType::String, "aws_lb.api.dns_name")
            })
            .build();

        // Parent + 2 children = 3 nodes
        assert_eq!(plan.graph.nodes.len(), 3);
        // 1 edge: vpc → cluster
        assert_eq!(plan.graph.edges.len(), 1);
        assert!(plan.verify().is_ok());
    }

    #[test]
    fn pids_assigned_sequentially() {
        let plan = CompositionBuilder::new("p", "P", "aws")
            .sub_workspace("a", |ws| ws.output("x", IacType::String, "r.x"))
            .sub_workspace("b", |ws| ws.output("y", IacType::String, "r.y"))
            .sub_workspace("c", |ws| ws.output("z", IacType::String, "r.z"))
            .build();

        assert_eq!(plan.children[0].pid, 2);
        assert_eq!(plan.children[1].pid, 3);
        assert_eq!(plan.children[2].pid, 4);
        assert!(plan.children.iter().all(|c| c.ppid == 1));
    }

    #[test]
    fn state_keys_nested() {
        let plan = CompositionBuilder::new("seph", "Seph", "aws")
            .sub_workspace("network", |ws| ws.output("id", IacType::String, "r.id"))
            .build();

        assert_eq!(plan.children[0].state_key, "pangea/seph/network");
    }

    #[test]
    fn deployment_order_respects_edges() {
        let plan = CompositionBuilder::new("app", "App", "aws")
            .sub_workspace("db", |ws| {
                ws.output("url", IacType::String, "aws_rds.main.endpoint")
            })
            .sub_workspace("api", |ws| {
                ws.input_from("db", "url", IacType::String)
                    .output("endpoint", IacType::String, "aws_lb.api.dns_name")
            })
            .sub_workspace("frontend", |ws| {
                ws.input_from("api", "endpoint", IacType::String)
            })
            .build();

        let order = plan.deployment_order().expect("no cycles");
        let pos = |suffix: &str| {
            order.iter().position(|id| id.0.ends_with(suffix)).unwrap()
        };

        assert!(pos("db") < pos("api"));
        assert!(pos("api") < pos("frontend"));
    }

    #[test]
    fn process_lookup_by_pid_and_name() {
        let plan = CompositionBuilder::new("p", "P", "aws")
            .sub_workspace("net", |ws| ws.output("x", IacType::String, "r.x"))
            .build();

        assert!(plan.process(2).is_some());
        assert_eq!(plan.process(2).unwrap().name, "net");
        assert!(plan.process_by_name("net").is_some());
        assert!(plan.process(99).is_none());
    }

    #[test]
    fn seph_platform_composition() {
        let plan = CompositionBuilder::new("seph", "Seph Platform", "aws")
            .sub_workspace("network", |ws| {
                ws.output("vpc_id", IacType::String, "aws_vpc.seph.id")
                    .output(
                        "subnet_ids",
                        IacType::List(Box::new(IacType::String)),
                        "aws_subnet.*.id",
                    )
            })
            .sub_workspace("dns", |ws| {
                ws.output("zone_id", IacType::String, "aws_route53_zone.main.zone_id")
            })
            .sub_workspace("cluster", |ws| {
                ws.input_from("network", "vpc_id", IacType::String)
                    .input_from(
                        "network",
                        "subnet_ids",
                        IacType::List(Box::new(IacType::String)),
                    )
                    .input_from("dns", "zone_id", IacType::String)
                    .output("endpoint", IacType::String, "aws_lb.api.dns_name")
            })
            .sub_workspace("secrets", |ws| {
                ws.input_from("cluster", "endpoint", IacType::String)
                    .output("gateway_url", IacType::String, "akeyless_gateway.dev.hostname")
            })
            .build();

        // Parent + 4 children
        assert_eq!(plan.graph.nodes.len(), 5);
        // network→cluster (2 inputs), dns→cluster (1), cluster→secrets (1) = 4 edges
        assert_eq!(plan.graph.edges.len(), 4);
        assert!(plan.verify().is_ok());

        // Deployment order
        let order = plan.deployment_order().expect("DAG");
        let pos = |s: &str| order.iter().position(|id| id.0.contains(s)).unwrap();
        assert!(pos("network") < pos("cluster"));
        assert!(pos("dns") < pos("cluster"));
        assert!(pos("cluster") < pos("secrets"));
    }
}
