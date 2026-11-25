// The Zod the web scripts import as `zod/mini`, through the import map. Only
// what they use is exported, since everything Zod has is 85 KB gzipped and the
// sign-in page loads this. tsconfig resolves `zod/mini` here too, so a script
// that uses something not listed fails the typecheck, not the browser.
// `just zod` bundles it into assets/zod.js, which the crate embeds.
export { array, boolean, enum, literal, nullable, number, object, optional, string, union } from "zod/v4/mini";
export type { infer, input, output } from "zod/v4/mini";
