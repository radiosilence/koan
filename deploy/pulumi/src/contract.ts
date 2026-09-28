/**
 * What this package needs from Kubernetes and hands back to whoever deploys it.
 *
 * Declared here rather than imported from a shared package, and that is the
 * point: this is a chart. It describes how to stand koan up and knows nothing
 * about the estate standing it up — so it depends on `@pulumi/*` and `zod`
 * and nothing else, and can be published and consumed by a deployment that
 * has never heard of jaritanet.
 *
 * The duplication is small and deliberate. A shared-primitives package would
 * have to be published too, and would make one deployment's conventions part of
 * every consumer's dependency tree.
 */
import * as z from "zod";

/** Absolute, because a container's working directory is not what you think. */
export const AbsolutePath = z
  .string()
  .startsWith("/", "must be an absolute path");

/** A Kubernetes resource quantity — `500m`, `2`, `64Mi`, `8Gi`. */
export const Quantity = z
  .string()
  .regex(
    /^\d+(\.\d+)?([munkMGTPE]|[KMGTPE]i)?$/,
    "must be a Kubernetes quantity, e.g. 500m, 2, 64Mi, 8Gi",
  );

/**
 * A hostname the deployment should publish, and the workload answering it.
 *
 * `service` is the Service's own name, so a route names the pair rather than
 * either half.
 */
export type Route = {
  service: string;
  hostname: string;
  paths?: string[];
  priority?: number;
};

/** What the deployer gets back: where this stands. */
export type Deployed = {
  routes: Route[];
};
