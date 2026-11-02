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
# Each release is fetched from static.crates.io, and trimmed as the copies
# are: no Cargo.lock, examples or benches.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
VENDOR=$ROOT/vendor
PATCHES=$VENDOR/patches
CRATES=(gpui-pre gpui-pre-apple gpui-pre-linux gpui-pre-macos gpui-pre-wgpu)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# The release a copy was made from, from its manifest.
version_of() {
    sed -n 's/^version = "\(.*\)"$/\1/p' "$VENDOR/$1/Cargo.toml" | head -1
}

# The release `$2` of crate `$1`, trimmed, at `$3`.
release() {
    local crate=$1 version=$2 into=$3
    local url="https://static.crates.io/crates/$crate/$crate-$version.crate"
    mkdir -p "$WORK/dl"
    curl -fsSL "$url" -o "$WORK/dl/$crate.crate" || {
        echo "error: could not fetch $crate $version from crates.io" >&2
        exit 1
    }
    tar -xzf "$WORK/dl/$crate.crate" -C "$WORK/dl"
    rm -rf "$into"
    mv "$WORK/dl/$crate-$version" "$into"
    rm -rf "$into/Cargo.lock" "$into/.cargo-ok" "$into/examples" "$into/benches"
}

# `$1`'s patch: the copy against its release.
diff_of() {
    local crate=$1
    release "$crate" "$(version_of "$crate")" "$WORK/a"
    rm -rf "$WORK/b" && cp -R "$VENDOR/$crate" "$WORK/b"
    (cd "$WORK" && git diff --no-index --binary --no-prefix a b) || true
}

case "${1:-}" in
--patches)
    mkdir -p "$PATCHES"
    for crate in "${CRATES[@]}"; do
        diff_of "$crate" > "$PATCHES/$crate.patch"
        echo "$crate: $(grep -c '^diff --git' "$PATCHES/$crate.patch") files"
    done
    ;;
--check)
    stale=()
    for crate in "${CRATES[@]}"; do
        if ! diff -q <(diff_of "$crate") "$PATCHES/$crate.patch" >/dev/null; then
            stale+=("$crate")
        fi
    done
    if ((${#stale[@]})); then
        echo "error: vendor/ and vendor/patches/ disagree for: ${stale[*]}" >&2
        echo "  after editing a vendored file, run: just vendor-gpui-patches" >&2
        exit 1
    fi
    echo "every vendored crate is its release plus its patch"
    ;;
"" | -*)
    sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
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
    # Every patch applied: only now is vendor/ touched.
    for crate in "${CRATES[@]}"; do
        rm -rf "$VENDOR/$crate"
        mv "$WORK/$crate" "$VENDOR/$crate"
    done
    echo "vendor/ is GPUI $version with every patch applied; update the pins in Cargo.toml"
    ;;
esac
