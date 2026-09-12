#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
for forbidden in keel-system keel-host keel-host-agent; do
  if cargo metadata --format-version 1 --no-deps | python3 -c '
import json, sys
meta = json.load(sys.stdin)
names = {p["name"] for p in meta["packages"]}
sys.exit(0 if sys.argv[1] in names else 1)
' "$forbidden"; then
    if cargo tree -p keel-kernel -e normal --prefix none 2>/dev/null | awk '{print $1}' | grep -qx "$forbidden"; then
      echo "keel-kernel depends on $forbidden" >&2
      fail=1
    fi
  fi
done

if cargo metadata --format-version 1 --no-deps | python3 -c '
import json, sys
meta = json.load(sys.stdin)
sys.exit(0 if any(p["name"] == "keel-kernel" for p in meta["packages"]) else 1)
'; then
  for required in keel-sdk keel-types; do
    if ! cargo tree -p keel-kernel -e normal --prefix none --depth 1 | awk '{print $1}' | grep -qx "$required"; then
      echo "keel-kernel does not depend on $required" >&2
      fail=1
    fi
  done
  echo "keel-kernel graph checked"
else
  echo "keel-kernel is not in the workspace yet; nothing to check"
fi

for forbidden in tokio async-std smol sqlx sea-orm axum hyper http reqwest; do
  if cargo tree -p keel-types -e normal --prefix none | awk '{print $1}' | grep -qx "$forbidden"; then
    echo "keel-types depends on $forbidden" >&2
    fail=1
  fi
done
echo "keel-types graph checked"

for forbidden in axum cookie cookie_store hyper http reqwest tower tower-http keel-kernel keel-system keel-host; do
  if cargo tree -p keel-sdk -e normal --prefix none | awk '{print $1}' | grep -qx "$forbidden"; then
    echo "keel-sdk depends on $forbidden" >&2
    fail=1
  fi
done
if ! cargo tree -p keel-sdk -e normal --prefix none --depth 1 | awk '{print $1}' | grep -qx "keel-types"; then
  echo "keel-sdk does not depend on keel-types" >&2
  fail=1
fi
echo "keel-sdk graph checked"

direct=$(cargo tree -p keel-sdk-probe -e normal --prefix none --depth 1 | awk 'NR > 1 {print $1}' | sort -u | tr '\n' ' ')
if [ "$direct" != "keel-sdk " ]; then
  echo "keel-sdk-probe depends on more than keel-sdk: $direct" >&2
  fail=1
fi
echo "keel-sdk-probe graph checked"

exit "$fail"
