import * as z from "zod";
import { AbsolutePath, Quantity } from "./contract.ts";
import { VERSIONS } from "./versions.ts";

const NetworkPolicyPeer = z.record(z.string(), z.unknown());

const ResourceQuantities = z.strictObject({
  cpu: Quantity,
  memory: Quantity,
});

export const KoanConfSchema = z.strictObject({
  /**
   * Fed to KOAN_GRAPHQL__ALLOWED_HOSTS and KOAN_SHARING__PUBLIC_URL: koan
   * refuses a Host it was not told about, and share links are built on it.
   */
  hostname: z.string().min(1),
  image: z
    .strictObject({
      repository: z.string().default("ghcr.io/radiosilence/koan"),
      /** Defaults to `v${APP_VERSION}` when empty. */
      tag: z.string().default(""),
      pullPolicy: z.string().default("IfNotPresent"),
    })
    .prefault({}),
  /** Passed to the container as --port. The image already defaults to 4000. */
  api: z
    .strictObject({
      port: z.number().int().positive().default(4000),
    })
    .prefault({}),
  service: z
    .strictObject({
      /** The API Service's name. Defaults to the release's full name. */
      name: z.string().default(""),
    })
    .prefault({}),
  /** The music library, always mounted read-only. Set exactly one field. */
  library: z
    .strictObject({
      hostPath: AbsolutePath.optional(),
      existingClaim: z.string().optional(),
    })
    .refine((v) => Boolean(v.existingClaim || v.hostPath), {
      message: "library.hostPath or library.existingClaim is required",
    })
    .prefault({}),
  /**
   * koan's config, index and auth keys. Set exactly one field; leaving both
   * unset uses an emptyDir, which does not survive the pod.
   */
  state: z
    .strictObject({
      hostPath: AbsolutePath.optional(),
      existingClaim: z.string().optional(),
    })
    .prefault({}),
  initPermissions: z
    .strictObject({
      /**
       * kubelet creates a missing hostPath as root, and koan runs as uid
       * 1000, so it cannot write what root owns. Only relevant when
       * state.hostPath is set — a claim or emptyDir is already owned by the
       * pod — and skipped otherwise regardless of this flag.
       */
      enabled: z.boolean().default(true),
      image: z.string().default(VERSIONS.alpine),
    })
    .prefault({}),
  /**
   * Push notifications to koan's iOS app, which reach a phone iOS has
   * suspended. Off unless `existingSecret` names a Secret holding the team's
   * APNs auth key (.p8) under `apns-key`: the key never appears in the
   * Deployment.
   */
  push: z
    .strictObject({
      existingSecret: z.string().default(""),
      /** The key's ten-character id. */
      keyId: z.string().default(""),
      /** The Apple developer team the key belongs to. */
      teamId: z.string().default(""),
    })
    .refine((v) => !v.existingSecret || (v.keyId && v.teamId), {
      message: "push.keyId and push.teamId are required with push.existingSecret",
    })
    .prefault({}),
  /**
   * Pin the pod to one node when state or library use a hostPath: the
   * Deployment uses the Recreate strategy, and a hostPath is only ever the
   * node's own disk.
   */
  nodeSelector: z.record(z.string(), z.string()).default({}),
  resources: z
    .strictObject({
      requests: ResourceQuantities.default({ cpu: "250m", memory: "256Mi" }),
      limits: ResourceQuantities.default({ cpu: "6", memory: "5Gi" }),
    })
    .prefault({}),
  networkPolicy: z
    .strictObject({
      /**
       * Restricts ingress to the API port and egress to DNS plus
       * the public internet (artwork, lyrics, artist-info lookups);
       * nothing inside the cluster is koan's business beyond the peers below.
       */
      enabled: z.boolean().default(true),
      api: z
        .strictObject({
          /** Defaults to a Traefik ingress controller by its pod-name label. */
          from: z.array(NetworkPolicyPeer).default([
            { podSelector: { matchLabels: { "app.kubernetes.io/name": "traefik" } } },
          ]),
        })
        .prefault({}),
      /**
       * Extra ingress rules, e.g. a node CIDR the CNI needs admitted for
       * kubelet's own readiness/liveness probes.
       */
      extraIngress: z.array(NetworkPolicyPeer).default([]),
      /**
       * Private and link-local ranges egress may not reach, even at
       * 0.0.0.0/0: everything the pod has no business talking to directly.
       */
      privateCidrs: z.array(z.string()).default([
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "169.254.0.0/16",
        "100.64.0.0/10",
      ]),
    })
    .prefault({}),
});
