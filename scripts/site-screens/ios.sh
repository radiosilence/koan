#!/usr/bin/env bash
# The iPhone's screenshots: a demo server on the library, the koan-dev
# simulator (an iPhone 17 Pro Max, 1320x2868) signed in to it, and the UI walk,
# whose shots are halved to the site's 660x1434. Boots one simulator and shuts
# it down again; run nothing else on a simulator meanwhile.
#
#   ios.sh <library> <site/public/screens>
set -euo pipefail
library=$(cd "$1" && pwd)
out=$(cd "$2" && pwd)
work=$(mktemp -d)
trap 'just _demo-server-stop "$work"; rm -rf "$work"' EXIT

url=$(just _demo-server "$library" "$work" | tail -1)
just ios-signin koan-dev "$url" owner "$(cat "$work/server.password")"
KOAN_WALK_SETTLE=6 just ios-walk koan-dev low

declare -A site=(
    [01-queue]=ios-queue-empty [02-library]=ios-library [03-albums]=ios-albums [04-album]=ios-album
    [05b-artist]=ios-artist [06-now-playing]=ios-now-playing [08-search]=ios-search [10-queue-playing]=ios-queue
)
for shot in target/ios-walk/*.png; do
    key=$(basename "$shot" .png)
    key=${key%%_*}
    name=${site[$key]:-}
    [ -n "$name" ] && [ "$name" != ios-queue-empty ] || continue
    sips -z 1434 660 "$shot" --out "$work/$name.png" >/dev/null
    cwebp -quiet -q 75 -m 6 "$work/$name.png" -o "$out/$name.webp"
    echo "$name.webp: $(( $(wc -c < "$out/$name.webp") / 1024 )) KB"
done
