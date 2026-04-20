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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_id(s: &str) -> WorkspaceId {
        WorkspaceId(s.to_string())
    }

    fn sample_output(name: &str) -> OutputPort {
        OutputPort {
            name: name.to_string(),
            field_type: IacType::String,
            source_resource: format!("aws_resource.thing.{name}"),
        }
    }

    fn sample_input(name: &str, src_ws: &str, src_out: &str) -> InputPort {
        InputPort {
            name: name.to_string(),
            field_type: IacType::String,
            source_workspace: sample_id(src_ws),
            source_output: src_out.to_string(),
        }
    }

    // --- WorkspaceId: Ord/Hash/Serde -----------------------------------

    #[test]
    fn workspace_id_ord_is_lexical() {
        // BTreeMap<WorkspaceId, _> iteration order depends on Ord.
        // If Ord ever diverges from the inner String's natural order,
        // deterministic graph emission breaks and every snapshot test
        // flaps silently.
        let ids = vec![sample_id("zebra"), sample_id("alpha"), sample_id("mid")];
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(sorted[0].0, "alpha");
        assert_eq!(sorted[1].0, "mid");
        assert_eq!(sorted[2].0, "zebra");
    }

    #[test]
    fn workspace_id_equal_when_inner_equal() {
        assert_eq!(sample_id("x"), sample_id("x"));
        assert_ne!(sample_id("x"), sample_id("y"));
    }

    #[test]
    fn workspace_id_is_hashable_as_hashmap_key() {
        let mut m: HashMap<WorkspaceId, u32> = HashMap::new();
        m.insert(sample_id("a"), 1);
        m.insert(sample_id("b"), 2);
        // Same-content ID overwrites, not inserts a duplicate.
        m.insert(sample_id("a"), 3);
        assert_eq!(m.len(), 2);
        assert_eq!(m.get(&sample_id("a")).copied(), Some(3));
    }

    #[test]
    fn workspace_id_serializes_as_bare_string_tuple() {
        // WorkspaceId is a tuple struct wrapping String — serde emits
        // it as the bare string by default (not {"0": "..."}). If
        // someone adds `#[serde(transparent)]` or removes the derive,
        // downstream JSON parsers break.
        let id = sample_id("seph-vpc");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"seph-vpc\"");
        let back: WorkspaceId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    // --- Port round-trips ----------------------------------------------

    #[test]
    fn output_port_round_trip() {
        let p = sample_output("zone_id");
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"name\":\"zone_id\""));
        assert!(json.contains("\"source_resource\":\"aws_resource.thing.zone_id\""));
        let back: OutputPort = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn input_port_round_trip_preserves_source_workspace() {
        let p = sample_input("vpc_id", "seph-vpc", "vpc_id");
        let json = serde_json::to_string(&p).unwrap();
        // `source_workspace` must round-trip as the bare string form
        // (not an object) — keep lockstep with workspace_id_serializes_*.
        assert!(json.contains("\"source_workspace\":\"seph-vpc\""));
        assert!(json.contains("\"source_output\":\"vpc_id\""));
        let back: InputPort = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.source_workspace, sample_id("seph-vpc"));
    }

    #[test]
    fn workspace_edge_round_trip_preserves_typed_field() {
        let edge = WorkspaceEdge {
            from: sample_id("pleme-dns"),
            to: sample_id("nix-builders"),
            output_name: "zone_id".into(),
            input_name: "zone_id".into(),
            field_type: IacType::String,
        };
        let json = serde_json::to_string(&edge).unwrap();
        let back: WorkspaceEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(back, edge);
    }

    #[test]
    fn workspace_node_round_trip_with_ports() {
        let node = WorkspaceNode {
            id: sample_id("seph-vpc"),
            name: "seph-vpc".into(),
            state_key: "pangea/seph/vpc".into(),
            provider: "aws".into(),
            outputs: vec![sample_output("vpc_id"), sample_output("private_subnet_ids")],
            inputs: vec![],
        };
        let json = serde_json::to_string(&node).unwrap();
        let back: WorkspaceNode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, node);
        assert_eq!(back.outputs.len(), 2);
    }

    // --- WorkspaceGraph -------------------------------------------------

    #[test]
    fn workspace_graph_default_is_empty() {
        let g = WorkspaceGraph::default();
        assert!(g.nodes.is_empty());
        assert!(g.edges.is_empty());
    }

    #[test]
    fn workspace_graph_btreemap_iterates_in_sorted_order() {
        // Deterministic iteration is the reason WorkspaceGraph uses
        // BTreeMap instead of HashMap. If a future refactor swaps the
        // container type, generated Terraform remote_state stanzas
        // (which are emitted in iteration order) would churn on every
        // re-emit.
        let mut g = WorkspaceGraph::default();
        for name in ["zulu", "alpha", "mike", "charlie"] {
            g.nodes.insert(
                sample_id(name),
                WorkspaceNode {
                    id: sample_id(name),
                    name: name.into(),
                    state_key: format!("pangea/{name}"),
                    provider: "aws".into(),
                    outputs: vec![],
                    inputs: vec![],
                },
            );
        }
        let order: Vec<&str> = g.nodes.keys().map(|w| w.0.as_str()).collect();
        assert_eq!(order, vec!["alpha", "charlie", "mike", "zulu"]);
    }

    #[test]
    fn workspace_graph_round_trip_with_nodes_and_edges() {
        let mut g = WorkspaceGraph::default();
        let src = WorkspaceNode {
            id: sample_id("pleme-dns"),
            name: "pleme-dns".into(),
            state_key: "pangea/pleme-dns".into(),
            provider: "aws".into(),
            outputs: vec![sample_output("zone_id")],
            inputs: vec![],
        };
        let dst = WorkspaceNode {
            id: sample_id("nix-builders"),
            name: "nix-builders".into(),
            state_key: "pangea/nix-builders".into(),
            provider: "aws".into(),
            outputs: vec![],
            inputs: vec![sample_input("zone_id", "pleme-dns", "zone_id")],
        };
        g.nodes.insert(src.id.clone(), src.clone());
        g.nodes.insert(dst.id.clone(), dst.clone());
        g.edges.push(WorkspaceEdge {
            from: src.id.clone(),
            to: dst.id.clone(),
            output_name: "zone_id".into(),
            input_name: "zone_id".into(),
            field_type: IacType::String,
        });

        let json = serde_json::to_string(&g).unwrap();
        let back: WorkspaceGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(back.nodes.len(), 2);
        assert_eq!(back.edges.len(), 1);
        assert_eq!(back.edges[0].from, src.id);
        assert_eq!(back.edges[0].to, dst.id);
        // Round-trip preserves the BTreeMap semantics — look up by id.
        assert_eq!(back.nodes.get(&src.id), Some(&src));
        assert_eq!(back.nodes.get(&dst.id), Some(&dst));
    }

    #[test]
    fn workspace_graph_empty_vecs_serialize_as_array_not_null() {
        // A workspace with no outputs/inputs should serialize those
        // fields as `[]`, not `null`. Downstream consumers (the
        // Pangea Ruby DSL, Terraform remote_state emitter) index into
        // these arrays positionally.
        let node = WorkspaceNode {
            id: sample_id("orphan"),
            name: "orphan".into(),
            state_key: "pangea/orphan".into(),
            provider: "aws".into(),
            outputs: vec![],
            inputs: vec![],
        };
        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("\"outputs\":[]"));
        assert!(json.contains("\"inputs\":[]"));
    }

    #[test]
    fn output_ports_with_same_name_different_resource_are_distinct() {
        // Two outputs named "id" from different resources must stay
        // distinguishable — their source_resource participates in
        // equality. Guards against a future refactor that treats
        // outputs as unique by name alone.
        let a = OutputPort {
            name: "id".into(),
            field_type: IacType::String,
            source_resource: "aws_vpc.main.id".into(),
        };
        let b = OutputPort {
            name: "id".into(),
            field_type: IacType::String,
            source_resource: "aws_subnet.main.id".into(),
        };
        assert_ne!(a, b);
    }
}
