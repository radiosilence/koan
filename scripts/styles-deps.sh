#!/usr/bin/env bash
# Installs the npm packages the stylesheets import (package.json at the root),
# writing the GitHub Packages token file .npmrc names first if it is missing.
# GH_TOKEN in CI, the gh login otherwise.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ ! -s .npmrc.auth ] || [ -n "${GH_TOKEN:-}" ]; then
  token="${GH_TOKEN:-$(gh auth token 2>/dev/null || true)}"
  if [ -z "$token" ]; then
    echo "no GitHub token for GitHub Packages: run 'gh auth login', or set GH_TOKEN" >&2
    exit 1
  fi
  printf '//npm.pkg.github.com/:_authToken=%s\n' "$token" > .npmrc.auth
fi

aube install
