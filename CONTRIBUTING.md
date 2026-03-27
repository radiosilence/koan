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

## Submitting a PR

1. Fork the repo and create a feature branch.
2. Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` before pushing. Zero warnings policy — fix them all.
3. Write tests for new features where practical.
4. Keep commits focused. We squash-merge PRs, so don't stress about perfect history.
5. Describe what your PR does and why in the PR description.

## Architecture

Two crates: `koan-core` (library — audio engine, player, database, indexer) and `koan-music` (binary — TUI, CLI). See [ARCHITECTURE.md](ARCHITECTURE.md) for the full technical manual.

If you're touching the audio path: the render callback must never allocate or lock. Read the threading model docs before changing anything in `audio/`.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
