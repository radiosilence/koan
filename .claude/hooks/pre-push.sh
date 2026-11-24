#!/usr/bin/env bash
# PreToolUse hook on Bash (.claude/settings.json). Before a `git push`, runs
# `cargo fmt --all` in the tree being pushed and refuses the push (exit 2,
# reason on stderr) if that leaves unstaged changes. Never commits or amends.
# Reads the hook's JSON on stdin: `.tool_input.command` and `.cwd`.
set -uo pipefail

input=$(cat)
if command -v jq >/dev/null 2>&1; then
  cmd=$(jq -r '.tool_input.command // empty' <<<"$input")
  cwd=$(jq -r '.cwd // empty' <<<"$input")
else
  cmd=$input
  cwd=
fi

# A push as a command, at the start of a line or after a separator, with any
# git options before it (`git -C dir push`). Mentions of the words, as in
# `grep 'git push'` or a commit message, are not pushes.
push='(^|[;&|(])[[:space:]]*git([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+push([[:space:]]|$)'
grep -Eq "$push" <<<"$cmd" || exit 0

# The tree being pushed: the session's directory (a worktree, if it moved
# into one), then a leading `cd dir &&` or a `git -C dir`.
cd "${cwd:-${CLAUDE_PROJECT_DIR:-.}}" 2>/dev/null || true
lead=$(sed -nE 's/^[[:space:]]*cd[[:space:]]+([^[:space:];&|]+)[[:space:]]*&&.*/\1/p' <<<"$cmd" | head -n1)
[ -n "$lead" ] && cd "$lead" 2>/dev/null
dir=$(grep -oE 'git[[:space:]]+-C[[:space:]]+[^[:space:];&|]+' <<<"$cmd" | head -n1 | awk '{print $3}')
[ -n "$dir" ] && cd "$dir" 2>/dev/null
root=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
cd "$root" || exit 0
# Only a Rust tree has anything to format.
[ -f Cargo.toml ] || exit 0

# Hooks run without the user's shell profile, so cargo may not be on PATH:
# take it through mise, which pins the toolchain, or from rustup's bin.
if command -v mise >/dev/null 2>&1; then
  fmt=(mise exec -- cargo fmt --all)
elif command -v cargo >/dev/null 2>&1; then
  fmt=(cargo fmt --all)
elif [ -x "$HOME/.cargo/bin/cargo" ]; then
  fmt=("$HOME/.cargo/bin/cargo" fmt --all)
else
  echo "Pre-push: no cargo found to run cargo fmt in $root." >&2
  exit 2
fi

if ! "${fmt[@]}" >&2; then
  echo "Pre-push: cargo fmt failed in $root; fix that before pushing." >&2
  exit 2
fi
if ! git diff --quiet; then
  {
    echo "Pre-push: unstaged changes in $root after cargo fmt:"
    git diff --name-only
    echo 'Review and commit them yourself, then push again. Refusing to commit or amend.'
  } >&2
  exit 2
fi
