use k8s_openapi::api::core::v1::{Service, ServicePort};
use kube::ResourceExt;

use crate::crd::RatholeConfiguration;
use crate::error::ServiceError;

/// The loadBalancerClass this controller claims.
pub const LB_CLASS: &str = "tatupesonen.rathole/tunnel";
/// Optional annotation selecting which RatholeConfiguration backs the Service.
pub const ANNOT_CONFIG: &str = "tatupesonen.rathole/configuration";
/// RatholeConfiguration name used when the annotation is absent.
pub const DEFAULT_CONFIG: &str = "default";

/// One forwarded service port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exposed {
    /// rathole service name — the `[*.services.<name>]` key, shared by client and server.
    pub name: String,
    /// `tcp` or `udp`.
    pub proto: String,
    /// Public port bound on the VPS (defaults to the Service port).
    pub public_port: i32,
    /// In-cluster target, e.g. `svc.ns.svc.cluster.local:443`.
    pub local_addr: String,
    /// Owning Service, for status + conflict reporting.
    pub svc_namespace: String,
    pub svc_name: String,
}

impl Exposed {
    pub fn svc_ref(&self) -> String {
        format!("{}/{}", self.svc_namespace, self.svc_name)
    }
}

/// The config name a Service targets, if it is one of ours (LoadBalancer with
/// our class). Returns `None` for Services we don't manage.
pub fn config_for_service(svc: &Service) -> Option<String> {
    let spec = svc.spec.as_ref()?;
    if spec.type_.as_deref() != Some("LoadBalancer") {
        return None;
    }
    if spec.load_balancer_class.as_deref() != Some(LB_CLASS) {
        return None;
    }
    Some(
        svc.annotations()
            .get(ANNOT_CONFIG)
            .cloned()
            .unwrap_or_else(|| DEFAULT_CONFIG.to_string()),
    )
}

/// Does this Service want to be exposed by the named config?
pub fn service_claimed_by(svc: &Service, config_name: &str) -> bool {
    config_for_service(svc).as_deref() == Some(config_name)
}

/// Build one [`Exposed`] per Service port (each result independent so a single
/// bad port doesn't sink the others).
pub fn build_exposed(svc: &Service) -> Vec<Result<Exposed, ServiceError>> {
    let ns = svc.namespace().unwrap_or_default();
    let svc_name = svc.name_any();
    let ports: Vec<ServicePort> = svc
        .spec
        .as_ref()
        .and_then(|s| s.ports.clone())
        .unwrap_or_default();

    if ports.is_empty() {
        return vec![Err(ServiceError {
            namespace: ns.clone(),
            name: svc_name.clone(),
            reason: "service has no ports".into(),
        })];
    }

    ports
        .into_iter()
        .map(|p| build_one(&ns, &svc_name, &p))
        .collect()
}

fn build_one(ns: &str, svc_name: &str, p: &ServicePort) -> Result<Exposed, ServiceError> {
    let err = |reason: String| ServiceError {
        namespace: ns.to_string(),
        name: svc_name.to_string(),
        reason,
    };

    let proto = match p.protocol.as_deref() {
        Some("UDP") => "udp",
        Some("TCP") | None => "tcp",
        Some(other) => {
            return Err(err(format!(
                "unsupported protocol {other:?} (tcp/udp only)"
            )))
        }
    }
    .to_string();

    let name = format!("{ns}-{svc_name}-{}", p.port);
    let local_addr = format!("{svc_name}.{ns}.svc.cluster.local:{}", p.port);

    Ok(Exposed {
        name,
        proto,
        public_port: p.port,
        local_addr,
        svc_namespace: ns.to_string(),
        svc_name: svc_name.to_string(),
    })
}

/// The rathole control port (parsed from `remoteAddr`, default 2333).
pub fn control_port(rc: &RatholeConfiguration) -> u16 {
    rc.spec
        .remote_addr
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .unwrap_or(2333)
}

/// The advertised external address (EXTERNAL-IP). Defaults to the host part of `remoteAddr`.
pub fn external_address(rc: &RatholeConfiguration) -> String {
    if let Some(ext) = &rc.spec.external_address {
        return ext.clone();
    }
    rc.spec
        .remote_addr
        .rsplit_once(':')
        .map(|(host, _)| host.to_string())
        .unwrap_or_else(|| rc.spec.remote_addr.clone())
}

/// Render the rathole **client** `client.toml` (forwards public ports inward).
pub fn render_client_toml(rc: &RatholeConfiguration, token: &str, exposed: &[Exposed]) -> String {
    let mut out = String::new();
    out.push_str("# Managed by rathole-operator — do not edit by hand.\n");
    out.push_str("[client]\n");
    out.push_str(&format!("remote_addr = \"{}\"\n", rc.spec.remote_addr));
    out.push_str(&format!("default_token = \"{token}\"\n"));
    // rathole requires the `services` table to exist even when empty.
    out.push_str("\n[client.services]\n");
    for e in exposed {
        out.push('\n');
        out.push_str(&format!("[client.services.{}]\n", e.name));
        out.push_str(&format!("type = \"{}\"\n", e.proto));
        out.push_str(&format!("local_addr = \"{}\"\n", e.local_addr));
    }
    out
}

