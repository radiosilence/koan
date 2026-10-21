/**
 * The version this package deploys: the relay crate's version, and the
 * package's own. The relay is versioned apart from kōan, and it and the
 * package that deploys it ship together, so one number says exactly which
 * build a pin gets. `relay.yml` refuses a build where
 * `crates/koan-push-relay/Cargo.toml`, `package.json` and this disagree.
 */
export const APP_VERSION = "0.1.0";

export const VERSIONS = {
  relay: `ghcr.io/radiosilence/koan-push-relay:v${APP_VERSION}`,
  alpine: "alpine:3.21",
} as const;
