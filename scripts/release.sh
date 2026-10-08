#!/usr/bin/env bash
# Prepare a release PR: branch from origin/main, bump the version everywhere
# check-version compares, turn the unreleased changelog fragments into the
# release, commit, push and open the PR. Never creates or pushes a tag.
#
#   scripts/release.sh X.Y.Z [--no-push]
set -euo pipefail

version=""
push=1
for arg in "$@"; do
  case "$arg" in
    --no-push) push=0 ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) version="$arg" ;;
  esac
done
[[ "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || {
  echo "usage: scripts/release.sh X.Y.Z [--no-push]" >&2
  exit 2
}

cd "$(git rev-parse --show-toplevel)"
tag="v$version"
branch="release/$tag"

[ -z "$(git status --porcelain)" ] || { echo "working tree is not clean" >&2; exit 1; }
git fetch origin --quiet
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null || [ -n "$(git ls-remote --tags origin "refs/tags/$tag")" ]; then
  echo "tag $tag already exists" >&2
  exit 1
fi
if git rev-parse -q --verify "refs/heads/$branch" >/dev/null; then
  echo "branch $branch already exists" >&2
  exit 1
fi

current=$(git show origin/main:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
newest=$(printf '%s\n%s\n' "$current" "$version" | sort -V | tail -1)
if [ "$version" = "$current" ] || [ "$newest" != "$version" ]; then
  echo "$version is not greater than the current version $current" >&2
  exit 1
fi

git switch -c "$branch" origin/main --no-track

# The workspace version and the internal crates' pins, which must move together.
sed -i.bak -e "s/^version = \"$current\"/version = \"$version\"/" \
  -e "/^koan-.*path = \"crates\//s/version = \"$current\"/version = \"$version\"/" Cargo.toml
sed -i.bak "s/^  \"version\": \"$current\"/  \"version\": \"$version\"/" deploy/pulumi/package.json
sed -i.bak "s/^export const APP_VERSION = \"$current\";/export const APP_VERSION = \"$version\";/" deploy/pulumi/src/versions.ts
rm -f Cargo.toml.bak deploy/pulumi/package.json.bak deploy/pulumi/src/versions.ts.bak

grep -q "^version = \"$version\"" Cargo.toml
grep -q "\"version\": \"$version\"" deploy/pulumi/package.json
grep -q "APP_VERSION = \"$version\"" deploy/pulumi/src/versions.ts

cargo update --workspace --offline

python3 scripts/changelog.py --release "$version"

git add -A
git commit -q -m "release: $tag"
echo "committed release: $tag on $branch"

if [ "$push" = 1 ]; then
  git push -u origin "$branch"
  section=$(awk -v h="## $version" '$0==h{f=1;next} f&&/^## /{exit} f' CHANGELOG.md)
  gh pr create --title "release: $tag" --body "$section"
else
  echo "not pushed (--no-push)"
fi

cat <<MSG

Merge nothing else to main until \`gh release view $tag\` succeeds, then run:
  gh workflow run update-apps.yml -R radiosilence/jaritanet
MSG
