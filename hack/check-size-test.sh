#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

sized() {
  dd if=/dev/zero of="$1" bs=1 count=0 seek="$2" 2> /dev/null
}

fail() {
  echo "$1" >&2
  exit 1
}

cap=$(tr -d '[:space:]' < hack/size-cap)
sized "$work/at-cap" "$cap"
sized "$work/above-cap" "$((cap + 1))"

hack/check-size.sh "$work/at-cap" > "$work/at-cap.log" 2>&1 \
  || fail "a binary of exactly the cap in hack/size-cap was refused: $(cat "$work/at-cap.log")"
if hack/check-size.sh "$work/above-cap" > "$work/above-cap.log" 2>&1; then
  fail "a binary one byte above the cap in hack/size-cap passed: $(cat "$work/above-cap.log")"
fi
grep -q "1 bytes above the cap" "$work/above-cap.log" \
  || fail "the refusal does not name the excess: $(cat "$work/above-cap.log")"

echo 1000 > "$work/cap"
sized "$work/small" 1000
sized "$work/large" 4096
hack/check-size.sh "$work/small" "$work/cap" > /dev/null 2>&1 \
  || fail "a binary at a cap given as the second argument was refused"
if hack/check-size.sh "$work/large" "$work/cap" > /dev/null 2>&1; then
  fail "a binary above a cap given as the second argument passed"
fi

echo "not a number" > "$work/garbled"
status=0
hack/check-size.sh "$work/small" "$work/garbled" > /dev/null 2>&1 || status=$?
[ "$status" -eq 2 ] || fail "a cap file without a number exits $status, not 2"

echo "the size check passes a binary at the cap of $cap bytes and refuses one byte more"
