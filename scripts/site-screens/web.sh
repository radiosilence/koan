#!/usr/bin/env bash
# The web UI's screenshots: a throwaway server on the demo library, with sign-in
# off so a headless browser needs no account, on a port of its own.
#
#   web.sh <config dir> <site/public/screens>
set -euo pipefail
config=$1
out=$(cd "$2" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
port=47931
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

KOAN_CONFIG_DIR="$config" KOAN_GRAPHQL__AUTH_ENABLED=false target/release/koan --headless --port "$port" \
    >"$work/server.log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; rm -rf "$work"' EXIT
for _ in $(seq 60); do
    curl -sf "http://127.0.0.1:$port/albums" >/dev/null && break
    sleep 0.5
done
base="http://127.0.0.1:$port"
# The record on the album page: one with a warm sleeve.
album=$(curl -s "$base/albums" | grep -o 'href="/album/[0-9]*"' | sed -n 3p | grep -o '[0-9]*')

desk() {
    "$chrome" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor="$3" \
        --virtual-time-budget=5000 --window-size=1600,1000 --screenshot="$work/$1.png" "$base$2" >/dev/null 2>&1
}
desk web-albums /albums 1
desk web-search "/search?q=low" 1.5
node "$here/phone.mjs" "$base/album/$album" "$work/web-phone-album.png" 2.4
node "$here/phone.mjs" "$base/favourites" "$work/web-phone-favourites.png" 1.6
# A phone's 812 points at 2.4 and 1.6 round up a pixel past the site's frames.
sips -c 1948 900 "$work/web-phone-album.png" >/dev/null
sips -c 1298 600 "$work/web-phone-favourites.png" >/dev/null

for name in web-albums web-search web-phone-album web-phone-favourites; do
    cwebp -quiet -q 75 -m 6 "$work/$name.png" -o "$out/$name.webp"
    echo "$name.webp: $(( $(wc -c < "$out/$name.webp") / 1024 )) KB"
done
