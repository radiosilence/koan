import * as k8s from "@pulumi/kubernetes";
import * as pulumi from "@pulumi/pulumi";
import type * as z from "zod";
import type { Deployed } from "./contract.ts";
import { KoanConfSchema } from "./koan.schemas.ts";
import { APP_VERSION } from "./versions.ts";

const NAME = "koan";
const STATE_MOUNT_PATH = "/config";

/**
 * A demo's container: wipe the state, index the library, and serve until the
 * clock reaches the hour, then again. A loop in the container rather than a
 * CronJob restarting the pod, which would need RBAC to do it. `$@` is the image's usual arguments. Each step
 * runs in the background, so a TERM is handled at once rather than after it.
 */
const DEMO_RUN = `
pid=
trap '[ -n "$pid" ] && kill -TERM "$pid" 2>/dev/null; wait; exit 0' TERM INT
while :; do
  find ${STATE_MOUNT_PATH} -mindepth 1 -delete
  koan scan /music >/dev/null & pid=$!
  wait "$pid"
  timeout "$(( 3600 - $(date +%s) % 3600 ))" koan --headless --bind 0.0.0.0 "$@" & pid=$!
  wait "$pid"
  sleep 1
done
`;

const labels = (instance: string) => ({
  ...selectorLabels(instance),
  "app.kubernetes.io/version": APP_VERSION,
  "app.kubernetes.io/managed-by": "pulumi",
});

const selectorLabels = (instance: string) => ({
  "app.kubernetes.io/name": NAME,
  "app.kubernetes.io/instance": instance,
});

/**
 * koan: a headless music server — GraphQL, Subsonic and MCP over a media
 * library, all on the API port.
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
  const instance = conf.name;
  const apiServiceName = conf.service.name || instance;

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
    "app.kubernetes.io/instance": instance,
    "app.kubernetes.io/version": APP_VERSION,
    "app.kubernetes.io/managed-by": "pulumi",
  };
  const check =
    conf.state.existingClaim || conf.state.hostPath
      ? new k8s.batch.v1.Job(
          `${instance}-check-db`,
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
    instance,
    {
      metadata: { name: instance, namespace, labels: labels(instance) },
      spec: {
        replicas: 1,
        // Host paths and one index: two pods would share neither safely.
        strategy: { type: "Recreate" },
        selector: { matchLabels: selectorLabels(instance) },
        template: {
          metadata: { labels: selectorLabels(instance) },
          spec: {
            ...podBase,
            containers: [
              {
                name: NAME,
                image,
                imagePullPolicy: conf.image.pullPolicy,
                ...(conf.demo.enabled && { command: ["sh", "-c", DEMO_RUN, "koan-demo"] }),
                args: ["--port", String(conf.api.port)],
                env: [
                  ...(conf.demo.enabled
                    ? [
                        { name: "KOAN_SERVER__DEMO", value: "true" },
                        // Kept in config.toml elsewhere, which the wipe takes.
                        { name: "KOAN_SUBSONIC__ENABLED", value: "true" },
                      ]
                    : []),
                  { name: "KOAN_LIBRARY__FOLDERS", value: '["/music"]' },
                  // koan refuses a Host it was not told about.
                  { name: "KOAN_GRAPHQL__ALLOWED_HOSTS", value: `["${conf.hostname}"]` },
                  // Served over HTTPS in front of the Service.
                  { name: "KOAN_GRAPHQL__COOKIE_SECURE", value: "true" },
                  // Share links and MCP's OAuth are built on the address
                  // strangers reach.
                  { name: "KOAN_SERVER__URL", value: `https://${conf.hostname}` },
                  { name: "KOAN_PUSH__ENABLED", value: String(conf.push.enabled) },
                ],
                ports: [{ name: "api", containerPort: conf.api.port }],
                readinessProbe: { tcpSocket: { port: "api" }, periodSeconds: 10 },
                // Not for a demo: the port is closed while it wipes and
                // rescans on the hour, which is no fault to restart for.
                ...(!conf.demo.enabled && {
                  livenessProbe: { tcpSocket: { port: "api" }, initialDelaySeconds: 30, periodSeconds: 30 },
                }),
                resources: { limits: conf.resources.limits, requests: conf.resources.requests },
                securityContext: {
                  allowPrivilegeEscalation: false,
                  readOnlyRootFilesystem: true,
                  capabilities: { drop: ["ALL"] },
                },
                volumeMounts: [
                  // A demo's image holds the library under /music.
                  { name: "music", mountPath: "/music", readOnly: true, ...(conf.demo.enabled && { subPath: "music" }) },
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
                // A demo's library is its image, mounted as it is: nothing
                // copied, and the node keeps the layers.
                ...(conf.demo.enabled
                  ? { image: { reference: conf.demo.library!, pullPolicy: "IfNotPresent" } }
                  : conf.library.existingClaim
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
      metadata: { name: apiServiceName, namespace, labels: labels(instance) },
      spec: { selector: selectorLabels(instance), ports: [{ port: 80, targetPort: "api" }] },
    },
    options,
  );

  if (conf.networkPolicy.enabled) {
    new k8s.networking.v1.NetworkPolicy(
      `${instance}-netpol`,
      {
        metadata: { name: instance, namespace, labels: labels(instance) },
        spec: {
          podSelector: { matchLabels: selectorLabels(instance) },
          policyTypes: ["Ingress", "Egress"],
          ingress: [
            // GraphQL and Subsonic, through whatever fronts the Service.
            {
              from: conf.networkPolicy.api.from,
              ports: [{ protocol: "TCP", port: conf.api.port }],
            },
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
            // Artwork, lyrics and artist-info lookups are public
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
  } satisfies Deployed & Record<string, unknown>;
}
