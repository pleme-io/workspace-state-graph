//! Real pleme-io infrastructure topology.
//!
//! Defines the actual workspace dependency DAG for the pleme-io platform.
//! Each workspace represents a Terraform state boundary with typed
//! inputs and outputs.

use crate::builder::WorkspaceGraphBuilder;
use crate::types::WorkspaceGraph;
use iac_forge::ir::IacType;

/// Build the complete pleme-io infrastructure graph.
///
/// This represents the real dependency topology:
/// ```text
/// state-backend (no deps)
///      |
/// seph-vpc (reads state-backend outputs)
///      |
/// pleme-dns (independent -- Route53 + Porkbun)
///      |  |
/// seph-cluster (reads seph-vpc + pleme-dns outputs)
///      |
/// akeyless-dev-config (reads seph-cluster outputs)
/// ```
#[must_use]
pub fn pleme_infrastructure_graph() -> WorkspaceGraph {
    WorkspaceGraphBuilder::new()
        // -- State Backend (root -- no dependencies) --
        .workspace(
            "state-backend",
            "State Backend",
            "pangea/state-backend",
            "aws",
        )
        .output(
            "state-backend",
            "bucket_name",
            IacType::String,
            "aws_s3_bucket.state.bucket",
        )
        .output(
            "state-backend",
            "dynamodb_table",
            IacType::String,
            "aws_dynamodb_table.lock.name",
        )
        .output(
            "state-backend",
            "bucket_arn",
            IacType::String,
            "aws_s3_bucket.state.arn",
        )
        // -- pleme-dns (independent -- Route53 + Porkbun) --
        .workspace("pleme-dns", "DNS Infrastructure", "pangea/pleme-dns", "aws")
        .output(
            "pleme-dns",
            "zone_id",
            IacType::String,
            "aws_route53_zone.zone.zone_id",
        )
        .output(
            "pleme-dns",
            "domain",
            IacType::String,
            "output.domain",
        )
        .output(
            "pleme-dns",
            "nameservers",
            IacType::List(Box::new(IacType::String)),
            "aws_route53_zone.zone.name_servers",
        )
        // -- seph-vpc (VPC infrastructure) --
        .workspace("seph-vpc", "Seph VPC", "pangea/seph-vpc", "aws")
        .output(
            "seph-vpc",
            "vpc_id",
            IacType::String,
            "aws_vpc.seph.id",
        )
        .output(
            "seph-vpc",
            "public_subnet_ids",
            IacType::List(Box::new(IacType::String)),
            "aws_subnet.public.*.id",
        )
        .output(
            "seph-vpc",
            "private_subnet_ids",
            IacType::List(Box::new(IacType::String)),
            "aws_subnet.private.*.id",
        )
        .output(
            "seph-vpc",
            "security_group_id",
            IacType::String,
            "aws_security_group.default.id",
        )
        // -- seph-cluster (K3s cluster -- depends on VPC + DNS) --
        .workspace(
            "seph-cluster",
            "Seph K3s Cluster",
            "pangea/seph-cluster",
            "aws",
        )
        .input(
            "seph-cluster",
            "vpc_id",
            IacType::String,
            "seph-vpc",
            "vpc_id",
        )
        .input(
            "seph-cluster",
            "subnet_ids",
            IacType::List(Box::new(IacType::String)),
            "seph-vpc",
            "private_subnet_ids",
        )
        .input(
            "seph-cluster",
            "dns_zone_id",
            IacType::String,
            "pleme-dns",
            "zone_id",
        )
        .output(
            "seph-cluster",
            "cluster_endpoint",
            IacType::String,
            "aws_lb.api.dns_name",
        )
        .output(
            "seph-cluster",
            "vpn_endpoint",
            IacType::String,
            "aws_lb.vpn.dns_name",
        )
        .output(
            "seph-cluster",
            "cluster_ca",
            IacType::String,
            "tls_self_signed_cert.ca.cert_pem",
        )
        // -- akeyless-dev-config (Akeyless secrets -- depends on cluster) --
        .workspace(
            "akeyless-dev-config",
            "Akeyless Dev Config",
            "pangea/akeyless-dev-config",
            "akeyless",
        )
        .input(
            "akeyless-dev-config",
            "cluster_endpoint",
            IacType::String,
            "seph-cluster",
            "cluster_endpoint",
        )
        .output(
            "akeyless-dev-config",
            "gateway_url",
            IacType::String,
            "akeyless_gateway.dev.hostname",
        )
        .build()
}

/// Build a minimal graph for testing (just VPC + DNS + cluster).
#[must_use]
pub fn minimal_graph() -> WorkspaceGraph {
    WorkspaceGraphBuilder::new()
        .workspace("vpc", "VPC", "pangea/vpc", "aws")
        .output("vpc", "vpc_id", IacType::String, "aws_vpc.main.id")
        .workspace("dns", "DNS", "pangea/dns", "aws")
        .output(
            "dns",
            "zone_id",
            IacType::String,
            "aws_route53_zone.main.zone_id",
        )
        .workspace("cluster", "Cluster", "pangea/cluster", "aws")
        .input("cluster", "vpc_id", IacType::String, "vpc", "vpc_id")
        .input("cluster", "zone_id", IacType::String, "dns", "zone_id")
        .output(
            "cluster",
            "endpoint",
            IacType::String,
            "aws_lb.api.dns_name",
        )
        .build()
}
