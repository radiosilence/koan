#!/usr/bin/env bash
# Other devices for the control menu to list, none of them real: a throwaway
# koan server with a demo account, and three windowless app instances signed in
# to it as "Living Room", "iPad" and "Kitchen", each playing a different demo
# record, muted. Nothing joins the local network: they reach each other through
# the server only. The instance that draws the screenshots signs in too.
#
#   devices.sh start <library> <work dir> <renderer config dir>
#   devices.sh stop <work dir>
set -euo pipefail
cmd=$1
work=$(mkdir -p "$2" && cd "$2" && pwd)

stop() {
    [ -f "$work/peers.pid" ] && while read -r pid; do kill "$pid" 2>/dev/null || true; done < "$work/peers.pid"
    just _demo-server-stop "$work"
    rm -rf "$work"/peer-* "$work/peers.pid"
}

if [ "$cmd" = stop ]; then
    stop
    exit 0
fi

library=$(cd "$3" && pwd)
renderer=$(cd "$4" && pwd)
app="$PWD/apps/macos/.build/pkg/kōan.app/Contents/MacOS/koan-app"
url=$(just _demo-server "$library" "$work" | tail -1)
password=$(cat "$work/server.password")

# Signed in, silent, alone on the network, on a port of its own.
account() {
    printf '[remote]\nenabled = true\nurl = "%s"\nusername = "owner"\n' "$url" >> "$1/config.toml"
    printf '[remote]\npassword = "%s"\n\n[playback]\nmuted = true\nrenderers = false\n\n[devices]\nnearby = false\ndiscoverable = false\nport = %s\n' \
        "$password" "$2" > "$1/config.local.toml"
}
account "$renderer" 47917

port=47941
: > "$work/peers.pid"
for peer in "Living Room|tvos|Paper Satellites|Ground Station" "iPad|ios|Velvet Underpass|Night Market" \
            "Kitchen|macos|Saltwater Radio|Harbour Lights"; do
    IFS='|' read -r name platform album track <<< "$peer"
    dir="$work/peer-$port"
    mkdir -p "$dir"
    printf '[library]\nfolders = ["%s"]\n\n' "$library" > "$dir/config.toml"
    account "$dir" "$port"
    KOAN_CONFIG_DIR="$dir" target/release/koan scan >/dev/null
    python3 scripts/site-screens/seed.py "$dir" "$album" "$track" 1 >/dev/null
    KOAN_CONFIG_DIR="$dir" KOAN_WINDOWLESS=1 KOAN_DEVICE_NAME="$name" KOAN_DEVICE_PLATFORM="$platform" \
        "$app" -ApplePersistenceIgnoreState YES >"$dir/app.log" 2>&1 &
    echo $! >> "$work/peers.pid"
    port=$((port + 1))
done
# Linked, and each telling the server what it is playing.
sleep 12
echo "three devices on $url"
