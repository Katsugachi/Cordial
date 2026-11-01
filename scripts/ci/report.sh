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
  # GitHub limits annotations per step. Put the complete tail in the first
  # annotation so CMake's underlying error is not lost behind Rust's wrapper.
  summary=$(tail -n 120 "$log" | sed -e 's/%/%25/g' -e 's//%0D/g' | awk '{printf "%s%%0A", $0}')
  printf '::error title=Build failure details::%s\n' "$summary"
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

  # Test failures: the assertion values matter more than the stack trace.
  while IFS= read -r line; do
    printf '::error::%s\n' "$(printf '%s' "$line" | sed -e 's/%/%25/g' -e 's/\r/%0D/g')"
    annotated=$((annotated + 1))
    [ "$annotated" -ge 44 ] && break
  done < <(grep -E 'panicked at|left:|right:|assertion|---- .* stdout ----' "$log" | head -8 || true)

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
