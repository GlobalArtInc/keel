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

testkit=$(cargo tree -p keel-testkit -e normal --prefix none --depth 1 | awk 'NR > 1 {print $1}' | grep '^keel' | sort -u | tr '\n' ' ')
if [ "$testkit" != "keel-kernel keel-sdk keel-types " ]; then
  echo "keel-testkit depends on workspace crates other than keel-kernel, keel-sdk and keel-types: $testkit" >&2
  fail=1
fi
echo "keel-testkit graph checked"

system=$(cargo tree -p keel-system --all-features -e normal --prefix none --depth 1 | awk 'NR > 1 {print $1}' | grep '^keel' | sort -u | tr '\n' ' ')
if [ "$system" != "keel-sdk " ]; then
  echo "keel-system depends on workspace crates other than keel-sdk: $system" >&2
  fail=1
fi
echo "keel-system graph checked"

conformance=$(cargo tree -p keel-conformance -e normal,build,dev --prefix none --depth 1 | awk 'NR > 1 {print $1}' | grep '^keel' | sort -u | tr '\n' ' ')
if [ "$conformance" != "keel-types " ]; then
  echo "keel-conformance depends on workspace crates other than keel-types: $conformance" >&2
  fail=1
fi
for forbidden in keel-kernel keel-sdk keel-system keel-testkit keel; do
  if cargo tree -p keel-conformance -e normal,build,dev --prefix none | awk '{print $1}' | grep -qx "$forbidden"; then
    echo "keel-conformance reaches $forbidden through its dependencies" >&2
    fail=1
  fi
done
echo "keel-conformance graph checked"

dev=$(cargo tree -p keel-dev -e normal,build,dev --prefix none --depth 1 | awk 'NR > 1 {print $1}' | grep '^keel' | sort -u | tr '\n' ' ')
if [ "$dev" != "keel-types " ]; then
  echo "keel-dev depends on workspace crates other than keel-types: $dev" >&2
  fail=1
fi
for forbidden in keel-kernel keel-sdk keel-system keel-testkit keel; do
  if cargo tree -p keel-dev -e normal,build,dev --prefix none | awk '{print $1}' | grep -qx "$forbidden"; then
    echo "keel-dev reaches $forbidden through its dependencies" >&2
    fail=1
  fi
done
echo "keel-dev graph checked"

facade=$(cargo tree -p keel -e normal --prefix none --depth 1 | awk 'NR > 1 {print $1}' | grep '^keel' | sort -u | tr '\n' ' ')
if [ "$facade" != "keel-host-agent keel-kernel keel-release keel-sdk keel-types " ]; then
  echo "the facade keel depends on workspace crates other than keel-host-agent, keel-kernel, keel-release, keel-sdk and keel-types: $facade" >&2
  fail=1
fi
echo "keel graph checked"

agent=$(cargo tree -p keel-host-agent -e normal --prefix none --depth 1 | awk 'NR > 1 {print $1}' | { grep -E '^keel(-|$)' || true; } | sort -u | tr '\n' ' ')
for direct in $agent; do
  case "$direct" in
    keel-sdk|keel-types) ;;
    *)
      echo "keel-host-agent depends on the workspace crate $direct; only keel-sdk and keel-types are allowed" >&2
      fail=1
      ;;
  esac
done
for forbidden in keel-kernel keel-system keel-host keel-testkit keel keel-release; do
  if cargo tree -p keel-host-agent -e normal --prefix none | awk '{print $1}' | grep -qx "$forbidden"; then
    echo "keel-host-agent reaches $forbidden through its dependencies" >&2
    fail=1
  fi
done
for forbidden in axum hyper http reqwest sea-orm sqlx tonic; do
  if cargo tree -p keel-host-agent -e normal --prefix none | awk '{print $1}' | grep -qx "$forbidden"; then
    echo "keel-host-agent depends on $forbidden; the agent knows nothing about the database or HTTP" >&2
    fail=1
  fi
done
echo "keel-host-agent graph checked"

release=$(cargo tree -p keel-release -e normal,build,dev --prefix none | awk 'NR > 1 {print $1}' | { grep -E '^keel(-|$)' || true; } | sort -u | tr '\n' ' ')
if [ -n "$release" ]; then
  echo "keel-release depends on workspace crates: $release" >&2
  fail=1
fi
for forbidden in tokio reqwest hyper axum sea-orm sqlx; do
  if cargo tree -p keel-release -e normal --prefix none | awk '{print $1}' | grep -qx "$forbidden"; then
    echo "keel-release depends on $forbidden" >&2
    fail=1
  fi
done
echo "keel-release graph checked"

for forbidden in keel-kernel keel-sdk keel-types; do
  if cargo tree -p keel-bin -e normal --prefix none --depth 1 | awk 'NR > 1 {print $1}' | grep -qx "$forbidden"; then
    echo "keel-bin depends directly on $forbidden; the binary composes through the facade keel" >&2
    fail=1
  fi
done
if ! cargo tree -p keel-bin -e normal --prefix none --depth 1 | awk 'NR > 1 {print $1}' | grep -qx "keel"; then
  echo "keel-bin does not depend on the facade keel" >&2
  fail=1
fi
echo "keel-bin graph checked"

spike=$(cargo tree -p keel-spike -e normal,build,dev --prefix none | awk 'NR > 1 {print $1}' | { grep -E '^keel(-|$)' || true; } | sort -u | tr '\n' ' ')
if [ -n "$spike" ]; then
  echo "keel-spike depends on workspace crates: $spike" >&2
  fail=1
fi
echo "keel-spike graph checked"

products=$(cargo metadata --format-version 1 --no-deps | python3 -c '
import json, sys
meta = json.load(sys.stdin)
root = meta["workspace_root"] + "/products/"
print(" ".join(sorted(p["name"] for p in meta["packages"] if p["manifest_path"].startswith(root))))
')
if cargo metadata --format-version 1 --no-deps | python3 -c '
import json, sys
meta = json.load(sys.stdin)
root = meta["workspace_root"]
products = {p["name"] for p in meta["packages"] if p["manifest_path"].startswith(root + "/products/")}
bad = 0
for package in meta["packages"]:
    if package["manifest_path"].startswith(root + "/bin/"):
        continue
    for dependency in package["dependencies"]:
        if dependency["name"] in products:
            kind = dependency["kind"] or "normal"
            print(package["name"] + " depends on the product crate " + dependency["name"] + " (" + kind + "); only bin/* may", file=sys.stderr)
            bad = 1
sys.exit(bad)
'; then
  echo "no package outside bin/* depends on a product crate"
else
  fail=1
fi
for product in $products; do
  direct=$(cargo tree -p "$product" -e normal --prefix none --depth 1 | awk 'NR > 1 {print $1}' | sort -u | tr '\n' ' ')
  if [ "$direct" != "keel " ] && [ "$direct" != "keel keel-system " ]; then
    echo "$product depends on more than the facade keel and the api modules of keel-system: $direct" >&2
    fail=1
  fi
  dependants=$(cargo tree -i "$product" -e normal --prefix none | awk 'NR > 1 {print $1}' | sort -u | tr '\n' ' ')
  if [ "$dependants" != "keel-bin " ]; then
    echo "$product is depended on by crates other than keel-bin: $dependants" >&2
    fail=1
  fi
  echo "$product graph checked"
done

exit "$fail"
