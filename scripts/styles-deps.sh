#!/usr/bin/env bash
# Installs the npm packages the web UI's scripts import (package.json at the root): Zod. The stylesheets need none,
# since they import the kōan web kit from koan-kit-web/ in the tree.
set -euo pipefail
cd "$(dirname "$0")/.."
aube install
