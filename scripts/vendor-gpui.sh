#!/usr/bin/env bash
# The vendored GPUI crates, kept as patches against their crates.io release.
#
#   scripts/vendor-gpui.sh VERSION   rebuild vendor/<crate> from the release
#                                    VERSION and vendor/patches/<crate>.patch,
#                                    failing on the first hunk that does not
#                                    apply
#   scripts/vendor-gpui.sh --patches write vendor/patches/<crate>.patch from
#                                    the copies as they are now: run it after
#                                    editing a vendored file
#   scripts/vendor-gpui.sh --check   fail when a copy is not its release plus
#                                    its patch (CI)
#
# The copies stay committed, so a build needs neither the network nor this
# script; the patches are what is reviewed and carried to the next release.
# Each release is fetched from static.crates.io, checked against the
# checksum the crates.io index publishes for it, and trimmed as the copies
# are: no Cargo.lock, examples or benches.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
VENDOR=$ROOT/vendor
PATCHES=$VENDOR/patches
CRATES=(gpui-pre gpui-pre-apple gpui-pre-linux gpui-pre-macos gpui-pre-wgpu)
# Beside the copies, on their filesystem, so a copy is replaced by a rename.
WORK=$(mktemp -d "$VENDOR/.tmp.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
# Git run in WORK must not find the repository around it: `git apply` would
# take the patch's paths as the repository's.
export GIT_CEILING_DIRECTORIES=$VENDOR

fail() {
    echo "error: $*" >&2
    exit 1
}

# The release a copy was made from, from its manifest.
version_of() {
    sed -n 's/^version = "\(.*\)"$/\1/p' "$VENDOR/$1/Cargo.toml" | head -1
}

# The SHA-256 crates.io's index records for release `$2` of crate `$1`.
index_cksum() {
    local crate=$1 version=$2 path
    case ${#crate} in
    1) path=1/$crate ;;
    2) path=2/$crate ;;
    3) path=3/${crate:0:1}/$crate ;;
    *) path=${crate:0:2}/${crate:2:2}/$crate ;;
    esac
    curl -fsSL "https://index.crates.io/$path" | python3 -c '
import json, sys
version = sys.argv[1]
for line in sys.stdin:
    entry = json.loads(line)
    if entry["vers"] == version:
        found = entry["cksum"]
print(found)
' "$version"
}

# The release `$2` of crate `$1`, its checksum verified, trimmed, at `$3`.
release() {
    local crate=$1 version=$2 into=$3
    local file="$WORK/dl/$crate-$version.crate"
    mkdir -p "$WORK/dl"
    curl -fsSL "https://static.crates.io/crates/$crate/$crate-$version.crate" -o "$file" ||
        fail "could not fetch $crate $version from crates.io"
    local expected actual
    expected=$(index_cksum "$crate" "$version") ||
        fail "no checksum for $crate $version in the crates.io index"
    actual=$(shasum -a 256 "$file" | cut -d' ' -f1)
    [ "$expected" = "$actual" ] ||
        fail "$crate $version does not match the crates.io index (expected $expected, got $actual)"
    tar -xzf "$file" -C "$WORK/dl"
    rm -rf "$into"
    mv "$WORK/dl/$crate-$version" "$into"
    rm -rf "$into/Cargo.lock" "$into/.cargo-ok" "$into/examples" "$into/benches"
}

# `$1`'s patch, the copy against its release, written to `$2`.
diff_of() {
    local crate=$1 out=$2 status=0
    release "$crate" "$(version_of "$crate")" "$WORK/a"
    rm -rf "$WORK/b" && cp -R "$VENDOR/$crate" "$WORK/b"
    # Whatever the user's git configuration says: plain, unconverted text.
    (cd "$WORK" && git -c core.autocrlf=false diff --no-index --binary --no-prefix \
        --no-color --no-ext-diff a b) > "$out" || status=$?
    # 1 means the two differ; anything above is git failing.
    [ "$status" -le 1 ] || fail "git diff failed for $crate (status $status)"
}

case "${1:-}" in
--patches)
    mkdir -p "$PATCHES"
    for crate in "${CRATES[@]}"; do
        diff_of "$crate" "$PATCHES/$crate.patch"
        echo "$crate: $(grep -c '^diff --git' "$PATCHES/$crate.patch") files"
    done
    ;;
--check)
    stale=()
    for crate in "${CRATES[@]}"; do
        diff_of "$crate" "$WORK/$crate.patch"
        cmp -s "$WORK/$crate.patch" "$PATCHES/$crate.patch" || stale+=("$crate")
    done
    if ((${#stale[@]})); then
        echo "error: vendor/ and vendor/patches/ disagree for: ${stale[*]}" >&2
        echo "  after editing a vendored file, run: just vendor-gpui-patches" >&2
        exit 1
    fi
    echo "every vendored crate is its release plus its patch"
    ;;
"" | -*)
    sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
*)
    version=$1
    for crate in "${CRATES[@]}"; do
        release "$crate" "$version" "$WORK/$crate"
        if ! (cd "$WORK/$crate" && git apply --check -p1 "$PATCHES/$crate.patch" 2>"$WORK/err"); then
            echo "error: $crate's patch does not apply to $version:" >&2
            sed 's/^/  /' "$WORK/err" >&2
            echo "  vendor/ is unchanged. Reapply the KOAN: hunks by hand against the release," >&2
            echo "  then run: just vendor-gpui-patches" >&2
            exit 1
        fi
        (cd "$WORK/$crate" && git apply -p1 "$PATCHES/$crate.patch")
    done
    # Every patch applied: only now is vendor/ touched. Each old copy is
    # renamed aside before its replacement is renamed in, and every one
    # swapped is put back if any rename fails.
    mkdir -p "$WORK/old"
    swapped=()
    restore() {
        for crate in "${swapped[@]}"; do
            rm -rf "${VENDOR:?}/$crate"
            mv "$WORK/old/$crate" "$VENDOR/$crate"
        done
    }
    for crate in "${CRATES[@]}"; do
        if mv "$VENDOR/$crate" "$WORK/old/$crate"; then
            swapped+=("$crate")
            mv "$WORK/$crate" "$VENDOR/$crate" && continue
        fi
        restore
        fail "could not replace vendor/$crate; vendor/ is as it was"
    done
    echo "vendor/ is GPUI $version with every patch applied; update the pins in Cargo.toml"
    ;;
esac
