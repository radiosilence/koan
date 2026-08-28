# Contributing to koan

PRs welcome. Here's how to keep things smooth.

## Before you start

- **Trivial fixes** (typos, docs, small bug fixes) — just open a PR.
- **Anything non-trivial** (new features, refactors, API changes) — open an issue first so we can discuss the approach before you write code.

## Development

```bash
# build from source
git clone https://github.com/radiosilence/koan.git && cd koan
cargo build --release

# run checks (tests + clippy)
just check

# format
just fmt
```

### Measuring the macOS app

The app emits signposts on the Points of Interest timeline, so a recording lines
its own regions up against what the CPU profiler and the SwiftUI instrument saw:

```bash
xcrun xctrace record --template 'SwiftUI' --attach koan-app --output t.trace
```

Opening a record also reports itself in plain text, because the interesting part
of that gesture is not in any view's body — it is the layout, the CoreAnimation
commit and the render server that follow it, and nothing a view can run reaches
them. `FrameTimer` times the tap against the display link instead:

```bash
log stream --level info --predicate 'subsystem == "cc.blit.koan"'
# tap-to-frame 155.1ms (body 16.8ms, draw 138.3ms) then 114.7ms, 70.1ms
```

`body` is koan working out what to draw, `draw` is everything between that and
the first frame that could carry it, and what follows is each further stall
before the run loop is back at cadence — a page does not arrive in one commit,
and the later ones are still time spent looking at the old page.

## Submitting a PR

1. Fork the repo and create a feature branch.
2. Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` before pushing. Zero warnings policy — fix them all.
3. Write tests for new features where practical.
4. Keep commits focused. We squash-merge PRs, so don't stress about perfect history.
5. Describe what your PR does and why in the PR description.

The build, test and lint jobs only run when a PR touches something that feeds
a build — `crates/`, `apps/`, `Cargo.toml`, `Cargo.lock`, `.cargo/`, `justfile`,
`mise.toml` or `.github/workflows/`. A documentation PR sees them reported as
skipped, which counts as passing. Adding a new top-level source directory means
adding it to the `changes` job in `.github/workflows/ci-cd.yml`, or CI will sit
the PR out.

## Architecture

Four crates: `koan-core` (library -- audio engine, player, database, indexer), `koan-tui` (TUI, visualizers, media keys), `koan-server` (GraphQL, Subsonic REST, MCP), and `koan-cli` (binary -- CLI entry point). See [ARCHITECTURE.md](ARCHITECTURE.md) for the full technical manual.

If you're touching the audio path: the render callback must never allocate or lock. Read the threading model docs before changing anything in `audio/`.

If you're modifying config programmatically (e.g. a new CLI command that writes settings): use `Config::update_base()`, not `save()`. `update_base()` reads only `config.toml`, applies your change, and writes back — safe. Calling `save()` on a `Config::load()` result would leak secrets from `config.local.toml` and env vars into `config.toml`. Use `save_local()` for sensitive values like passwords.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
