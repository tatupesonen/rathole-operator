use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// API group for the operator's CRDs.
pub const GROUP: &str = "rathole.dev";
pub const VERSION: &str = "v1alpha1";
pub const KIND: &str = "RatholeConfiguration";

/// `RatholeConfiguration` defines a tunnel backend: a rathole server on a public
/// VPS that this cluster dials out to. It is the "cloud provider" for our
/// LoadBalancer implementation — Services of `type: LoadBalancer` with
/// `loadBalancerClass: rathole.dev/tunnel` are exposed through it.
///
/// Cluster-scoped: Services in any namespace bind to it (by name via the
/// `rathole.dev/configuration` annotation, or the `default`-named config).
#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "rathole.dev",
    version = "v1alpha1",
    kind = "RatholeConfiguration",
    plural = "ratholeconfigurations",
    shortname = "rhc",
    status = "RatholeConfigurationStatus",
    printcolumn = r#"{"name":"External","type":"string","jsonPath":".status.externalAddress"}"#,
    printcolumn = r#"{"name":"Ready","type":"boolean","jsonPath":".status.ready"}"#,
    printcolumn = r#"{"name":"Status","type":"string","jsonPath":".status.message"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct RatholeConfigurationSpec {
    /// rathole server control-channel address the cluster dials out to,
    /// e.g. `vps.example.com:2333`.
    pub remote_addr: String,

    /// Shared `default_token`, read from a Secret. Set the SAME value as
    /// `default_token` on the rathole server (the operator pushes it there if
    /// `serverConfigPush` is configured).
    pub default_token: TokenSource,

    /// Public address advertised back into each Service's
    /// `status.loadBalancer.ingress` (the EXTERNAL-IP). Defaults to the host
    /// part of `remoteAddr`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_address: Option<String>,

    /// If set, the operator renders `server.toml` and pushes it to a receiver
    /// on the VPS so the rathole server opens/closes ports dynamically. If
    /// omitted, only the cluster (client) side is managed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_config_push: Option<ServerConfigPush>,

    /// Image for the managed rathole client Deployment.
    #[serde(default = "default_image")]
    pub image: String,

    /// Namespace the managed Secret + Deployment are created in.
    #[serde(default = "default_deployment_namespace")]
    pub deployment_namespace: String,
}

/// Reference to a key in a Secret.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenSource {
    pub secret_name: String,
    #[serde(default = "default_deployment_namespace")]
    pub secret_namespace: String,
    #[serde(default = "default_token_key")]
    pub key: String,
}

/// Push the rendered `server.toml` to a receiver sidecar on the VPS.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigPush {
    /// Receiver endpoint, e.g. `https://vps.example.com:2334/config`.
    pub url: String,
    /// Bearer token for the receiver. Defaults to `defaultToken`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<TokenSource>,
    /// Accept a self-signed TLS cert on the receiver endpoint.
    #[serde(default)]
    pub insecure_skip_verify: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RatholeConfigurationStatus {
    /// True once the client Deployment is applied (and server config pushed, if enabled).
    #[serde(default)]
    pub ready: bool,
    /// The advertised external address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_address: Option<String>,
    /// rathole service names currently exposed.
    #[serde(default)]
    pub exposed_services: Vec<String>,
    /// Generation last reconciled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    /// Human-readable status / warnings (skipped Services, port conflicts, push errors).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn default_image() -> String {
    "docker.io/rapiz1/rathole:v0.5.0".to_string()
}

fn default_deployment_namespace() -> String {
    "rathole-system".to_string()
}

fn default_token_key() -> String {
    "token".to_string()
}
