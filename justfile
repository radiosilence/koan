# koan — bit-perfect macOS music player

# Build release binary
build:
    cargo build --release

# Build + run CLI in release mode
cli *ARGS:
    cargo run --release -p koan-music -- {{ARGS}}

# Run tests + clippy
check:
    cargo test --all-targets
    cargo clippy --all-targets -- -D warnings

# Format
fmt:
    cargo fmt

# Clean build artifacts
clean:
    cargo clean
