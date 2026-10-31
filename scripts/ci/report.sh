#!/usr/bin/env bash
# Runs a command and, on failure, republishes its diagnostics as GitHub
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
  free -m 2>/dev/null | head -3 || true
  df -h . 2>/dev/null | tail -1 || true

  annotated=0

  # Rustc/clippy diagnostics first, with the lines that follow each one: the
  # location is useful, but "expected X, found Y" is what actually explains it.
  while IFS= read -r line; do
    printf '::error::%s\n' "$(printf '%s' "$line" | sed -e 's/%/%25/g' -e 's/\r/%0D/g')"
    annotated=$((annotated + 1))
    [ "$annotated" -ge 36 ] && break
  done < <(grep -A 4 -E '^error(\[|:)|^error: could not compile' "$log" | head -36 || true)

  # Then the tail, which catches failures without diagnostics (killed builds).
  if [ "$annotated" -lt 40 ]; then
    remaining=$((40 - annotated))
    tail -n "$remaining" "$log" \
      | sed -e 's/%/%25/g' -e 's/\r/%0D/g' \
      | while IFS= read -r line; do
          printf '::error::%s\n' "$line"
        done
  fi
fi

rm -f "$log"
exit "$status"
