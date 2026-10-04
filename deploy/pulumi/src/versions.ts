/**
 * The version this package deploys: the workspace's version, and the
 * package's own. koan and the package that deploys it ship together, so one
 * number says exactly which build a pin gets. CI refuses a release where
 * `Cargo.toml`, `package.json` and this disagree.
 */
export const APP_VERSION = "0.51.0";

export const VERSIONS = {
  koan: `ghcr.io/radiosilence/koan:v${APP_VERSION}`,
  alpine: "alpine:3.21",
} as const;
