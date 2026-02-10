#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

echo "=== koan full build ==="

echo "Step 1/4: Building Rust..."
./scripts/build-rust.sh

echo "Step 2/4: Generating bindings..."
./scripts/generate-bindings.sh

echo "Step 3/4: Creating xcframework..."
./scripts/build-xcframework.sh

echo "Step 4/4: Building Swift..."
cd swift
swift build

echo "=== Build complete ==="
