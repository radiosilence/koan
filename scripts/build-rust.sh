#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> Building Rust crates..."
cargo build --release --workspace
echo "==> Rust build complete."
