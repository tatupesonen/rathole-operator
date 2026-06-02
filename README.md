# rathole-operator

A Kubernetes **LoadBalancer controller** (kube-rs) that gives Services in a
NAT'd / private cluster a public address via a [rathole](https://github.com/rapiz1/rathole)
tunnel to a VPS — like [inlets-operator](https://github.com/inlets/inlets-operator),
for rathole.

Set `type: LoadBalancer` + `loadBalancerClass: rathole.dev/tunnel` on a Service.
The operator opens the matching public port on the VPS, runs the rathole client
that forwards it inward, and writes the VPS address into
`status.loadBalancer.ingress` — so `kubectl get svc` shows a real `EXTERNAL-IP`
(and ExternalDNS works for free).

## How it works

```
RatholeConfiguration "default"           Service (type: LoadBalancer)
  remoteAddr  vps:2333                      loadBalancerClass: rathole.dev/tunnel
  defaultToken -> Secret                    ports: 25565/TCP, 27015/UDP
  externalAddress 203.0.113.10                   │
  serverConfigPush -> https://vps:2334           │
        │                                        ▼
        ▼                ┌──── operator (reconcile) ◀── watch LB Services of our class
   render client.toml ──┤  • allocate public ports (= Service port), detect conflicts
   render server.toml   │  • SSA Secret(client.toml) + Deployment(rathole --client)
        │               │  • POST server.toml ──HTTPS(bearer)──┐
        │               │  • status.loadBalancer.ingress = EXTERNAL-IP
        ▼               └──────────────────────────────────────│───────
   rathole client ── tunnel ──▶ rathole server (VPS)           ▼
                                receiver writes server.toml IN PLACE
                                rathole watches (Modify) → opens/closes port
```

- **`RatholeConfiguration` is cluster-scoped** — the "cloud provider" (which VPS,
  which token). Services pick one by name via the `rathole.dev/configuration`
  annotation, defaulting to the config named `default`.
- **Public port = the Service port.** One VPS IP, so ports must be unique per
  config; conflicts are skipped and reported in status (losing Service stays pending).
- **Dynamic server.** The operator renders `server.toml` and POSTs it to a
  receiver on the VPS, which writes it **in place** (rathole's hot-reload watcher
  reacts to `Modify` events and ignores rename/move). rathole opens/closes ports
  with no restart. Omit `serverConfigPush` to manage only the cluster side.
- Each reconcile rebuilds config from the live Service set, so deleting a Service
  drops its port.

## TCP and UDP

Both work — rathole tunnels TCP streams and UDP datagrams. The protocol is taken
from each Service port (`TCP`/`UDP`), so a single Service can expose both (e.g. a
game on TCP + a query/voice port on UDP). **UDP matters for game servers**
(Source/Steam, Minecraft Bedrock, Wireguard, etc.), which is a primary use case.

Caveat: the backend sees the **rathole client pod** as the source IP, not the
real client (no PROXY-protocol passthrough) — so per-client IP bans/limits at the
game server won't see real addresses.

> **Next: encryption in transit.** The tunnel hop (cluster ↔ VPS) is currently
> plaintext TCP; only already-encrypted payloads (e.g. HTTPS) stay protected
> end-to-end. We'll enable rathole's **Noise protocol** transport
> (`[transport] type = "noise"` with a keypair on both ends, surfaced via the
> CRD) to encrypt the hop itself — important for plaintext game/UDP traffic.

## Quick start

```bash
# 1. CRD + RBAC + operator
kubectl apply -k deploy/

# 2. Backend config + shared token (edit the token + addresses)
kubectl apply -f examples/ratholeconfiguration.yaml

# 3. On the VPS: rathole server + receiver + Caddy(TLS) — see
#    examples/vps-docker-compose.yml (bootstrap, cert, and perms steps in comments)

# 4. Expose a workload
kubectl apply -f examples/loadbalancer-service.yaml

kubectl get rhc                 # EXTERNAL + READY + status
kubectl get svc -A              # EXTERNAL-IP populated on our LoadBalancer services
```

## Components

| Binary | Role |
|---|---|
| `rathole-operator` | the controller (in-cluster) |
| `rathole-config-receiver` | runs on the VPS (nonroot uid 65532); writes `server.toml` in place |
| `crdgen` | `cargo run --bin crdgen > deploy/crd.yaml` |

## Develop

```bash
cargo test                         # rendering + class-matching unit tests
cargo clippy --all-targets -- -D warnings
cargo run --bin rathole-operator   # runs out-of-cluster against your kubecontext
```

CI (`.github/workflows/ci.yml`) runs fmt + clippy + tests, and on pushes to
`main`/tags builds and publishes the image to
`ghcr.io/tatupesonen/rathole-operator`.

## Notes & trade-offs

- **Single shared IP, port-multiplexed** — conflicts are detected, not auto-remapped.
- **Security** — the receiver's bearer token is a real trust boundary; serve it
  over TLS (the example fronts it with Caddy). The shared config dir must be
  writable by the receiver's nonroot uid (65532).
- **HTTP apps** generally shouldn't each be a LoadBalancer — put them behind a
  Gateway/Ingress and make only the gateway's data-plane Service a `rathole.dev/tunnel`
  LoadBalancer.

## License

MIT
