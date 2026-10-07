# Contributing to koan

PRs welcome.

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

### A body reads only what it draws

`@Observable` subscribes a body to every property it reads while running, and
nothing else — so a read is a subscription, and a read high in the tree re-runs
everything below it when that property moves. Keep reads in the leaf that draws
them:

- Something that changes often gets its own small view that reads it. A toast,
  a spinner, a count on a sidebar row.
- A reaction to a model changing belongs in the model's `didSet`, not in an
  `.onChange(of:)` on a view: the `onChange` makes that view a reader.
- Pass a model into a view rather than a value taken from it, so the read
  happens in the view that uses it.
- Nothing in the Scene body reads state that changes. The Scene body is the
  whole window.
- An `NSViewRepresentable` reads its bindings in `updateNSView`, and that read
  is charged to the body it sits in. Wrap it in a view of its own.
- A `List`'s selection is `@State` its body must not read. Rows learn they are
  selected from `@Environment(\.backgroundProminence)`; anything else that
  needs the set — a count, a mirror to a model — takes the binding into a
  child and reads it there.
- Something that moves every frame — a level meter, a position — does not go
  through observation at all. Hand it to a layer.

A re-run is not free even where nothing changes: the toolbar rebuilds its
AppKit-backed items when the body declaring them re-runs, which throws away the
filter field and the focus in it. `let _ = Self._printChanges()` at the top of
a body prints what made it run.

### Layout is paid for by everything mounted

Hiding a view with `opacity` leaves it in layout, and a window-wide layout pass
lays out everything mounted in it. Two things follow:

- The window toolbar declares the same items on every page; a control that does
  not apply to a page is left out of its item, not the item out of the toolbar.
  Adding or removing an item makes AppKit re-tile the toolbar, and a re-tile is
  a window-wide layout pass.
- Pages are not kept mounted behind the one on screen to keep their scroll
  position. They are rebuilt and put back where they were — `ScrollPosition` for
  a scroll view, `ScrollViewReader` for a `List`. The queue is the exception.

`Hangs` and `os_signpost` in Instruments show it: a `click-to-page` region that
ends quickly followed by a hang on the main thread is the page being laid out,
not read.

## Submitting a PR

1. Fork the repo and create a feature branch.
2. Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` before pushing. Zero warnings policy — fix them all.
3. Write tests for new features where practical.
4. Keep commits focused. PRs are squash-merged, so branch history does not need tidying.
5. Describe what your PR does and why in the PR description.

The build, test and lint jobs only run when a PR touches something that feeds
a build — `crates/`, `apps/`, `Cargo.toml`, `Cargo.lock`, `.cargo/`, `justfile`,
`mise.toml` or `.github/workflows/`. A documentation PR sees them reported as
skipped, which counts as passing. Adding a new top-level source directory means
adding it to the `changes` job in `.github/workflows/ci-cd.yml`, or CI will sit
the PR out.

The macOS jobs (Test, Clippy and Build on macOS, and the macOS and tvOS app
builds) run on a PR only when it touches what the Linux jobs cannot check: the
apps, `koan-ffi`, the bindings generator, `justfile`, dependencies or
toolchains, the workflows, or a Rust file with code for an Apple target
(`target_os = "macos"`, `"ios"`, `"tvos"`, or a module declared for Apple
only). Otherwise the macOS checks report success without running. A release PR
(branch `release-*`, or one that changes the version) runs everything, so a
change to shared Rust that breaks only on macOS is caught there at the latest.

## Architecture

Five crates: `koan-core` (library -- audio engine, player, database, indexer), `koan-tui` (TUI, visualizers, media keys), `koan-server` (GraphQL, Subsonic REST, MCP), `koan-ffi` (uniffi bindings for the macOS and iOS apps), and `koan-cli` (binary -- CLI entry point). See [ARCHITECTURE.md](ARCHITECTURE.md) for the full technical manual.

If you're touching the audio path: the render callback must never allocate or lock. Read the threading model docs before changing anything in `audio/`.

If you're modifying config programmatically (e.g. a new CLI command that writes settings): use `Config::persist()`. It applies your mutation, diffs it against the two files, and writes each changed key to the file `config::layer_of` assigns it, so secrets and machine paths land in `config.local.toml` and comments survive. A new machine-scoped key needs adding to `layer_of`.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
