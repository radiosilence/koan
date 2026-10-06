#!/usr/bin/env bash
# Notarise FILE with Apple, resumably, and wait for the outcome.
#
#   scripts/notarise.sh FILE [TIMEOUT]
#
# FILE's name is what the submission is known by, so give it one that names
# its contents (a version and a hash): a run that finds a submission of that
# name already in progress, or accepted, waits on it or takes it, rather than
# submitting again. A re-run after a timeout therefore picks up where Apple
# is, and costs no second place in its queue.
#
# Needs APPLE_API_KEY_PATH, APPLE_API_KEY_ID and APPLE_API_ISSUER_ID. Exits
# non-zero, with Apple's log, unless the submission is accepted.
set -euo pipefail

file=$1
timeout=${2:-170m}
name=$(basename "$file")
auth=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID")

# The newest submission of this name that is still alive: in progress, or
# accepted. One that was refused is not worth waiting on again.
id=$(xcrun notarytool history "${auth[@]}" --output-format json | python3 -c '
import json, sys
name = sys.argv[1]
for s in json.load(sys.stdin).get("history", []):
    if s.get("name") == name and s.get("status") in ("In Progress", "Accepted"):
        print(s["id"])
        break
' "$name")

if [ -n "$id" ]; then
  echo "$name: picking up submission $id"
else
  id=$(xcrun notarytool submit "$file" "${auth[@]}" --output-format json \
    | python3 -c 'import json, sys; print(json.load(sys.stdin)["id"])')
  echo "$name: submitted as $id"
fi

# `wait` returns once the submission ends, or at the timeout with it still in
# progress; the status is asked for afterwards either way.
xcrun notarytool wait "$id" "${auth[@]}" --timeout "$timeout" >/dev/null || true
status=$(xcrun notarytool info "$id" "${auth[@]}" --output-format json \
  | python3 -c 'import json, sys; print(json.load(sys.stdin)["status"])')

case "$status" in
  Accepted)
    echo "$name: notarised ($id)"
    ;;
  "In Progress")
    echo "::error::$name: Apple has not finished notarising submission $id after $timeout. Re-run this job to keep waiting on it; nothing is submitted again."
    exit 1
    ;;
  *)
    echo "::error::$name: submission $id finished as $status"
    xcrun notarytool log "$id" "${auth[@]}" || true
    exit 1
    ;;
esac
