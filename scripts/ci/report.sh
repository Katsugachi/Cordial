#!/usr/bin/env bash
# Runs a command and, on failure, republishes the tail of its output as GitHub
# annotations. Annotations travel through the checks API, so failures stay
# readable even where job logs cannot be downloaded.
set -uo pipefail

log="$(mktemp)"
"$@" >"$log" 2>&1
status=$?

cat "$log"

if [ "$status" -ne 0 ]; then
  echo
  echo "==== failure summary (also published as annotations) ===="
  # Keep the annotations under GitHub's 50-per-step budget and escape the
  # characters the workflow-command parser treats specially.
  tail -n 40 "$log" \
    | sed -e 's/%/%25/g' -e 's/\r/%0D/g' \
    | while IFS= read -r line; do
        printf '::error::%s\n' "$line"
      done
fi

rm -f "$log"
exit "$status"
