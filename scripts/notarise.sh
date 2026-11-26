#!/usr/bin/env bash
# Notarise FILE with Apple, resumably, and wait for the outcome.
#
#   scripts/notarise.sh FILE [TIMEOUT]
#
# FILE's name is what the submission is known by, so give it one that names
# its contents (a version and a hash): a run that finds a submission of that
# name already in progress, or accepted, waits on it or takes it, rather than
# submitting again. A re-run after a timeout therefore picks up where Apple
# is, and costs no second place in its queue. TIMEOUT is in seconds, 170
# minutes unless given.
#
# Speaks to Apple's Notary API through rcodesign (apple-codesign), so it runs
# on Linux as well as macOS. Needs APPLE_API_KEY_PATH, APPLE_API_KEY_ID and
# APPLE_API_ISSUER_ID. Exits non-zero, with Apple's log, unless the
# submission is accepted.
set -euo pipefail

file=$1
timeout=${2:-10200}
name=$(basename "$file")

key=$(mktemp)
trap 'rm -f "$key"' EXIT
rcodesign encode-app-store-connect-api-key --output-path "$key" \
  "$APPLE_API_ISSUER_ID" "$APPLE_API_KEY_ID" "$APPLE_API_KEY_PATH"
auth=(--api-key-file "$key")

# Apple's recent submissions, newest first, one a line: id, date, name, type
# and status, the status in lower case ("in progress" is two words). Each awk
# below reads the list to its end: rcodesign panics on a closed pipe, which
# pipefail would make a failure.
submissions() { rcodesign notary-list "${auth[@]}"; }

# The newest submission of this name that is still alive: in progress, or
# accepted. One that was refused is not worth waiting on again.
id=$(submissions | awk -v name="$name" '
  !found && $3 == name && ($5 == "accepted" || $5 " " $6 == "in progress") { print $1; found = 1 }')

if [ -n "$id" ]; then
  echo "$name: picking up submission $id"
else
  # rcodesign logs the submission's id on stderr as it creates it, then uploads.
  id=$(rcodesign notary-submit "${auth[@]}" "$file" 2>&1 | tee /dev/stderr \
    | sed -n 's/.*created submission ID: \([-0-9a-fA-F]*\).*/\1/p')
  if [ -z "$id" ]; then
    echo "::error::$name: rcodesign made no submission"
    exit 1
  fi
  echo "$name: submitted as $id"
fi

# `notary-wait` returns once the submission ends, printing Apple's log, or
# fails at the timeout with it still in progress; the status is asked for
# afterwards either way.
rcodesign notary-wait "${auth[@]}" --max-wait-seconds "$timeout" "$id" || true
status=$(submissions | awk -v id="$id" '$1 == id { $1 = $2 = $3 = $4 = ""; sub(/^ +/, ""); print }')

case "$status" in
  accepted)
    echo "$name: notarised ($id)"
    ;;
  "in progress")
    echo "::error::$name: Apple has not finished notarising submission $id after ${timeout}s. Re-run this job to keep waiting on it; nothing is submitted again."
    exit 1
    ;;
  *)
    echo "::error::$name: submission $id finished as ${status:-an unknown status}"
    rcodesign notary-log "${auth[@]}" "$id" || true
    exit 1
    ;;
esac
