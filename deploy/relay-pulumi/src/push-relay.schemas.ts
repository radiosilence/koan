import * as z from "zod";
import { AbsolutePath, Quantity } from "./contract.ts";
import { VERSIONS } from "./versions.ts";

const NetworkPolicyPeer = z.record(z.string(), z.unknown());

const ResourceQuantities = z.strictObject({
  cpu: Quantity,
  memory: Quantity,
});

/** The relay refuses zero, and anything past 2^30. */
const Limit = z.number().int().positive().max(2 ** 30).optional();

const ApnsKey = z.strictObject({
  /** The key's ten-character id. */
  keyId: z.string().min(1),
  /**
   * The Secret's entry holding the key (.p8, PEM). Defaults to
   * `apns-<gateway>`; a team key serves both gateways from one entry.
   */
  secretKey: z.string().optional(),
});

export const PushRelayConfSchema = z.strictObject({
  image: z
    .strictObject({
      repository: z.string().default("ghcr.io/radiosilence/koan-push-relay"),
      /** Defaults to `v${APP_VERSION}` when empty. */
      tag: z.string().default(""),
      pullPolicy: z.string().default("IfNotPresent"),
    })
    .prefault({}),
  service: z
    .strictObject({
      name: z.string().default("koan-push-relay"),
    })
    .prefault({}),
  /**
   * A Secret holding every credential, so none appears in the Deployment:
   * `token-key` and `handle-key-<id>` (32 random bytes each, base64), and
   * the APNs keys named below. The token key is never rotated: the
   * revocation table is keyed by it.
   */
  existingSecret: z.string().min(1),
  /** The app's bundle id, which every push is sent to. */
  topic: z.string().default("cc.blit.koan"),
  /**
   * The key for each of Apple's gateways: production for TestFlight and App
   * Store builds, sandbox for development builds. At least one is required.
   */
  apns: z
    .strictObject({
      teamId: z.string().min(1),
      production: ApnsKey.optional(),
      sandbox: ApnsKey.optional(),
      /** HTTP/2 connections kept to each gateway. */
      connections: z.number().int().min(1).max(64).default(4),
      /** Pushes waiting on Apple at once; beyond it, 503. */
      maxInFlight: z.number().int().positive().max(2 ** 30).default(4096),
    })
    .refine((v) => v.production || v.sandbox, {
      message: "apns.production or apns.sandbox is required",
    }),
  /**
   * The handle keys in the Secret, by id, and the one new handles are sealed
   * with. Rotate by adding a key and making it current; remove the old one
   * once `lifetimeDays` have passed.
   */
  handles: z
    .strictObject({
      ids: z.array(z.number().int().min(1).max(255)).min(1).default([1]),
      current: z.number().int().min(1).max(255).default(1),
      lifetimeDays: z.number().int().min(1).max(365).default(60),
    })
    .refine((v) => v.ids.includes(v.current), {
      message: "handles.current must be one of handles.ids",
    })
    .prefault({}),
  /**
   * Where the caller's address is read, written by the proxy in front.
   * Traefik overwrites X-Real-Ip on every request from a source it does not
   * trust, so it holds the caller alone.
   */
  clientIp: z
    .strictObject({
      header: z.string().default("x-real-ip"),
      hops: z.number().int().min(1).default(1),
    })
    .prefault({}),
  /**
   * The revocation table. Required: losing it brings back every handle a
   * device has revoked. Set exactly one field.
   */
  store: z
    .strictObject({
      hostPath: AbsolutePath.optional(),
      existingClaim: z.string().optional(),
    })
    .refine((v) => Boolean(v.hostPath) !== Boolean(v.existingClaim), {
      message: "exactly one of store.hostPath or store.existingClaim is required",
    }),
  initPermissions: z
    .strictObject({
      image: z.string().default(VERSIONS.alpine),
    })
    .prefault({}),
  /**
   * Overrides for the relay's quotas; anything unset keeps the relay's
   * default. See `[limits]` in `docs/reference/protocol/push-relay.md`.
   */
  limits: z
    .strictObject({
      handleAlertsPerHour: Limit,
      handleWakesPerHour: Limit,
      deviceAlertsPerHour: Limit,
      deviceWakesPerHour: Limit,
      deviceRevocationsPerHour: Limit,
      ipPushesPerMinute: Limit,
      ipRegistrationsPerMinute: Limit,
      ipRevocationsPerMinute: Limit,
      globalPushesPerSecond: Limit,
      proofsPerSecond: Limit,
      ipBadTokensPerHour: Limit,
      badTokensPerHour: Limit,
      maxTracked: Limit,
      maxRevocations: Limit,
      maxRequests: Limit,
    })
    .prefault({}),
  /** The relay's own log level; its libraries log at warn whatever this says. */
  logLevel: z.enum(["error", "warn", "info", "debug", "trace"]).default("info"),
  /** Pin the pod to the node holding `store.hostPath`. */
  nodeSelector: z.record(z.string(), z.string()).default({}),
  resources: z
    .strictObject({
      requests: ResourceQuantities.default({ cpu: "50m", memory: "64Mi" }),
      limits: ResourceQuantities.default({ cpu: "2", memory: "1Gi" }),
    })
    .prefault({}),
  networkPolicy: z
    .strictObject({
      /**
       * Only the proxy may reach the relay's port. The relay limits callers
       * by the address that proxy writes, so anything else reaching it could
       * choose its own address, and with it its own allowance.
       */
      from: z.array(NetworkPolicyPeer).default([
        {
          namespaceSelector: {},
          podSelector: { matchLabels: { "app.kubernetes.io/name": "traefik" } },
        },
      ]),
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
