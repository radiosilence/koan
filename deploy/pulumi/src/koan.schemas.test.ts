import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { KoanConfSchema } from "./koan.schemas.ts";
import { VERSIONS } from "./versions.ts";

const base = { hostname: "koan.example.com", library: { hostPath: "/mnt/kontent/music" } };

describe("KoanConfSchema", () => {
  it("fills defaults", () => {
    const c = KoanConfSchema.parse(base);
    expect(c.image.repository).toBe("ghcr.io/radiosilence/koan");
    expect(c.api.port).toBe(4000);
    expect(c.initPermissions.image).toBe("alpine:3.21");
    expect(c.resources.requests).toEqual({ cpu: "250m", memory: "256Mi" });
    expect(c.resources.limits).toEqual({ cpu: "6", memory: "5Gi" });
    expect(c.networkPolicy.privateCidrs).toContain("10.0.0.0/8");
  });

  it("requires a hostname", () => {
    expect(() => KoanConfSchema.parse({ library: { hostPath: "/mnt/kontent/music" } })).toThrow();
  });

  it("requires exactly one of library.hostPath or library.existingClaim", () => {
    expect(() => KoanConfSchema.parse({ hostname: "koan.example.com" })).toThrow();
    expect(() =>
      KoanConfSchema.parse({ hostname: "koan.example.com", library: { existingClaim: "koan-music" } }),
    ).not.toThrow();
  });

  it("refuses a relative library path, which would mount the wrong thing silently", () => {
    expect(() => KoanConfSchema.parse({ ...base, library: { hostPath: "music" } })).toThrow();
  });

  it("pushes through kōan's relay by default, and can be turned off", () => {
    const c = KoanConfSchema.parse(base);
    expect(c.push).toEqual({ enabled: true });
    expect(KoanConfSchema.parse({ ...base, push: { enabled: false } }).push.enabled).toBe(false);
    expect(() => KoanConfSchema.parse({ ...base, push: { existingSecret: "koan-apns" } })).toThrow();
  });

  it("refuses invalid resource quantities and unknown keys", () => {
    expect(() =>
      KoanConfSchema.parse({ ...base, resources: { requests: { cpu: "lots", memory: "256Mi" } } }),
    ).toThrow();
    expect(() => KoanConfSchema.parse({ ...base, extra: true })).toThrow();
  });

  it("takes a demo without a library or state, and refuses one with either", () => {
    const c = KoanConfSchema.parse({ hostname: "demo.example.com", demo: { enabled: true } });
    expect(c.demo.library).toBe(VERSIONS.demoLibrary);
    expect(() => KoanConfSchema.parse({ ...base, demo: { enabled: true } })).toThrow();
    expect(() =>
      KoanConfSchema.parse({ hostname: "demo.example.com", demo: { enabled: true }, state: { hostPath: "/var/lib/koan" } }),
    ).toThrow();
  });
});

describe("VERSIONS.demoLibrary", () => {
  it("is tagged by the content of deploy/demo, as the Demo library workflow tags what it builds", () => {
    const read = (name: string) => readFileSync(new URL(`../../demo/${name}`, import.meta.url));
    const tag = createHash("sha256").update(read("Dockerfile")).update(read("library.tsv")).digest("hex").slice(0, 12);
    expect(VERSIONS.demoLibrary).toBe(`ghcr.io/radiosilence/koan-demo-library:${tag}`);
  });
});
