use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Secret, Service};
use kube::api::{DeleteParams, ListParams, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::finalizer::{finalizer, Event as Finalizer};
use kube::runtime::reflector::ObjectRef;
use kube::runtime::watcher;
use kube::{Api, Client, ResourceExt};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::crd::{RatholeConfiguration, ServerConfigPush, TokenSource};
use crate::error::{Error, Result};
use crate::lb::{self, Exposed};
use crate::{push, resources};

const FINALIZER: &str = "tatupesonen.rathole/cleanup";
const FIELD_MANAGER: &str = "rathole-operator";
const REQUEUE: Duration = Duration::from_secs(300);
const RETRY: Duration = Duration::from_secs(60);

pub struct Context {
    pub client: Client,
}

/// Start the controller and run until shutdown.
pub async fn run(client: Client) {
    let configs: Api<RatholeConfiguration> = Api::all(client.clone());
    let services: Api<Service> = Api::all(client.clone());
    let ctx = Arc::new(Context {
        client: client.clone(),
    });

    Controller::new(configs, watcher::Config::default())
        // A LoadBalancer Service of our class re-reconciles its backing config.
        .watches(services, watcher::Config::default(), |svc| {
            lb::config_for_service(&svc).map(|name| ObjectRef::new(&name))
        })
        .shutdown_on_signal()
        .run(reconcile, error_policy, ctx)
        .for_each(|res| async move {
            match res {
                Ok((obj, _)) => info!("reconciled {obj:?}"),
                Err(e) => warn!("reconcile failed: {e}"),
            }
        })
        .await;
}

async fn reconcile(rc: Arc<RatholeConfiguration>, ctx: Arc<Context>) -> Result<Action> {
    let configs: Api<RatholeConfiguration> = Api::all(ctx.client.clone());
    finalizer(&configs, FINALIZER, rc, |event| async {
        match event {
            Finalizer::Apply(rc) => apply(rc, ctx.clone()).await,
            Finalizer::Cleanup(rc) => cleanup(rc, ctx.clone()).await,
        }
    })
    .await
    .map_err(|e| Error::Finalizer(Box::new(e)))
}

fn error_policy(rc: Arc<RatholeConfiguration>, err: &Error, _ctx: Arc<Context>) -> Action {
    warn!(config = %rc.name_any(), error = %err, "requeue after error");
    Action::requeue(Duration::from_secs(30))
}

async fn apply(rc: Arc<RatholeConfiguration>, ctx: Arc<Context>) -> Result<Action> {
    let client = &ctx.client;
    let name = rc.name_any();

    // 1. Discover the LoadBalancer Services this config backs, allocating public
    //    ports and detecting conflicts (single VPS IP → ports must be unique).
    let services: Api<Service> = Api::all(client.clone());
    let all = services.list(&ListParams::default()).await?;
    let mut claimed: Vec<&Service> = all
        .iter()
        .filter(|s| lb::service_claimed_by(s, &name))
        .collect();
    claimed.sort_by_key(|s| (s.namespace().unwrap_or_default(), s.name_any()));

    let mut exposed: Vec<Exposed> = Vec::new();
    let mut per_service: BTreeMap<(String, String), Vec<Exposed>> = BTreeMap::new();
    let mut used_ports: BTreeMap<i32, String> = BTreeMap::new();
    let mut skipped: Vec<String> = Vec::new();

    for svc in &claimed {
        for result in lb::build_exposed(svc) {
            match result {
                Ok(e) => match used_ports.get(&e.public_port) {
                    Some(owner) if owner != &e.svc_ref() => {
                        skipped.push(format!(
                            "{}: public port {} already claimed by {}",
                            e.svc_ref(),
                            e.public_port,
                            owner
                        ));
                    }
                    _ => {
                        used_ports.insert(e.public_port, e.svc_ref());
                        per_service
                            .entry((e.svc_namespace.clone(), e.svc_name.clone()))
                            .or_default()
                            .push(e.clone());
                        exposed.push(e);
                    }
                },
                Err(se) => {
                    warn!(%se, "skipping service port");
                    skipped.push(se.to_string());
                }
            }
        }
    }
    exposed.sort_by(|a, b| a.name.cmp(&b.name));

    // 2. Render + apply the client side.
    let token = read_token(client, &rc.spec.default_token).await?;
    let client_toml = lb::render_client_toml(&rc, &token, &exposed);
    let config_hash = short_hash(&client_toml);

    let uid = rc.uid().ok_or(Error::MissingField("metadata.uid"))?;
    let owner = resources::owner_ref(&rc, &uid);
    let pp = PatchParams::apply(FIELD_MANAGER).force();
    let ns = &rc.spec.deployment_namespace;

    Api::<Secret>::namespaced(client.clone(), ns)
        .patch(
            &resources::managed_name(&rc),
            &pp,
            &Patch::Apply(resources::secret(&rc, &client_toml, &owner)),
        )
        .await?;
    Api::<Deployment>::namespaced(client.clone(), ns)
        .patch(
            &resources::managed_name(&rc),
            &pp,
            &Patch::Apply(resources::deployment(&rc, &config_hash, &owner)),
        )
        .await?;

    // 3. Push the server config to the VPS (if enabled). If it fails, the public
    //    ports aren't open, so don't advertise EXTERNAL-IP yet — retry sooner.
    if let Some(pushcfg) = &rc.spec.server_config_push {
        let server_toml = lb::render_server_toml(&rc, &token, &exposed);
        let push_token = match &pushcfg.token {
            Some(src) => read_token(client, src).await?,
            None => token.clone(),
        };
        if let Err(e) = do_push(pushcfg, &push_token, server_toml).await {
            warn!(config = %name, error = %e, "server config push failed");
            patch_status(
                client,
                &name,
                false,
                &lb::external_address(&rc),
                &exposed,
                rc.metadata.generation,
                Some(format!("server config push failed: {e}")),
            )
            .await?;
            return Ok(Action::requeue(RETRY));
        }
    }

    // 4. Write EXTERNAL-IP into each backed Service, then update config status.
    let external = lb::external_address(&rc);
    for ((svc_ns, svc_name), ports) in &per_service {
        if let Err(e) = patch_service_ingress(client, svc_ns, svc_name, &external, ports).await {
            warn!(service = %format!("{svc_ns}/{svc_name}"), error = %e, "failed to set ingress status");
        }
    }

    let message = if skipped.is_empty() {
        format!("{} port(s) exposed", exposed.len())
    } else {
        format!(
            "{} exposed, {} skipped: {}",
            exposed.len(),
            skipped.len(),
            skipped.join("; ")
        )
    };
    patch_status(
        client,
        &name,
        true,
        &external,
        &exposed,
        rc.metadata.generation,
        Some(message),
    )
    .await?;

    info!(config = %name, exposed = exposed.len(), skipped = skipped.len(), "applied");
    Ok(Action::requeue(REQUEUE))
}

async fn cleanup(rc: Arc<RatholeConfiguration>, ctx: Arc<Context>) -> Result<Action> {
    // OwnerReferences also GC these, but delete explicitly for promptness.
    let ns = &rc.spec.deployment_namespace;
    let name = resources::managed_name(&rc);
    let dp = DeleteParams::default();

    ignore_not_found(
        Api::<Deployment>::namespaced(ctx.client.clone(), ns)
            .delete(&name, &dp)
            .await,
    )?;
    ignore_not_found(
        Api::<Secret>::namespaced(ctx.client.clone(), ns)
            .delete(&name, &dp)
            .await,
    )?;

    // Best-effort: clear the server config on the VPS by pushing an empty service set.
    if let Some(pushcfg) = &rc.spec.server_config_push {
        if let Ok(token) = read_token(&ctx.client, &rc.spec.default_token).await {
            let push_token = match &pushcfg.token {
                Some(src) => read_token(&ctx.client, src)
                    .await
                    .unwrap_or_else(|_| token.clone()),
                None => token.clone(),
            };
            let empty = lb::render_server_toml(&rc, &push_token, &[]);
            let _ = do_push(pushcfg, &push_token, empty).await;
        }
    }

    info!(config = %rc.name_any(), "cleaned up managed resources");
    Ok(Action::await_change())
}

async fn do_push(cfg: &ServerConfigPush, token: &str, body: String) -> Result<()> {
    push::push_server_config(&cfg.url, token, body, cfg.insecure_skip_verify).await
}

async fn read_token(client: &Client, src: &TokenSource) -> Result<String> {
    let secrets: Api<Secret> = Api::namespaced(client.clone(), &src.secret_namespace);
    let secret = secrets.get(&src.secret_name).await?;
    let bytes = secret
        .data
        .as_ref()
        .and_then(|d| d.get(&src.key))
        .ok_or_else(|| Error::TokenUnavailable(src.secret_name.clone(), src.key.clone()))?;
    String::from_utf8(bytes.0.clone()).map_err(|_| Error::TokenNotUtf8)
}

/// Patch `status.loadBalancer.ingress` on a backed Service (the EXTERNAL-IP).
async fn patch_service_ingress(
    client: &Client,
    ns: &str,
    name: &str,
    external: &str,
    ports: &[Exposed],
) -> Result<()> {
    let port_status: Vec<Value> = ports
        .iter()
        .map(|e| {
            json!({
                "port": e.public_port,
                "protocol": if e.proto == "udp" { "UDP" } else { "TCP" },
            })
        })
        .collect();

    let mut ingress = serde_json::Map::new();
    if external.parse::<std::net::IpAddr>().is_ok() {
        ingress.insert("ip".into(), json!(external));
    } else {
        ingress.insert("hostname".into(), json!(external));
    }
    ingress.insert("ports".into(), json!(port_status));

    let status = json!({ "status": { "loadBalancer": { "ingress": [Value::Object(ingress)] } } });
    Api::<Service>::namespaced(client.clone(), ns)
        .patch_status(name, &PatchParams::default(), &Patch::Merge(&status))
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn patch_status(
    client: &Client,
    name: &str,
    ready: bool,
    external: &str,
    exposed: &[Exposed],
    generation: Option<i64>,
    message: Option<String>,
) -> Result<()> {
    let names: Vec<&str> = exposed.iter().map(|e| e.name.as_str()).collect();
    let status = json!({
        "status": {
            "ready": ready,
            "externalAddress": external,
            "exposedServices": names,
            "observedGeneration": generation,
            "message": message,
        }
    });
    Api::<RatholeConfiguration>::all(client.clone())
        .patch_status(name, &PatchParams::default(), &Patch::Merge(&status))
        .await?;
    Ok(())
}

fn ignore_not_found<T>(res: std::result::Result<T, kube::Error>) -> Result<()> {
    match res {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
        Err(e) => Err(Error::Kube(e)),
    }
}

fn short_hash(s: &str) -> String {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}
