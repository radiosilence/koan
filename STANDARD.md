# The kōan standard

How radiosilence projects are built: what we use, how it fits together, and why. Give it to a person or an agent starting a project or auditing one.

- **Building:** follow it. Where it is silent, copy the reference implementation closest to what you are building.
- **Auditing:** check the project against every section; §23 is the checklist. Report each gap as the rule, the file, and the fix.
- **Departing:** any rule may be broken if the reason is written where the break is (a comment, the project's `CLAUDE.md`, the PR). A break without a reason is a defect.
- **Changing it:** a convention that applies across projects changes here first, in its own PR.

| Building | Start from |
|---|---|
| A small server-rendered web app | `tubele-rs` |
| A service with a web UI, OIDC sign-in, GraphQL, MCP and metrics | `slsk-mcp` |
| A public service that holds a credential for others, with a client crate | `koan-push-relay` |
| A CLI and MCP server over someone else's API | `fastmail-cli` |
| An engine that parses hostile input; benchmarks, fuzzing, a threat model | `sbr` |
| An app on every platform over a Rust core; the design system | `koan` (`koan-core`, `koan-runtime`, `koan-gui`, `koan-kit`) |
| A static site | `blit` |
| How it all deploys (context, not a template) | `jaritanet` |

## 1. Values

In order of precedence when they conflict:

1. **Never lose, leak or corrupt the user's data.** Correctness and security come before speed and features.
2. **Work where it is used, against what it talks to.** Run on the real device against the real servers first; optimise second.
3. **Fast and frugal, as measured.** Nothing runs at rest, nothing blocks a UI, every buffer has a cap, and a claim about speed carries a number.
4. **Simple, and one of each.** One language for what runs, one TLS stack, one version per repository, one pipeline, one required check. Platform features before libraries, libraries before frameworks. An abstraction waits for its third use.
5. **Owned end to end.** Nothing phones home or leans on a service the user did not configure. Our own design system, icons and forks; fixes proposed upstream.

Always:

- **Make the wrong thing impossible, or loud:** types, one owner per invariant, strict parsing that names what it refused, tests named for the invariant they guard.
- **Fail closed, never guess.** Bad config stops startup; an unknown diff runs every check; an ambiguous answer from an API fails the release.
- **Automate the discipline.** A rule that matters has a check: a lint, a test, a ratchet, a freshness check.
- **Say why.** Every rule, constant, comment, commit and PR carries its reason.
- **Details are the product.** Accessibility, motion, copy and alignment are designed, not left to defaults.

## 2. Stack

| Need | Use | Why |
|---|---|---|
| Everything that runs | Rust, edition 2024, latest stable | One language from an audio callback to a web server; static binaries; the compiler holds invariants |
| Async | tokio | What axum, reqwest and rmcp are built on |
| HTTP server | axum + tower-http | Limits, compression and panic catching as layers |
| HTTP client | reqwest, `default-features = false`, `rustls` | One TLS stack; no OpenSSL |
| Crypto | rustls on aws-lc-rs; RustCrypto (`argon2`, `chacha20poly1305`, `sha2`, `hmac`, `subtle`) | Hardware AES and ChaCha; reviewed primitives |
| Storage | SQLite through rusqlite (`bundled`) | One file, no server; sqlx cannot share a binary with rusqlite, which koan and sift use |
| HTML | askama | A template is bound to a struct at compile time |
| Interactivity | Datastar over SSE, vendored | The server keeps the state; about 13 KB |
| CSS | Tailwind v4 standalone CLI + `@radiosilence/theme` | Shared tokens; no Node in the build |
| Type | Geist Mono | One face everywhere |
| API | GraphQL (async-graphql) | One typed schema for the UI, scripts and agents |
| Agents | MCP (rmcp), exposing the GraphQL schema | Two tools cost a session almost nothing |
| CLI | clap (derive) + clap_complete | The interface is a type |
| Errors | thiserror in libraries, anyhow in binaries | Callers match one; people read the other |
| Logs | tracing + tracing-subscriber | Structured fields, filtered per crate |
| Time, ids | jiff; uuid v7 | chrono is unmaintained; v7 sorts by creation |
| App config | serde + figment; toml_edit for writes | Layered files and environment; edits keep comments |
| Desktop and mobile UI | GPUI through `koan-kit`; SwiftUI only where GPUI cannot go (tvOS); Compose on Android | One look, drawn on the GPU |
| Native bindings | uniffi for Swift and Kotlin; a C ABI where only C will do, its header checked by a test | Bindings the compiler checks |
| Static sites | A Rust generator or hand-written HTML, served by nano-web | The browser gets HTML, CSS and a font |
| Packaging | Static musl binary in a `scratch` image on GHCR | Nothing in the image but the binary |
| Deployment | A Pulumi component package per service (TypeScript, Zod), deployed by jaritanet to k3s | The service ships how it is run |
| Tasks, tools, hooks | just, mise, lefthook | The same commands on a laptop and in CI |
| Lint | clippy, rustfmt, actionlint + shellcheck, oxlint + oxfmt | Machines settle what review would otherwise argue about |
| TypeScript | Node 24 type stripping, aube, tsc, vitest | Private code needs no build step |
| Repo scripts | Python 3 standard library, bash | Nothing to install |

**Not used:** SPA frameworks or bundlers; OpenSSL or native-tls; ORMs; chrono in new code; `lazy_static`/`once_cell` (std has both); compiling inside a Dockerfile.

## 3. Repository

```
CLAUDE.md  README.md  LICENSE  CHANGELOG.md
Cargo.toml  Cargo.lock              one package at the root, or crates/<name>/ when there are several
justfile  mise.toml  lefthook.yml  .cargo/audit.toml
.github/workflows/  ci-cd.yml  release.yml  cancel-closed-pr.yml
.github/dependabot.yml
changelog.d/  README.md  unreleased/
scripts/  changelog.py  test_changelog.py  release.sh
Dockerfile  deploy/pulumi/                        a service
migrations/  templates/  styles/  assets/         a web UI
ARCHITECTURE.md  docs/                            when there are threads, protocols or users to teach
```

- **Crates are layered by what they may touch:** a protocol crate with no I/O, an engine over it, the app over that (`slsk-proto` → `slsk-engine` → `slsk-mcp`; `sbr-config` → `sbr-core` → `sbr-engine`). A test double is a crate of its own (`slsk-testserver`).
- **`LICENSE` matches the manifests' `license`.**
- **Not committed:** generated Xcode projects (XcodeGen), uniffi bindings, `dist/`. **Committed with a freshness check** (§15): what a build embeds or a reader needs without a toolchain, such as compiled CSS, the theme's font, icon code and API references.

## 4. Architecture

- **The core owns the logic; front ends are shims.** A capability one front end needs goes into the core, so every front end has it. Crate boundaries enforce the direction: `koan-core` knows no UI, `koan-kit` knows no engine.
- **A UI that can link the core does,** in process. A daemon, a port and an auth surface buy nothing. A network API is for clients that cannot link: browsers, remotes, agents.
- **Nothing blocks a UI thread or an async worker.** A call that can block is async at the boundary. SQLite and file I/O run on a blocking pool or a named thread. Commands that must keep their order go down one ordered lane, and they name their target (an album, not its tracks) so no client reads before it sends.
- **State goes out as whole snapshots, never deltas,** in slices cut by rate of change. Each slice has a sequence number and each client a cursor, so falling behind loses intermediate values, never the fact of a change. A snapshot cannot be applied wrongly. A continuous value goes out as an anchor (a position and when) that clients count on from. A value that changes every frame bypasses state for a layer or the GPU.
- **Store facts, derive the rest.** Status is computed from what happened, not kept beside it. An event is a reason to look again, never a reading.
- **Identify by id, never by position or name.** Public ids are UUIDv7 `uid`s; row ids stay inside the database, which reuses them.
- **One module owns each invariant** and is the only writer of the columns it covers (`db/queries/sources.rs` alone decides which track a file is).
- **The server is authoritative.** Anything a client could cheat with or leak stays on the server until the client has earned it. In tubele the answer reaches a template only through a view that exists for a finished board.
- **Everything input can grow is bounded:** queues, tables, caches and buffers, capped in bytes as well as entries. Bounded channels and shedding, never an unbounded queue.
- **Work runs only while something needs it.** An analyser with no reader parks; a hidden window draws nothing; an idle connection holds no timer.
- **Contact only what the configuration names.** No built-in URLs for probes, updates or telemetry.
- **Read external formats strictly.** An unknown field is refused with its path, never ignored.
- **Configuration:** a service reads environment variables once at start into a typed struct, and a missing or malformed value stops it with one line naming the variable. An app people configure by hand layers defaults → `config.toml` → `config.local.toml` → `<APP>_*` variables (figment, `__` for nesting). One function writes config: it routes secrets and machine-specific keys to the local file and leaves comments and untouched keys alone.

## 5. Rust

- **Edition 2024, resolver 3, the latest stable toolchain** from `mise.toml`; no `rust-version`.
- **A dependency two crates share is declared once** in `[workspace.dependencies]`, so versions and features cannot drift.
- **Take only what you use:** `default-features = false` with the features named. A dependency's line says why it is there when that is not obvious, and why it is pinned exactly when it is.
- **Release profile:** `strip = true`, `lto = "thin"`, `codegen-units = 1`. Depart with a measurement: fat LTO where throughput is the product (sbr), `opt-level = "s"` with `panic = "abort"` for a tiny service whose recovery is a restart, more codegen units where build time was measured to buy nothing.
- **Clippy's default lints are errors** (`-D warnings`). A suppression is `#[expect(lint, reason = "…")]`, never a bare `#[allow]`.
- **Errors:** libraries return `thiserror` enums whose variant docs say what the caller should do ("worth retrying", "asking again changes nothing"), `#[non_exhaustive]` when published. Binaries use `anyhow` with short lowercase context (`.context("binding {addr}")`). A failure at startup prints one legible line and exits 1; it does not panic.
- **No `unwrap` outside tests.** `expect` names the invariant that makes it safe (`expect("checked above")`), not what failed.
- **Panics unwind and are caught at boundaries:** `CatchPanicLayer` in a server, `resume_unwind` from a blocking task, `catch_unwind` at every FFI entry point, a restarting loop where a packet loop must survive. `panic = "abort"` only where the process is the unit of recovery.
- **Locks are `parking_lot`, and none is held across `.await`.** A real-time path (an audio callback) neither allocates nor locks: atomics and lock-free rings only.
- **`unsafe` only for FFI and platform calls,** each block with a `// SAFETY:` comment. A crate with none has `#![forbid(unsafe_code)]`.
- **Secrets are not `Debug`.** A struct holding one does not derive it; an error about one carries a `&'static str`, so the secret cannot be formatted into it.
- **Statics are `std::sync::LazyLock` and `OnceLock`.**
- **Every file opens with a `//!` doc** saying what it holds and why it is shaped so. A constant carries the reason for its value (`KEEP_IDLE`, `DRAIN`, `BUSY_TIMEOUT`).
- **rustfmt defaults.**

## 6. Data

- **SQLite, one file, rusqlite `bundled`.** WAL; `synchronous = NORMAL`, or `FULL` where an acknowledgement promises durability; `busy_timeout` of at least 5 s; a write transaction opens with `BEGIN IMMEDIATE`, so it waits for the lock up front instead of failing to upgrade a read.
- **Statements never run on an async worker:** a small pool of kept connections (`KEEP_IDLE`), used through the blocking pool (`Db::run`) or the core's own threads.
- **Migrations are `migrations/NNNN_slug.sql`,** embedded with `include_str!`, counted by `PRAGMA user_version`, each in one transaction. A build refuses a database newer than it knows. Upgrades are tested from old fixtures.
- **SQL lives in documented constants** that say why each query is shaped so ("two statements rather than `?1 IS NULL OR status = ?1`, which no index can answer").
- **Instants are INTEGER Unix milliseconds, UTC;** one unit per schema.
- **Automated deletion is guarded:** a rescan that would remove more than a fifth of a large folder is refused, and an I/O error never counts as "gone".
- **Tests use a temporary file, not `:memory:`,** so every pooled connection sees the same data, and remove it after.

## 7. Services

- **Listeners are split by trust:** the public UI; an internal API that believes credential headers, reachable only from its named caller; metrics on a port of their own.
- **Every HTTP server has a perimeter,** innermost first: `CatchPanicLayer` (inside the timeout), a request timeout, a body limit, then `concurrency_limit` behind `load_shed`, which sheds rather than queues. Add a header-read timeout and a connection cap: `axum::serve` sets neither. Size them per endpoint (koan's GraphQL: 30 s, 2 MiB, 64 in flight; the push relay: 15 s, 8 KiB).
- **PID 1 handles SIGTERM and SIGINT.** Nothing installs a default handler in a `scratch` image, so an ignored SIGTERM makes every rollout wait out the grace period. On a signal: keep serving for a short drain (5 s) while the ingress drops the endpoint, stop accepting, and let in-flight work finish within the grace period.
- **`/healthz` answers `ok`** for liveness and startup probes.
- **Errors log their cause in full and tell the client almost nothing.** One `AppError` maps to a status; 5xx logs at `error`, 4xx at `debug`; only a bad request's own message goes back. Upstream text, SQL and paths never reach a client.
- **A server bound to localhost or a LAN refuses a foreign `Host`** (DNS rebinding) **and a foreign `Origin`.** CORS is off unless a configured allowlist names the origin; never `*`.
- **Logs:** `tracing` text to stderr, filtered by `RUST_LOG`, defaulting to `<crate>=info,warn`. No request logs at `info`. Never logged: credentials, tokens, URLs carrying either, anything that identifies a person or a device. A library whose debug output would log one is held at `warn`.
- **Metrics:** Prometheus text written by hand, named `<app>_<noun>_total` with HELP and TYPE lines. Label values come only from fixed enums, so cardinality is bounded by construction. Gauges are computed at scrape time from the source of truth, not counted alongside it.

## 8. Web UI

- **The server renders HTML (askama) and Datastar patches it over SSE;** the browser holds no state that matters. `templates/` holds a shell, `pages/`, and a directory of fragments per area.
- **Script is the exception.** Each file says what it does that HTML and Datastar cannot. Small scripts are plain ES modules; larger ones are erasable-only TypeScript, stripped at build (oxc in `build.rs`) and typechecked in CI. No framework, no bundler, no `node_modules` at run time. A static site ships none.
- **Native elements before script:** `<dialog>` opened by `commandfor`/`command="show-modal"` and closed by `closedby="any"` or `<form method="dialog">`; `popover`; `autofocus`.
- **Links have real `href`s,** and what can work without script does.
- **An element that starts hidden uses `hidden` with `data-attr:hidden`,** never `data-show`, so it does not flash before Datastar loads.
- **No value goes inside a Datastar expression.** Put it in a `data-*` attribute or `href` and read it with `el.dataset`; only ids and numbers the server made are interpolated.
- **A state-changing request is a POST** carrying Datastar's `datastar-request` header, which a cross-site form cannot set. A plain form checks `Sec-Fetch-Site`, or that `Origin` matches `Host`. Nothing changes state on GET, sign-out included.
- **Escaping stays on.** Markup the server built is a type (`Html`), made where it is built; templates use no `|safe`.
- **Styles:** Tailwind utilities in the templates over `@radiosilence/theme`; `@import "tailwindcss" source(none)` with explicit `@source`s; element rules inside `@layer base` (unlayered CSS beats every layer); class names in Rust as literal strings so the scanner finds them; every `class` attribute quoted; no `style=` attributes. The compiled CSS and the theme's font are committed side by side (the binary embeds them) and CI fails when they are stale. A test fails on a rendered class that has no rule.
- **Assets are embedded in the binary** and served under a content hash (`?v=<hash>`, or in the file name) with `Cache-Control: public, max-age=31536000, immutable`; an unhashed URL gets an hour. Fonts included. Signed-in pages are `no-store`.
- **Every response carries:**
  ```
  Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:;
    font-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'
  X-Content-Type-Options: nosniff
  X-Frame-Options: DENY
  Referrer-Policy: no-referrer
  ```
  A page that loads Datastar adds `'unsafe-eval'` to `script-src` (its expressions compile with `Function`); an import map is allowed by its `'sha256-…'`. Never `'unsafe-inline'`. A test asserts the headers.
- **Cookies:** `__Host-` prefix, `Path=/; HttpOnly; Secure; SameSite=Lax`, holding an opaque random id; the server stores its SHA-256. An app meant for plain HTTP on a LAN makes `Secure` a setting.
- **Accessibility:** a skip link, labelled controls, ARIA patterns where native elements fall short (a combobox with `aria-activedescendant`), a live region for results, `prefers-reduced-motion` honoured, text contrast of at least 4.5:1.
- **Language:** `lang="en-GB"`. Translations are gettext `.po`, with the English in the template as the msgid and CLDR plural rules (blit's `askama_gettext`).

## 9. APIs and agents

- **GraphQL is the API:** one schema for the web UI's scripts, other clients and agents. Depth and complexity are limited (koan: 12 and 2000). Lists below a list are batched by one schema-wide DataLoader with caching off and a short window (10 ms); a test asserts the batching as a property ("fewer than 10 batches for 100 rows"), not an exact count. Collections are connections: `nodes`, `pageInfo`, `first`/`after`, and `totalCount` computed only when selected. Long work is a job to poll.
- **Descriptions state cost:** "This is the expensive field…", "the cost of a page is the page, not the library".
- **The client gets generic errors** ("internal error"), except domain errors it can act on.
- **MCP exposes the schema as two tools,** `<app>_schema` (the SDL, which can be sliced by type) and `<app>` (a query, with variables as a JSON string). Two tools cost a session almost nothing, and the schema carries the rest. A failure returns as a tool result with an error, not a protocol error. stdio for local use; HTTP at `/mcp` behind OAuth 2.1 or the gateway's token.
- **Server instructions are short and operational:** read the schema once, ask for everything in one query, what is expensive, which device or account is the default, and the safety rules below.
- **An action that sends, publishes, deletes or cannot be undone is PREVIEW, then CONFIRM.** The preview returns a token bound to a SHA-256 of the exact arguments: single use, consumed even on a mismatch, valid for 15 minutes. The instructions tell the agent to show the preview and confirm only on the user's explicit approval. A local, reversible write takes `preview: true` for a dry run instead.
- **Text from third parties is marked untrusted** in the instructions and the field descriptions (an email, a peer's message, a file's tags): data to act on, never instructions to follow. MCP access is capped below admin, since that text may carry an instruction.
- **A protocol announces a capability before anyone uses it,** by name (koan's `koan*` Subsonic extensions). Clients gate features on the announcement, never on a server's name or version.

## 10. Security

- **State the bound.** A service that holds a credential for others or faces hostile input says what the worst a hostile party can do is (`docs/push-relay.md` opens with it). A PR that moves the bound says so.
- **Write the threat model and the audit down** for anything facing hostile networks or holding credentials. `docs/threat-model.md`: adversaries with evidence and confidence, a capability matrix with a reason per row, gaps ranked by impact (struck through when fixed), out of scope. `docs/security-audit.md`: scope, method, fuzzing table, findings by severity with the fixing commit or issue, what was checked and found sound, dated follow-ups. sbr's are the templates.
- **Hostile input is capped in bytes before it is buffered,** and every length field is checked before use. Library defaults are inputs too: h2 accepts 16 MiB of headers, the regex crate compiles 10 MiB per set. Every parser of untrusted bytes has a fuzz target in `fuzz/`, and the audit records its runs.
- **A URL someone else chose** is fetched over https only, every resolved address public, the connection pinned to the address checked, and every redirect checked again (or none followed), with no proxy.
- **A file someone else named** never leaves its directory: canonicalise and check containment; refuse Windows-reserved names and trailing dots; extract an archive entry only when its name is enclosed and it is not a symlink, capping the bytes unpacked; type uploads by magic bytes and cap their size.
- **A risky parser runs in a child process** with a cleared environment, a private temp directory, a time limit and an output cap. That bounds a crash, not code execution, and the code says so.
- **Credentials:** passwords hashed with Argon2id, with an unknown user paying the same cost. API keys, refresh tokens and session ids stored as SHA-256 and compared in constant time (`subtle`). Secrets at rest sealed with XChaCha20-Poly1305. Tokens never travel in query strings, which proxies log.
- **Sign-in as an OIDC client:** authorization code with PKCE S256 even with a client secret, a one-shot server-side `state`, keeping only the subject. One session layer wraps the whole router, so a new route is protected by construction. A no-auth mode refuses a non-loopback bind; a development login exists only under `cfg(debug_assertions)`.
- **As an OAuth 2.1 server** (for MCP): PKCE S256 required, the issuer from configuration and never from request headers, exact redirect URIs, a consent page that names the redirect host, single-use codes valid for 60 s whose reuse revokes the grant, refresh tokens hashed.
- **Least privilege:** read-only CI tokens (§15), non-root images holding only the binary (§17), network policies that admit named callers only, an MCP role below admin, separate listeners for separate trust.
- **Privacy:** keep the minimum (tubele knows a random cookie id and nothing else), bound how long anything is kept, and log nothing that identifies a person or a device.
- **Supply chain:** the advisory audit blocks CI (§15). Packages install only once a release is 7 days old (`minimumReleaseAge`), our own `@radiosilence/*` excepted; build scripts run from an allowlist, jailed. Registry auth goes in a gitignored `.npmrc.auth`, never an `${ENV}` in a checked-in `.npmrc`.
- **Secrets** come from the environment or mounted files in production and from the password manager (1Password's `op`) on a laptop. Never in a repository, PR, issue or log.

## 11. Performance

- **Every hot path has a number and a tool that measures it.** koan's GUI walk fails a page whose 95th-percentile scrolling frame exceeds 6.9 ms (144 Hz); at rest an app draws nothing and wakes nothing; sbr's iOS tunnel must fit in about 50 MB.
- **Measure before changing.** Profile first; compare A against B back to back; report CPU-seconds per GB or per operation as well as throughput ("throughput on a shared box lies"); give ranges, not means.
- **Record benchmarks as `BENCHMARKS.md` does:** date, commit, machine and its load, the reference and its version, the method, min–max figures, the cause behind each result, what was not measured, and the command that reproduces it.
- **Free at rest:** deadlines, never polling timers; nothing is drawn, decoded or analysed for nobody.
- **No allocation per packet or chunk on a hot path;** buffers are reused. Round trips are batched (DataLoaders, chunked transactions, paged sync) and writes coalesced.
- **UI:** the main thread only swaps in finished values; decoding, I/O and laying out long lists happen elsewhere. Motion is drawn by the GPU from keyframes. Pages off screen are rebuilt when shown, not kept mounted.
- **A change made for speed quotes before and after** in its PR and changelog entry.

## 12. Reliability

- **Every network call has a timeout:** a connect deadline, and a stall timeout per read rather than one for a whole transfer.
- **Retries back off exponentially to a cap, with jitter,** honour `Retry-After`, and reconnect at once when an app returns to the foreground.
- **Anything owed to another system goes through a durable outbox,** sent in batches and dated to when it happened.
- **A file is written to a temporary sibling, checked, then renamed** into place.
- **An upstream that refuses our credential trips a breaker:** stop sending, probe once a minute, answer 503 with `Retry-After` meanwhile.
- **Every spawned task has a path that stops it,** and resources are released when their owner closes, not when the next user arrives. Cancelling stops at the next safe point; it does not undo.

## 13. Testing

- **A test is named as a sentence stating the behaviour** (`an_unfinished_board_never_names_the_answer`), with no `test_` prefix. Assertion messages say what went wrong.
- **Unit tests are inline** (`#[cfg(test)] mod tests`); a large suite moves to a sibling `tests.rs`; end-to-end tests live in `tests/`.
- **Test against real things:** in-process stand-ins over real sockets (`slsk-testserver`, a fake APNs gateway), reference implementations for interoperability (sing-box, Xray), the shipped binary for subprocess paths, a C program against a C ABI. Byte-exactness over coverage: push at least 1 MB through a transport and compare.
- **Every invariant that matters has a test named for it:** the answer never leaks, metrics name no device, the security headers are present, an idle page draws nothing, the N+1 stays fixed. Assert the property, not an incidental number.
- **A bug fix brings the test that fails without it,** or the PR says why it cannot.
- **Time is injected** (tokio's paused clock, or a `Clock` the code takes) and randomness is seeded, so a failure replays.
- **Tests are hermetic:** temporary directories, never the developer's configuration or data. koan's `just check` fails if a test writes to a canary config directory.
- **Slow or environmental tests are `#[ignore = "reason"]`,** with the command that runs them in their doc comment (soak, load, a device on the network). A test that needs an external binary reads its path from an environment variable and prints that it skipped.
- **Resources are tested:** after two identical rounds of work, tasks, file descriptors and heap must not have grown (a counting allocator, in a test binary of its own).
- **Apps are driven whole:** a headless smoke run over generated data fails on a panic, a hang or no progress; a walk renders every page dark and light; accessibility is tested by acting as a screen reader would.

## 14. Apps and design

The look is specified in [docs/design/koan-theme.md](docs/design/koan-theme.md) and built in [docs/design/koan-kit.md](docs/design/koan-kit.md). This is the summary.

- **A GPUI app is built from `koan-kit`.** Pages declare content; containers own spacing, size and colour. A page reaches GPUI only through `koan_kit::app`; anything else is a named `koan_kit::raw` import with its reason, counted by a ratchet whose counts only fall. A look the kit lacks is added to the kit, not the page.
- **The rules,** each with its *why* in koan-theme.md: one prominent action per screen; shared edges; one spacing scale (4, 8, 12, 16, 22, 32); hairline rules; square corners and no shadows; no glass, materials, blur or system chrome; the wash is the only gradient; dense tables for technical data. A screen that breaks one says why in its PR.
- **Type:** Geist Mono only, in seven roles, with tabular figures.
- **Colour:** tokens by role, each with its contrast documented; one accent (in koan, the playing record's, tone-mapped in OKLCH to keep contrast; mint otherwise); Increase Contrast palettes at 7:1.
- **Motion says what happened:** arrive at once, leave with a fade, nothing jumps. Durations come from the tokens (80–250 ms) on the quick-out curve, drawn from GPU keyframes rather than timers. Reduce Motion is honoured whole.
- **Icons are our own:** one set of SVGs on a 24-unit grid, the stroke set by the type size, compiled into every platform's code.
- **Copy:** app words are lowercase (navigation, headings, buttons); names and people's text keep their case. British English, terse. An error says what happened and the exact fix. No exclamation marks, apologies or "please".
- **Accessibility lives in the components:** roles, names and states for screen readers (AccessKit), one keyboard model for every list, focus kept inside sheets, 44 pt touch targets (48 dp on Android), the system's text size and Reduce Transparency honoured. The accessibility tree is built only while assistive technology is attached.
- **Platforms:** Swift 6 with its data-race checks, compiled through SIL in CI; Xcode projects and bindings generated, not committed; entitlements the minimum, each with its reason; releases signed with Developer ID and notarised; a self-signed development certificate keeps a dev build's identity, and its permissions, across rebuilds.
- **A storybook** shows every component in every state, and tests render it dark and light.

## 15. CI/CD

One workflow, `.github/workflows/ci-cd.yml`, in two halves, so CD ships exactly what CI built.

- **`Checks` is the only required status.** It needs every CI job, runs `if: always()`, and fails on any result but `success` or `skipped`, so jobs can be added, renamed or skipped without touching branch protection.
- **`changes` decides which domains a diff touches** (`rust`, `web`, `package`, `docker`, `deps`…), each an allowlist of paths; every job runs under an `if:` on its domain. A PR diffs against its base; a push diffs against the last `main` commit whose run passed, so a cancelled or failed run's commits are covered by the next. Markdown counts for nothing. A change under `.github/`, a manual run, a release, or a diff that cannot be worked out runs everything. The cost of allowlists: a new top-level source directory is invisible until added. A new domain is an output of `changes`, its allowlist, an `if:` on its jobs, and those jobs in `Checks`' `needs`.
- **Least privilege:** `permissions: contents: read` at the top. A job widens only what it needs, and names `contents: read` too, since naming one permission drops the rest.
- **Every action is pinned to a full commit SHA** with its version as a comment, and every container image by digest. Dependabot moves them, weekly and grouped.
- **Every job has `timeout-minutes`; every cargo command is `--locked`.**
- **Concurrency:** a PR's newer push cancels its older run; on `main`, a newer push cancels an older run that releases nothing; a release or manual run is never cancelled. `cancel-closed-pr.yml` stops a closed PR's runs, and `make-room` cancels queued PR runs so a release gets the scarce runners.
- **Caches are saved from `main` only:** a branch's cache helps no other branch and evicts `main`'s. Docker layer caches live in the registry. A repository that goes quiet for a week builds `main` on a schedule twice a week, since GitHub drops caches unread for seven days.
- **The checks:** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, the advisory audit, `actionlint` with shellcheck, the changelog lint, the version agreement, and the freshness of every generated file.
- **The audit blocks:** `cargo audit` (or `cargo deny check advisories`) at a pinned version, run directly so its exit code is the gate. A PR audits when a manifest, the lockfile or `.cargo/` changes; `main` audits every push and weekly, since advisories land against code that has not changed. A known advisory is ignored in `.cargo/audit.toml` (or `deny.toml`) with why it does not reach this build and when to remove it: an ignore that outlives its cause is worse than no gate.
- **A generated file is committed with a freshness check:** compiled CSS, `CHANGELOG.md`, schemas, API references, icon code. A stale one fails CI with the command that regenerates it.
- **Linux binaries are built once per architecture on native runners** (`ubuntu-24.04`, `ubuntu-24.04-arm`), static musl, tarred to keep the executable bit. The image copies them in and the release attaches the same tarballs. Nothing compiles in a Dockerfile: Cargo's cache works per crate where a Docker layer is all or nothing, and no architecture runs emulated.
- **Image tags:** a PR builds the image without pushing, so the token that writes packages never runs a PR's Dockerfile. `main` pushes `:<sha>`; CD moves `main` and `sha-<short>` onto it, forward only (the `org.opencontainers.image.revision` label and the compare API decide). A release adds `vX.Y.Z`, `vX.Y`, `vX` and `latest`. Retagging never rebuilds, and nothing deploys a commit tag.
- **A job that ships states the results it needs** (`!cancelled() && needs.<job>.result == 'success'`) rather than relying on the default, which skips it whenever anything upstream skipped.
- **Builds that publish nothing until release** (macOS, signing, notarisation) run only for release PRs and releases, start alongside the checks, and gate publishing.
- **CI runs what a contributor runs.** A workflow supplies the environment (checkout, `mise`, credentials, caches) and calls recipes; a step that cannot run locally is in the wrong file.
- **Every job carries a comment saying why it exists and why it is shaped so.** In `run:` scripts under `bash -eo pipefail`, ask `grep` inside a condition and never `grep -q` on a pipe.

## 16. Versions and releases

- **One version per repository,** shared by everything it ships: the crates, the Pulumi package's `package.json`, `APP_VERSION` in `deploy/pulumi/src/versions.ts`, an Android `versionName`. SemVer. `just versions` fails when they disagree, on PRs as well as `main`.
- **`just release patch|minor|major|X.Y.Z`** runs `scripts/release.sh`: branch `release-vX.Y.Z` from `origin/main`, bump every version, `cargo update --workspace` (the workspace's own entries only), `changelog.py --release`, commit `release: vX.Y.Z`, push, open the PR with the notes as its body. It refuses a dirty tree, an existing tag, a version not above `main`'s, or a branch with an open PR. The `Release` workflow does the same from GitHub with `RELEASE_TOKEN`, since a PR opened with the workflow's own token starts no workflows and could never go green.
- **Merging the release PR is the release.** CD publishes in dependency order: the image's version tags, then the packages (Pulumi, crates.io), then the GitHub release, last, with the binaries attached and the changelog section as notes. Consumers poll releases, so nothing may name an artifact that does not exist yet.
- **Every publish step is idempotent and fails rather than guesses.** It skips what is already out; only an HTTP 404 means "not out", and any other error fails the job, since a guess releases twice or never. A failed release is retried with `gh workflow run ci-cd.yml` on `main`, which releases whatever version is not out.
- **Tags are never pushed by hand.** CI tags the commit it built.

## 17. Packaging and deployment

- **The image:** `FROM scratch` with the static binary. Add the CA bundle (from `gcr.io/distroless/static`, by digest) only if the binary reads the system trust store, or embed the pinned roots of a fixed upstream. `gcr.io/distroless/cc-debian13:nonroot` when the binary must `dlopen` (glibc no newer than the base's); `debian:*-slim` only for runtime packages. Every base pinned by digest.
- **In it:** a numeric non-root `USER` (65532 by default; a uid of its own for a workload that owns data on a host), an exec-form `ENTRYPOINT` with an absolute path, the OCI labels `title`, `source` and `licenses` (CI adds `revision`), configuration only from the environment, HTTP on 8080 and metrics on their own port. It runs with a read-only root, writing only to mounted volumes and `/tmp`. No `HEALTHCHECK` or `VOLUME`: the orchestrator owns both. The header comment says how to build it by hand (`docker build -f Dockerfile dist`).
- **A service ships its deployment:** `deploy/pulumi/` publishes `@radiosilence/<app>-pulumi` to GitHub Packages at the app's version, deploying `ghcr.io/radiosilence/<app>:v${APP_VERSION}`.
  ```
  src/index.ts          re-exports
  src/<app>.ts          create<App>(provider, namespace, args, opts?)
  src/<app>.schemas.ts  Zod: strictObject throughout, .default() leaves, .prefault({}) blocks, .refine() invariants; and its tests
  src/contract.ts       the shapes the deployer reads (Route, Deployed), declared here rather than imported
  src/versions.ts       APP_VERSION and the images it pins
  ```
  The constructor takes `z.input<Schema>`, parses it, and returns `{…} satisfies Deployed`. It makes the Deployment, Services, NetworkPolicy, Secrets and volumes, and no Ingress: the deployer owns hostnames, TLS, DNS, OIDC client secrets and generated keys. `@pulumi/*` and `zod` are peer dependencies; TypeScript is `strict` with `noUncheckedIndexedAccess` and `verbatimModuleSyntax`; oxlint, oxfmt, and vitest with Pulumi's mocks.
- **The pod:** a Deployment. One replica with `strategy: Recreate` when it owns SQLite or a host directory, since a surge would put two writers on it. `runAsNonRoot` with the image's uid, `seccompProfile: RuntimeDefault`, `allowPrivilegeEscalation: false`, `readOnlyRootFilesystem: true`, every capability dropped (a file capability on the binary where it must bind a low port), `automountServiceAccountToken: false`, `enableServiceLinks: false`. Root only in an init container that chowns a host path, keeping only the capabilities that needs.
- **Resources:** request the measured working set (p90 over a week); a generous memory limit (at least 1 Gi) as a ceiling for a leak, not a budget; no CPU limit, since CPU throttling only shows up as stalls.
- **Network policy denies by default.** The HTTP port admits the ingress controller only; a port that believes headers admits its named caller only; metrics admit the scraper only. Egress reaches only what the service talks to: nothing, DNS, or public addresses with the private ranges excluded.
- **Probes** hit `/healthz`, with a startup probe where start is slow. A hash of the configuration in a pod annotation restarts the pod when it changes. Secrets arrive by `secretKeyRef` or as files mounted `0440`. Metrics are announced by `prometheus.io/*` annotations on a declared port.
- **Sign-in:** the package takes `oidc: { issuer, clientId }` and returns its exact redirect URI (`https://<host>/auth/callback`); the deployer registers the client and generates its secret. The app reads `PUBLIC_URL`, `OIDC_ISSUER`, `OIDC_CLIENT_ID` and `OIDC_CLIENT_SECRET`.
- **Updates:** jaritanet's updater moves a pin when a GitHub release (neither draft nor prerelease) appears whose tag matches the package's version, which is why the release is cut last. A repository that releases several things tags `<app>-vX.Y.Z`; a site's image is tagged `sha-<short>` for every commit on `main`. A pin is a quoted literal, `key: "value"`, appearing once in its file.

## 18. Tooling

- **`mise.toml` pins every tool,** so a laptop and CI install the same things (`jdx/mise-action`). Rust tracks the latest stable with `rustfmt` and `clippy`. A tool whose version changes a lockfile's format (aube) is pinned exactly, and CI passes the same version.
- **`just` runs every task;** each recipe has a comment, which `just --list` shows. Every repository has `check` (everything CI checks short of the audit and release builds), `fast` (the checks that take seconds: `Cargo.lock` clean, `fmt --check`), `fmt`, `hooks`, `changelog` and `release`, and where they apply `css`, `versions` and `dev`.
- **lefthook:** pre-commit formats the staged files and restages them; pre-push runs `just fast`. Lint and tests are CI's, so a push never waits on a build.
- **Agents get the same guard:** a Claude Code `PreToolUse` hook on `git push` runs the formatter and refuses the push if anything changed, rather than amending.
- **A repository script is Python 3 standard library or bash with `set -euo pipefail`,** opening with a comment on what it does and how to call it.

## 19. Documentation and writing

Docs change in the PR that changes the behaviour they describe.

| File | Holds |
|---|---|
| `CLAUDE.md` | What the project is, what it ships and who consumes it; then *Git*, *Changelog*, *Releasing*, *CI/CD*, *Check*; then its invariants and where things live. The whole brief for an agent with no other context. |
| `README.md` | What it is; why it exists (what is wrong with the alternatives); running it, with a configuration table (`Variable \| Default \| Meaning`); how it is put together (a directory map); security notes; licence and attribution. |
| `ARCHITECTURE.md` | The contract for code with threads, protocols or a data model: the crates and what may depend on what, the rules and their reasons, how to add a protocol, page or service, the performance rules. |
| `docs/` | The user guide and reference. A generated reference is written by a tool and checked with `--check`; a plain-text copy serves readers that take one document. |
| `HANDOVER.md` | When work moves between sessions or machines: the state, the user's real setup, next steps in priority order, the rules the user set, gotchas, a map. |
| `docs/threat-model.md`, `docs/security-audit.md` | §10. |

- **British English, plain declaratives, no marketing.** Say what a thing does and why. No "seamless", "robust" or "powerful".
- **Comments say why; the code says what.** A comment records the constraint, the measurement or the incident behind the line (`#933`, "measured at 168 s against 15 s"). A docblock describes its parameters in prose, without `@param`.
- **Bounds and departures are written where they are:** the cap and its reason beside the constant; a rule broken on purpose carries a marked comment (`// theme: raw`) or a line in the PR.
- **Design docs give each rule its *why*,** in the form `- **Rule.** Detail. *Why:* reason.`

## 20. Changelog

- **`CHANGELOG.md` is generated, never edited.** Each PR adds one fragment, `changelog.d/unreleased/<section>/<pr>-<slug>.md`, holding one line: `- **Lead.** Detail.` Sections: `breaking`, `added`, `security`, `changed`, `removed`, `fixed`, `tests`, `known-issues`, `internal`. Two open PRs never conflict over the changelog.
- **`scripts/changelog.py` and `scripts/test_changelog.py` are copied verbatim between repositories,** never forked. `--lint` on every PR, `--check` when `CHANGELOG.md` is written, `--release X.Y.Z` cuts a version, `--stdout` previews.
- **The lead is the outcome as a user sees it; the detail says what was wrong and what happens now,** with any step a user must take. Internal changes go in `internal`, written the same way.

  ```markdown
  - **The container runs as non-root.** The MCP server holds a mail account's token for each request and parses mail from the internet, so the image now builds on distroless's `nonroot` variant and runs as uid 65532 rather than root.
  ```

## 21. Git and pull requests

- **Every change is a squash-merged PR.** A repository may let small changes go straight to `main` if its `CLAUDE.md` says so; workflow changes and releases still go through a PR.
- **Merge `main` into a branch; never rebase or force-push a pushed branch.** Squashing makes branch history irrelevant, and a rewrite discards what reviewers and CI saw.
- **Commits are signed.**
- **The title says what is now true,** in sentence case, prefixed by the area when there is one: `Run the container as non-root`, `koan-kit: Meter, and Chart's scrolling time series`. No `feat:` or `fix:`; the only fixed forms are `release: vX.Y.Z` and the updater's `chore: update <x> to <v>`. The title becomes the squash commit.
- **The body says what changed and why.** A fix gives *Cause*, *Change* and *Verify* (the steps that show it working) and `Closes #N`. A release PR's body is its changelog section.
- **One concern per PR, complete:** code, tests, docs and the changelog fragment together.
- **An issue states the symptom, where and since when it is seen, and the candidate causes with their PRs.**

## 22. Working on a project

- **Read `CLAUDE.md` and `ARCHITECTURE.md` first. Trust the code over plans and older docs,** and fix the doc that is wrong.
- **Fix the cause.** Understand a bug before changing anything, and name the cause in the PR.
- **Zero warnings:** compiler, clippy and linters alike, fixed when they appear.
- **Be critical, not validating.** Say when an idea is wrong, and why.
- **Order the work by what unblocks real use:** make it run where it will run, against what it will talk to; then make it fast, measured; then tidy. A speculative feature waits.
- **Parallel agents** each work in their own worktree and branch. A manager coordinates and hands each one a self-contained task: the branch, the change, the command that verifies it, and what to report. Remove a worktree and its `target/` once its PR merges, keep at most one simulator booted, never drive or capture the user's screen, and stop only processes you started.
- **Secrets for a command** are exported from the password manager to scratch for that command alone, and deleted after.

## 23. Audit checklist

Run what can be run, read the rest, and report each gap as *rule (§) → file → fix*.

- [ ] `CLAUDE.md`, `README.md`, a `LICENSE` matching the manifests, a generated `CHANGELOG.md` with `changelog.d/`; `scripts/changelog.py`, `test_changelog.py` and `release.sh` match koan's.
- [ ] `mise.toml` pins every tool CI uses; `just check`, `fast`, `fmt`, `hooks`, `changelog` and `release` exist, and `just check` passes; lefthook formats before a commit.
- [ ] One `ci-cd.yml` ending in `Checks`, with every source directory in a `changes` allowlist; top-level `permissions: contents: read`; actions pinned by SHA and images by digest; `timeout-minutes` on every job; cargo `--locked`; `release.yml`, `cancel-closed-pr.yml` and Dependabot present.
- [ ] The advisory audit blocks, runs on every push to `main` and weekly, and every ignore carries its reason.
- [ ] One version across the crates, the package and `APP_VERSION`, checked on PRs. Releases publish image → packages → GitHub release, and each step is idempotent.
- [ ] Edition 2024, resolver 3, shared dependencies in the workspace, the release profile; no bare `#[allow]`, no `unwrap` outside tests, a `SAFETY` comment on every `unsafe`, no `Debug` on a struct holding a secret.
- [ ] Configuration read and validated once at start; a bad value stops startup.
- [ ] SIGTERM handled with a drain; `/healthz`; the perimeter layers; listeners split by trust; `Host` and `Origin` checked on a local server.
- [ ] tracing to stderr; no credential, token or personal data in logs or metric labels.
- [ ] SQLite pragmas, `BEGIN IMMEDIATE` for writes, numbered migrations, a newer schema refused, statements off async workers.
- [ ] Web: the CSP without `'unsafe-inline'` (`'unsafe-eval'` only with Datastar) and the other headers, with a test; every state change a POST behind the Datastar header or an `Origin` check; no `|safe`, `style=`, or `data-show` on what starts hidden; committed CSS fresh; assets fingerprinted; `__Host-` cookies.
- [ ] MCP: two tools; PREVIEW → CONFIRM on anything outward or irreversible; third-party text marked untrusted; access below admin.
- [ ] Every parser of untrusted bytes capped and fuzzed; fetched URLs and named paths guarded.
- [ ] Tests named as sentences; every invariant guarded; tests hermetic; every `#[ignore]` with a reason.
- [ ] The image: `scratch` or distroless, digest-pinned, a numeric non-root uid, the labels. The Pulumi package's layout, the pod baseline, no CPU limit, network policy denying by default.
- [ ] Every doc matches the code.
