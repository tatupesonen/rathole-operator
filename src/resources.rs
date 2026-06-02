use serde_json::{json, Value};

use kube::ResourceExt;

use crate::crd::RatholeConfiguration;

/// Name of the managed Secret + Deployment for a given config.
pub fn managed_name(rc: &RatholeConfiguration) -> String {
    format!("rathole-{}", rc.name_any())
}

fn labels(rc: &RatholeConfiguration) -> Value {
    json!({
        "app.kubernetes.io/name": "rathole-client",
        "app.kubernetes.io/instance": rc.name_any(),
        "app.kubernetes.io/managed-by": "rathole-operator",
    })
}

/// OwnerReference pointing at the (cluster-scoped) RatholeConfiguration, so the
/// managed Secret/Deployment are garbage-collected when the config is deleted.
pub fn owner_ref(rc: &RatholeConfiguration, uid: &str) -> Value {
    json!({
        "apiVersion": format!("{}/{}", crate::crd::GROUP, crate::crd::VERSION),
        "kind": crate::crd::KIND,
        "name": rc.name_any(),
        "uid": uid,
        "controller": true,
        "blockOwnerDeletion": true,
    })
}

/// The Secret holding the rendered `client.toml`.
pub fn secret(rc: &RatholeConfiguration, client_toml: &str, owner: &Value) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": managed_name(rc),
            "namespace": rc.spec.deployment_namespace,
            "labels": labels(rc),
            "ownerReferences": [owner],
        },
        "type": "Opaque",
        "stringData": { "client.toml": client_toml },
    })
}

/// The single-replica rathole client Deployment. `config_hash` lives in the pod
/// template so a config change rolls the pod (rathole has no live reload).
pub fn deployment(rc: &RatholeConfiguration, config_hash: &str, owner: &Value) -> Value {
    let name = managed_name(rc);
    let selector = json!({
        "app.kubernetes.io/name": "rathole-client",
        "app.kubernetes.io/instance": rc.name_any(),
    });
    json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {
            "name": name,
            "namespace": rc.spec.deployment_namespace,
            "labels": labels(rc),
            "ownerReferences": [owner],
        },
        "spec": {
            // Exactly one replica — multiple clients would fight over the tunnel.
            "replicas": 1,
            "strategy": { "type": "Recreate" },
            "selector": { "matchLabels": selector },
            "template": {
                "metadata": {
                    "labels": labels(rc),
                    "annotations": { "rathole.dev/config-hash": config_hash },
                },
                "spec": {
                    "securityContext": {
                        "runAsNonRoot": true,
                        "runAsUser": 65532,
                        "runAsGroup": 65532,
                    },
                    "containers": [{
                        "name": "rathole",
                        "image": rc.spec.image,
                        "args": ["--client", "/config/client.toml"],
                        "volumeMounts": [{
                            "name": "config",
                            "mountPath": "/config",
                            "readOnly": true,
                        }],
                        "resources": {
                            "requests": { "cpu": "25m", "memory": "32Mi" },
                            "limits": { "memory": "64Mi" },
                        },
                        "securityContext": {
                            "allowPrivilegeEscalation": false,
                            "readOnlyRootFilesystem": true,
                            "capabilities": { "drop": ["ALL"] },
                        },
                    }],
                    "volumes": [{
                        "name": "config",
                        "secret": {
                            "secretName": name,
                            "items": [{ "key": "client.toml", "path": "client.toml" }],
                        },
                    }],
                },
            },
        },
    })
}
