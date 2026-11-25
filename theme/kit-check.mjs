#!/usr/bin/env node
// kit-check: fails on classes outside the kōan look in an app's templates and the code that writes markup.
//
//   kit-check templates src web
//
// It reads class attributes (`class="…"`, `class=\"…\"` in Rust strings, JSX's `className`) and `classList` calls,
// across lines, and reports rounded corners, shadows, rings, blur, gradients, Tailwind's palette and type sizes,
// arbitrary colours and font sizes, and inline `style=` attributes. tokens.css already removes most of these from
// Tailwind, so they compile to nothing; this says where they are. A deliberate exception carries `kit: raw` and its
// reason on the line or the line above.
import { lstatSync, readdirSync, readFileSync } from "node:fs";
import { extname, join } from "node:path";

const EXTENSIONS = new Set([".html", ".askama", ".jinja", ".j2", ".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs"]);
const CODE = new Set([".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs"]);
const SKIP = new Set(["node_modules", "target", "dist", ".git"]);

const PALETTE =
  "slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose";

const RULES = [
  [/^rounded(?!-none$)(-.+)?$/, "corners are square"],
  [/^(inset-)?shadow(-.+)?$|^(drop|text)-shadow(-.+)?$/, "no shadows: a region is told apart by a rule"],
  [/^(inset-)?ring(-.+)?$/, "a control's edge is its border; focus is the theme's outline"],
  [/^(backdrop-)?blur(-.+)?$|^backdrop-/, "no blur or materials"],
  [/^bg-(linear|radial|conic|gradient)(-.+)?$/, "no gradients"],
  // Gradient stops read only in class attributes: in a code string, `from-` and `to-` start ordinary words.
  [/^(from|via|to)-/, "no gradients", "attribute"],
  [/^text-(xs|sm|base|lg|\d?xl)(\/\S+)?$|^text-\[\d/, "type comes from the theme's roles: fine, meta, control, body, title-sm, title, display"],
  [new RegExp(`^[a-z-]+-(${PALETTE})-\\d`), "colours come from the theme's tokens"],
  [/-\[(#|rgba?\(|hsla?\(|oklch\(|color)/, "colours come from the theme's tokens"],
];

const CLASS_VALUES = [
  /\bclass(?:Name)?\s*=\s*\\?"([^"\\]*)\\?"/g,
  /\bclass(?:Name)?\s*=\s*'([^']*)'/g,
  /\bclassName=\{`([^`]*)`\}/g,
  /classList\.(?:add|remove|toggle)\(([^)]*)\)/g,
];

// Template expressions are not classes; what they produce is checked where it is written.
const strip = (value) => value.replace(/\{\{.*?\}\}|\{%.*?%\}|\{#.*?#\}|\$\{.*?\}/gs, " ");

// `hover:md:!-rounded-lg` is `rounded-lg`.
const base = (token) => token.split(":").pop().replace(/^!/, "").replace(/^-/, "").replace(/!$/, "");

function* files(path) {
  const stat = lstatSync(path);
  if (stat.isSymbolicLink()) return;
  if (stat.isFile()) {
    if (EXTENSIONS.has(extname(path))) yield path;
    return;
  }
  if (!stat.isDirectory()) return;
  for (const name of readdirSync(path)) {
    if (!SKIP.has(name)) yield* files(join(path, name));
  }
}

const paths = process.argv.slice(2);
if (paths.length === 0 || paths.includes("--help")) {
  console.error("usage: kit-check <path>...");
  process.exit(paths.length === 0 ? 2 : 0);
}

const problems = [];
for (const root of paths) {
  try {
    lstatSync(root);
  } catch {
    console.error(`kit-check: no such file or directory: ${root}`);
    process.exit(2);
  }
  for (const file of files(root)) {
    const text = readFileSync(file, "utf8");
    const lines = text.split("\n");
    const lineOf = (index) => text.slice(0, index).split("\n").length;
    const excused = (line) => lines[line - 1].includes("kit: raw") || (line > 1 && lines[line - 2].includes("kit: raw"));
    const report = (index, message) => {
      const line = lineOf(index);
      if (!excused(line)) problems.push(`${file}:${line}: ${message}`);
    };

    for (const match of text.matchAll(/\sstyle=\\?["'{]/g)) {
      report(match.index, "style= — inline styles are blocked by the CSP; use a class");
    }
    // Code also keeps class lists in constants and builds them in strings. Any string literal is read there, but only
    // for hyphenated tokens, since a bare word in prose ("ring", "blur") is not a class.
    if (CODE.has(extname(file))) {
      for (const match of text.matchAll(/"((?:[^"\\\n]|\\.)*)"|`([^`]*)`/g)) {
        const value = strip(match[1] ?? match[2]);
        for (const token of value.split(/\s+/).filter((t) => t.includes("-"))) {
          const rule = RULES.find(([re, , only]) => only !== "attribute" && re.test(base(token)));
          if (rule) report(match.index + 1 + Math.max(0, value.indexOf(token)), `${token} — ${rule[1]}`);
        }
      }
    }
    for (const pattern of CLASS_VALUES) {
      for (const match of text.matchAll(pattern)) {
        const value = strip(match[1]);
        for (const token of value.replace(/["',]/g, " ").split(/\s+/).filter(Boolean)) {
          const rule = RULES.find(([re]) => re.test(base(token)));
          if (!rule) continue;
          const offset = match[0].indexOf(match[1]) + Math.max(0, match[1].indexOf(token));
          report(match.index + offset, `${token} — ${rule[1]}`);
        }
      }
    }
  }
}

for (const problem of problems) console.error(problem);
if (problems.length > 0) {
  console.error(`\n${problems.length} outside the kit. Mark a deliberate exception with \`kit: raw\` and its reason.`);
  process.exit(1);
}
