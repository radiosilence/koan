// Compiles the web UI's scripts, crates/koan-server/scripts/*.ts, into
// crates/koan-server/assets/*.js by removing their types and nothing else, so
// the output reads as the source does. `--check` writes nothing and fails if
// the committed output is stale. Either way, output left without a source fails
// the run. Typechecking is tsc's (`just js`).
//
// Node blanks the types out with spaces, keeping every other character where it
// was. Each blanked run, with the spaces inside it, is then cut, and a line left
// empty by a cut is dropped.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { join } from "node:path";

const src = "crates/koan-server/scripts";
const out = "crates/koan-server/assets";
const check = process.argv.includes("--check");
// Vendored, not compiled from a source here.
const vendored = new Set(["datastar.js"]);

function strip(ts) {
  const blanked = stripTypeScriptTypes(ts, { mode: "strip" });
  if (blanked.length !== ts.length) throw new Error("the stripper moved text");
  // Cuts are marked with \0 until the lines they emptied are dropped.
  let js = "";
  let i = 0;
  while (i < ts.length) {
    if (blanked[i] === ts[i]) { js += ts[i++]; continue; }
    // A run of removed text, through any whitespace up to the next removal.
    let end = i;
    for (let j = i; j < ts.length; j++) {
      if (blanked[j] !== ts[j]) end = j + 1;
      else if (!/\s/.test(ts[j])) break;
    }
    js += "\0" + blanked.slice(i, end).replace(/[^\S\n]/g, "").replaceAll("\n", "\n\0");
    i = end;
  }
  return js
    .split("\n")
    .filter((line) => !line.includes("\0") || line.replaceAll("\0", "").trim())
    .join("\n")
    .replaceAll("\0", "");
}

const stale = [];
for (const name of readdirSync(src)) {
  if (!name.endsWith(".ts") || name.endsWith(".d.ts")) continue;
  const js = strip(readFileSync(join(src, name), "utf8"));
  const path = join(out, name.replace(/\.ts$/, ".js"));
  let current = null;
  try { current = readFileSync(path, "utf8"); } catch {}
  if (current === js) continue;
  if (check) stale.push(path);
  else writeFileSync(path, js);
}
// Output whose source was renamed or deleted.
const sources = new Set(readdirSync(src).filter((n) => n.endsWith(".ts")).map((n) => n.replace(/\.ts$/, ".js")));
const orphans = readdirSync(out)
  .filter((n) => n.endsWith(".js") && !vendored.has(n) && !sources.has(n))
  .map((n) => join(out, n));
if (stale.length) console.error(`stale, run \`just js\`: ${stale.join(", ")}`);
if (orphans.length) console.error(`no source in ${src}, delete them: ${orphans.join(", ")}`);
if (stale.length || orphans.length) process.exit(1);
