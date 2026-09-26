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
if [ "$testkit" != "keel-host-agent keel-kernel keel-release keel-sdk keel-types " ]; then
  echo "keel-testkit depends on workspace crates other than keel-host-agent, keel-kernel, keel-release, keel-sdk and keel-types: $testkit" >&2
  fail=1
fi
echo "keel-testkit graph checked"

system=$(cargo tree -p keel-system --all-features -e normal --prefix none --depth 1 | awk 'NR > 1 {print $1}' | grep '^keel' | sort -u | tr '\n' ' ')
if [ "$system" != "keel-sdk " ]; then
  echo "keel-system depends on workspace crates other than keel-sdk: $system" >&2
  fail=1
fi
for forbidden in keel-kernel keel-host keel-host-agent; do
  if cargo tree -p keel-system --all-features -e normal --prefix none | awk '{print $1}' | grep -qx "$forbidden"; then
    echo "keel-system reaches $forbidden through its dependencies" >&2
    fail=1
  fi
done
echo "keel-system graph checked"

host=$(cargo tree -p keel-host -e normal --prefix none --depth 1 | awk 'NR > 1 {print $1}' | { grep -E '^keel(-|$)' || true; } | sort -u | tr '\n' ' ')
if [ "$host" != "keel-sdk " ]; then
  echo "keel-host depends on workspace crates other than keel-sdk: $host" >&2
  fail=1
fi
for forbidden in keel-kernel keel-system keel-host-agent keel-testkit keel; do
  if cargo tree -p keel-host -e normal --prefix none | awk '{print $1}' | grep -qx "$forbidden"; then
    echo "keel-host reaches $forbidden through its dependencies" >&2
    fail=1
  fi
done
echo "keel-host graph checked"

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

dev=$(cargo tree -p keel-dev -e normal,build --prefix none --depth 1 | awk 'NR > 1 {print $1}' | grep '^keel' | sort -u | tr '\n' ' ')
if [ "$dev" != "keel-release keel-types " ]; then
  echo "keel-dev depends on workspace crates other than keel-release and keel-types: $dev" >&2
  fail=1
fi
for forbidden in keel-kernel keel-sdk keel-system keel-testkit keel; do
  if cargo tree -p keel-dev -e normal,build --prefix none | awk '{print $1}' | grep -qx "$forbidden"; then
    echo "keel-dev reaches $forbidden through its dependencies" >&2
    fail=1
  fi
done
tests=$(cargo tree -p keel-dev -e normal,build,dev --prefix none --depth 1 | awk 'NR > 1 {print $1}' | grep '^keel' | sort -u | tr '\n' ' ')
if [ "$tests" != "keel-release keel-sdk keel-system keel-testkit keel-types " ]; then
  echo "the tests of keel-dev depend on workspace crates other than keel-release, keel-sdk, keel-system, keel-testkit and keel-types: $tests" >&2
  fail=1
fi
if cargo tree -p keel-dev -e normal,build,dev --prefix none | awk '{print $1}' | grep -qx keel; then
  echo "the tests of keel-dev reach the facade keel" >&2
  fail=1
fi
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

products=$(cargo metadata --format-version 1 --no-deps | python3 -c '
import json, sys
meta = json.load(sys.stdin)
root = meta["workspace_root"] + "/products/"
print(" ".join(sorted(p["name"] for p in meta["packages"] if p["manifest_path"].startswith(root))))
')
if cargo metadata --format-version 1 --no-deps | python3 -c '
import json, os, sys
meta = json.load(sys.stdin)
root = meta["workspace_root"]
dirs = {p["name"]: os.path.dirname(p["manifest_path"]) for p in meta["packages"] if p["manifest_path"].startswith(root + "/products/")}
bad = 0
def bundle_of(path):
    parent = os.path.dirname(path)
    return parent if os.path.basename(parent) == "crates" else None
for package in meta["packages"]:
    if package["manifest_path"].startswith(root + "/bin/"):
        continue
    here = os.path.dirname(package["manifest_path"])
    own = here + "/crates/"
    for dependency in package["dependencies"]:
        name = dependency["name"]
        sibling = package["name"] in dirs and name in dirs and bundle_of(here) is not None and bundle_of(here) == bundle_of(dirs[name])
        bundles = package["name"] in dirs and name in dirs and os.path.dirname(here) == root + "/products" and os.path.dirname(dirs[name]) == root + "/products"
        bridged = bundles and dependency["kind"] == "dev"
        if name in dirs and not sibling and not bridged and not (package["name"] in dirs and dirs[name].startswith(own)):
            kind = dependency["kind"] or "normal"
            print(package["name"] + " depends on the product crate " + name + " (" + kind + "); only bin/*, the bundle that owns a module crate, the module crates of that bundle and the tests of another bundle may", file=sys.stderr)
            bad = 1
sys.exit(bad)
'; then
  echo "no package outside bin/* depends on a product crate other than a module crate of its own bundle, from a module crate another module crate of that bundle, or from a bundle's tests another bundle"
else
  fail=1
fi
product_rules() {
  cargo metadata --format-version 1 --no-deps | python3 -c '
import json, os, sys
meta = json.load(sys.stdin)
root = meta["workspace_root"]
dirs = {p["name"]: os.path.dirname(p["manifest_path"]) for p in meta["packages"] if p["manifest_path"].startswith(root + "/products/")}
name = sys.argv[1]
modules = sorted(other for other, path in dirs.items() if path.startswith(dirs[name] + "/crates/"))
owners = sorted(other for other, path in dirs.items() if dirs[name].startswith(path + "/crates/"))
parent = os.path.dirname(dirs[name])
siblings = sorted(other for other, path in dirs.items() if other != name and os.path.basename(parent) == "crates" and os.path.dirname(path) == parent)
print(" ".join(modules))
print(" ".join(sorted(["keel-bin"] + owners)) + " ")
print(" ".join(siblings))
' "$1"
}
for product in $products; do
  rules=$(product_rules "$product")
  modules=$(sed -n 1p <<<"$rules")
  expected_dependants=$(sed -n 2p <<<"$rules")
  siblings=$(sed -n 3p <<<"$rules")
  direct=$(cargo tree -p "$product" --all-features -e normal --prefix none --depth 1 | awk 'NR > 1 {print $1}' | { grep -vx keel-host || true; } | sort -u | tr '\n' ' ')
  for module in $modules $siblings; do
    direct=$(tr ' ' '\n' <<<"$direct" | { grep -vx "$module" || true; } | { grep -v '^$' || true; } | tr '\n' ' ')
  done
  if [ "$direct" != "keel " ] && [ "$direct" != "keel keel-system " ]; then
    echo "$product depends on more than the facade keel, keel-host, the api modules of keel-system, its own module crates and the other module crates of its bundle: $direct" >&2
    fail=1
  fi
  dependants=$(cargo tree -i "$product" -e normal --prefix none | awk 'NR > 1 {print $1}' | sort -u | tr '\n' ' ')
  for sibling in $siblings; do
    dependants=$(tr ' ' '\n' <<<"$dependants" | { grep -vx "$sibling" || true; } | { grep -v '^$' || true; } | tr '\n' ' ')
  done
  if [ "$dependants" != "$expected_dependants" ]; then
    echo "$product is depended on by $dependants; only $expected_dependants may" >&2
    fail=1
  fi
  echo "$product graph checked"
done
exit "$fail"
