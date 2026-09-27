# koan

Helm chart for koan as a headless server: GraphQL and Subsonic over a media
library, plus MCP over HTTP for a gateway. See
[docs/guide/headless-server.md](../../docs/guide/headless-server.md) for what
the server does; this chart only deploys it.

## Install

```bash
helm install koan oci://ghcr.io/radiosilence/charts/koan --version 0.36.3 \
  --set hostname=koan.example.com \
  --set library.hostPath=/mnt/music
```

The chart's version tracks koan's own release version; `appVersion` is
always the same value, and CI refuses a release where they drift.

No user exists until one is created. After the Deployment is up:

```bash
kubectl exec -it deploy/koan -- koan auth setup
```

## Values

| Key | Default | Description |
|-----|---------|-------------|
| `hostname` | `""` | Required. Fed to `allowed_hosts` and the share-link `public_url`. |
| `image.repository` | `ghcr.io/radiosilence/koan` | Container image. |
| `image.tag` | `""` | Defaults to `v{{ .Chart.AppVersion }}`. |
| `image.pullPolicy` | `IfNotPresent` | |
| `service.name` | full name | The API Service's name, for routing that already expects one. |
| `api.port` | `4000` | GraphQL/Subsonic port. |
| `mcp.enabled` | `true` | Serve MCP over HTTP. |
| `mcp.port` | `8081` | MCP port. Only a trusted gateway should ever reach it. |
| `mcp.serviceName` | `<full name>-mcp` | The MCP Service's name. |
| `library.hostPath` | `""` | Host path to the music library, mounted read-only at `/music`. |
| `library.existingClaim` | `""` | PVC name to use instead of a host path. |
| `state.hostPath` | `""` | Host path for config, index and auth keys, mounted at `/config`. |
| `state.existingClaim` | `""` | PVC name to use instead of a host path. |
| `state.*` unset | | Falls back to an `emptyDir`, which does not survive the pod. |
| `initPermissions.enabled` | `true` | Chown/chmod the state directory before koan starts. Only applies when `state.hostPath` is set. |
| `initPermissions.image` | `alpine:3.21` | Image for that init container. |
| `nodeSelector` | `{}` | Pin the pod to a node -- required in practice when using a hostPath, since the Deployment uses the `Recreate` strategy. |
| `resources` | see `values.yaml` | Sized for a Subsonic client's full sync and a scan at once. |
| `ingress.enabled` | `false` | koan creates no Ingress of its own; enable this only where nothing else routes to the Service. |
| `ingress.className` | `""` | |
| `ingress.annotations` | `{}` | |
| `ingress.hosts` | `[]` | Defaults to `[hostname]`. |
| `ingress.tls` | `[]` | |
| `networkPolicy.enabled` | `true` | Restrict ingress to the API/MCP ports and egress to DNS plus the public internet. |
| `networkPolicy.api.from` | Traefik pod selector | Peers allowed to reach `api.port`. |
| `networkPolicy.mcp.from` | `app: mcp-gateway` pod selector | Peers allowed to reach `mcp.port`. |
| `networkPolicy.extraIngress` | `[]` | Extra ingress rules, e.g. a node CIDR the CNI needs admitted for kubelet's probes. |
| `networkPolicy.privateCidrs` | RFC1918 + link-local + CGNAT | Ranges egress may not reach even at `0.0.0.0/0`. |