/// Render the rathole **server** `server.toml` (opens the public ports).
pub fn render_server_toml(rc: &RatholeConfiguration, token: &str, exposed: &[Exposed]) -> String {
    let mut out = String::new();
    out.push_str("# Managed by rathole-operator — do not edit by hand.\n");
    out.push_str("[server]\n");
    out.push_str(&format!("bind_addr = \"0.0.0.0:{}\"\n", control_port(rc)));
    out.push_str(&format!("default_token = \"{token}\"\n"));
    // rathole requires the `services` table to exist even when empty.
    out.push_str("\n[server.services]\n");
    for e in exposed {
        out.push('\n');
        out.push_str(&format!("[server.services.{}]\n", e.name));
        out.push_str(&format!("type = \"{}\"\n", e.proto));
        out.push_str(&format!("bind_addr = \"0.0.0.0:{}\"\n", e.public_port));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{ServicePort, ServiceSpec};
    use kube::api::ObjectMeta;
    use std::collections::BTreeMap;

    fn lb_svc(
        ns: &str,
        name: &str,
        class: Option<&str>,
        annots: &[(&str, &str)],
        ports: Vec<ServicePort>,
    ) -> Service {
        let annotations: BTreeMap<String, String> = annots
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Service {
            metadata: ObjectMeta {
                name: Some(name.into()),
                namespace: Some(ns.into()),
                annotations: Some(annotations),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                type_: Some("LoadBalancer".into()),
                load_balancer_class: class.map(String::from),
                ports: Some(ports),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn port(name: &str, num: i32, proto: &str) -> ServicePort {
        ServicePort {
            name: Some(name.into()),
            port: num,
            protocol: Some(proto.into()),
            ..Default::default()
        }
    }

    #[test]
    fn claims_only_our_class() {
        let ours = lb_svc(
            "games",
            "mc",
            Some(LB_CLASS),
            &[],
            vec![port("mc", 25565, "TCP")],
        );
        assert_eq!(config_for_service(&ours).as_deref(), Some("default"));
        assert!(service_claimed_by(&ours, "default"));

        let other = lb_svc(
            "games",
            "mc",
            Some("cilium.io/lb"),
            &[],
            vec![port("mc", 25565, "TCP")],
        );
        assert!(config_for_service(&other).is_none());
    }

    #[test]
    fn config_annotation_selects_backend() {
        let s = lb_svc(
            "a",
            "b",
            Some(LB_CLASS),
            &[(ANNOT_CONFIG, "homelab")],
            vec![port("p", 1, "TCP")],
        );
        assert!(service_claimed_by(&s, "homelab"));
        assert!(!service_claimed_by(&s, "default"));
    }

    #[test]
    fn exposes_each_port_with_inferred_proto() {
        let s = lb_svc(
            "games",
            "mc",
            Some(LB_CLASS),
            &[],
            vec![port("mc", 25565, "TCP"), port("voice", 27015, "UDP")],
        );
        let exposed: Vec<_> = build_exposed(&s).into_iter().map(Result::unwrap).collect();
        assert_eq!(exposed[0].name, "games-mc-25565");
        assert_eq!(exposed[0].proto, "tcp");
        assert_eq!(exposed[0].public_port, 25565);
        assert_eq!(exposed[0].local_addr, "mc.games.svc.cluster.local:25565");
        assert_eq!(exposed[1].proto, "udp");
    }

    #[test]
    fn sctp_rejected() {
        let s = lb_svc("a", "b", Some(LB_CLASS), &[], vec![port("p", 1, "SCTP")]);
        assert!(build_exposed(&s)[0].is_err());
    }

    fn rc() -> RatholeConfiguration {
        use crate::crd::{RatholeConfigurationSpec, TokenSource};
        RatholeConfiguration::new(
            "default",
            RatholeConfigurationSpec {
                remote_addr: "vps.example.com:2333".into(),
                default_token: TokenSource {
                    secret_name: "t".into(),
                    secret_namespace: "rathole-system".into(),
                    key: "token".into(),
                },
                external_address: None,
                server_config_push: None,
                image: "img".into(),
                deployment_namespace: "rathole-system".into(),
            },
        )
    }

    #[test]
    fn empty_render_still_has_services_table() {
        // rathole rejects configs missing the `services` table.
        let c = render_client_toml(&rc(), "tok", &[]);
        let s = render_server_toml(&rc(), "tok", &[]);
        assert!(c.contains("[client.services]"), "client:\n{c}");
        assert!(s.contains("[server.services]"), "server:\n{s}");
        assert!(s.contains("bind_addr = \"0.0.0.0:2333\""));
    }

    #[test]
    fn server_render_binds_public_ports() {
        let e = Exposed {
            name: "games-mc-25565".into(),
            proto: "tcp".into(),
            public_port: 25565,
            local_addr: "mc.games.svc.cluster.local:25565".into(),
            svc_namespace: "games".into(),
            svc_name: "mc".into(),
        };
        let s = render_server_toml(&rc(), "tok", std::slice::from_ref(&e));
        assert!(s.contains("[server.services.games-mc-25565]"));
        assert!(s.contains("bind_addr = \"0.0.0.0:25565\""));
        let c = render_client_toml(&rc(), "tok", &[e]);
        assert!(c.contains("local_addr = \"mc.games.svc.cluster.local:25565\""));
    }
}
