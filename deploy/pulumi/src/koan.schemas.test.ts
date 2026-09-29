import { describe, expect, it } from "vitest";
import { KoanConfSchema } from "./koan.schemas.ts";

const base = { hostname: "koan.example.com", library: { hostPath: "/mnt/kontent/music" } };

describe("KoanConfSchema", () => {
  it("fills defaults", () => {
    const c = KoanConfSchema.parse(base);
    expect(c.image.repository).toBe("ghcr.io/radiosilence/koan");
    expect(c.api.port).toBe(4000);
    expect(c.mcp.enabled).toBe(true);
    expect(c.mcp.port).toBe(8081);
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

  it("leaves push off by default, and wants the key's ids with its Secret", () => {
    expect(KoanConfSchema.parse(base).push.existingSecret).toBe("");
    expect(() => KoanConfSchema.parse({ ...base, push: { existingSecret: "koan-apns" } })).toThrow();
    expect(() =>
      KoanConfSchema.parse({
        ...base,
        push: { existingSecret: "koan-apns", keyId: "ABC123DEFG", teamId: "TEAM123456" },
      }),
    ).not.toThrow();
  });

  it("refuses invalid resource quantities and unknown keys", () => {
    expect(() =>
      KoanConfSchema.parse({ ...base, resources: { requests: { cpu: "lots", memory: "256Mi" } } }),
    ).toThrow();
    expect(() => KoanConfSchema.parse({ ...base, extra: true })).toThrow();
  });
});
