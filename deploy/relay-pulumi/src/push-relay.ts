import * as k8s from "@pulumi/kubernetes";
import type * as pulumi from "@pulumi/pulumi";
import type * as z from "zod";
import type { Deployed } from "./contract.ts";
import { PushRelayConfSchema } from "./push-relay.schemas.ts";
import { APP_VERSION } from "./versions.ts";

const NAME = "koan-push-relay";
const HTTP_PORT = 8080;
const METRICS_PORT = 9090;
const DATA = "/data";
const KEYS = "/keys";
/** The image's user: distroless `nonroot`. */
const UID = 65532;

const selectorLabels = {
  "app.kubernetes.io/name": NAME,
  "app.kubernetes.io/instance": NAME,
};

const labels = {
  ...selectorLabels,
  "app.kubernetes.io/version": APP_VERSION,
  "app.kubernetes.io/managed-by": "pulumi",
};

/**
 * kōan's push relay, which forwards pushes to the iOS app for servers that
 * do not hold Apple's key. See `docs/reference/protocol/push-relay.md`.
 *
 * One replica, replaced rather than rolled: the revocation table is a SQLite
 * file with one writer, and a rolling update would briefly run two.
 *
 * No Ingress: the deployer routes `hostname` to the Service.
 */
export function createPushRelay(
  provider: k8s.Provider,
  namespace: pulumi.Input<string>,
  hostname: string,
  confArgs: z.input<typeof PushRelayConfSchema>,
) {
  const conf = PushRelayConfSchema.parse(confArgs);
  const options = { provider };
  const image = `${conf.image.repository}:${conf.image.tag || `v${APP_VERSION}`}`;
  const secret = (key: string) => ({ secretKeyRef: { name: conf.existingSecret, key } });
  const gateways = (["production", "sandbox"] as const).filter((g) => conf.apns[g]);

  const env = [
    { name: "KOAN_RELAY__LISTEN", value: `0.0.0.0:${HTTP_PORT}` },
    { name: "KOAN_RELAY__METRICS_LISTEN", value: `0.0.0.0:${METRICS_PORT}` },
    { name: "KOAN_RELAY__CLIENT_IP_HEADER", value: conf.clientIp.header },
    { name: "KOAN_RELAY__CLIENT_IP_HOPS", value: String(conf.clientIp.hops) },
    { name: "KOAN_RELAY__STORE", value: `${DATA}/relay.db` },
    { name: "KOAN_RELAY__TOPIC", value: conf.topic },
    { name: "KOAN_RELAY__TOKEN_KEY", valueFrom: secret("token-key") },
    { name: "KOAN_RELAY__HANDLES__CURRENT", value: String(conf.handles.current) },
    { name: "KOAN_RELAY__HANDLES__LIFETIME_DAYS", value: String(conf.handles.lifetimeDays) },
    ...conf.handles.ids.map((id) => ({
      name: `KOAN_RELAY__HANDLES__KEYS__${id}`,
      valueFrom: secret(`handle-key-${id}`),
    })),
    { name: "KOAN_RELAY__APNS__TEAM_ID", value: conf.apns.teamId },
    ...gateways.flatMap((g) => [
      { name: `KOAN_RELAY__APNS__${g.toUpperCase()}__KEY_ID`, value: conf.apns[g]!.keyId },
      { name: `KOAN_RELAY__APNS__${g.toUpperCase()}__KEY_PATH`, value: `${KEYS}/${g}.p8` },
    ]),
    ...Object.entries(conf.limits).map(([limit, value]) => ({
      name: `KOAN_RELAY__LIMITS__${limit.toUpperCase()}`,
      value: String(value),
    })),
  ];

  const deployment = new k8s.apps.v1.Deployment(
    NAME,
    {
      metadata: { name: NAME, namespace, labels },
      spec: {
        replicas: 1,
        strategy: { type: "Recreate" },
        selector: { matchLabels: selectorLabels },
        template: {
          metadata: {
            labels: selectorLabels,
            annotations: {
              "prometheus.io/scrape": "true",
              "prometheus.io/port": String(METRICS_PORT),
              "prometheus.io/path": "/metrics",
            },
          },
          spec: {
            automountServiceAccountToken: false,
            // A Service named after the relay would inject KOAN_RELAY_*
            // variables beside the relay's own configuration.
            enableServiceLinks: false,
            ...(Object.keys(conf.nodeSelector).length > 0 && { nodeSelector: conf.nodeSelector }),
            securityContext: {
              runAsUser: UID,
              runAsGroup: UID,
              runAsNonRoot: true,
              seccompProfile: { type: "RuntimeDefault" },
            },
            // kubelet creates a missing hostPath as root.
            ...(conf.store.hostPath && {
              initContainers: [
                {
                  name: "store",
                  image: conf.initPermissions.image,
                  command: ["sh", "-c", `chown ${UID}:${UID} ${DATA} && chmod 700 ${DATA}`],
                  securityContext: {
                    runAsUser: 0,
                    runAsNonRoot: false,
                    allowPrivilegeEscalation: false,
                    capabilities: { drop: ["ALL"], add: ["CHOWN", "FOWNER"] },
                  },
                  resources: { requests: { cpu: "5m", memory: "8Mi" }, limits: { memory: "32Mi" } },
                  volumeMounts: [{ name: "store", mountPath: DATA }],
                },
              ],
            }),
            containers: [
              {
                name: NAME,
                image,
                imagePullPolicy: conf.image.pullPolicy,
                env,
                ports: [
                  { name: "http", containerPort: HTTP_PORT },
                  { name: "metrics", containerPort: METRICS_PORT },
                ],
                readinessProbe: { httpGet: { path: "/healthz", port: "metrics" }, periodSeconds: 10 },
                livenessProbe: { httpGet: { path: "/healthz", port: "metrics" }, periodSeconds: 30 },
                resources: { limits: conf.resources.limits, requests: conf.resources.requests },
                securityContext: {
                  allowPrivilegeEscalation: false,
                  readOnlyRootFilesystem: true,
                  capabilities: { drop: ["ALL"] },
                },
                volumeMounts: [
                  { name: "store", mountPath: DATA },
                  { name: "keys", mountPath: KEYS, readOnly: true },
                  { name: "tmp", mountPath: "/tmp" },
                ],
              },
            ],
            volumes: [
              {
                name: "store",
                ...(conf.store.existingClaim
                  ? { persistentVolumeClaim: { claimName: conf.store.existingClaim } }
                  : { hostPath: { path: conf.store.hostPath!, type: "DirectoryOrCreate" } }),
              },
              {
                name: "keys",
                secret: {
                  secretName: conf.existingSecret,
                  defaultMode: 0o400,
                  items: gateways.map((g) => ({
                    key: conf.apns[g]!.secretKey ?? `apns-${g}`,
                    path: `${g}.p8`,
                  })),
                },
              },
              { name: "tmp", emptyDir: {} },
            ],
          },
        },
      },
    },
    options,
  );

  const service = new k8s.core.v1.Service(
    conf.service.name,
    {
      metadata: { name: conf.service.name, namespace, labels },
      spec: { selector: selectorLabels, ports: [{ port: 80, targetPort: "http" }] },
    },
    options,
  );

  new k8s.networking.v1.NetworkPolicy(
    `${NAME}-netpol`,
    {
      metadata: { name: NAME, namespace, labels },
      spec: {
        podSelector: { matchLabels: selectorLabels },
        policyTypes: ["Ingress", "Egress"],
        ingress: [
          { from: conf.networkPolicy.from, ports: [{ protocol: "TCP", port: HTTP_PORT }] },
          // Metrics and health, for scrapers that may run on the host
          // network. Nothing routes here from outside the cluster.
          { ports: [{ protocol: "TCP", port: METRICS_PORT }] },
        ],
        egress: [
          {
            to: [{ namespaceSelector: { matchLabels: { "kubernetes.io/metadata.name": "kube-system" } } }],
            ports: [
              { protocol: "UDP", port: 53 },
              { protocol: "TCP", port: 53 },
            ],
          },
          // Apple's gateways, whose addresses are not published as a list.
          {
            to: [{ ipBlock: { cidr: "0.0.0.0/0", except: conf.networkPolicy.privateCidrs } }],
            ports: [{ protocol: "TCP", port: 443 }],
          },
        ],
      },
    },
    options,
  );

  return {
    routes: [{ service: conf.service.name, hostname }],
    deployment,
    service,
  } satisfies Deployed & Record<string, unknown>;
}
