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
   * The instance: the Deployment's and NetworkPolicy's names, the
   * `app.kubernetes.io/instance` label, and the Pulumi resource names, so two
   * servers can share a namespace. Changing it replaces the resources.
   */
  name: z
    .string()
    .regex(/^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/, "must be a DNS label")
    .default("koan"),
  /**
   * Fed to KOAN_GRAPHQL__ALLOWED_HOSTS and KOAN_SERVER__URL: koan
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
  /**
   * The music library, always mounted read-only. Set exactly one field,
   * unless this is a demo, whose library comes from `demo.library`.
   */
  library: z
    .strictObject({
      hostPath: AbsolutePath.optional(),
      existingClaim: z.string().optional(),
    })
    .prefault({}),
  /**
   * A public demo server: one shared `demo` account (koan's `server.demo`),
   * the library copied out of an image, and state on an emptyDir that is
   * wiped and rebuilt at the top of every hour.
   */
  demo: z
    .strictObject({
      enabled: z.boolean().default(false),
      /** An image holding the library at /music, with `cp`. */
      library: z.string().default(VERSIONS.demoLibrary),
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
   * suspended, sent through kōan's push relay. On by default; `enabled:
   * false` keeps the server off the relay, and the apps ask for no
   * notifications.
   */
  push: z
    .strictObject({
      enabled: z.boolean().default(true),
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
}).superRefine((c, ctx) => {
  if (c.demo.enabled) {
    if (c.library.hostPath || c.library.existingClaim || c.state.hostPath || c.state.existingClaim) {
      ctx.addIssue({
        code: "custom",
        message: "a demo brings its own library and keeps no state: leave library and state unset",
      });
    }
  } else if (!c.library.hostPath && !c.library.existingClaim) {
    ctx.addIssue({ code: "custom", message: "library.hostPath or library.existingClaim is required" });
  }
});
