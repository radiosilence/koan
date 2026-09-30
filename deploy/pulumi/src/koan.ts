import * as k8s from "@pulumi/kubernetes";
import * as pulumi from "@pulumi/pulumi";
import type * as z from "zod";
import type { Deployed } from "./contract.ts";
import { KoanConfSchema } from "./koan.schemas.ts";
import { APP_VERSION } from "./versions.ts";

const NAME = "koan";
const STATE_MOUNT_PATH = "/config";

const labels = () => ({
  ...selectorLabels(),
  "app.kubernetes.io/version": APP_VERSION,
  "app.kubernetes.io/managed-by": "pulumi",
});

const selectorLabels = () => ({
  "app.kubernetes.io/name": NAME,
  "app.kubernetes.io/instance": NAME,
});

/**
 * koan: a headless music server — GraphQL and Subsonic over a media library,
 * plus MCP over HTTP for a gateway.
 *
 * No Ingress: the deployer routes to the API Service, so this creates only
 * the Deployment, its Services and its NetworkPolicy.
 */
export function createKoan(
  provider: k8s.Provider,
  namespace: pulumi.Input<string>,
  confArgs: z.input<typeof KoanConfSchema>,
) {
  const conf = KoanConfSchema.parse(confArgs);
  const options = { provider };
  const image = `${conf.image.repository}:${conf.image.tag || `v${APP_VERSION}`}`;
  const apiServiceName = conf.service.name || NAME;
  const mcpServiceName = conf.mcp.serviceName || `${NAME}-mcp`;

  // What every pod here shares: the node, the user, and the state directory
  // made writable by that user.
  const podBase = {
    ...(Object.keys(conf.nodeSelector).length > 0 && { nodeSelector: conf.nodeSelector }),
    // The image runs as uid 1000; the library is readable through its
    // group or other bits, but the state directory is made 1000's below.
    securityContext: {
      runAsUser: 1000,
      runAsGroup: 1000,
      runAsNonRoot: true,
      seccompProfile: { type: "RuntimeDefault" },
    },
    // kubelet creates a missing hostPath as root, and koan cannot
    // write what root owns. Only the state directory is mounted here.
    ...(conf.initPermissions.enabled && conf.state.hostPath
      ? {
          initContainers: [
            {
              name: "state",
              image: conf.initPermissions.image,
              command: ["sh", "-c", `chown 1000:1000 ${STATE_MOUNT_PATH} && chmod 700 ${STATE_MOUNT_PATH}`],
              securityContext: {
                runAsUser: 0,
                runAsNonRoot: false,
                allowPrivilegeEscalation: false,
                capabilities: { drop: ["ALL"], add: ["CHOWN", "FOWNER"] },
              },
              resources: { requests: { cpu: "5m", memory: "8Mi" }, limits: { memory: "32Mi" } },
              volumeMounts: [{ name: "state", mountPath: STATE_MOUNT_PATH }],
            },
          ],
        }
      : {}),
  };
  const stateVolume = {
    name: "state",
    ...(conf.state.existingClaim
      ? { persistentVolumeClaim: { claimName: conf.state.existingClaim } }
      : conf.state.hostPath
        ? { hostPath: { path: conf.state.hostPath, type: "DirectoryOrCreate" } }
        : { emptyDir: {} }),
  };

  // The new version opens a snapshot of the live database, migrations and
  // all, before the Deployment is touched. The Deployment is Recreate, so a
  // migration that fails on the real library would otherwise take the server
  // down with nothing to fall back to; failing here leaves the running
  // version serving. Skipped when state does not persist, as there is nothing
  // to migrate.
  // Not the server's selector labels: the Service would route to this pod.
  const checkLabels = {
    "app.kubernetes.io/name": `${NAME}-check-db`,
    "app.kubernetes.io/instance": NAME,
    "app.kubernetes.io/version": APP_VERSION,
    "app.kubernetes.io/managed-by": "pulumi",
  };
  const check =
    conf.state.existingClaim || conf.state.hostPath
      ? new k8s.batch.v1.Job(
          `${NAME}-check-db`,
          {
            metadata: {
              namespace,
              labels: checkLabels,
              annotations: { "pulumi.com/replaceUnready": "true" },
            },
            spec: {
              backoffLimit: 0,
              template: {
                metadata: { labels: checkLabels },
                spec: {
                  ...podBase,
                  restartPolicy: "Never",
                  containers: [
                    {
                      name: "check-db",
                      image,
                      imagePullPolicy: conf.image.pullPolicy,
                      // The image's entrypoint starts the server.
                      command: ["koan", "check-db"],
                      resources: { requests: { cpu: "100m", memory: "128Mi" }, limits: conf.resources.limits },
                      securityContext: {
                        allowPrivilegeEscalation: false,
                        readOnlyRootFilesystem: true,
                        capabilities: { drop: ["ALL"] },
                      },
                      volumeMounts: [
                        { name: "state", mountPath: STATE_MOUNT_PATH },
                        // The snapshot is written here.
                        { name: "tmp", mountPath: "/tmp" },
                      ],
                    },
                  ],
                  volumes: [stateVolume, { name: "tmp", emptyDir: {} }],
                },
              },
            },
          },
          options,
        )
      : undefined;

  const deployment = new k8s.apps.v1.Deployment(
    NAME,
    {
      metadata: { name: NAME, namespace, labels: labels() },
      spec: {
        replicas: 1,
        // Host paths and one index: two pods would share neither safely.
        strategy: { type: "Recreate" },
        selector: { matchLabels: selectorLabels() },
        template: {
          metadata: { labels: selectorLabels() },
          spec: {
            ...podBase,
            containers: [
              {
                name: NAME,
                image,
                imagePullPolicy: conf.image.pullPolicy,
                args: ["--port", String(conf.api.port)],
                env: [
                  ...(conf.mcp.enabled
                    ? [
                        { name: "KOAN_MCP_BIND", value: `0.0.0.0:${conf.mcp.port}` },
                        // The gateway forwards each user's koan account; a
                        // request without one is refused rather than run at a
                        // default role.
                        { name: "KOAN_MCP_REQUIRE_LOGIN", value: "1" },
                      ]
                    : []),
                  { name: "KOAN_LIBRARY__FOLDERS", value: '["/music"]' },
                  // koan refuses a Host it was not told about.
                  { name: "KOAN_GRAPHQL__ALLOWED_HOSTS", value: `["${conf.hostname}"]` },
                  // Served over HTTPS in front of the Service.
                  { name: "KOAN_GRAPHQL__COOKIE_SECURE", value: "true" },
                  // Share links are built on the address strangers reach.
                  { name: "KOAN_SHARING__PUBLIC_URL", value: `https://${conf.hostname}` },
                  ...(conf.push.existingSecret
                    ? [
                        {
                          name: "KOAN_PUSH__KEY",
                          valueFrom: {
                            secretKeyRef: { name: conf.push.existingSecret, key: "apns-key" },
                          },
                        },
                        { name: "KOAN_PUSH__KEY_ID", value: conf.push.keyId },
                        { name: "KOAN_PUSH__TEAM_ID", value: conf.push.teamId },
                      ]
                    : []),
                ],
                ports: [
                  { name: "api", containerPort: conf.api.port },
                  ...(conf.mcp.enabled ? [{ name: "mcp", containerPort: conf.mcp.port }] : []),
                ],
                readinessProbe: { tcpSocket: { port: "api" }, periodSeconds: 10 },
                livenessProbe: { tcpSocket: { port: "api" }, initialDelaySeconds: 30, periodSeconds: 30 },
                resources: { limits: conf.resources.limits, requests: conf.resources.requests },
                securityContext: {
                  allowPrivilegeEscalation: false,
                  readOnlyRootFilesystem: true,
                  capabilities: { drop: ["ALL"] },
                },
                volumeMounts: [
                  { name: "music", mountPath: "/music", readOnly: true },
                  { name: "state", mountPath: STATE_MOUNT_PATH },
                  // Caches (artwork, lyrics) go under $HOME; they are
                  // rebuilt on demand, so they need not outlive the pod.
                  { name: "home", mountPath: "/home/koan" },
                  { name: "tmp", mountPath: "/tmp" },
                ],
              },
            ],
            volumes: [
              {
                name: "music",
                ...(conf.library.existingClaim
                  ? { persistentVolumeClaim: { claimName: conf.library.existingClaim } }
                  // The schema's refine guarantees one of the two is set.
                  : { hostPath: { path: conf.library.hostPath!, type: "Directory" } }),
              },
              stateVolume,
              { name: "home", emptyDir: {} },
              { name: "tmp", emptyDir: {} },
            ],
          },
        },
      },
    },
    { ...options, dependsOn: check ? [check] : [] },
  );

  const apiService = new k8s.core.v1.Service(
    apiServiceName,
    {
      metadata: { name: apiServiceName, namespace, labels: labels() },
      spec: { selector: selectorLabels(), ports: [{ port: 80, targetPort: "api" }] },
    },
    options,
  );

  const mcpService = conf.mcp.enabled
    ? new k8s.core.v1.Service(
        mcpServiceName,
        {
          metadata: { name: mcpServiceName, namespace, labels: labels() },
          spec: { selector: selectorLabels(), ports: [{ port: conf.mcp.port, targetPort: "mcp" }] },
        },
        options,
      )
    : undefined;

  if (conf.networkPolicy.enabled) {
    new k8s.networking.v1.NetworkPolicy(
      "koan-netpol",
      {
        metadata: { name: NAME, namespace, labels: labels() },
        spec: {
          podSelector: { matchLabels: selectorLabels() },
          policyTypes: ["Ingress", "Egress"],
          ingress: [
            // GraphQL and Subsonic, through whatever fronts the Service.
            {
              from: conf.networkPolicy.api.from,
              ports: [{ protocol: "TCP", port: conf.api.port }],
            },
            // MCP has no credential check of its own: the gateway only.
            ...(conf.mcp.enabled
              ? [{ from: conf.networkPolicy.mcp.from, ports: [{ protocol: "TCP", port: conf.mcp.port }] }]
              : []),
            ...conf.networkPolicy.extraIngress,
          ],
          egress: [
            {
              to: [{ namespaceSelector: { matchLabels: { "kubernetes.io/metadata.name": "kube-system" } } }],
              ports: [
                { protocol: "UDP", port: 53 },
                { protocol: "TCP", port: 53 },
              ],
            },
            // Artwork, lyrics and similar-artist lookups are public
            // services; nothing inside the cluster or a private network is
            // koan's business.
            { to: [{ ipBlock: { cidr: "0.0.0.0/0", except: conf.networkPolicy.privateCidrs } }] },
          ],
        },
      },
      options,
    );
  }

  return {
    routes: [{ service: apiServiceName, hostname: conf.hostname }],
    deployment,
    apiService,
    ...(mcpService && { mcpService }),
  } satisfies Deployed & Record<string, unknown>;
}
