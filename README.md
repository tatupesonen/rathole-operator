# rathole-operator

A Kubernetes **LoadBalancer controller** (kube-rs) that exposes Services from a
private/NAT'd cluster via a [rathole](https://github.com/rapiz1/rathole) tunnel
to a VPS, like inlets-operator, for rathole.

Set `type: LoadBalancer` + `loadBalancerClass: tatupesonen.rathole/tunnel`; the operator
opens the port on the VPS, runs the rathole client, and writes the VPS address
into the Service's `EXTERNAL-IP`. TCP and UDP (UDP matters for game servers). A
`RatholeConfiguration` (cluster-scoped) holds the VPS address + shared token;
the operator pushes `server.toml` to a receiver on the VPS, which rathole
hot-reloads, so ports open/close dynamically.

## Deploy

**VPS** (rathole server + receiver + Caddy TLS), via `examples/vps-docker-compose.yml`
(bootstrap/cert/token steps in its comments):

```bash
docker compose -f examples/vps-docker-compose.yml up -d   # opens 2333 + 2334
```

**Cluster** (operator):

```bash
kubectl apply -k deploy/                                   # CRD + RBAC + operator
kubectl -n rathole-system create secret generic rathole-token \
  --from-literal=token="$TOKEN"                            # same token as the VPS
kubectl apply -f examples/ratholeconfiguration.yaml        # edit addrs/url first
```

**Expose a Service:**

```bash
kubectl patch svc myapp -p '{"spec":{"type":"LoadBalancer","loadBalancerClass":"tatupesonen.rathole/tunnel"}}'
kubectl get svc myapp        # EXTERNAL-IP = your VPS; reach it at VPS:port
```

See `examples/loadbalancer-service.yaml` for a multi-port TCP+UDP game example.

## Develop

```bash
cargo test                                    # rendering + class-matching tests
cargo clippy --all-targets -- -D warnings
cargo run --bin crdgen > deploy/crd.yaml      # regenerate CRD
```

CI builds and publishes `ghcr.io/tatupesonen/rathole-operator` on push to `main`.

## Notes

- Single VPS IP, port-multiplexed: public port = Service port; conflicts are
  reported in status, not remapped.
- Backend sees the rathole client pod as source IP (no PROXY protocol yet).
- The tunnel hop is plaintext today (no encryption in transit).

## TODO

- **Encryption in transit (rathole Noise protocol).** Wrap the cluster↔VPS hop
  with rathole's Noise transport (`[transport] type = "noise"` + a keypair on
  both ends), surfaced through the CRD. Today the hop is plaintext.
- **Preserve client source IP (PROXY protocol).** Emit PROXY protocol over the
  tunnel so a backend/gateway sees the real client IP (e.g. an HTTP gateway sets
  `X-Forwarded-For`). Needs rathole PROXY support ([open upstream PR](https://github.com/rapiz1/rathole/issues/250))
  and a PROXY-aware backend; true source IP for arbitrary UDP would instead need
  an L3 (WireGuard) routing path.

## License

Apache-2.0
