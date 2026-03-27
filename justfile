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

# Install dev build to ~/.local/bin/koan-dev
install-dev:
    cargo build --release
    mkdir -p ~/.local/bin
    cp target/release/koan ~/.local/bin/koan-dev
    @echo "Installed to ~/.local/bin/koan-dev"

# Clean build artifacts
clean:
    cargo clean
