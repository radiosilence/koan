# The kōan standard

How radiosilence's projects, not only koan, are built: what we use, how it fits together, and why.

- **Building:** follow it. Where it is silent, copy the closest reference implementation below.
- **Auditing:** work through §23.
- **Departing:** a rule may be broken when the reason is written beside the break: a comment naming the rule (`// theme: raw`), or the project's `CLAUDE.md` for a project-wide break. A break without one is a defect.
- **Changing it:** a convention that applies across projects changes here first, in its own PR.

Reference implementations, at `github.com/radiosilence/<name>`. They show the shape, not perfection; audit them too.

| Building | Start from |
|---|---|
| A small server-rendered web app | `tubele-rs` (its structure; it keeps the original game's look) |
| A service with a web UI, OIDC sign-in, GraphQL, MCP and metrics | `slsk-mcp` |
| A public service holding a credential for others, with a client crate | `koan-push-relay` |
| A CLI and MCP server over someone else's API | `fastmail-cli` |
| An engine that parses hostile input; benchmarks, fuzzing, a threat model | `sbr` |
| An app on every platform over a Rust core; the design system | `koan` (`koan-core`, `koan-runtime`, `koan-gui`, `koan-kit`) |
| A static site | `blit` |
| How it all deploys (context, not a template) | `jaritanet` |

## 1. Values

In order of precedence when they conflict:

1. **Never lose, leak or corrupt the user's data.**
2. **Work where it is used, against what it talks to.** Run on the real device against the real servers first, then make it fast, then tidy.
3. **Fast and frugal, as measured.** Our software runs on phones, televisions, desktops, a small VPS and a home server; a claim about speed carries a number (§12).
4. **Simple, and one of each.** Rust for what runs (Swift, Kotlin and TypeScript only where a platform or the deployer needs them), one TLS stack, one version per release, one pipeline, one required check. Platform features before libraries, libraries before frameworks. An abstraction waits for its third use, a feature for a real one.
5. **Owned end to end.** No telemetry, and nothing reaches a service the person cannot see and turn off. Our own design system and icons; a fork only where upstream blocks us, each generic change proposed upstream so the fork shrinks.

Always:

- **Make the wrong thing impossible, or loud:** types, one owner per invariant, strict parsing that names what it refused, tests named for the invariant they guard.
- **Fail closed, never guess.** Bad config stops startup; an unknown diff runs every check; an ambiguous answer from an API fails the release.
- **Automate the discipline.** A rule that matters has a check: a lint, a test, a ratchet (a count that may only fall), a freshness check.
- **Say why.** Every rule, constant, comment, commit and PR carries its reason.
- **Details are the product.** Accessibility, motion, copy and alignment are designed, not left to defaults.

## 2. Stack

| Need | Use | Why |
|---|---|---|
| Programs | Rust (§5) | One language from an audio callback to a web server; static binaries; the compiler holds invariants |
| Async | `tokio` | What `axum`, `reqwest` and `rmcp` are built on |
| HTTP server | `axum` + `tower-http` | Limits, compression and panic catching as layers |
| HTTP client | `reqwest`, `default-features = false`, `rustls` | One TLS stack; no OpenSSL |
| Crypto | `rustls` on `aws-lc-rs`; RustCrypto (`argon2`, `chacha20poly1305`, `sha2`, `hmac`, `subtle`) | Hardware AES and ChaCha; reviewed primitives |
| Storage | SQLite through `rusqlite` (`bundled`) | One file, no server; `rusqlite` because koan and the shared `sift` crate use it, and two bundled SQLites cannot share a binary |
| HTML | `askama` | A template is bound to a struct at compile time |
| Interactivity | Datastar over SSE, vendored | The server keeps the state; about 13 KB gzipped |
| CSS | Tailwind v4's standalone CLI from mise + `@radiosilence/theme` | Shared tokens; the compiled CSS is committed, so `cargo build` needs no Node or Tailwind |
| Type | Geist Mono | One monospace face: figures align, and every surface reads alike |
| API | GraphQL (`async-graphql`) | One typed schema for remotes, scripts and agents (§4) |
| Agents | MCP (`rmcp`), exposing the GraphQL schema | Agents get the same API (§9) |
| CLI | `clap` (derive) + `clap_complete` | The interface is a type |
| Errors | `thiserror` in libraries, `anyhow` in binaries | Callers match one; people read the other |
| Logs | `tracing` + `tracing-subscriber` | Structured fields, filtered per crate |
| Time, ids | `jiff`; `uuid` v7 for ids, 128 random bits for secrets | `chrono` is unmaintained; v7 sorts by creation |
| App config | `serde` + `figment`; `toml_edit` for writes | Layered files and environment; edits keep comments |
| Desktop and mobile UI | GPUI through `koan-kit`; SwiftUI on tvOS and Compose on Android, where GPUI does not run | One codebase and one look, drawn on the GPU |
| Native bindings | `uniffi` for Swift and Kotlin; a C ABI where only C will do, its header checked by a test | Bindings the compiler checks |
| Static sites | A Rust generator, pandoc over `docs/`, or hand-written HTML, served by nano-web | The browser gets HTML, CSS and a font |
| Packaging | A static musl binary in a `scratch` image on GHCR (glibc only where the binary needs the system's libraries) | Nothing in the image to patch or exploit but the binary |
| Deployment | A Pulumi component package per service (TypeScript, Zod), deployed by jaritanet to k3s | The service ships how it is run |
| Deploy code; scripts in a TypeScript repository | TypeScript (§5) on Node 24; `aube` (package manager), `tsc`, Zod, vitest | Private code runs from source; only a published package is built |
| Repository scripts | Python 3's standard library, bash (§15) | Nothing to install |
| Tasks, tools, hooks | `just`, `mise`, `lefthook` | The same commands on a laptop and in CI |
| Lint | `clippy`, `rustfmt`, `actionlint` + `shellcheck`, `oxlint` + `oxfmt` | Machines settle what review would otherwise argue about |

**Not used:** SPA frameworks or bundlers (§8); OpenSSL or native-tls; ORMs (queries are shaped for their indexes, §6); `chrono` in new code; `lazy_static` or `once_cell` (std's `LazyLock` and `OnceLock` replace them); compiling in a Dockerfile (§16).

## 3. Repository

Every file the standard asks for, and when:

```
CLAUDE.md  README.md  LICENSE                   always (§19)
CHANGELOG.md  changelog.d/                      always (§20)
scripts/changelog.py  scripts/test_changelog.py always (§20)
Cargo.toml  Cargo.lock                          one package at the root, or crates/<name>/ when there are several
justfile  mise.toml  lefthook.yml               always (§15)
.claude/settings.json                           always: the agents' push guard (§15)
.cargo/audit.toml  or deny.toml                 always (§16)
.github/workflows/ci-cd.yml                     always (§16)
.github/workflows/cancel-closed-pr.yml          always (§16)
.github/dependabot.yml                          always (§16)
.github/workflows/release.yml                   when it releases (§18)
scripts/release.sh                              when it releases (§18)
tests/                                          end-to-end tests (§14)
migrations/                                     SQLite (§6)
templates/  styles/  assets/                    a web UI (§8)
Dockerfile  deploy/pulumi/                      a service (§17)
ARCHITECTURE.md                                 threads of its own, a wire protocol, or a data model with invariants (§19)
docs/                                           users to teach (§19)
HANDOVER.md                                     work moving between sessions or machines (§19)
docs/threat-model.md                            an exposed service (§11)
docs/security-audit.md  fuzz/                   an exposed service (§11)
BENCHMARKS.md  and its benches                  a hot path (§12)
```

- **`LICENSE` matches the manifests' `license`** (`LICENSE-MIT` and `LICENSE-APACHE` for `MIT OR Apache-2.0`).
- **A generated file is committed only if a build embeds it or a reader needs it without the toolchain** (compiled CSS and its font, icon code, schemas, API references, `CHANGELOG.md`), and CI checks it is fresh (§16). Xcode projects, `uniffi` bindings and `dist/` are not committed.

## 4. Architecture

- **Crates are layered by what they may touch:** a protocol crate with no I/O, an engine over it, the app over that (`slsk-proto` → `slsk-engine` → `slsk-mcp`), so the protocol can be tested and fuzzed with no network, and the compiler holds the direction. A test double is a crate of its own that only tests link (`slsk-testserver`).
- **The core owns the logic; front ends are shims.** A capability one front end needs goes into the core, so every front end has it. Crate boundaries enforce the direction: `koan-core` knows no UI, `koan-kit` knows no engine.
- **A UI that can link the core does,** in process: a daemon, a port and an auth surface buy nothing. A network API is for what can neither link the core nor be rendered by the server: remotes, scripts, agents.
- **Nothing blocks a UI thread or an async worker.** The core may be synchronous (threads, channels, blocking I/O); a call into it that can block is async at the boundary. SQLite, file I/O, decoding and long-list layout run on a blocking pool or a named thread. Commands that must keep their order go down one ordered lane and name what they act on (an album, not the tracks a client read), which the engine resolves on the lane against current state.
- **An engine's state reaches its front ends as whole snapshots, never a chain of deltas,** in slices cut by rate of change. Each slice has a sequence number and each client a cursor, so falling behind loses intermediate values, never the fact of a change. A snapshot cannot be applied wrongly; a delta can. A slice too large to resend (koan's queue: 50,000 rows is 13 MB, #933) goes whole on an edit and otherwise as the rows that changed against that named base, so the latest patch is all a client needs. A continuous value goes out as an anchor (a position, and when) that clients count on from; a value that changes every frame bypasses state for a layer or the GPU.
- **Store facts, derive the rest.** Status is computed from what happened, since a stored copy drifts from it. An event is a reason to re-read the source, never a value to trust.
- **Identify by id, never by position or name.** Public ids are UUIDv7 `uid`s; row ids stay inside the database, which reuses them.
- **One module owns each invariant** and is the only writer of the columns it covers (`db/queries/sources.rs` alone decides which track a file is).
- **The server is authoritative.** Anything a client could cheat with or leak stays on the server until the client has earned it: tubele's answer reaches a template only through a view that exists for a finished board.
- **Everything input drives is bounded:** queues, tables, caches and buffers, capped in bytes as well as entries, so load exhausts a limit, not memory.
- **Nothing is contacted unasked (§1).** An engine contacts only what its configuration names. An app reaches a service of ours or a public source (its own updates, the push relay, Wikidata) only as a feature its settings name.
- **Read what people write strictly, and what other software sends leniently.** Configuration, and requests to a service that holds a bound, refuse an unknown field with its path (one that cannot change behaviour may pass with a warning). Responses from others' APIs, and messages between versions of our own software, ignore unknown fields; a capability is announced before it is used (§9).
- **Configuration:** a service reads environment variables once at start into a typed struct, and a missing or malformed value stops it with one line naming the variable. An app people configure by hand layers defaults → `config.toml` → `config.local.toml` → `<APP>_*` variables (`figment`, `__` for nesting). One function writes config: it routes secrets and machine-specific keys to the local file, and leaves comments and untouched keys alone.

## 5. Code

### Rust

- **Edition 2024, resolver 3, the latest stable toolchain** from `mise.toml`; no `rust-version`, since only the latest stable is tested.
- **A dependency two crates share is declared once** in `[workspace.dependencies]`, so versions and features cannot drift.
- **A crate takes only what it uses:** `default-features = false` with the features named. An exact pin (`=x.y.z`) carries a comment saying why.
- **Release profile:** `strip = true`, `lto = "thin"`, `codegen-units = 1`. A departure carries a measurement: fat LTO where throughput is the product (sbr), `opt-level = "s"` with `panic = "abort"` for a tiny service whose recovery is a restart, more codegen units where they were measured to cost nothing at run time.
- **Clippy's default lints are errors** (`-D warnings`). A suppression is `#[expect(lint, reason = "…")]`, never a bare `#[allow]`, since `expect` warns once the lint stops firing.
- **Libraries return `thiserror` enums** whose variant docs say what the caller should do ("worth retrying", "asking again changes nothing"), `#[non_exhaustive]` when published. Binaries use `anyhow` with short lowercase context (`.context("binding {addr}")`). A failure at startup prints one legible line and exits 1; it does not panic.
- **No `unwrap` outside tests.** `expect` names the invariant that makes it safe (`expect("checked above")`), not what failed.
- **Panics unwind and are caught at boundaries:** `CatchPanicLayer` in a server, `resume_unwind` from a blocking task, `catch_unwind` at every hand-written FFI entry point (`uniffi` catches its own), a restarting loop where a packet loop must survive.
- **Synchronous locks are `parking_lot` (no poisoning) and are never held across `.await`.** Exclusive access that must span an await (a cursor, one job at a time) is a `tokio::sync::Mutex` whose declaration says why. A real-time path (an audio callback) neither allocates nor locks: atomics and lock-free rings only.
- **`unsafe` is only for FFI and platform calls,** each block with a `// SAFETY:` comment. A crate with none sets `[lints.rust] unsafe_code = "forbid"`.
- **Secrets never reach `Debug`.** A struct holding one derives no `Debug`: it writes one that leaves the secret out (`finish_non_exhaustive`), or has none. An error about a secret carries a `&'static str`, so the secret cannot be formatted into it.
- **Every file opens with a `//!` doc** saying what it holds and why it is shaped so. A constant's doc gives the reason for its value.
- **`rustfmt` with its defaults.**

### TypeScript

For deployment packages, scripts in a TypeScript repository, and browser scripts (§8).

- **TypeScript is written so Node 24 runs it as written,** so one file runs as a script, a Pulumi program or a test with no build step. Erasable syntax only: no `enum`, `namespace`, parameter properties or legacy decorators, but `as const` objects and unions. Types are imported with `import type`, relative imports carry their `.ts` extension, modules are ES modules (`"type": "module"`), and no `paths` alias stands where Node must resolve it. Every tsconfig sets `erasableSyntaxOnly`, `verbatimModuleSyntax`, `rewriteRelativeImportExtensions`, `strict`, `noUncheckedIndexedAccess` and `noEmit` (`tsconfig.build.json` extends it and turns `noEmit` off); extending `@tsconfig/node24` and `@tsconfig/node-ts`, as jaritanet does, sets the first four.
- **Private code (`"private": true`) runs from source.** A workspace package's `exports` point at `./src/index.ts`; Pulumi runs with `runtime.options.typescript: false`, so Node strips the types itself; a script runs under `node`; `tsc` only typechecks.
- **Published code ships JavaScript,** since Node strips no types under `node_modules`: `tsconfig.build.json` emits `dist/` with declarations and rewrites `./x.ts` imports to `./x.js`.
- **No `any`;** `satisfies` and `as const` rather than casts. Configuration and arguments are validated with Zod's `strictObject`, since a plain object drops unknown keys silently. Tests sit beside the source as `*.test.ts`.

## 6. Data

- **Pragmas:** WAL, so readers never block the writer; `foreign_keys = ON`; `synchronous = NORMAL`, or `FULL` where an acknowledgement promises durability; `busy_timeout` of at least 5 s. A write transaction opens with `BEGIN IMMEDIATE`, so it waits for the lock up front instead of failing to upgrade a read.
- **Connections are opened once and kept in a small pool,** since opening one checks the schema and is not per-query work.
- **Migrations are `migrations/NNNN_slug.sql`,** embedded with `include_str!`, counted by `PRAGMA user_version` and applied in a transaction (`foreign_keys` is switched outside it, since the pragma does nothing inside one). A build refuses a database newer than it knows. A test migrates a fixture of every released schema to the latest.
- **SQL lives in named `const`s, never inline;** a query shaped for an index says why ("two statements rather than `?1 IS NULL OR status = ?1`, which no index can answer").
- **Instants are INTEGER Unix milliseconds, UTC,** which sort and compare with no parsing.
- **Automated deletion is guarded:** a pass that would remove more than a fifth of at least 100 rows is refused unless forced, and an I/O error never counts as "gone", since a failed mount looks like everything deleted.
- **Tests use a temporary file, not `:memory:`,** so every pooled connection sees the same data.

## 7. Services

- **Listeners are split by trust:** the public UI; an internal API that trusts identity headers, reachable only from the caller that sets them; metrics on a port of their own.
- **Every HTTP server has a perimeter,** innermost first: `CatchPanicLayer`, a body limit, a request timeout, then `concurrency_limit` behind `load_shed`, which sheds rather than queues. Defaults: 2 MiB, 30 s and 64 in flight (koan's GraphQL), shrunk to what an endpoint takes (the push relay: 8 KiB, 15 s). A stream (SSE, WebSocket, MCP's event stream) takes no request timeout; the shutdown deadline ends it. A server also sets a header-read timeout and a connection cap, since `axum::serve` sets neither.
- **PID 1 handles SIGTERM and SIGINT.** Nothing installs a default handler in a `scratch` image, so an ignored SIGTERM makes every rollout wait out the grace period. On SIGTERM it keeps serving for 5 s while the ingress drops the endpoint (Ctrl-C skips this), then stops accepting and gives in-flight requests and streams a deadline (koan: 10 s) within the grace period.
- **`/healthz` answers `ok` from the process alone,** without checking dependencies, so a dependency's blip restarts nothing.
- **Errors log their cause in full and tell the client only what it can act on.** One error type (`AppError`) maps to a status; 5xx logs at `error`, 4xx at `debug`; only a bad request's own message or a domain error goes back. Upstream text, SQL and paths never reach a client.
- **A server bound to localhost or a LAN refuses a `Host` or `Origin` that is not its own,** against DNS rebinding. CORS is off unless a configured allowlist names the origin; never `*`.
- **Logs:** `tracing` text to stderr, filtered by `RUST_LOG`, defaulting to `<crate>=info,warn`. No request logs at `info`. Never logged: credentials, tokens, URLs carrying either, or anything that identifies a person or device (addresses, names, emails, device ids). A library whose debug output would log one is held at `warn`.
- **Metrics:** Prometheus text written by hand: counters `<app>_<noun>_total`, gauges `<app>_<noun>`, with HELP and TYPE lines. Label values come only from fixed enums, so cardinality is bounded by construction. Gauges are computed at scrape time from the source of truth, not counted beside it.

## 8. Web UI

- **The server renders HTML (`askama`) and Datastar patches it over SSE,** so reloading a page loses nothing. `templates/` holds a shell, `pages/`, and a directory of fragments per area.
- **Script is the exception.** Each file says what it does that HTML and Datastar cannot. No framework or bundler. A UI whose scripts import one another writes them in TypeScript (§5), its imports through the page's import map at versioned URLs, stripped by oxc in `build.rs` and typechecked in CI, so `cargo build` needs no Node; a lone script may be a plain ES module. A static site ships none.
- **Native elements before script:** `<dialog>` opened by `commandfor` and `command="show-modal"`, closed by `closedby="any"` or `<form method="dialog">`; `popover`; `autofocus`.
- **Links have real `href`s and forms real `action`s,** so they work before Datastar loads and a modified click (a new tab) is the browser's.
- **An element that starts hidden uses `hidden` with `data-attr:hidden`,** never `data-show`, so it does not flash before Datastar loads.
- **No text goes inside a Datastar expression,** which runs as script. It goes in a `data-*` attribute or `href`, read with `el.dataset`; only ids and numbers the server made are interpolated.
- **A state-changing request is a POST** carrying Datastar's `datastar-request` header, which a cross-site form cannot set. A plain form checks `Sec-Fetch-Site`, or that `Origin` matches `Host`. Nothing changes state on GET, sign-out included.
- **Escaping stays on.** Markup the server built is a newtype (`Html`) implementing askama's `HtmlSafe`, made where it is built, so it renders unescaped with no `|safe`. `|safe` appears only on what the server made from nothing a person or peer supplied (an import map, the repository's own Typst output).
- **Styles:** Tailwind utilities in the templates over `@radiosilence/theme`; `@import "tailwindcss" source(none)` with explicit `@source`s; element rules inside `@layer base`, since unlayered CSS beats every layer; class names in Rust as literal strings and every `class` attribute quoted, or the scanner misses them; no `style=` attributes, which the CSP blocks. A test fails on a rendered class that has no rule.
- **Assets are embedded in the binary** and served under a content hash (`?v=<hash>`, or in the file name) with `Cache-Control: public, max-age=31536000, immutable`; an unhashed URL gets an hour. Signed-in pages are `no-store`.
- **Every response carries:**
  ```
  Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:;
    font-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'
  X-Content-Type-Options: nosniff
  X-Frame-Options: DENY
  Referrer-Policy: no-referrer
  ```
  A page that loads Datastar adds `'unsafe-eval'` to `script-src` (its expressions compile with `Function`); an import map is allowed by its `'sha256-…'`. Never `'unsafe-inline'`. A test asserts the headers.
- **Cookies:** `__Host-` prefix, `Path=/; HttpOnly; Secure; SameSite=Lax`, holding an opaque random id stored hashed (§11). `Lax`, since `Strict` drops the cookie on the redirect back from sign-in. An app for plain HTTP on a LAN makes `Secure` a setting and then drops `__Host-`, which browsers refuse without `Secure`.
- **Accessibility:** a skip link, labelled controls, ARIA patterns where native elements fall short (a combobox with `aria-activedescendant`), a live region for results, `prefers-reduced-motion` honoured, text contrast of at least 4.5:1.
- **Language:** `lang="en-GB"`. Translations are gettext `.po`, with the English in the template as the msgid and CLDR plural rules (blit's `askama_gettext`).

## 9. APIs and agents

- **GraphQL is bounded and batched.** Depth is capped, since the graph has cycles; complexity is capped only where strangers can query it (koan: depth 12, complexity 2000). Elsewhere nothing is refused, since refusing an expensive but legitimate query leaves the caller guessing at a threshold it cannot see; instead each field's description states its cost ("This is the expensive field…", "the cost of a page is the page, not the library"), since agents choose fields by them. Lists below a list are batched by DataLoaders with a 10 ms window (the default 1 ms splits a wide selection), caching nothing past the request, and a test proves it (§14). Collections are connections: `nodes`, `pageInfo`, `first`/`after`, and `totalCount` computed only when selected. Long work is a job to poll.
- **MCP exposes the schema as two tools,** `<app>_schema` (the SDL, which can be sliced by type) and `<app>` (a query, with variables as a JSON string). Two tools cost a session almost nothing, and the schema carries the rest. A failure returns as a tool result the model can read, not a protocol error. stdio for local use; HTTP at `/mcp` behind OAuth 2.1 (§11) or radiosilence/mcp-gateway's token.
- **Server instructions are short and operational:** read the schema once, ask for everything in one query, what is expensive, which device or account is the default, and the safety rules below.
- **An action that sends, publishes, deletes or cannot be undone is PREVIEW, then CONFIRM.** The preview returns a token bound to a SHA-256 of the exact arguments: single use, consumed even on a mismatch, valid for 15 minutes. The instructions tell the agent to show the preview and confirm only on the user's explicit approval. A local, reversible write takes `preview: true` for a dry run instead.
- **Text from third parties is marked untrusted** in the instructions and the field descriptions (an email, a peer's message, a file's tags): data, never instructions. MCP access is capped below admin, since that text may carry an instruction, and refuses mutations that move files or change configuration whatever the role.
- **A protocol announces a capability before anyone uses it,** by name (koan's `koan*` Subsonic extensions). Clients gate features on the announcement, never on a server's name or version.

## 10. Apps and design

The look is specified in koan's [docs/design/koan-theme.md](https://github.com/radiosilence/koan/blob/main/docs/design/koan-theme.md) and built in [docs/design/koan-kit.md](https://github.com/radiosilence/koan/blob/main/docs/design/koan-kit.md), which give each rule its reason.

- **A GPUI app is built from `koan-kit`, so the look cannot drift.** Outside koan it is a git dependency pinned to a revision, with GPUI's revision beside it (sbr). Pages declare content; containers own spacing, size and colour. A page reaches GPUI only through `koan_kit::app`; anything else is a named `koan_kit::raw` import with its reason, counted by a ratchet. A look the kit lacks is added to the kit, not to the page.
- **The rules:** one prominent action per screen; shared edges; one spacing scale (2, 4, 8, 12, 16, 22, 32); hairline rules; square corners and no shadows; no glass, system materials or system chrome (the one blur is koan-kit's `Frost`, under a bar while content passes beneath); the wash (koan's blurred field of the playing record's sleeve) is the only gradient; dense tables for technical data.
- **Type:** Geist Mono only, in seven roles (`display`, `title`, `title-sm`, `body`, `control`, `meta`, `fine`), with tabular figures.
- **Colour:** tokens by role, each with its contrast documented; one accent (in koan the playing record's, tone-mapped in OKLCH to keep contrast; mint otherwise); Increase Contrast palettes at 7:1.
- **Motion says what happened.** A state shows at once and lingers as it goes; a change mid-flight starts from what is on screen; nothing moves for decoration. Durations and curves come from koan-kit's `Motion`: 80 ms a state, 120 ms a marker, 130 ms a state leaving, 200 ms a sleeve arriving, 250 ms a panel, 2 s a record's dissolve. Quick-out (`cubic-bezier(0.2, 0.9, 0.3, 1)`: no tail, delay or overshoot) for what arrives or settles; ease-in-out for what leaves or dissolves in place; a spring only for what a finger drags; linear only for progress. Motion is drawn from GPU keyframes, so it costs the main thread nothing. Reduce Motion is honoured whole.
- **A page off screen wakes nothing.** It is dropped and made again on arrival, or kept for state a visit must not lose (the queue's selection and scroll) and drawn only while shown.
- **Icons are our own:** one set of SVGs on a 24-unit grid, the stroke set by the type size, compiled into every platform's code.
- **Copy:** app words are lowercase (navigation, headings, buttons); names and people's text keep their case. Terse: an error says what happened and the exact fix. No exclamation marks, apologies or "please".
- **Accessibility lives in the components:** roles, names and states for screen readers (AccessKit), one keyboard model for every list (koan-kit's `list_keys`), focus kept inside sheets, 44 pt touch targets (48 dp on Android), the system's text size and Reduce Transparency honoured. The accessibility tree is built only while assistive technology is attached.
- **Platforms:** Swift 6, built in CI with `-emit-sil`, since the data-race checks run on SIL and a typecheck skips them; entitlements the minimum, each with its reason; releases signed with Developer ID and notarised; a self-signed development certificate keeps a dev build's identity, and so its permissions, across rebuilds.
- **A storybook shows every component in every state,** and tests render it dark and light.

## 11. Security

- **An exposed service states its bound:** its spec or threat model opens with the worst a hostile party can do (`docs/push-relay.md`), and a PR that moves the bound says so. A service is exposed when it holds credentials for others or parses untrusted bytes beyond HTTP requests (a wire protocol, files, mail).
- **An exposed service writes its threat model and audit down:** `docs/threat-model.md` (adversaries with evidence, what resists and what fails, gaps ranked by impact) and `docs/security-audit.md` (method, findings with their fixes, fuzz runs). sbr's are the templates.
- **Hostile input is capped in bytes before it is buffered,** and every length field is checked before use. Library defaults are inputs too: h2 accepts 16 MiB of headers, the `regex` crate compiles 10 MiB per set. Every parser of untrusted bytes has a fuzz target in `fuzz/`, and the audit records its runs.
- **A URL someone else chose is fetched over HTTPS only,** every resolved address public, the connection pinned to the address checked, and every redirect checked again (or none followed); no proxy, which would resolve the name itself.
- **A file someone else named never leaves its directory.** The path is canonicalised and checked for containment; Windows-reserved names and trailing dots are refused; an archive entry is extracted only when its name is enclosed and it is not a symlink, with the unpacked bytes capped; an upload is typed by its magic bytes and capped in size.
- **A parser of untrusted input not written in safe Rust runs in a child process** with a cleared environment, a private temp directory, a time limit and an output cap. That bounds a crash, not code execution, and the code says so.
- **Credentials:** passwords hashed with Argon2id, an unknown user paying the same cost. API keys, refresh tokens and session ids stored as SHA-256 and compared in constant time (`subtle`). Secrets at rest sealed with an AEAD that is safe under random nonces (XChaCha20-Poly1305, or AES-256-GCM-SIV in `aws-lc-rs`), with their owner (a user id, a key id) as associated data. Tokens never travel in query strings, which proxies log, unless a protocol we implement forces it (Subsonic).
- **Sign-in as an OIDC client:** authorization code with PKCE S256 even with a client secret, a one-shot server-side `state`, keeping only the subject. One session layer wraps the whole router, so a new route is protected by construction. A no-auth mode refuses a non-loopback bind; a development login exists only under `cfg(debug_assertions)`.
- **As an OAuth 2.1 server (for MCP):** PKCE S256 required, the issuer from configuration and never from request headers, exact redirect URIs, a consent page that names the redirect host, single-use codes valid for 60 s whose reuse revokes the grant, refresh tokens hashed.
- **Privacy:** keep the minimum (tubele knows a random cookie id and nothing else), and give everything kept about a person a stated lifetime and the code that deletes it.
- **Supply chain:** npm packages other than our own `@radiosilence/*` install only once a release is 7 days old (`minimumReleaseAge`), since a hijacked release is usually caught within days; build scripts run from an allowlist, jailed. Registry auth goes in a gitignored `.npmrc.auth`, never an `${ENV}` in a checked-in `.npmrc`.
- **Secrets come from the environment or mounted files in production,** and on a laptop from the password manager (1Password's `op`), exported to scratch for one command and deleted after. Never in a repository, PR, issue or log.

## 12. Performance

- **Every hot path (per frame, per packet, per row of a long list) has a budget a command checks.** koan's GUI walk fails a page whose 95th-percentile scrolling frame exceeds 6.9 ms (144 Hz); sbr's tunnel check fails a bulk transfer under 20 MB/s (`mise run test:apple-tunnel`).
- **Speed is measured before it is changed:** profiled first, then A against B back to back, reporting CPU-seconds per GB or per operation as well as throughput, which a shared machine's load distorts, and ranges rather than means.
- **Benchmarks are recorded as sbr's `BENCHMARKS.md` records them:** date, commit, machine and load, the reference and its version, ranges, the cause behind each result, what was not measured, and the command that reproduces it.
- **Free at rest:** nothing is drawn, decoded or analysed for nobody (an analyser with no reader parks, a hidden window draws nothing), and nothing polls: a wait ends at a computed deadline or an event. A timer that must run while idle (a keepalive through proxies, an upkeep sweep) says why.
- **No allocation per packet or chunk on a hot path;** buffers are reused. Round trips are batched (DataLoaders, chunked transactions, paged sync) and writes coalesced.
- **A change made for speed quotes before and after** in its PR and changelog entry.

## 13. Reliability

- **Every network call has a timeout:** a connect deadline, and a stall timeout per read rather than one for a whole transfer, which a large download rightly exceeds.
- **Retries back off exponentially to a cap, with jitter,** honour `Retry-After`, and reconnect at once when an app returns to the foreground.
- **A write another system must receive (a scrobble) goes through a durable outbox,** sent in batches and dated to when it happened, so it survives being offline.
- **A file is written to a temporary sibling, verified against its size or hash where known, then renamed** into place.
- **An upstream that refuses our credential trips a breaker,** since sending on with a refused credential is what gets a key looked at: stop sending, probe once a minute, and answer 503 with `Retry-After` meanwhile.
- **Every spawned task has a path that stops it,** and its resources are released when its owner closes, not when the next user arrives. Cancelling stops at the next safe point; it does not undo.

## 14. Testing

- **A test is named as a sentence stating the behaviour** (`an_unfinished_board_never_names_the_answer`), with no `test_` prefix. Assertion messages say what went wrong.
- **Unit tests are inline** (`#[cfg(test)] mod tests`); a suite longer than the code it tests moves to a sibling `tests.rs`; end-to-end tests live in `tests/`.
- **Tests run against real things:** in-process stand-ins over real sockets (`slsk-testserver`, a fake APNs gateway), reference implementations for interoperability (sing-box, Xray), the shipped binary for subprocess paths, a C program against a C ABI. Byte-exactness over coverage: at least 1 MB pushed through a transport and compared.
- **Every invariant `CLAUDE.md` or `ARCHITECTURE.md` states has a test named for it:** the answer never leaks, metrics name no device, the security headers are present, an idle page draws nothing, the N+1 stays fixed (fewer than 10 batches for 100 rows). A test asserts the property, not an incidental number.
- **A bug fix brings the test that fails without it,** or the PR says why it cannot.
- **Time is injected** (tokio's paused clock, or a `Clock` the code takes) and randomness is seeded, so a failure replays.
- **Tests are hermetic:** temporary directories, never the developer's configuration or data. An app with a config directory points it at a canary in `just check` (koan: `KOAN_CONFIG_DIR`) and fails if a test writes there.
- **Slow or environmental tests are `#[ignore = "reason"]`,** with the command that runs them in their doc comment (soak, load, a device on the network). A test that needs an external binary reads its path from an environment variable and prints that it skipped.
- **A long-running service or engine tests its resources:** after two identical rounds of work, tasks, file descriptors and heap must not have grown (a counting allocator, in a test binary of its own).
- **Apps are driven whole:** a headless smoke run over generated data fails on a panic, a hang or no progress; a walk renders every page dark and light; accessibility is tested by acting as a screen reader would.

## 15. Tooling

- **`mise.toml` pins every tool but Rust,** so a laptop and CI install the same things (`jdx/mise-action`); Rust tracks the latest stable (§5) with `rustfmt` and `clippy`. A tool whose version changes a lockfile's format (`aube`) is pinned exactly, and CI passes the same version.
- **`just` runs every task;** each recipe has a comment, which `just --list` shows. Every repository has `check` (everything CI checks short of the audit and release builds), `fast` (the checks that take seconds: `Cargo.lock` clean, `fmt --check`), `fmt`, `hooks` (installs lefthook) and `changelog`; and `release`, `css`, `versions` and `dev` where it releases, has CSS, has more than one version file, or has something to serve.
- **lefthook's pre-commit formats the staged files alone and restages them** (`rustfmt --edition 2024` on stdin: given a path it also rewrites the modules that file declares, and on stdin it cannot read the edition from `Cargo.toml`); its pre-push runs `just fast`. Lint and tests are CI's, so a push never waits on a build.
- **Agents get the same guard:** a `PreToolUse` hook on Bash in `.claude/settings.json` reads the command from its JSON input on stdin; on `git push` it runs the formatter and blocks the call (exit 2, the reason on stderr) if anything changed, rather than amending, since an agent's checkout may lack the git hooks.
- **A repository script needs nothing the repository does not already install:** Python 3's standard library, bash with `set -euo pipefail`, or, in a TypeScript repository, TypeScript run by `node` (§5). It opens with a comment saying what it does and how to call it.

## 16. CI/CD

One workflow for the change pipeline, `.github/workflows/ci-cd.yml`, in two halves, so CD ships exactly what CI built. A scheduled job that must not reach CD (blit's weekly audit), an updater, an App Store submission, an upload retried by hand and a site's deploy are workflows of their own.

- **`Checks` is the only required status.** It needs every CI job, runs `if: always()`, and fails on any result but `success` or `skipped`, so jobs can be added, renamed or skipped without touching branch protection.
- **`changes` decides which domains a diff touches** (`rust`, `web`, `package`, `docker`, `deps`…), each an allowlist of paths, and every job that builds or tests runs under an `if:` on its domain. A PR diffs against its base; a push diffs against the last `main` commit whose run passed, so a cancelled or failed run's commits are covered by the next. Markdown counts for no domain unless a build consumes it. A change under `.github/`, a manual or scheduled run, a release, or a diff that cannot be worked out runs everything. The cost of allowlists: a new top-level source directory is invisible until added. A new domain is an output of `changes`, its allowlist, an `if:` on its jobs, and those jobs in `Checks`' `needs`.
- **Least privilege:** the least the workflow needs at the top (`contents: read`, or `actions: write` alone for `cancel-closed-pr.yml`). A job widens only what it needs, and names `contents: read` too, since naming one permission drops the rest.
- **Every action is pinned to a full commit SHA** with its version as a comment, and every container image by digest, since a tag can be moved. Dependabot moves them, weekly and grouped; its PRs carry no changelog fragment.
- **Every job has `timeout-minutes` and every cargo command is `--locked`.** The default timeout is six hours.
- **Concurrency:** a PR's newer push cancels its older run; on `main`, a newer push cancels an older run that releases nothing; a release or manual run is never cancelled, since a cancelled release is half-published. `cancel-closed-pr.yml` stops a closed PR's runs, and a releasing run's `make-room` job (the only job in `ci-cd.yml` with `actions: write`) cancels PR runs and `main`'s earlier non-release runs, queued or running, so the release's builds find runners free.
- **Caches are saved from `main` only,** since a branch's cache helps no other branch and evicts `main`'s. Docker layer caches live in the registry. A repository that can go a week without a push also runs `ci-cd.yml` on `main` twice a week (`schedule:`), since GitHub drops caches unread for 7 days; a scheduled run delivers nothing.
- **The checks:** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, the advisory audit, `actionlint` with `shellcheck`, the changelog lint, the version agreement, and a freshness check on every committed generated file (§3), which fails with the command that regenerates it.
- **The audit blocks:** `cargo audit` (or `cargo deny check advisories`) at a pinned version, run directly so its exit code is the gate. A PR audits when a manifest, the lockfile or `.cargo/` changes; `main` audits every push and weekly, since advisories land against code that has not changed. A known advisory is ignored in `.cargo/audit.toml` (or `deny.toml`) with why it does not reach this build and when to remove it, since an ignore whose reason has lapsed hides a live advisory.
- **Linux binaries are built once per architecture on native runners** (`ubuntu-24.04`, `ubuntu-24.04-arm`): static musl where the binary needs nothing from the system (otherwise glibc, no newer than the image's), tarred to keep the executable bit. The image copies them in and the release attaches the same tarballs. Nothing compiles in a Dockerfile: Cargo's cache works per crate where a Docker layer is all or nothing, and no architecture runs emulated.
- **Image tags:** a PR builds the image without pushing, so the token that writes packages never runs a PR's Dockerfile. `main` pushes `:<sha>`; CD tags it `sha-<short>` and moves `main` onto it, forward only (the `org.opencontainers.image.revision` label and the compare API decide). A release adds `vX.Y.Z`, `vX.Y`, `vX` and `latest`. Retagging never rebuilds, and nothing deploys `:<sha>`.
- **A job that ships states the results it needs** (`!cancelled() && needs.<job>.result == 'success'`) rather than relying on the default, which skips it whenever anything upstream skipped.
- **Builds only a release needs** (a macOS release binary, signing) run only for release PRs and releases, since macOS runners are scarce; they start alongside the checks and gate publishing. What takes hours or is best effort (Apple's notary, a Windows zip) runs after the release and replaces or adds its asset; releases stay mutable for it, and anything that installs (a Homebrew cask) follows only the final asset.
- **CI runs what a contributor runs:** `just check`'s commands, with the same flags. A workflow supplies the environment (checkout, `mise`, credentials, caches); logic that cannot run on a laptop is in the wrong file.
- **Every job carries a comment saying why it exists and why it is shaped so.**
- **In a `run:` script `grep` is only a condition,** since a step runs under `bash -e` and a non-match exits 1. Under `shell: bash`, which adds `pipefail`, no `grep -q` sits on a pipe: its early exit fails the writer.

## 17. Packaging and deployment

- **The image is `FROM scratch` with the static binary.** It adds the CA bundle (from `gcr.io/distroless/static`) only if the binary reads the system trust store; a binary that trusts one upstream embeds its roots and needs no bundle (jaritanet's auth: `include_bytes!("../roots.pem")`). `gcr.io/distroless/cc-debian13:nonroot` when the binary must `dlopen` (glibc no newer than the base's); `debian:*-slim` only for runtime packages, with its reason.
- **The Dockerfile runs a numeric non-root `USER`,** 65532 by default (`runAsNonRoot` cannot check a name), or a uid of its own for a workload that owns data on a host. It has an exec-form `ENTRYPOINT` with an absolute path and the OCI labels `title`, `source` and `licenses` (CI adds `revision`). Configuration comes from the environment, or a file it names; HTTP listens on 8080. It runs with a read-only root, writing only to mounted volumes and `/tmp`. No `HEALTHCHECK` or `VOLUME`: the orchestrator owns both. Its header comment says how to build it by hand (`docker build -f Dockerfile dist`).
- **A service ships its deployment:** `deploy/pulumi/` publishes `@radiosilence/<app>-pulumi` to GitHub Packages at the app's version, deploying `ghcr.io/radiosilence/<app>:v${APP_VERSION}`.
  ```
  src/index.ts          re-exports
  src/<app>.ts          create<App>(provider, namespace, args, opts)
  src/<app>.schemas.ts  Zod: strictObject throughout, .default() leaves, .prefault({}) blocks, .refine() invariants; and its tests
  src/contract.ts       the shapes the deployer reads (Route, Deployed), declared here so the package imports nothing of the deployer's
  src/versions.ts       APP_VERSION and the images it pins
  ```
  The constructor takes `args` as `z.input<Schema>` and parses it; `opts` carries what the deployer owns (hostname, node, OIDC client, secrets). It returns `{…} satisfies Deployed`. It makes the Deployment, Services, NetworkPolicy, Secrets and volumes, and no Ingress: the deployer (jaritanet) owns hostnames, TLS, DNS, OIDC client secrets and generated keys. `@pulumi/*` and `zod` are peer dependencies, so the deployer's one copy is used.
- **The pod:** a Deployment. One replica with `strategy: Recreate` when it owns SQLite or a host directory, since a surge would put two writers on it. `runAsNonRoot` with the image's uid, `seccompProfile: RuntimeDefault`, `allowPrivilegeEscalation: false`, `readOnlyRootFilesystem: true`, every capability dropped (a file capability on the binary where it must bind a low port), `automountServiceAccountToken: false`, `enableServiceLinks: false`. Root only in an init container that chowns a host path, keeping only the capabilities that needs.
- **Resources:** request the measured working set (p90 over a week); a generous memory limit (at least 1 Gi) as a ceiling for a leak, not a budget; no CPU limit, since CPU throttling only shows up as stalls.
- **Network policy denies by default.** The HTTP port admits the ingress controller only (Cilium admits the kubelet's probes by itself; under another CNI the node must be admitted too, or the pod fails its probes and is killed); a port that trusts identity headers admits only the caller that sets them; metrics admit the scraper only. Egress reaches only what the service talks to: nothing, DNS, or public addresses with the private ranges excluded.
- **The orchestrator probes `/healthz`,** with a startup probe where start is slow. A hash of the configuration in a pod annotation restarts the pod when it changes. Secrets arrive by `secretKeyRef` or as files mounted `0440`. Metrics are announced by `prometheus.io/*` annotations on a declared port.
- **Data survives the deployment:** a volume that holds data is `Retain` with `retainOnDelete`, and clearing one is a deliberate manual act. A schema upgrade is first run by a job against a copy of the live database (koan's `check-db`), so a failure leaves the running version serving.
- **Sign-in:** the package takes `oidc: { issuer, clientId }` and returns its exact redirect URI (`https://<host>/auth/callback`); the deployer registers the client and generates its secret, so the redirect allowlist is derived, never typed. The app reads `PUBLIC_URL`, `OIDC_ISSUER`, `OIDC_CLIENT_ID` and `OIDC_CLIENT_SECRET`.
- **Updates:** jaritanet's updater moves a pin to the version of the latest GitHub release (neither draft nor prerelease), so that tag must name a version whose package and image exist (§18). A repository that releases several things tags `<app>-vX.Y.Z`; a site's image is tagged `sha-<short>` for every commit on `main`. A pin is a quoted literal, `key: "value"`, appearing once in its file, since the updater rewrites it as text. After a release, run the updater (`gh workflow run update-apps.yml -R radiosilence/jaritanet`) rather than waiting for its sweep.

## 18. Versions and releases

- **One version for each thing a repository releases,** shared by everything that ships with it, so one number names one build of all of it: the crates, the Pulumi package's `package.json`, `APP_VERSION` in `deploy/pulumi/src/versions.ts`, an Android `versionName`. SemVer. `just versions` fails when they disagree, on PRs as well as `main`.
- **`just release patch|minor|major|X.Y.Z` runs `scripts/release.sh`:** branch `release-vX.Y.Z` from `origin/main`, bump every version, `cargo update --workspace` (the workspace's own entries only), `changelog.py --release`, commit `release: vX.Y.Z`, push, and open the PR with the notes as its body. It refuses a dirty tree, an existing tag, a version not above `main`'s, or a branch with an open PR. The `Release` workflow does the same from GitHub with `RELEASE_TOKEN`, since a PR opened with the workflow's own token starts no workflows and could never go green.
- **Merging the release PR is the release.** CD publishes in dependency order: the image's version tags, then the packages (Pulumi, crates.io), then the GitHub release with the binaries attached and the changelog section as notes. Consumers poll releases, so nothing may name an artefact that does not exist yet (the exceptions run after it, §16).
- **Every publish step is idempotent and fails rather than guesses.** It skips what is already out; only an HTTP 404 means "not out", and any other error fails the job, since a guess releases twice or never. A failed release is retried with `gh workflow run ci-cd.yml` on `main`, which releases whatever version is not out.
- **Before a release, the docs are checked against what merged since the last one.**
- **Tags are never pushed by hand.** CI tags the commit it built.

## 19. Documentation and writing

| File | Holds |
|---|---|
| `CLAUDE.md` | What the project is, what it ships and who consumes it; then *Git*, *Changelog*, *Releasing*, *CI/CD*, *Check*; then its invariants and where things live. The whole brief for an agent with no other context. |
| `README.md` | What it is; why it exists (what is wrong with the alternatives); running it, with a configuration table (`Variable \| Default \| Meaning`); how it is put together (a directory map); security notes; licence and attribution. |
| `ARCHITECTURE.md` | The contract (§3 says when): the crates and what may depend on what, the rules and their reasons, how to add a protocol, page or service, the performance rules. |
| `docs/` | The user guide and reference. A generated reference is written by a tool and checked with `--check`; a reference agents read is also one plain-text file. |
| `HANDOVER.md` | Only when work moves between sessions or machines: the state, the user's real setup, next steps in priority order, the rules the user set, gotchas, a map. |

- **British English, plain declaratives, no marketing:** no "seamless", "robust" or "powerful".
- **Comments say why; the code says what.** A comment records the constraint, the measurement or the incident behind the line (`#933`, "measured at 168 s against 15 s"). A docblock describes its parameters in prose, without `@param`.
- **Design docs give each rule its *why*,** in the form `- **Rule.** Detail. *Why:* reason.`

## 20. Changelog

- **`CHANGELOG.md` is generated, never edited.** Each PR but Dependabot's (§16) adds one fragment, `changelog.d/unreleased/<section>/<pr>-<slug>.md` (`<slug>.md` for a change pushed straight to `main`), holding one line: `- **Lead.** Detail.` Sections: `breaking`, `added`, `security`, `changed`, `removed`, `fixed`, `tests`, `known-issues`, `internal`. Two open PRs never conflict over the changelog.
- **`scripts/changelog.py` and `scripts/test_changelog.py` are copied verbatim from koan,** never forked, so a fix reaches every repository. `--lint` runs on every PR, `--check` when `CHANGELOG.md` is written; `--release X.Y.Z` cuts a version and `--stdout` previews.
- **The lead is the outcome as a user sees it; the detail says what was wrong and what happens now,** with any step a user must take. Internal changes go in `internal`, written the same way.

  ```markdown
  - **The container runs as non-root.** The MCP server holds a mail account's token for each request and parses mail from the internet, so the image now builds on distroless's `nonroot` variant and runs as uid 65532 rather than root.
  ```

## 21. Git and pull requests

- **Every change is a squash-merged PR.** A repository may let small changes go straight to `main` if its `CLAUDE.md` says so; workflow changes and releases still go through a PR.
- **`main` is merged into a branch; a pushed branch is never rebased or force-pushed.** Squashing makes branch history irrelevant, and a rewrite discards what reviewers and CI saw.
- **Commits are signed,** so every commit's author is proven.
- **The title says what the change does,** in sentence case, prefixed by the area when there is one: `Run the container as non-root`, `koan-kit: Meter, and Chart's scrolling time series`. No `feat:` or `fix:`; the only fixed forms are `release: vX.Y.Z`, the updater's `chore: update <x> to <v>` and Dependabot's `build(deps): bump …`. The title becomes the squash commit.
- **The body says what changed and why.** A fix gives *Cause*, *Change* and *Verify* (the steps that show it working) and `Closes #N`.
- **One concern per PR, complete:** code, tests, docs and the changelog fragment together. A visible change shows before and after screenshots.
- **An issue states the symptom,** where and since when it is seen, and the candidate causes with their PRs.

## 22. Working on a project

- **Read `CLAUDE.md` and `ARCHITECTURE.md` first.** Trust the code over plans and older docs, and fix the doc that is wrong.
- **Fix the cause.** Understand a bug before changing anything.
- **Zero warnings:** compiler, clippy and linters alike, fixed when they appear.
- **Be critical, not validating.** Say when an idea is wrong, and why.
- **Parallel agents each work in their own worktree and branch.** A manager coordinates and hands each one a self-contained task: the branch, the change, the command that verifies it, and what to report. Remove a worktree and its `target/` (gigabytes each) once its PR merges, keep at most one simulator booted, never drive or capture the user's screen, and stop only processes you started.

## 23. Audit checklist

One line per section that can be checked. Run what can be run, read the rest, and report each gap as rule (§) → file → fix.

- [ ] **§2** The stack's choices, and nothing from its *Not used* list.
- [ ] **§3** Every file §3 lists for this kind of project; `LICENSE` matching the manifests; generated files committed only where §3 allows.
- [ ] **§4** One writer per invariant; uids, not row ids, leave the database; everything input drives bounded; no telemetry; configuration read and validated once at start.
- [ ] **§5** Edition 2024, resolver 3, shared dependencies in the workspace, the release profile; no bare `#[allow]`, no `unwrap` outside tests, a `SAFETY` comment on every `unsafe`, no derived `Debug` on a struct holding a secret. Every tsconfig sets the six flags (the build one, `noEmit` off); private TypeScript runs from source.
- [ ] **§6** The pragmas, `BEGIN IMMEDIATE` for writes, numbered migrations, a newer schema refused, statements off async workers.
- [ ] **§7** Listeners split by trust; the perimeter layers; SIGTERM handled with a drain; `/healthz`; `Host` and `Origin` checked on a local server; `tracing` to stderr; nothing identifying in logs or metric labels.
- [ ] **§8** The CSP without `'unsafe-inline'` (`'unsafe-eval'` only with Datastar) and the other headers, with a test; every state change a POST behind the Datastar header or an `Origin` check; `|safe`, `style=` and `data-show` only as §8 allows; assets fingerprinted; `__Host-` cookies.
- [ ] **§9** GraphQL depth capped and lists batched; MCP as two tools; PREVIEW → CONFIRM on anything outward or irreversible; third-party text marked untrusted; MCP below admin.
- [ ] **§10** Built from `koan-kit` under its ratchet; the design rules; accessibility tested; a storybook.
- [ ] **§11** An exposed service's bound, threat model, audit and fuzz targets; fetched URLs and named paths guarded; credentials hashed or sealed as §11 says.
- [ ] **§12** Budgets with the commands that check them; nothing running at rest without a stated reason.
- [ ] **§13** A timeout on every network call; backoff on retries; an outbox for what others must receive; atomic writes.
- [ ] **§14** Tests named as sentences; every documented invariant tested; tests hermetic; every `#[ignore]` with a reason.
- [ ] **§15** `mise.toml` pinning every tool but Rust; the `just` recipes, with `just check` passing; lefthook formatting before a commit; the agents' push guard.
- [ ] **§16** One `ci-cd.yml` ending in `Checks`, with every source directory in a `changes` allowlist; least privilege; actions pinned by SHA and images by digest; `timeout-minutes` everywhere; cargo `--locked`; a blocking audit, every ignore with its reason; `cancel-closed-pr.yml` and Dependabot.
- [ ] **§17** The image as §17 describes it, its bases digest-pinned (§16); the Pulumi package's layout; the pod baseline; no CPU limit; network policy denying by default; data volumes retained.
- [ ] **§18** One version per release, checked on PRs; `release.sh` bumping every version; publishing in order, each step idempotent.
- [ ] **§19** `CLAUDE.md`, `README.md` and the docs §3 requires, and every command, path and setting they name existing.
- [ ] **§20** `changelog.py` and `test_changelog.py` byte-identical to koan's; fragments well formed.
- [ ] **§21** Squash merges, signed commits, titles saying what the change does.
