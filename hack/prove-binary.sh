#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

timeout=${KEEL_PROVE_TIMEOUT:-30}
keel=${1:-}
admin_url=${KEEL_TEST_DATABASE_URL:-}
scheme=https
cacert=""
database=""
site=""
serve_pid=""
serve_log=""
channel_pid=""
available_version=""

usage() {
  echo "usage: hack/prove-binary.sh [path/to/keel]" >&2
  echo "  KEEL_TEST_DATABASE_URL must point at a Postgres superuser connection;" >&2
  echo "  without a path the binary is built with cargo build -p keel-bin" >&2
}

fail() {
  echo "FAILED: $1" >&2
  exit 1
}

check() {
  local name=$1
  shift
  if "$@"; then
    echo "ok: $name"
  else
    fail "$name"
  fi
}

cleanup() {
  local code=$?
  trap - EXIT
  if [ -n "$serve_pid" ]; then
    echo "stopping keel serve ($serve_pid)"
    if ! stop_serve; then
      code=1
    fi
  fi
  if [ -n "$channel_pid" ]; then
    kill "$channel_pid" 2>/dev/null || true
    wait "$channel_pid" 2>/dev/null || true
    echo "stopped the release channel ($channel_pid)"
  fi
  if [ -n "$database" ]; then
    if psql "$admin_url" -qAt -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS \"$database\" WITH (FORCE)" >/dev/null; then
      echo "dropped database $database"
    else
      echo "could not drop database $database" >&2
      code=1
    fi
  fi
  if [ -n "$site" ] && [ -d "$site" ]; then
    rm -rf "$site"
    echo "removed $site"
  fi
  if [ "$code" -eq 0 ]; then
    echo "the binary is proven"
  fi
  exit "$code"
}

database_url() {
  local url=$1 name=$2
  local base=${url%%\?*}
  local query=${url#"$base"}
  local rest=${base#*://}
  local authority=${rest%%/*}
  local scheme=${base%"$rest"}
  echo "${scheme}${authority}/${name}${query}"
}

free_port() {
  python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}

json_is() {
  local file=$1 expression=$2
  python3 - "$file" "$expression" <<'EOF'
import json, sys
with open(sys.argv[1]) as handle:
    doc = json.load(handle)
if not eval(sys.argv[2], {"doc": doc, "re": __import__("re")}):
    print("  " + json.dumps(doc)[:400], file=sys.stderr)
    sys.exit(1)
EOF
}

json_get() {
  local file=$1 expression=$2
  python3 - "$file" "$expression" <<'EOF'
import json, sys
with open(sys.argv[1]) as handle:
    doc = json.load(handle)
print(eval(sys.argv[2], {"doc": doc, "json": json}))
EOF
}

header_of() {
  local file=$1 name=$2
  grep -i "^$name:" "$file" | head -n 1 | cut -d' ' -f2- | tr -d '\r'
}

run_keel() {
  "$keel" --config "$site/etc/config.yaml" "$@"
}

keel_ok() {
  local out=$1
  shift
  if ! run_keel "$@" >"$out" 2>"$site/stderr"; then
    cat "$site/stderr" >&2
    return 1
  fi
}

keel_fails() {
  local out=$1
  shift
  if run_keel "$@" >"$site/stdout" 2>"$out"; then
    cat "$site/stdout" >&2
    return 1
  fi
}

stdout_is() {
  local file=$1 expected=$2
  local actual
  actual=$(cat "$file")
  if [ "$actual" != "$expected" ]; then
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    return 1
  fi
}

stdout_has_line() {
  local file=$1 expected=$2
  if ! grep -qxF -- "$expected" "$file"; then
    echo "  expected a line: $expected" >&2
    sed 's/^/  actual:   /' "$file" >&2
    return 1
  fi
}

table_is() {
  local file=$1 expected=$2
  python3 - "$file" "$expected" <<'EOF'
import json, sys
with open(sys.argv[1]) as handle:
    rows = [line.split() for line in handle.read().splitlines()]
if rows != json.loads(sys.argv[2]):
    print("  " + json.dumps(rows, ensure_ascii=False), file=sys.stderr)
    sys.exit(1)
EOF
}

curl_tls() {
  if [ -n "$cacert" ]; then
    curl --cacert "$cacert" "$@"
  else
    curl "$@"
  fi
}

ready() {
  if [ -n "$cacert" ] && [ ! -f "$cacert" ]; then
    return 1
  fi
  [ "$(curl_tls -s -o /dev/null -w '%{http_code}' "$scheme://127.0.0.1:$port/readyz" 2>/dev/null)" = "200" ]
}

http() {
  local out=$1 method=$2 path=$3 body=${4:-} credential=${5:-} extra=${6:-}
  local status
  local -a headers=(-H 'Content-Type: application/json')
  case "$credential" in
    Bearer\ *) headers+=(-H "Authorization: $credential") ;;
    ?*) headers+=(-H "Cookie: $credential") ;;
  esac
  if [ -n "$extra" ]; then
    headers+=(-H "$extra")
  fi
  if [ -n "$body" ]; then
    status=$(curl_tls -sS -o "$out" -D "$out.headers" -w '%{http_code}' -X "$method" "${headers[@]}" --data "$body" "$scheme://127.0.0.1:$port$path")
  else
    status=$(curl_tls -sS -o "$out" -D "$out.headers" -w '%{http_code}' -X "$method" "${headers[@]}" "$scheme://127.0.0.1:$port$path")
  fi
  echo "$status"
}

svg_is_served() {
  local file=$1
  local name expected actual
  for name in content-type cache-control x-content-type-options; do
    case "$name" in
      content-type) expected="image/svg+xml" ;;
      cache-control) expected="no-store" ;;
      x-content-type-options) expected="nosniff" ;;
    esac
    actual=$(header_of "$file.headers" "$name")
    if [ "$actual" != "$expected" ]; then
      echo "  $name: expected '$expected', got '$actual'" >&2
      return 1
    fi
  done
}

index_is_served() {
  local file=$1
  if ! grep -q '<keel-shell>' "$file"; then
    echo "  the body carries no <keel-shell> element" >&2
    return 1
  fi
  local name expected actual
  for name in cache-control content-security-policy x-content-type-options referrer-policy; do
    case "$name" in
      cache-control) expected="no-store" ;;
      content-security-policy) expected="default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'" ;;
      x-content-type-options) expected="nosniff" ;;
      referrer-policy) expected="same-origin" ;;
    esac
    actual=$(header_of "$file.headers" "$name")
    if [ "$actual" != "$expected" ]; then
      echo "  $name: expected '$expected', got '$actual'" >&2
      return 1
    fi
  done
}

http_is() {
  local out=$1 expected=$2 method=$3 path=$4 body=${5:-} credential=${6:-} extra=${7:-}
  local status
  status=$(http "$out" "$method" "$path" "$body" "$credential" "$extra")
  if [ "$status" != "$expected" ]; then
    echo "  $method $path answered $status, expected $expected" >&2
    sed 's/^/  /' "$out" >&2
    return 1
  fi
}

totp_code() {
  local secret=$1 offset=${2:-0}
  python3 - "$secret" "$offset" <<'EOF'
import base64, hashlib, hmac, struct, sys, time
secret = sys.argv[1]
key = base64.b32decode(secret + "=" * (-len(secret) % 8))
step = int(time.time()) // 30 + int(sys.argv[2])
digest = hmac.new(key, struct.pack(">Q", step), hashlib.sha1).digest()
offset = digest[-1] & 15
print("%06d" % ((struct.unpack(">I", digest[offset:offset + 4])[0] & 0x7FFFFFFF) % 1000000))
EOF
}

header_matches() {
  local file=$1 name=$2 pattern=$3
  header_of "$file" "$name" | grep -qi "$pattern"
}

session_cookie_of() {
  local file=$1
  local value
  value=$(header_of "$file" set-cookie | cut -d';' -f1)
  case "$value" in
    __Host-keel_session=ks_*) echo "$value" ;;
    *) echo "  Set-Cookie is not a session cookie: ${value:-absent}" >&2; return 1 ;;
  esac
}

start_serve() {
  local label=$1
  serve_log="$site/serve-$label.log"
  "$keel" --config "$site/etc/config.yaml" serve >"$serve_log" 2>&1 &
  serve_pid=$!
  local waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if ! kill -0 "$serve_pid" 2>/dev/null; then
      sed 's/^/  /' "$serve_log" >&2
      serve_pid=""
      fail "keel serve ($label) exited before it was ready"
    fi
    if ready; then
      echo "ok: keel serve ($label) is ready on $scheme://127.0.0.1:$port"
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  sed 's/^/  /' "$serve_log" >&2
  fail "keel serve ($label) did not answer /readyz within ${timeout}s"
}

stop_serve() {
  if [ -z "$serve_pid" ]; then
    return 0
  fi
  local pid=$serve_pid
  serve_pid=""
  if ! kill -0 "$pid" 2>/dev/null; then
    wait "$pid" 2>/dev/null || true
    return 0
  fi
  kill -TERM "$pid" 2>/dev/null || true
  local waited=0
  while kill -0 "$pid" 2>/dev/null && [ "$waited" -lt "$((timeout * 10))" ]; do
    sleep 0.1
    waited=$((waited + 1))
  done
  if kill -0 "$pid" 2>/dev/null; then
    echo "keel serve did not stop on SIGTERM within ${timeout}s; sending SIGKILL" >&2
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    return 1
  fi
  local status=0
  wait "$pid" || status=$?
  if [ "$status" -ne 0 ]; then
    echo "keel serve exited with $status" >&2
    [ -n "$serve_log" ] && sed 's/^/  /' "$serve_log" >&2
    return 1
  fi
}

serve_exits_with() {
  local expected=$1
  if [ -z "$serve_pid" ]; then
    echo "  keel serve is not running" >&2
    return 1
  fi
  local pid=$serve_pid
  local waited=0
  while kill -0 "$pid" 2>/dev/null && [ "$waited" -lt "$((timeout * 10))" ]; do
    sleep 0.1
    waited=$((waited + 1))
  done
  if kill -0 "$pid" 2>/dev/null; then
    echo "  keel serve did not exit within ${timeout}s" >&2
    sed 's/^/  /' "$serve_log" >&2
    return 1
  fi
  serve_pid=""
  local status=0
  wait "$pid" || status=$?
  if [ "$status" -ne "$expected" ]; then
    echo "  keel serve exited with $status, not $expected" >&2
    sed 's/^/  /' "$serve_log" >&2
    return 1
  fi
}

socket_is_gone() {
  [ ! -e "$site/run/core.sock" ]
}

task_reaches() {
  local out=$1 id=$2 state=$3 at_least=${4:-0}
  local waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if run_keel ctl task.get "id=$id" >"$out" 2>"$site/stderr" && json_is "$out" "doc['ok'] is True and doc['data']['state'] == '$state' and doc['data']['done'] >= $at_least" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  echo "  task $id did not reach $state within ${timeout}s" >&2
  sed 's/^/  /' "$out" >&2
  return 1
}

task_sql() {
  psql "$(database_url "$admin_url" "$database")" -qAt -c "$1"
}

sql_is() {
  local sql=$1 expected=$2
  local actual
  actual=$(task_sql "$sql")
  if [ "$actual" != "$expected" ]; then
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    return 1
  fi
}

unclaimed_section_logged() {
  local section=$1
  grep 'claimed by no loaded module' "$serve_log" | grep -q "\"$section\""
}

write_channel() {
  local dir=$1 running=$2 available=$3
  python3 - "$dir" "$running" "$available" <<'EOF'
import base64, json, os, sys
root, running, available = sys.argv[1:4]

def signed(path, manifest):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    payload = base64.b64encode(json.dumps(manifest).encode()).decode()
    with open(path, "w") as handle:
        json.dump({"payload": payload, "signature": base64.b64encode(bytes(64)).decode()}, handle)

signed(os.path.join(root, "stable", "manifest.json"), {"version": available, "sha256": "00", "min_upgrade_from": running, "contract": 1, "notes": "Rehearsal release " + available})
signed(os.path.join(root, "versions", available, "manifest.json"), {"version": available, "sha256": "00", "min_upgrade_from": running, "contract": 1, "notes": "Rehearsal release " + available})
signed(os.path.join(root, "versions", running, "manifest.json"), {"version": running, "sha256": "00", "min_upgrade_from": "0.0.1", "contract": 1, "notes": "Running release " + running})
EOF
}

start_channel() {
  local dir=$1 port=$2
  python3 -m http.server "$port" --bind 127.0.0.1 --directory "$dir" >"$site/channel.log" 2>&1 &
  channel_pid=$!
  local waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if curl -sf -o /dev/null "http://127.0.0.1:$port/stable/manifest.json"; then
      echo "ok: the release channel answers on http://127.0.0.1:$port/"
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  sed 's/^/  /' "$site/channel.log" >&2
  fail "the release channel did not come up within ${timeout}s"
}

if [ -z "$admin_url" ]; then
  usage
  echo "KEEL_TEST_DATABASE_URL is not set" >&2
  exit 2
fi
for tool in psql curl python3; do
  if ! command -v "$tool" >/dev/null; then
    echo "$tool is not installed" >&2
    exit 2
  fi
done

if [ -z "$keel" ]; then
  cargo build -p keel-bin
  keel="${CARGO_TARGET_DIR:-target}/debug/keel"
fi
case "$keel" in
  /*) ;;
  *) keel="$PWD/$keel" ;;
esac
if [ ! -x "$keel" ]; then
  fail "$keel is not an executable binary"
fi

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

version=$("$keel" --version | awk '{print $2}')
tmp=${TMPDIR:-/tmp}
site=$(mktemp -d "${tmp%/}/keel-prove.XXXXXX")
database="keel_test_prove_$(date +%s)_$$"
port=$(free_port)
psql "$admin_url" -qAt -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$database\" TEMPLATE template1" >/dev/null
echo "created database $database"
mkdir -p "$site/run" "$site/lib" "$site/channel" "$site/etc"
if [ "$scheme" = https ]; then
  cacert="$site/lib/tls/cert.pem"
fi
channel_port=$(free_port)
available_version=$(python3 -c 'import sys; major, minor, patch = sys.argv[1].split("-")[0].split("."); print(f"{major}.{int(minor) + 1}.0")' "$version")
write_channel "$site/channel" "$version" "$available_version"
start_channel "$site/channel" "$channel_port"
cat >"$site/etc/config.yaml" <<EOF
listen: 127.0.0.1:$port
socket: $site/run/core.sock
data_dir: $site/lib
database: $(database_url "$admin_url" "$database")
log: { level: info, format: text }
host: { socket: /nowhere/agent.sock }
update: { channel_url: http://127.0.0.1:$channel_port/ }
EOF
echo "site $site, binary $keel $version, channel offers $available_version"

echo "--- run 1: the demo bundle enabled"
check "product enable demo" keel_ok "$site/enable.out" product enable demo
check "product enable demo prints demo: enabled" stdout_is "$site/enable.out" "demo: enabled"
check "product list" keel_ok "$site/list.out" product list
check "product list shows demo enabled" stdout_has_line "$site/list.out" "$(printf 'demo\tenabled\t%s' "$version")"
start_serve demo
check "the unclaimed host section is logged and ignored" unclaimed_section_logged host

check "ctl keel.account.create kind=user" keel_ok "$site/account.json" ctl keel.account.create kind=user
check "keel.account.create answers ok with an id" json_is "$site/account.json" 'doc["ok"] is True and re.fullmatch("[0-7][0-9A-HJKMNP-TV-Z]{25}", doc["data"]["id"])'
check "GET /healthz is 200" http_is "$site/healthz.json" 200 GET /healthz
check "healthz reports ok" json_is "$site/healthz.json" 'doc == {"status": "ok"}'
check "GET / is 200" http_is "$site/index.html" 200 GET /
check "the root answers the shell's index no-store with the policy headers" index_is_served "$site/index.html"
check "GET /f/note.list is 200" http_is "$site/screen.html" 200 GET /f/note.list
check "a screen path falls back to the same index" cmp -s "$site/index.html" "$site/screen.html"
check "GET /missing.png is 404" http_is "$site/missing.json" 404 GET /missing.png
check "a missing asset answers the route_not_found envelope" json_is "$site/missing.json" 'doc["ok"] is False and doc["error"]["key"] == "keel.error.route_not_found"'
check "GET /api/v1/boot is 200" http_is "$site/boot.json" 200 GET /api/v1/boot
check "boot lists the demo product" json_is "$site/boot.json" 'any(p["id"] == "demo" for p in doc["products"])'
check "boot carries the demo brand" json_is "$site/boot.json" 'doc["brand"]["name"] == "product.demo.brand" and doc["brand"]["hue"] == 264 and doc["brand"]["chroma"] == 0.18'
check "boot names the keel version" json_is "$site/boot.json" "doc['keel']['version'] == '$version' and doc['keel']['api'] == 1"
check "boot describes the demo product with its brand" json_is "$site/boot.json" 'doc["products"][0] == {"id": "demo", "name": "product.demo.brand", "edition": None, "brand": {"name": "product.demo.brand", "logo": "/brand/demo/logo.svg", "hue": 264, "chroma": 0.18}}'
check "GET /brand/logo.svg is 200" http_is "$site/logo.svg" 200 GET /brand/logo.svg
check "the effective logo is the demo's SVG" cmp -s "$site/logo.svg" products/demo/brand/logo.svg
check "the effective logo is served as image/svg+xml no-store" svg_is_served "$site/logo.svg"
check "GET /brand/demo/logo.svg is 200" http_is "$site/logo-demo.svg" 200 GET /brand/demo/logo.svg
check "the demo's logo route answers the same SVG" cmp -s "$site/logo-demo.svg" products/demo/brand/logo.svg
check "GET /brand/nothing/logo.svg is 404" http_is "$site/logo-nothing.json" 404 GET /brand/nothing/logo.svg
check "an unknown product's logo answers not_found" json_is "$site/logo-nothing.json" 'doc["ok"] is False and doc["error"]["key"] == "keel.error.not_found"'
check "ctl settings.brand.set for panel.example" keel_ok "$site/brand-set.json" ctl settings.brand.set host=panel.example name="Panel Example" hue=350 chroma=0.2
check "settings.brand.set answers the row without a logo" json_is "$site/brand-set.json" 'doc["ok"] is True and doc["data"]["host"] == "panel.example" and doc["data"]["hue"] == 350 and doc["data"]["logo"] is None'
check "ctl settings.brand.logo uploads the logo of panel.example" keel_ok "$site/brand-logo.json" ctl settings.brand.logo host=panel.example logo="$(cat crates/kernel/assets/keel.svg)"
check "settings.brand.logo answers the row with the logo" json_is "$site/brand-logo.json" 'doc["ok"] is True and doc["data"]["host"] == "panel.example" and doc["data"]["logo"].startswith("<svg")'
check "settings.brand.list flags the logo" keel_ok "$site/brand-list.json" ctl settings.brand.list
check "the brand row carries logo true" json_is "$site/brand-list.json" 'doc["ok"] is True and [row["logo"] for row in doc["data"]["data"] if row["host"] == "panel.example"] == [True]'
check "GET /api/v1/boot for panel.example is 200" http_is "$site/boot-brand.json" 200 GET /api/v1/boot '' '' 'Host: panel.example:1500'
check "boot for panel.example carries the patched brand" json_is "$site/boot-brand.json" 'doc["brand"] == {"name": "Panel Example", "logo": "/brand/logo.svg", "hue": 350, "chroma": 0.2}'
check "boot for another host keeps the demo brand" json_is "$site/boot.json" 'doc["brand"]["name"] == "product.demo.brand"'
check "GET /brand/logo.svg for panel.example is 200" http_is "$site/logo-brand.svg" 200 GET /brand/logo.svg '' '' 'Host: panel.example'
check "the effective logo for panel.example is the patched SVG" stdout_is "$site/logo-brand.svg" "$(cat crates/kernel/assets/keel.svg)"
check "ctl settings.brand.logo with a script element is refused" keel_fails "$site/brand-script.json" ctl settings.brand.logo host=panel.example logo='<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>'
check "the scripted logo answers 422 on logo" json_is "$site/brand-script.json" 'doc["ok"] is False and doc["error"]["code"] == "validation" and doc["error"]["fields"][0]["path"] == "logo" and doc["error"]["fields"][0]["key"] == "settings.error.brand_svg"'
check "ctl settings.brand.delete for panel.example" keel_ok "$site/brand-delete.json" ctl settings.brand.delete host=panel.example
check "settings.brand.delete answers deleted" json_is "$site/brand-delete.json" 'doc["ok"] is True and doc["data"] == {"deleted": True}'
check "GET /api/v1/boot for panel.example after the delete is 200" http_is "$site/boot-brand-gone.json" 200 GET /api/v1/boot '' '' 'Host: panel.example:1500'
check "boot for panel.example is back to the demo brand" json_is "$site/boot-brand-gone.json" 'doc["brand"]["name"] == "product.demo.brand" and doc["brand"]["hue"] == 264'
check "ctl keel.product.list --out json" keel_ok "$site/products.json" ctl keel.product.list --out json
check "keel.product.list answers the demo enabled with its modules" json_is "$site/products.json" 'doc["ok"] is True and [(row["id"], row["state"], row["enabledBy"], row["modules"]) for row in doc["data"]["data"]] == [("demo", "enabled", "install", ["note"])]'
check "update check --out json" keel_ok "$site/update-check.json" update check --out json
check "update check answers the running version, the channel head and that the update is allowed" json_is "$site/update-check.json" "doc['ok'] is True and doc['data']['running'] == '$version' and doc['data']['available'] == '$available_version' and doc['data']['channel'] == 'stable' and doc['data']['channelUrl'] == 'http://127.0.0.1:$channel_port/' and doc['data']['minUpgradeFrom'] == '$version' and doc['data']['contract'] == 1 and doc['data']['runningContract'] == 1 and doc['data']['allowed'] is True and doc['data']['upToDate'] is False and doc['data']['blockedBy'] is None and doc['data']['previous'] is None and doc['data']['notes'] == 'Rehearsal release $available_version'"
check "update check --out table" keel_ok "$site/update-check.out" update check --out table
check "the table carries the running and the available version" grep -q "running *$version" "$site/update-check.out"
check "ctl update.check answers the same status over the socket" keel_ok "$site/update-check-ctl.json" ctl update.check --out json
check "update.check over ctl and update check agree" cmp -s <(json_get "$site/update-check.json" 'json.dumps(doc["data"], sort_keys=True)') <(json_get "$site/update-check-ctl.json" 'json.dumps(doc["data"], sort_keys=True)')
check "ctl note.edit title=x" keel_ok "$site/edit.json" ctl note.edit title=x
check "note.edit answers the saved note" json_is "$site/edit.json" 'doc["ok"] is True and doc["data"]["title"] == "x" and doc["meta"]["principal"]["login"] == "owner"'
check "ctl note.list --out json" keel_ok "$site/notes.json" ctl note.list --out json
check "note.list answers the page with the note" json_is "$site/notes.json" 'doc["ok"] is True and [row["title"] for row in doc["data"]["data"]] == ["x"]'
check "ctl note.import.check with an empty source is refused" keel_fails "$site/import-check.json" ctl note.import.check text=
check "note.import.check answers note.error.empty on text" json_is "$site/import-check.json" 'doc["ok"] is False and doc["error"]["code"] == "validation" and doc["error"]["fields"][0]["path"] == "text" and doc["error"]["fields"][0]["key"] == "note.error.empty"'
check "ctl note.import with two lines" keel_ok "$site/import.json" ctl note.import "text=$(printf 'alpha\nbeta')" 'tags=["proven"]' kind=todo
check "note.import answers two imported notes" json_is "$site/import.json" 'doc["ok"] is True and doc["data"]["imported"] == 2 and len(doc["data"]["ids"]) == 2'
check "ctl note.list --out json after the import" keel_ok "$site/notes-imported.json" ctl note.list --out json 'sort={"key":"title","dir":"asc"}'
check "note.list answers the imported todos with their tags" json_is "$site/notes-imported.json" 'doc["ok"] is True and [(row["title"], row["kind"], row["tags"]) for row in doc["data"]["data"]] == [("alpha", "todo", ["proven"]), ("beta", "todo", ["proven"]), ("x", "memo", [])]'
check "ctl note.tag.list --out table" keel_ok "$site/tags.out" ctl note.tag.list --out table
check "note.tag.list renders the tag table with translated headers" table_is "$site/tags.out" '[["Tag", "Count"], ["proven", "2"]]'
check "ctl note.tag.list --out table --lang ru" keel_ok "$site/tags-ru.out" ctl note.tag.list --out table --lang ru
ru_tag_headers=$(python3 -c 'import json; c = json.load(open("products/demo/i18n/note/ru.json"))["note"]["tag"]["column"]; print(json.dumps([[c["tag"], c["count"]], ["proven", "2"]], ensure_ascii=False))')
check "note.tag.list renders the Russian headers" table_is "$site/tags-ru.out" "$ru_tag_headers"
check "POST /api/v1/call/note.list without a credential is 401" http_is "$site/call.json" 401 POST /api/v1/call/note.list '{}'
check "the call answers the unauthenticated envelope" json_is "$site/call.json" 'doc["ok"] is False and doc["error"]["code"] == "unauthenticated" and doc["error"]["key"] == "keel.error.unauthenticated"'
account_id=$(json_get "$site/account.json" 'doc["data"]["id"]')
check "ctl keel.principal.create login=alice" keel_ok "$site/principal.json" ctl keel.principal.create login=alice level=user "account=$account_id"
check "keel.principal.create answers ok with an id" json_is "$site/principal.json" 'doc["ok"] is True and re.fullmatch("[0-7][0-9A-HJKMNP-TV-Z]{25}", doc["data"]["id"])'
principal_id=$(json_get "$site/principal.json" 'doc["data"]["id"]')
check "ctl auth.password.set for alice" keel_ok "$site/password.json" ctl auth.password.set "principal=$principal_id" password=correct-horse
check "ctl settings.list --out json" keel_ok "$site/settings.json" ctl settings.list --out json
check "settings.list answers the six auth definitions, cron.overrides, update.channel and webhook.private_targets with their defaults" json_is "$site/settings.json" 'doc["ok"] is True and {row["key"]: (row["default"], row["value"], row["scope"]) for row in doc["data"]["data"]} == {"auth.lockout.minutes": (15, 15, None), "auth.lockout.threshold": (5, 5, None), "auth.password.min_length": (12, 12, None), "auth.rate.guest_per_minute": (30, 30, None), "auth.session.absolute_hours": (12, 12, None), "auth.session.idle_minutes": (30, 30, None), "cron.overrides": ({}, {}, None), "update.channel": ("stable", "stable", None), "webhook.private_targets": (False, False, None)}'
check "ctl settings.set auth.password.min_length=16" keel_ok "$site/setting.json" ctl settings.set key=auth.password.min_length value=16
check "settings.set answers the root value" json_is "$site/setting.json" 'doc["ok"] is True and doc["data"]["value"] == 16 and doc["data"]["scope"] == "00000000000000000000000000"'
check "ctl auth.password.set with twelve characters is refused" keel_fails "$site/password-short.json" ctl auth.password.set "principal=$principal_id" password=twelve-chars
check "the short password answers auth.error.password_short with the raised minimum" json_is "$site/password-short.json" 'doc["ok"] is False and doc["error"]["fields"][0]["path"] == "password" and doc["error"]["fields"][0]["key"] == "auth.error.password_short" and doc["error"]["fields"][0]["params"]["min"] == 16'
check "ctl settings.set auth.password.min_length=12 restores the default" keel_ok "$site/setting-back.json" ctl settings.set key=auth.password.min_length value=12
check "ctl settings.set with a string is 422 on value" keel_fails "$site/setting-wrong.json" ctl settings.set key=auth.password.min_length value=many
check "the wrong value answers a schema error on value" json_is "$site/setting-wrong.json" 'doc["ok"] is False and doc["error"]["fields"][0]["path"] == "value" and doc["error"]["fields"][0]["key"] == "keel.schema.type"'
check "ctl settings.set of an unknown key is not_found" keel_fails "$site/setting-unknown.json" ctl settings.set key=auth.password.max_length value=1
check "the unknown key answers not_found" json_is "$site/setting-unknown.json" 'doc["ok"] is False and doc["error"]["code"] == "not_found"'
check "ctl settings.get auth.password.min_length" keel_ok "$site/setting-get.json" ctl settings.get key=auth.password.min_length
check "settings.get answers the restored value at the root" json_is "$site/setting-get.json" 'doc["ok"] is True and doc["data"]["value"] == 12 and doc["data"]["scope"] == "00000000000000000000000000"'
check "auth.password.set answers the principal" json_is "$site/password.json" "doc['ok'] is True and doc['data']['principal'] == '$principal_id'"
check "ctl note.edit title=hers in the account of alice" keel_ok "$site/hers.json" ctl note.edit title=hers "account=$account_id"
check "ctl keel.account.list --as alice" keel_ok "$site/accounts-as-alice.json" ctl keel.account.list --as alice
check "keel.account.list as alice answers her account with the owner as actor" json_is "$site/accounts-as-alice.json" "doc['ok'] is True and [row['id'] for row in doc['data']['data']] == ['$account_id'] and doc['meta']['principal']['login'] == 'alice' and doc['meta']['principal']['actor']['login'] == 'owner' and doc['meta']['principal']['actor']['via'] == 'socket'"
check "ctl keel.account.list --as nobody is refused" keel_fails "$site/as-nobody.json" ctl keel.account.list --as nobody
check "ctl auth.token.create --as alice" keel_ok "$site/token-as-alice.json" ctl auth.token.create name=cli --as alice
check "auth.token.create as alice answers the plaintext with the owner as actor" json_is "$site/token-as-alice.json" "doc['ok'] is True and re.fullmatch('kat_[0-9a-f]{64}', doc['data']['token']) is not None and doc['meta']['principal']['login'] == 'alice' and doc['meta']['principal']['actor']['login'] == 'owner'"
check "keel.account.list as nobody answers not_found" json_is "$site/as-nobody.json" 'doc["ok"] is False and doc["error"]["code"] == "not_found" and doc["error"]["key"] == "keel.error.principal_not_found" and doc["error"]["params"]["login"] == "nobody"'
check "POST /api/v1/call/auth.login with a wrong password is 401" http_is "$site/wrong.json" 401 POST /api/v1/call/auth.login '{"login":"alice","password":"wrong"}'
check "the wrong password answers auth.error.credentials" json_is "$site/wrong.json" 'doc["ok"] is False and doc["error"]["code"] == "unauthenticated" and doc["error"]["key"] == "auth.error.credentials"'
for attempt in 2 3 4 5; do
  check "wrong password $attempt of 5 is 401" http_is "$site/wrong-$attempt.json" 401 POST /api/v1/call/auth.login '{"login":"alice","password":"wrong"}'
done
check "POST /api/v1/call/auth.login with the right password after five failures is 403" http_is "$site/locked.json" 403 POST /api/v1/call/auth.login '{"login":"alice","password":"correct-horse"}'
check "the locked pair answers auth.error.locked" json_is "$site/locked.json" 'doc["ok"] is False and doc["error"]["code"] == "forbidden" and doc["error"]["key"] == "auth.error.locked"'
check "ctl auth.unlock login=alice" keel_ok "$site/unlock.json" ctl auth.unlock login=alice
check "auth.unlock answers the cleared pair" json_is "$site/unlock.json" 'doc["ok"] is True and doc["data"] == {"login": "alice", "cleared": 1}'
check "ctl auth.unlock login=nobody clears nothing" keel_ok "$site/unlock-nobody.json" ctl auth.unlock login=nobody
check "auth.unlock answers zero for a login without failures" json_is "$site/unlock-nobody.json" 'doc["ok"] is True and doc["data"] == {"login": "nobody", "cleared": 0}'
check "POST /api/v1/call/auth.login is 200" http_is "$site/login.json" 200 POST /api/v1/call/auth.login '{"login":"alice","password":"correct-horse"}'
check "auth.login answers the authenticated state for alice" json_is "$site/login.json" 'doc["ok"] is True and doc["data"]["state"] == "authenticated" and doc["data"]["principal"]["login"] == "alice"'
cookie=$(session_cookie_of "$site/login.json.headers") || fail "auth.login sets the session cookie"
check "auth.login sets the session cookie" test -n "$cookie"
check "the cookie carries the fixed attributes" header_matches "$site/login.json.headers" set-cookie '^__Host-keel_session=ks_[0-9a-f]*; Path=/; Secure; HttpOnly; SameSite=Lax$'
check "GET /api/v1/boot with the cookie is 200" http_is "$site/boot-alice.json" 200 GET /api/v1/boot '' "$cookie"
check "boot names alice as the principal" json_is "$site/boot-alice.json" 'doc["principal"]["login"] == "alice" and doc["principal"]["level"] == "user"'
check "boot answers no zone before a preference is set" json_is "$site/boot-alice.json" 'doc["principal"]["tz"] is None and doc["principal"]["lang"] == "en"'
check "POST /api/v1/call/settings.pref.set tz=Asia/Tokyo with the cookie is 200" http_is "$site/pref-tz.json" 200 POST /api/v1/call/settings.pref.set '{"key":"tz","value":"Asia/Tokyo"}' "$cookie"
check "settings.pref.set answers the preference" json_is "$site/pref-tz.json" 'doc["ok"] is True and doc["data"] == {"key": "tz", "value": "Asia/Tokyo"}'
check "POST /api/v1/call/settings.pref.set lang=ru with the cookie is 200" http_is "$site/pref-lang.json" 200 POST /api/v1/call/settings.pref.set '{"key":"lang","value":"ru"}' "$cookie"
check "POST /api/v1/call/settings.pref.set with an unknown zone is 422" http_is "$site/pref-bad.json" 422 POST /api/v1/call/settings.pref.set '{"key":"tz","value":"Mars/Olympus"}' "$cookie"
check "the unknown zone answers settings.error.pref_tz on value" json_is "$site/pref-bad.json" 'doc["ok"] is False and doc["error"]["fields"][0]["path"] == "value" and doc["error"]["fields"][0]["key"] == "settings.error.pref_tz"'
check "GET /api/v1/boot with the cookie shows the zone and the language" http_is "$site/boot-pref.json" 200 GET /api/v1/boot '' "$cookie"
check "boot answers tz Asia/Tokyo and lang ru" json_is "$site/boot-pref.json" 'doc["principal"]["tz"] == "Asia/Tokyo" and doc["principal"]["lang"] == "ru"'
check "GET /api/v1/i18n without lang serves ru" http_is "$site/i18n-pref.json" 200 GET /api/v1/i18n '' "$cookie"
check "the catalogue answers lang ru" json_is "$site/i18n-pref.json" 'doc["ok"] is True and doc["data"]["lang"] == "ru"'
check "POST /api/v1/call/settings.pref.set lang=en restores the language" http_is "$site/pref-lang-en.json" 200 POST /api/v1/call/settings.pref.set '{"key":"lang","value":"en"}' "$cookie"
check "ctl settings.pref.set over the socket is refused" keel_fails "$site/pref-owner.json" ctl settings.pref.set key=tz value=UTC
check "the owner answers settings.error.pref_owner, not internal" json_is "$site/pref-owner.json" 'doc["ok"] is False and doc["error"]["code"] == "forbidden" and doc["error"]["key"] == "settings.error.pref_owner"'
check "POST /api/v1/call/note.list with the cookie is 200" http_is "$site/notes-alice.json" 200 POST /api/v1/call/note.list '{}' "$cookie"
check "note.list answers the page of alice" json_is "$site/notes-alice.json" 'doc["ok"] is True and [row["title"] for row in doc["data"]["data"]] == ["hers"] and doc["meta"]["principal"]["login"] == "alice"'
root_account=00000000000000000000000000
check "ctl keel.principal.create login=root level=admin" keel_ok "$site/root.json" ctl keel.principal.create login=root level=admin "account=$root_account"
root_id=$(json_get "$site/root.json" 'doc["data"]["id"]')
check "ctl auth.password.set for root" keel_ok "$site/root-password.json" ctl auth.password.set "principal=$root_id" password=correct-horse
check "POST /api/v1/call/auth.login as root is 200" http_is "$site/login-root.json" 200 POST /api/v1/call/auth.login '{"login":"root","password":"correct-horse"}'
root_cookie=$(session_cookie_of "$site/login-root.json.headers") || fail "auth.login sets the session cookie for root"
check "GET /api/v1/boot as root is 200" http_is "$site/boot-root.json" 200 GET /api/v1/boot '' "$root_cookie"
check "boot names root without an actor" json_is "$site/boot-root.json" 'doc["principal"]["login"] == "root" and doc["principal"]["level"] == "admin" and doc["principal"]["actor"] is None'
check "boot as root lists the Updates screen in the system group" json_is "$site/boot-root.json" 'any(item["func"] == "update.check" and item["kind"] == "get" and item["icon"] == "package" for group in doc["menu"] for item in group["items"])'
check "POST /api/v1/call/keel.account.list as root is 200" http_is "$site/accounts-root.json" 200 POST /api/v1/call/keel.account.list '{}' "$root_cookie"
check "keel.account.list as root lists every account" json_is "$site/accounts-root.json" "doc['ok'] is True and doc['data']['total'] >= 2 and doc['meta']['principal']['actor'] is None"
check "POST /api/v1/call/auth.su into alice is 200" http_is "$site/su.json" 200 POST /api/v1/call/auth.su "{\"principal\":\"$principal_id\"}" "$root_cookie"
check "auth.su answers alice with root as the actor" json_is "$site/su.json" "doc['ok'] is True and doc['data']['principal']['login'] == 'alice' and doc['data']['principal']['actor']['login'] == 'root' and doc['data']['principal']['actor']['via'] == 'su'"
check "auth.su sets no cookie" test -z "$(header_of "$site/su.json.headers" set-cookie)"
check "POST /api/v1/call/keel.account.list under su is 200" http_is "$site/accounts-su.json" 200 POST /api/v1/call/keel.account.list '{}' "$root_cookie"
check "keel.account.list under su lists the account of alice with root as the actor" json_is "$site/accounts-su.json" "doc['ok'] is True and [row['id'] for row in doc['data']['data']] == ['$account_id'] and doc['meta']['principal']['login'] == 'alice' and doc['meta']['principal']['actor']['login'] == 'root' and doc['meta']['principal']['actor']['via'] == 'su'"
check "GET /api/v1/boot under su is 200" http_is "$site/boot-su.json" 200 GET /api/v1/boot '' "$root_cookie"
check "boot under su names alice with root as the actor" json_is "$site/boot-su.json" 'doc["principal"]["login"] == "alice" and doc["principal"]["actor"]["login"] == "root" and doc["principal"]["actor"]["via"] == "su"'
check "the su session is a second row of kind su naming root as its actor" test "$(psql "$(database_url "$admin_url" "$database")" -qAt -c "SELECT count(*) FROM keel.auth_session child JOIN keel.auth_session parent ON parent.su_id_hash = child.id_hash AND child.parent_id_hash = parent.id_hash WHERE child.kind = 'su' AND child.actor_principal_id = '$root_id'")" = "1"
check "POST /api/v1/call/auth.su.exit is 200" http_is "$site/su-exit.json" 200 POST /api/v1/call/auth.su.exit '{}' "$root_cookie"
check "auth.su.exit answers root" json_is "$site/su-exit.json" 'doc["ok"] is True and doc["data"]["principal"]["login"] == "root" and doc["data"]["principal"]["actor"] is None'
check "POST /api/v1/call/keel.account.list after su.exit is 200" http_is "$site/accounts-after-su.json" 200 POST /api/v1/call/keel.account.list '{}' "$root_cookie"
check "keel.account.list after su.exit is wide again without an actor" json_is "$site/accounts-after-su.json" "doc['data']['total'] >= 2 and doc['meta']['principal']['login'] == 'root' and doc['meta']['principal']['actor'] is None"
check "POST /api/v1/call/auth.su.exit outside a delegation is 422" http_is "$site/su-exit-again.json" 422 POST /api/v1/call/auth.su.exit '{}' "$root_cookie"
check "auth.su.exit outside a delegation answers auth.error.not_delegated" json_is "$site/su-exit-again.json" 'doc["ok"] is False and doc["error"]["key"] == "auth.error.not_delegated"'
check "ctl auth.key.issue for alice" keel_ok "$site/key.json" ctl auth.key.issue "principal=$principal_id" ttl=60 redirect=/f/note.list
check "auth.key.issue answers a kok_ key and the relative login url" json_is "$site/key.json" 'doc["ok"] is True and re.fullmatch("kok_[0-9a-f]{64}", doc["data"]["key"]) is not None and doc["data"]["url"] == "/login?key=" + doc["data"]["key"]'
login_key=$(json_get "$site/key.json" 'doc["data"]["key"]')
check "POST /api/v1/call/auth.key.redeem cross-site is 403" http_is "$site/redeem-cross.json" 403 POST /api/v1/call/auth.key.redeem "{\"key\":\"$login_key\"}" '' 'Origin: https://evil.example'
check "the cross-site redeem answers keel.error.cross_site and sets no cookie" json_is "$site/redeem-cross.json" 'doc["ok"] is False and doc["error"]["key"] == "keel.error.cross_site"'
check "the cross-site redeem leaves the key unused" test "$(psql "$(database_url "$admin_url" "$database")" -qAt -c "SELECT count(*) FROM keel.auth_login_key WHERE used_at IS NULL")" = "1"
check "POST /api/v1/call/auth.key.redeem is 200" http_is "$site/redeem.json" 200 POST /api/v1/call/auth.key.redeem "{\"key\":\"$login_key\"}"
check "auth.key.redeem answers the redirect, alice and the issuer" json_is "$site/redeem.json" "doc['ok'] is True and doc['data']['redirect'] == '/f/note.list' and doc['data']['principal']['login'] == 'alice' and doc['data']['issuedBy'] == '$root_account'"
key_cookie=$(session_cookie_of "$site/redeem.json.headers") || fail "auth.key.redeem sets the session cookie"
check "GET /api/v1/boot with the redeemed cookie names alice" http_is "$site/boot-key.json" 200 GET /api/v1/boot '' "$key_cookie"
check "boot names alice after the redeem" json_is "$site/boot-key.json" 'doc["principal"]["login"] == "alice"'
check "POST /api/v1/call/auth.key.redeem with the spent key is 401" http_is "$site/redeem-spent.json" 401 POST /api/v1/call/auth.key.redeem "{\"key\":\"$login_key\"}"
check "the spent key answers auth.error.key" json_is "$site/redeem-spent.json" 'doc["ok"] is False and doc["error"]["code"] == "unauthenticated" and doc["error"]["key"] == "auth.error.key"'
check "auth_login_key holds the hash, not the key" test "$(psql "$(database_url "$admin_url" "$database")" -qAt -c "SELECT count(*) FROM keel.auth_login_key k WHERE k::text LIKE '%kok\\_%'")" = "0"
check "POST /api/v1/call/auth.token.create with the cookie is 200" http_is "$site/token.json" 200 POST /api/v1/call/auth.token.create '{"name":"api","scope":{"functions":["note.list"]}}' "$cookie"
check "auth.token.create answers the plaintext token" json_is "$site/token.json" 'doc["ok"] is True and re.fullmatch("kat_[0-9a-f]{64}", doc["data"]["token"]) is not None'
token="Bearer $(json_get "$site/token.json" 'doc["data"]["token"]')"
check "POST /api/v1/call/note.list with the token is 200" http_is "$site/notes-token.json" 200 POST /api/v1/call/note.list '{}' "$token"
check "note.list answers the page of alice under the token" json_is "$site/notes-token.json" 'doc["ok"] is True and [row["title"] for row in doc["data"]["data"]] == ["hers"] and doc["meta"]["principal"]["login"] == "alice"'
check "POST /api/v1/call/note.edit with the token is 403" http_is "$site/edit-token.json" 403 POST /api/v1/call/note.edit '{"title":"no"}' "$token"
check "note.edit answers keel.error.token_scope under the token" json_is "$site/edit-token.json" 'doc["ok"] is False and doc["error"]["code"] == "forbidden" and doc["error"]["key"] == "keel.error.token_scope" and doc["error"]["params"]["function"] == "note.edit"'
check "POST /api/v1/call/auth.token.create with an Idempotency-Key is 200" http_is "$site/token-keyed.json" 200 POST /api/v1/call/auth.token.create '{"name":"keyed"}' "$cookie" 'Idempotency-Key: token-keyed'
check "the keyed auth.token.create answers the plaintext token" json_is "$site/token-keyed.json" 'doc["ok"] is True and re.fullmatch("kat_[0-9a-f]{64}", doc["data"]["token"]) is not None'
check "the same Idempotency-Key replays auth.token.create" http_is "$site/token-replay.json" 200 POST /api/v1/call/auth.token.create '{"name":"keyed"}' "$cookie" 'Idempotency-Key: token-keyed'
check "the replay is marked" test "$(header_of "$site/token-replay.json.headers" x-keel-idempotent-replay)" = "true"
check "the replay answers the masked token" json_is "$site/token-replay.json" 'doc["ok"] is True and doc["data"]["token"] == "***"'
check "no column of auth_token holds a kat_ value" test "$(psql "$(database_url "$admin_url" "$database")" -qAt -c "SELECT count(*) FROM keel.auth_token t WHERE t::text LIKE '%kat\\_%'")" = "0"
check "no idempotency record holds a kat_ value" test "$(psql "$(database_url "$admin_url" "$database")" -qAt -c "SELECT count(*) FROM keel.keel_idempotency WHERE response_json::text LIKE '%kat\\_%'")" = "0"
check "POST /api/v1/call/auth.totp.enroll with the cookie is 200" http_is "$site/enroll.json" 200 POST /api/v1/call/auth.totp.enroll '{}' "$cookie"
check "auth.totp.enroll answers the base32 secret and the otpauth uri" json_is "$site/enroll.json" 'doc["ok"] is True and re.fullmatch("[A-Z2-7]{32}", doc["data"]["secret"]) is not None and doc["data"]["uri"].startswith("otpauth://totp/keel:alice?secret=" + doc["data"]["secret"] + "&issuer=keel")'
totp_secret=$(json_get "$site/enroll.json" 'doc["data"]["secret"]')
check "POST /api/v1/call/auth.totp.confirm with a wrong code is 422" http_is "$site/confirm-wrong.json" 422 POST /api/v1/call/auth.totp.confirm '{"code":"000000"}' "$cookie"
check "the wrong code lands on the code field" json_is "$site/confirm-wrong.json" 'doc["ok"] is False and doc["error"]["fields"][0]["path"] == "code" and doc["error"]["fields"][0]["key"] == "auth.error.code"'
check "POST /api/v1/call/auth.totp.confirm with the current code is 200" http_is "$site/confirm.json" 200 POST /api/v1/call/auth.totp.confirm "{\"code\":\"$(totp_code "$totp_secret")\"}" "$cookie"
check "auth.totp.confirm answers ten recovery codes of ten base32 characters" json_is "$site/confirm.json" 'doc["ok"] is True and len(doc["data"]["recoveryCodes"]) == 10 and all(re.fullmatch("[A-Z2-7]{10}", code) for code in doc["data"]["recoveryCodes"])'
recovery_code=$(json_get "$site/confirm.json" 'doc["data"]["recoveryCodes"][0]')
check "GET /api/v1/boot with the cookie shows totp enrolled" http_is "$site/boot-totp.json" 200 GET /api/v1/boot '' "$cookie"
check "boot answers principal.totp true" json_is "$site/boot-totp.json" 'doc["principal"]["login"] == "alice" and doc["principal"]["totp"] is True'
check "POST /api/v1/call/auth.login now answers the totp state" http_is "$site/login-totp.json" 200 POST /api/v1/call/auth.login '{"login":"alice","password":"correct-horse"}'
check "auth.login answers a challenge and no principal" json_is "$site/login-totp.json" 'doc["ok"] is True and doc["data"]["state"] == "totp" and re.fullmatch("[0-9a-f]{64}", doc["data"]["challenge"]) is not None and "principal" not in doc["data"]'
check "auth.login with a pending second factor sets no cookie" test -z "$(header_of "$site/login-totp.json.headers" set-cookie)"
challenge=$(json_get "$site/login-totp.json" 'doc["data"]["challenge"]')
check "POST /api/v1/call/auth.totp.verify with a wrong code is 401" http_is "$site/verify-wrong.json" 401 POST /api/v1/call/auth.totp.verify "{\"challenge\":\"$challenge\",\"code\":\"000000\"}"
check "the wrong code answers auth.error.code" json_is "$site/verify-wrong.json" 'doc["ok"] is False and doc["error"]["code"] == "unauthenticated" and doc["error"]["key"] == "auth.error.code"'
check "POST /api/v1/call/auth.totp.verify with a recovery code is 200" http_is "$site/verify.json" 200 POST /api/v1/call/auth.totp.verify "{\"challenge\":\"$challenge\",\"code\":\"$recovery_code\"}"
check "auth.totp.verify answers the authenticated state for alice" json_is "$site/verify.json" 'doc["ok"] is True and doc["data"]["state"] == "authenticated" and doc["data"]["principal"]["login"] == "alice"'
verified_cookie=$(session_cookie_of "$site/verify.json.headers") || fail "auth.totp.verify sets the session cookie"
check "the verified session differs from the first one" test "$verified_cookie" != "$cookie"
check "POST /api/v1/call/auth.totp.verify with the spent challenge is 401" http_is "$site/verify-spent.json" 401 POST /api/v1/call/auth.totp.verify "{\"challenge\":\"$challenge\",\"code\":\"$recovery_code\"}"
check "the spent challenge answers auth.error.challenge" json_is "$site/verify-spent.json" 'doc["ok"] is False and doc["error"]["key"] == "auth.error.challenge"'
check "auth_totp holds the sealed secret, not the base32 one" test "$(psql "$(database_url "$admin_url" "$database")" -qAt -c "SELECT count(*) FROM keel.auth_totp WHERE secret LIKE '%$totp_secret%' OR secret NOT LIKE '%A256GCM%'")" = "0"
check "auth_recovery_code holds hashes, not the codes" test "$(psql "$(database_url "$admin_url" "$database")" -qAt -c "SELECT count(*) FROM keel.auth_recovery_code WHERE code_hash = '$recovery_code' OR length(code_hash) <> 64")" = "0"
check "auth_login_challenge holds the hash, not the challenge" test "$(psql "$(database_url "$admin_url" "$database")" -qAt -c "SELECT count(*) FROM keel.auth_login_challenge WHERE challenge_hash = '$challenge'")" = "0"
check "POST /api/v1/call/auth.totp.disable with the next code is 200" http_is "$site/disable.json" 200 POST /api/v1/call/auth.totp.disable "{\"code\":\"$(totp_code "$totp_secret" 1)\"}" "$verified_cookie"
check "auth.totp.disable answers disabled" json_is "$site/disable.json" 'doc["ok"] is True and doc["data"]["disabled"] is True'
check "GET /api/v1/boot after the disable shows totp off" http_is "$site/boot-disabled.json" 200 GET /api/v1/boot '' "$verified_cookie"
check "boot answers principal.totp false" json_is "$site/boot-disabled.json" 'doc["principal"]["totp"] is False'
check "POST /api/v1/call/auth.logout with the cookie is 200" http_is "$site/logout.json" 200 POST /api/v1/call/auth.logout '{}' "$cookie"
check "auth.logout clears the cookie" header_matches "$site/logout.json.headers" set-cookie '^__Host-keel_session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0$'
check "POST /api/v1/call/note.list with the revoked cookie is 401" http_is "$site/revoked.json" 401 POST /api/v1/call/note.list '{}' "$cookie"
check "POST /api/v1/call/note.list with the token after the logout is 200" http_is "$site/notes-token-after.json" 200 POST /api/v1/call/note.list '{}' "$token"
echo "--- note.export as a task"
check "ctl task.list --out json before the first export" keel_ok "$site/tasks-before.json" ctl task.list --out json
check "task.list answers an empty page" json_is "$site/tasks-before.json" 'doc["ok"] is True and doc["data"]["total"] == 0 and doc["data"]["data"] == []'
check "ctl note.export format=csv" keel_ok "$site/export.json" ctl note.export format=csv
check "note.export answers a task ref" json_is "$site/export.json" 'doc["ok"] is True and list(doc["data"]) == ["task"] and re.fullmatch("[0-7][0-9A-HJKMNP-TV-Z]{25}", doc["data"]["task"]["id"])'
export_id=$(json_get "$site/export.json" 'doc["data"]["task"]["id"]')
check "task.get answers done within ${timeout}s" task_reaches "$site/export-done.json" "$export_id" done
check "task.get answers the note count as progress, the exported count, the download and a log line per note" json_is "$site/export-done.json" "doc['data']['function'] == 'note.export' and doc['data']['class'] == 'export' and doc['data']['progress'] == '4/4' and doc['data']['result'] == {'exported': 4, 'download': {'function': 'task.download', 'params': {'id': '$export_id'}}} and sorted(line.split(' ', 1)[1] for line in doc['data']['logTail']) == ['exported alpha', 'exported beta', 'exported hers', 'exported x']"
check "the export file holds the header and one record per note" test "$(cut -d, -f2 "$site/lib/tasks/$export_id.out" | sort | tr '\n' ' ')" = "alpha beta hers title x "
check "GET /api/v1/x/task/download as root streams the export file" http_is "$site/export-download.csv" 200 GET "/api/v1/x/task/download?id=$export_id" '' "$root_cookie"
check "the download answers the stored file byte for byte" cmp -s "$site/export-download.csv" "$site/lib/tasks/$export_id.out"
check "the download names the file and its type" header_matches "$site/export-download.csv.headers" content-disposition '^attachment; filename="notes.csv"$'
check "the download is served as csv no-store" test "$(header_of "$site/export-download.csv.headers" content-type)" = "text/csv; charset=utf-8" -a "$(header_of "$site/export-download.csv.headers" cache-control)" = "no-store"
check "GET /api/v1/x/task/download as alice is 404 for the owner's export" http_is "$site/export-download-alice.json" 404 GET "/api/v1/x/task/download?id=$export_id" '' "$verified_cookie"
check "POST /api/v1/call/task.download is 422 naming the raw route" http_is "$site/export-call-route.json" 422 POST /api/v1/call/task.download "{\"id\":\"$export_id\"}" "$root_cookie"
check "the call route answers keel.error.download_route" json_is "$site/export-call-route.json" 'doc["ok"] is False and doc["error"]["key"] == "keel.error.download_route" and doc["error"]["params"]["route"] == "/api/v1/x/task/download"'
check "ctl task.download is refused with the raw route" keel_fails "$site/export-ctl-route.json" ctl task.download "id=$export_id"
check "ctl task.download answers keel.error.download_route" json_is "$site/export-ctl-route.json" 'doc["ok"] is False and doc["error"]["key"] == "keel.error.download_route"'
check "GET /api/v1/x/keel/export for note.list as root streams a csv" http_is "$site/export.csv" 200 GET "/api/v1/x/keel/export?func=note.list&format=csv" '' "$root_cookie"
check "the export names the list and the day" header_matches "$site/export.csv.headers" content-disposition '^attachment; filename="note.list-[0-9]\{4\}-[0-9]\{2\}-[0-9]\{2\}\.csv"$'
check "the export is served as csv no-store" test "$(header_of "$site/export.csv.headers" content-type)" = "text/csv; charset=utf-8" -a "$(header_of "$site/export.csv.headers" cache-control)" = "no-store"
check "the export header row carries the translated column labels" test "$(head -n 1 "$site/export.csv" | tr -d '\r')" = "Title,Kind,Tags,Due,Updated,Account"
check "the export holds one record per note" test "$(tail -n +2 "$site/export.csv" | tr -d '\r' | grep -c .)" = "4"
check "GET /api/v1/x/keel/export for note.list as alice streams her csv" http_is "$site/export-alice.csv" 200 GET "/api/v1/x/keel/export?func=note.list" '' "$verified_cookie"
check "alice's header row lacks the account column" test "$(head -n 1 "$site/export-alice.csv" | tr -d '\r')" = "Title,Kind,Tags,Due,Updated"
check "alice's export holds her note alone" test "$(tail -n +2 "$site/export-alice.csv" | tr -d '\r' | cut -d, -f1)" = "hers"
check "POST /api/v1/x/keel/export with columns accountId as alice is 422" http_is "$site/export-columns.json" 422 POST /api/v1/x/keel/export '{"func":"note.list","columns":["accountId"]}' "$verified_cookie"
check "GET /api/v1/x/keel/export with a token scoped to note.list alone is 403" http_is "$site/export-token.json" 403 GET "/api/v1/x/keel/export?func=note.list" '' "$token"
check "the scoped token is refused by the token scope guard" json_is "$site/export-token.json" 'doc["ok"] is False and doc["error"]["key"] == "keel.error.token_scope"'
check "the refused export names the columns field" json_is "$site/export-columns.json" 'doc["ok"] is False and doc["error"]["fields"][0]["path"] == "columns" and doc["error"]["fields"][0]["key"] == "keel.error.export_columns"'
check "GET /api/v1/x/keel/export for note.list as json is one array" http_is "$site/export.json" 200 GET "/api/v1/x/keel/export?func=note.list&format=json" '' "$root_cookie"
check "the json export holds one object per note" json_is "$site/export.json" 'isinstance(doc, list) and len(doc) == 4 and sorted(row["title"] for row in doc) == ["alpha", "beta", "hers", "x"]'
check "ctl note.export format=csv pace=500" keel_ok "$site/export-paced.json" ctl note.export format=csv pace=500
check "the paced export answers a task ref" json_is "$site/export-paced.json" 'doc["ok"] is True and list(doc["data"]) == ["task"]'
paced_id=$(json_get "$site/export-paced.json" 'doc["data"]["task"]["id"]')
check "task.get answers running after the first note of the paced export" task_reaches "$site/export-running.json" "$paced_id" running 1
check "keel serve stops on SIGTERM while the export runs" stop_serve
check "the export is interrupted with its checkpoint kept" test "$(task_sql "SELECT state || ' ' || (checkpoint_json->>'written' = progress_done::text)::text FROM keel.task_job WHERE id = '$paced_id'")" = "interrupted true"
start_serve resumed
check "task.get answers done after the restart" task_reaches "$site/export-resumed.json" "$paced_id" done
check "the resumed export answers the note count with each exported line exactly once" json_is "$site/export-resumed.json" "doc['data']['state'] == 'done' and doc['data']['progress'] == '4/4' and doc['data']['result'] == {'exported': 4, 'download': {'function': 'task.download', 'params': {'id': '$paced_id'}}} and doc['data']['error'] is None and sorted(line.split(' ', 1)[1] for line in doc['data']['logTail']) == ['exported alpha', 'exported beta', 'exported hers', 'exported x']"
check "the resumed export file holds each record once" test "$(cut -d, -f2 "$site/lib/tasks/$paced_id.out" | sort | tr '\n' ' ')" = "alpha beta hers title x "
check "ctl note.delete of two ids runs inline" keel_ok "$site/delete-inline.json" ctl note.delete "ids=$(json_get "$site/import.json" 'json.dumps(doc["data"]["ids"])')"
check "note.delete answers the deleted count" json_is "$site/delete-inline.json" 'doc["ok"] is True and doc["data"] == {"deleted": 2}'
echo "--- backup over the CLI and as a task"
check "ctl note.list before the backup" keel_ok "$site/notes-at-backup.json" ctl note.list --out json
KEEL_BACKUP_PASSPHRASE=prove-passphrase check "backup --with-secret-key writes the archive while the service runs" keel_ok "$site/backup.out" backup --out "$site/backup.tar.zst" --with-secret-key
check "backup prints the archive, its size and the manifest line" grep -q "^wrote $site/backup.tar.zst ([0-9]* bytes): keel $version taken .*; products: demo $version; .*; secret key included$" "$site/backup.out"
check "the archive is written with mode 0600" test "$(stat -f '%Lp' "$site/backup.tar.zst" 2>/dev/null || stat -c '%a' "$site/backup.tar.zst")" = "600"
check "backup inspect prints the manifest" keel_ok "$site/backup-manifest.json" backup inspect "$site/backup.tar.zst"
check "the manifest names the version, the demo bundle, the note table, the configuration and the sealed key" json_is "$site/backup-manifest.json" "doc['keel'] == '$version' and [(p['id'], p['version']) for p in doc['products']] == [('demo', '$version')] and any(t['name'] == 'note_item' and 'title' in t['columns'] for t in doc['tables']) and any(m['id'] == 'keel' and m['ledger'] == 'keel_migration' and m['applied'] == 2 for m in doc['modules']) and doc['config'] == ['config.yaml'] and 'instance.id' in doc['data'] and 'tls/key.pem' in doc['data'] and 'secret.key.sealed' in doc['data'] and doc['secretKey'] is True"
check "ctl backup.create runs as a task" keel_ok "$site/backup-create.json" ctl backup.create
check "backup.create answers a task ref" json_is "$site/backup-create.json" 'doc["ok"] is True and list(doc["data"]) == ["task"]'
backup_task=$(json_get "$site/backup-create.json" 'doc["data"]["task"]["id"]')
check "task.get answers done for the backup task" task_reaches "$site/backup-task.json" "$backup_task" "done"
check "the backup task names the archive under backups, the tables and the download" json_is "$site/backup-task.json" "doc['data']['state'] == 'done' and doc['data']['progress'] == '2/2' and doc['data']['result']['file'].startswith('keel-$version-') and doc['data']['result']['file'].endswith('.tar.zst') and doc['data']['result']['archive'] == '$site/lib/backups/' + doc['data']['result']['file'] and doc['data']['result']['tables'] > 10 and doc['data']['result']['products'] == ['demo'] and doc['data']['result']['download'] == {'function': 'task.download', 'params': {'id': '$backup_task'}}"
task_archive=$(json_get "$site/backup-task.json" 'doc["data"]["result"]["file"]')
check "the task's archive exists under backups" test -f "$site/lib/backups/$task_archive"
check "GET /api/v1/x/task/download as root streams the archive" http_is "$site/backup-download.tar.zst" 200 GET "/api/v1/x/task/download?id=$backup_task" '' "$root_cookie"
check "the download answers the archive byte for byte" cmp -s "$site/backup-download.tar.zst" "$site/lib/backups/$task_archive"
check "the download names the archive" header_matches "$site/backup-download.tar.zst.headers" content-disposition "^attachment; filename=\"$task_archive\"$"
check "ctl backup.list --out json" keel_ok "$site/backup-list.json" ctl backup.list --out json
check "backup.list answers the task's archive with the demo bundle and no secret key" json_is "$site/backup-list.json" "doc['ok'] is True and doc['data']['total'] == 1 and doc['data']['data'][0]['file'] == '$task_archive' and doc['data']['data'][0]['products'] == 'demo' and doc['data']['data'][0]['secretKey'] is False and doc['data']['data'][0]['keel'] == '$version'"
check "ctl backup.list --out table" keel_ok "$site/backup-list.out" ctl backup.list --out table
check "the table carries the translated header" grep -q "Archive" "$site/backup-list.out"
echo "--- doctor and the support bundle"
mkdir -p "$site/units" "$site/opt/versions/$version" "$site/opt/bin" "$site/bin"
cp deploy/systemd/keel.service deploy/systemd/keel-privileged.service deploy/systemd/keel-update.service deploy/systemd/keel-update.path "$site/units/"
ln -s "$keel" "$site/opt/versions/$version/keel"
ln -s "$site/opt/versions/$version/keel" "$site/opt/bin/current"
cat >"$site/bin/systemctl" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$(dirname "$0")/systemctl.log"
case "$1" in is-enabled) echo enabled ;; is-active) echo active ;; esac
EOF
chmod 0755 "$site/bin/systemctl"
layout=(--opt "$site/opt" --unit-dir "$site/units" --systemctl "$site/bin/systemctl" --agent-socket "$site/run/agent.sock")
check "doctor exits 0 on the proven site" keel_ok "$site/doctor.out" doctor "${layout[@]}"
check "doctor prints fourteen lines and none fails" test "$(grep -cE '^(ok|warn|fail) ' "$site/doctor.out")" = "14" -a "$(grep -c '^fail ' "$site/doctor.out")" = "0"
for line in "units .*keel.service \(keel\), keel-privileged.service \(root\), keel-update.service \(root\), keel-update.path are installed" "install_dir .*bin/current -> $site/opt/versions/$version/keel" "secret_key .*secret.key has mode 0600" "tls .*the generated certificate .* is valid until" "database .*PostgreSQL .* the schema keel exists and the role may create in it" "migrations .* ledger\(s\) match the binary" "agent .*not in use" "license .*not required" "products .*demo enabled and carried by the binary" "orphans .*no module is orphaned" "accounts .*no account is left in deleting"; do
  check "doctor: ok $line" grep -qE "^ok +$line" "$site/doctor.out"
done
check "doctor prints the totals line" grep -qE '^[0-9]+ ok, [0-9]+ warn, 0 fail; keel '"$version"' at ' "$site/doctor.out"
check "the stub service manager was asked with fixed argument vectors" grep -qx "is-enabled keel-update.path" "$site/bin/systemctl.log"
check "doctor --out json" keel_ok "$site/doctor.json" doctor --out json "${layout[@]}"
check "the json report carries the fourteen checks in order" json_is "$site/doctor.json" 'len(doc["checks"]) == 14 and [c["id"] for c in doc["checks"]] == ["data_dir", "config_dir", "install_dir", "secret_key", "tls", "units", "database", "migrations", "agent", "license", "space", "products", "orphans", "accounts"] and all(c["status"] in ("ok", "warn") for c in doc["checks"]) and doc["version"] == "'"$version"'"'
check "doctor without the units of the design fails on this host" keel_fails "$site/doctor-default.err" doctor --opt "$site/opt" --systemctl "$site/bin/systemctl"
check "the failing report names the missing units" grep -qE '^fail +units .*keel.service is missing from /etc/systemd/system' "$site/stdout"
check "ctl keel.doctor.run --out table" keel_ok "$site/doctor-table.out" ctl keel.doctor.run --out table
check "the health screen renders the translated headers" grep -q "Check" "$site/doctor-table.out"
check "ctl keel.doctor.run answers the checks with the design's layout" keel_ok "$site/doctor-run.json" ctl keel.doctor.run --out json 'filters=[{"key":"status","op":"eq","value":"ok"}]'
check "keel.doctor.run lists the database and the products as ok" json_is "$site/doctor-run.json" 'doc["ok"] is True and {row["check"] for row in doc["data"]["data"]} >= {"database", "migrations", "products", "secret_key", "tls", "license", "agent"} and all(row["status"] == "ok" for row in doc["data"]["data"])'
check "ctl keel.logs.tail limit=5" keel_ok "$site/logs-tail.json" ctl keel.logs.tail limit=5 --out json
check "keel.logs.tail answers five entries in order from a ring of ten thousand" json_is "$site/logs-tail.json" 'doc["ok"] is True and len(doc["data"]["entries"]) == 5 and [e["seq"] for e in doc["data"]["entries"]] == sorted(e["seq"] for e in doc["data"]["entries"]) and doc["data"]["capacity"] == 10000 and all(set(e) >= {"seq", "at", "level", "target", "message"} for e in doc["data"]["entries"])'
check "ctl keel.logs.tail level=warn" keel_ok "$site/logs-warn.json" ctl keel.logs.tail level=warn --out json
check "keel.logs.tail with a level floor answers warnings and errors alone" json_is "$site/logs-warn.json" 'doc["ok"] is True and all(e["level"] in ("warn", "error") for e in doc["data"]["entries"])'
check "support-bundle writes the archive" keel_ok "$site/support.out" support-bundle --out "$site/support.tar.zst" "${layout[@]}"
check "support-bundle prints the archive, its size and its entries" grep -q "^wrote $site/support.tar.zst ([0-9]* bytes): config.yaml, logs.jsonl, ledgers.json, audit.jsonl, problems.json, readiness.json, doctor.txt, manifest.json$" "$site/support.out"
check "the bundle is written with mode 0600" test "$(stat -f '%Lp' "$site/support.tar.zst" 2>/dev/null || stat -c '%a' "$site/support.tar.zst")" = "600"
mkdir -p "$site/support"
zstd -d -c "$site/support.tar.zst" | tar -x -C "$site/support"
token_plaintext=$(json_get "$site/token-as-alice.json" 'doc["data"]["token"]')
db_password=$(python3 -c 'import sys; from urllib.parse import urlparse; print(urlparse(sys.argv[1]).password or "")' "$admin_url")
check "the bundle carries no API token" test -z "$(grep -rl -- "$token_plaintext" "$site/support")"
check "the bundle carries no password of a principal" test -z "$(grep -rl -- "correct-horse" "$site/support")"
check "the bundle carries no totp secret or recovery code" test -z "$(grep -rlE -- "$totp_secret|$recovery_code" "$site/support")"
if [ -n "$db_password" ]; then
  check "the bundle carries no database password" test -z "$(grep -rl -- ":$db_password@" "$site/support")"
  check "the bundle's configuration masks the database password" grep -q ':\*\*\*@' "$site/support/config.yaml"
fi
check "the bundle's configuration keeps the listen address" grep -q "^listen: 127.0.0.1:$port$" "$site/support/config.yaml"
check "the bundle's log ring carries the ready line of the service" grep -q '"message":"keel is ready"' "$site/support/logs.jsonl"
python3 -c 'import json,sys; print(json.dumps([json.loads(l) for l in open(sys.argv[1])]))' "$site/support/audit.jsonl" >"$site/support-audit.json"
check "the bundle's audit tail masks the password of the failed logins" json_is "$site/support-audit.json" 'any(r["function"] == "auth.login" and r["params_redacted"]["password"] == "***" for r in doc) and all(r["params_redacted"].get("password", "***") == "***" for r in doc)'
check "the bundle's readiness answer is ready" json_is "$site/support/readiness.json" 'doc["status"] == 200 and doc["body"]["status"] == "ready"'
check "the bundle's ledgers name the kernel and the note module" json_is "$site/support/ledgers.json" 'any(l["module"] == "keel" and l["table"] == "keel_migration" and l["present"] and not l["pending"] for l in doc) and any(l["module"] == "note" for l in doc)'
check "the bundle's doctor report has no failure" grep -qE '^[0-9]+ ok, [0-9]+ warn, 0 fail; keel ' "$site/support/doctor.txt"
check "the bundle's manifest lists the entries and the version" json_is "$site/support/manifest.json" 'doc["keel"] == "'"$version"'" and doc["entries"][0] == "config.yaml" and "readiness.json" in doc["notes"]'
check "support-bundle never overwrites an archive" keel_fails "$site/support-again.err" support-bundle --out "$site/support.tar.zst" "${layout[@]}"
echo "--- the product switch on the running installation"
note_ledger_sql="SELECT string_agg(id::text || ':' || timestamp::text || ':' || name, ',' ORDER BY id) FROM keel.keel_migration_note"
note_tables_sql="SELECT string_agg(table_name::text, ',' ORDER BY table_name) FROM information_schema.tables WHERE table_schema = 'keel' AND table_name LIKE 'note_%'"
note_state_sql="SELECT state FROM keel.keel_module WHERE id = 'note'"
note_ledger=$(task_sql "$note_ledger_sql")
note_tables=$(task_sql "$note_tables_sql")
check "the note ledger holds the applied migrations" test -n "$note_ledger"
check "the note tables exist" test -n "$note_tables"
check "product disable demo over the control socket" keel_ok "$site/disable-online.out" product disable demo
check "product disable prints the restart" stdout_is "$site/disable-online.out" "demo: disabled; the service restarts without its modules"
check "keel serve exits with the restart status" serve_exits_with 75
check "the socket file is removed by the restart" socket_is_gone
restart_dump=$(ls -d "$site"/lib/backups/pre-restart-"$version"-* | head -n 1)
check "the pre-restart dump names the reason, the version and the note table" json_is "$restart_dump/manifest.json" "doc['keel'] == '$version' and doc['reason'] == 'product disable demo' and doc['schema'] == 'keel' and doc['format'] == 'copy-text' and 'note_item' in doc['tables'] and 'keel_kv' in doc['tables'] and 'task_job' in doc['tables']"
check "the pre-restart dump holds one file per table" test "$(ls "$restart_dump"/*.copy | wc -l | tr -d ' ')" = "$(json_get "$restart_dump/manifest.json" 'len(doc["tables"])')"
check "the pre-restart dump carries the notes" grep -q "hers" "$restart_dump/note_item.copy"
check "the note tables survive the disable" sql_is "$note_tables_sql" "$note_tables"
check "the note ledger survives the disable" sql_is "$note_ledger_sql" "$note_ledger"
check "the restart task ended done" sql_is "SELECT string_agg(state, ',') FROM keel.task_job WHERE function = 'keel.restart'" "done"
start_serve switched-off
check "the note module is orphaned after the restart" sql_is "$note_state_sql" "orphaned"
check "ctl note.list is refused while demo is disabled" keel_fails "$site/notes-off.json" ctl note.list
check "note.list answers not_found while demo is disabled" json_is "$site/notes-off.json" 'doc["ok"] is False and doc["error"]["code"] == "not_found"'
check "ctl keel.product.list --out json after the disable" keel_ok "$site/products-off.json" ctl keel.product.list --out json
check "keel.product.list answers demo disabled" json_is "$site/products-off.json" 'doc["ok"] is True and [(row["id"], row["state"], row["enabledBy"]) for row in doc["data"]["data"]] == [("demo", "disabled", None)]'
check "product disable demo while disabled prints already disabled" keel_ok "$site/disable-again.out" product disable demo
check "the second disable requests no restart" stdout_is "$site/disable-again.out" "demo: already disabled"
check "product enable demo over the control socket" keel_ok "$site/enable-online.out" product enable demo
check "product enable prints the restart" stdout_is "$site/enable-online.out" "demo: enabled; the service restarts to load its modules"
check "keel serve exits with the restart status again" serve_exits_with 75
check "the note ledger is unchanged by the enable" sql_is "$note_ledger_sql" "$note_ledger"
start_serve switched-on
check "the start applied no note migration" grep -q '("note", \[\])' "$serve_log"
check "the note module is active again" sql_is "$note_state_sql" "active"
check "the note ledger is unchanged after the start" sql_is "$note_ledger_sql" "$note_ledger"
check "the note tables are the ones from before the switch" sql_is "$note_tables_sql" "$note_tables"
check "ctl note.list answers again" keel_ok "$site/notes-on.json" ctl note.list --out json
check "note.list answers the notes written before the switch" cmp -s <(json_get "$site/notes-at-backup.json" 'json.dumps(sorted(row["title"] for row in doc["data"]["data"]))') <(json_get "$site/notes-on.json" 'json.dumps(sorted(row["title"] for row in doc["data"]["data"]))')
check "keel.product.list answers demo enabled by the owner" keel_ok "$site/products-on.json" ctl keel.product.list --out json
check "the enabled set records the owner of the socket" json_is "$site/products-on.json" 'doc["ok"] is True and [(row["id"], row["state"], row["enabledBy"]) for row in doc["data"]["data"]] == [("demo", "enabled", "owner")]'
check "product enable demo while enabled prints already enabled" keel_ok "$site/enable-again.out" product enable demo
check "the second enable requests no restart" stdout_is "$site/enable-again.out" "demo: already enabled"
check "product enable of an unknown bundle is refused over the socket" keel_fails "$site/enable-unknown.err" product enable nothing
check "the refusal names the unknown product" grep -q "keel.product.error.unknown" "$site/enable-unknown.err"
check "the two restart tasks are done with their three steps" sql_is "SELECT string_agg(state || ':' || progress_done || '/' || progress_total, ',' ORDER BY created_at) FROM keel.task_job WHERE function = 'keel.restart'" "done:3/3,done:3/3"
echo "--- the ip rule and the update"
check "ctl auth.ip_rule.create deny 127.0.0.1/32 at level user" keel_ok "$site/ip-rule.json" ctl auth.ip_rule.create action=deny cidr=127.0.0.1/32 level=user comment=prove
check "auth.ip_rule.create answers the rule" json_is "$site/ip-rule.json" 'doc["ok"] is True and doc["data"]["action"] == "deny" and doc["data"]["cidr"] == "127.0.0.1/32" and doc["data"]["level"] == "user" and doc["data"]["comment"] == "prove" and re.fullmatch("[0-7][0-9A-HJKMNP-TV-Z]{25}", doc["data"]["id"])'
ip_rule_id=$(json_get "$site/ip-rule.json" 'doc["data"]["id"]')
check "ctl auth.ip_rule.list --out json" keel_ok "$site/ip-rules.json" ctl auth.ip_rule.list --out json
check "auth.ip_rule.list answers the rule" json_is "$site/ip-rules.json" "doc['ok'] is True and [row['id'] for row in doc['data']['data']] == ['$ip_rule_id']"
check "update apply --wait --out json" keel_ok "$site/update-apply.json" update apply --wait --out json
check "update apply ends done with the versions, the owner and the dump" json_is "$site/update-apply.json" "doc['ok'] is True and doc['data']['state'] == 'done' and doc['data']['progress'] == '3/3' and doc['data']['result']['kind'] == 'apply' and doc['data']['result']['from'] == '$version' and doc['data']['result']['to'] == '$available_version' and doc['data']['result']['channel'] == 'stable' and doc['data']['result']['requestedBy'] == 'owner' and doc['data']['result']['leftRunning'] == 0 and doc['data']['result']['tables'] > 10 and any('the root unit takes over' in line for line in doc['data']['logTail'])"
check "the update request names the version, the channel and the owner" json_is "$site/lib/update.request" "doc == {'version': '$available_version', 'channel': 'stable', 'requested_by': 'owner'}"
check "the pre-update dump holds one file per table" test "$(ls "$site"/lib/backups/pre-update-*/*.copy | wc -l | tr -d ' ')" = "$(json_get "$site/update-apply.json" 'doc["data"]["result"]["tables"]')"
check "the pre-update dump carries the task table with the update task" grep -q "update.apply" "$site"/lib/backups/pre-update-*/task_job.copy
check "the dump manifest names the running version and the schema" json_is "$(ls "$site"/lib/backups/pre-update-*/manifest.json)" "doc['keel'] == '$version' and doc['schema'] == 'keel' and doc['format'] == 'copy-text' and 'task_job' in doc['tables']"
check "keel serve stops on SIGTERM" stop_serve
check "the socket file is removed" socket_is_gone
audit_sql() {
  psql "$(database_url "$admin_url" "$database")" -qAt -c "$1"
}
check "the stopped worker left no audit event unwritten" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE function = 'auth.ip_rule.create'")" = "1"
check "the audit trail records the switches over the socket, the restarts they requested and the refused bundle" sql_is "SELECT string_agg(function || ':' || via || ':' || status || ':' || coalesce(params_redacted->>'id', params_redacted->>'reason'), ' ' ORDER BY at) FROM keel.audit_call WHERE function IN ('keel.product.disable', 'keel.product.enable', 'keel.restart')" "keel.product.disable:socket:committed:demo keel.restart:internal:committed:product disable demo keel.product.disable:socket:committed:demo keel.product.enable:socket:committed:demo keel.restart:internal:committed:product enable demo keel.product.enable:socket:committed:demo keel.product.enable:socket:failed:nothing"
check "the failed login is a failed row with the password masked" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE function = 'auth.login' AND status = 'failed' AND error_key = 'auth.error.credentials' AND principal_id IS NULL AND account_id = '$root_account' AND params_redacted->>'password' = '***' AND params_redacted->>'login' = 'alice'")" = "5"
check "the committed logins mask the password the same way" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE function = 'auth.login' AND status = 'committed' AND params_redacted->>'password' <> '***'")" = "0"
check "auth.password.set records the masked password" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE function = 'auth.password.set' AND params_redacted->>'password' <> '***'")" = "0"
check "no session, token or key reaches the trail" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE params_redacted::text ~ '(ks|kat|kok)_' OR coalesce(after::text, '') ~ '(ks|kat|kok)_' OR coalesce(subject, '') ~ '(ks|kat|kok)_'")" = "0"
check "the totp secret and the recovery code are not in the trail" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE params_redacted::text LIKE '%$totp_secret%' OR params_redacted::text LIKE '%$recovery_code%' OR coalesce(after::text, '') LIKE '%$totp_secret%'")" = "0"
check "auth.su is recorded for root and the calls inside it carry alice with root as actor" test "$(audit_sql "SELECT string_agg(function || ':' || principal_login || ':' || coalesce(actor_login, '-') || ':' || via || ':' || status || ':' || coalesce(error_key, '-'), ' ' ORDER BY function, status) FROM keel.audit_call WHERE function IN ('auth.su', 'auth.su.exit')")" = "auth.su:root:-:http:committed:- auth.su.exit:alice:root:http:committed:- auth.su.exit:root:-:http:failed:auth.error.not_delegated"
check "the frame with --as records alice with the owner as actor via socket" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE function = 'auth.token.create' AND via = 'socket' AND principal_login = 'alice' AND actor_login = 'owner' AND ip IS NULL")" = "1"
check "socket frames record via socket without an address" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE function = 'keel.principal.create' AND via = 'socket' AND ip IS NULL AND principal_login = 'owner' AND actor_login IS NULL")" = "2"
check "cookie calls record via http with the peer address and a request id" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE function = 'auth.token.create' AND via = 'http' AND ip = '127.0.0.1' AND request_id LIKE 'req-%'")" = "2"
check "the idempotent replay left no second row" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE function = 'auth.token.create' AND params_redacted->>'name' = 'keyed'")" = "1"
check "read calls leave no row" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE function IN ('note.list', 'keel.account.list', 'settings.list', 'auth.ip_rule.list')")" = "0"
check "note.edit records the masked result as after and no subject on a create" test "$(audit_sql "SELECT string_agg(after->>'title', ' ' ORDER BY after->>'title') FROM keel.audit_call WHERE function = 'note.edit' AND subject IS NULL AND after IS NOT NULL")" = "hers x"
check "auth.token.create records neither subject nor after" test "$(audit_sql "SELECT count(*) FROM keel.audit_call WHERE function = 'auth.token.create' AND (subject IS NOT NULL OR after IS NOT NULL)")" = "0"

echo "--- run 2: no bundle enabled"
check "product disable demo" keel_ok "$site/disable.out" product disable demo
check "product disable demo prints demo: disabled" stdout_is "$site/disable.out" "demo: disabled"
start_serve bare
check "GET /brand/logo.svg without a product is 200" http_is "$site/logo-bare.svg" 200 GET /brand/logo.svg
check "the effective logo without a product is the kernel's mark" cmp -s "$site/logo-bare.svg" crates/kernel/assets/keel.svg
check "GET /brand/demo/logo.svg of the disabled bundle is 200" http_is "$site/logo-demo-bare.svg" 200 GET /brand/demo/logo.svg
check "the disabled bundle's logo route still answers its asset" cmp -s "$site/logo-demo-bare.svg" products/demo/brand/logo.svg
check "ctl keel.account.list" keel_ok "$site/accounts.json" ctl keel.account.list
check "keel.account.list answers the accounts" json_is "$site/accounts.json" 'doc["ok"] is True and len(doc["data"]["data"]) >= 2'
check "POST /api/v1/call/auth.login as alice below the rule's level is 200" http_is "$site/login-2.json" 200 POST /api/v1/call/auth.login '{"login":"alice","password":"correct-horse"}'
cookie=$(session_cookie_of "$site/login-2.json.headers") || fail "auth.login sets the session cookie in run 2"
check "POST /api/v1/call/keel.account.list with the cookie is 403 under the surviving rule" http_is "$site/accounts-denied.json" 403 POST /api/v1/call/keel.account.list '{}' "$cookie"
check "the denied call answers auth.error.ip_denied" json_is "$site/accounts-denied.json" 'doc["ok"] is False and doc["error"]["code"] == "forbidden" and doc["error"]["key"] == "auth.error.ip_denied"'
check "ctl auth.ip_rule.delete over the socket" keel_ok "$site/ip-rule-delete.json" ctl auth.ip_rule.delete "id=$ip_rule_id"
check "auth.ip_rule.delete answers deleted" json_is "$site/ip-rule-delete.json" 'doc["ok"] is True and doc["data"] == {"deleted": True}'
check "POST /api/v1/call/keel.account.list with the cookie is 200 after the delete" http_is "$site/accounts-allowed.json" 200 POST /api/v1/call/keel.account.list '{}' "$cookie"
check "keel.account.list answers the account of alice" json_is "$site/accounts-allowed.json" "doc['ok'] is True and [row['id'] for row in doc['data']['data']] == ['$account_id'] and doc['meta']['principal']['login'] == 'alice'"
check "ctl audit.list --out json" keel_ok "$site/audit.json" ctl audit.list --out json filters='[{"key":"function","op":"eq","value":"auth.su"}]'
check "audit.list answers the su row with root as the principal" json_is "$site/audit.json" 'doc["ok"] is True and doc["data"]["total"] == 1 and doc["data"]["data"][0]["principalLogin"] == "root" and doc["data"]["data"][0]["actorLogin"] is None and doc["data"]["data"][0]["status"] == "committed"'
check "ctl audit.list --as alice sees her account only" keel_ok "$site/audit-alice.json" ctl audit.list --out json --as alice
check "audit.list as alice answers rows of her account alone" json_is "$site/audit-alice.json" "doc['ok'] is True and doc['data']['total'] >= 1 and all(row['accountId'] == '$account_id' for row in doc['data']['data'])"
check "ctl task.list --out json" keel_ok "$site/tasks.json" ctl task.list --out json
check "task.list keeps the two finished exports, the backup, the two restarts and the update after the bundle is disabled" json_is "$site/tasks.json" 'doc["ok"] is True and doc["data"]["total"] == 6 and sorted((row["function"], row["state"], row["progress"]) for row in doc["data"]["data"]) == [("backup.create", "done", "2/2"), ("keel.restart", "done", "3/3"), ("keel.restart", "done", "3/3"), ("note.export", "done", "4/4"), ("note.export", "done", "4/4"), ("update.apply", "done", "3/3")]'
check "ctl cron.job.list --out json" keel_ok "$site/cron-jobs.json" ctl cron.job.list --out json
check "cron.job.list answers the poll job of notify and the daily check of update" json_is "$site/cron-jobs.json" 'doc["ok"] is True and sorted((row["id"], row["module"], row["schedule"]) for row in doc["data"]["data"]) == [("notify.problems.poll", "notify", "* * * * *"), ("update.poll", "update", "30 4 * * *")]'
check "ctl cron.run.list --out json" keel_ok "$site/cron-runs.json" ctl cron.run.list --out json
check "cron.run.list answers runs of the two jobs alone" json_is "$site/cron-runs.json" 'doc["ok"] is True and all(row["job"] in ("notify.problems.poll", "update.poll") and row["outcome"] in ("ok", "running") for row in doc["data"]["data"])'
check "doctor exits 0 with the demo disabled" keel_ok "$site/doctor-2.out" doctor "${layout[@]}"
check "doctor names the orphaned note module" grep -qE '^warn +orphans .*note' "$site/doctor-2.out"
check "doctor reports that no product is enabled" grep -qE '^ok +products .*no product is enabled' "$site/doctor-2.out"
check "ctl notify.problem.list --out json" keel_ok "$site/problems.json" ctl notify.problem.list --out json
check "notify.problem.list answers an empty page" json_is "$site/problems.json" 'doc["ok"] is True and doc["data"]["total"] == 0 and doc["data"]["data"] == []'
check "ctl notify.message.list --out json" keel_ok "$site/messages.json" ctl notify.message.list --out json
check "notify.message.list answers an empty page for the owner" json_is "$site/messages.json" 'doc["ok"] is True and doc["data"]["total"] == 0'
check "ctl notify.problem.raise is refused on the socket" keel_fails "$site/internal.json" ctl notify.problem.raise key=probe.disk subject=/ severity=warning
check "notify.problem.raise answers forbidden with internal_only" json_is "$site/internal.json" 'doc["ok"] is False and doc["error"]["code"] == "forbidden" and doc["error"]["key"] == "keel.error.internal_only"'
check "ctl webhook.subscription.list --out json" keel_ok "$site/webhooks.json" ctl webhook.subscription.list --out json
check "webhook.subscription.list answers an empty page" json_is "$site/webhooks.json" 'doc["ok"] is True and doc["data"]["total"] == 0 and doc["data"]["data"] == []'
check "ctl webhook.subscription.set with a loopback url is refused while private targets are off" keel_fails "$site/webhook-private.json" ctl webhook.subscription.set name=loop url=http://127.0.0.1:9/hook 'events=["note.*"]'
check "the refused subscription answers webhook.error.url_private on url" json_is "$site/webhook-private.json" 'doc["ok"] is False and doc["error"]["fields"][0]["path"] == "url" and doc["error"]["fields"][0]["key"] == "webhook.error.url_private" and doc["error"]["fields"][0]["params"]["reason"] == "scheme"'
check "ctl webhook.subscription.set with a short secret is refused on secret" keel_fails "$site/webhook-short.json" ctl webhook.subscription.set name=short url=https://hooks.example/in secret=too-short 'events=["note.*"]'
check "the short secret answers webhook.error.secret_short" json_is "$site/webhook-short.json" 'doc["ok"] is False and doc["error"]["fields"][0]["path"] == "secret" and doc["error"]["fields"][0]["key"] == "webhook.error.secret_short"'
check "ctl webhook.delivery.list --out json" keel_ok "$site/webhook-deliveries.json" ctl webhook.delivery.list --out json
check "webhook.delivery.list answers an empty page" json_is "$site/webhook-deliveries.json" 'doc["ok"] is True and doc["data"]["total"] == 0'
check "ctl note.list is refused" keel_fails "$site/orphan.json" ctl note.list
check "note.list answers not_found" json_is "$site/orphan.json" 'doc["ok"] is False and doc["error"]["code"] == "not_found"'
check "ctl backup.restore while the service runs writes the restore request" keel_ok "$site/backup-restore.json" ctl backup.restore "archive=$task_archive"
restore_task=$(json_get "$site/backup-restore.json" 'doc["data"]["task"]["id"]')
check "task.get answers done for the restore request" task_reaches "$site/restore-task.json" "$restore_task" "done"
check "the restore task names the archive, the bundle and the request" json_is "$site/restore-task.json" "doc['data']['state'] == 'done' and doc['data']['result']['archive'] == '$site/lib/backups/$task_archive' and doc['data']['result']['products'] == ['demo'] and doc['data']['result']['request'] == '$site/lib/restore.request' and doc['data']['result']['requestedBy'] == 'owner'"
check "the restore request names the archive and the owner" json_is "$site/lib/restore.request" "doc == {'archive': '$site/lib/backups/$task_archive', 'requested_by': 'owner'}"
check "keel serve stops on SIGTERM" stop_serve
check "the socket file is removed" socket_is_gone

echo "--- run 3: the pending restore runs before the start, then the CLI restore with the secret key"
start_serve restored
check "the start consumed the restore request" test ! -e "$site/lib/restore.request"
check "the start logged the pending restore and the enabled set it changed" grep -q "restored from a backup; the enabled set changed: +demo" "$serve_log"
check "product list shows demo enabled again after the restore" keel_ok "$site/list-restored.out" product list
check "the restored enabled set carries demo" stdout_has_line "$site/list-restored.out" "$(printf 'demo\tenabled\t%s' "$version")"
check "ctl note.list after the restore" keel_ok "$site/notes-restored.json" ctl note.list --out json
check "note.list answers the notes of the backup" cmp -s <(json_get "$site/notes-at-backup.json" 'json.dumps(sorted(row["title"] for row in doc["data"]["data"]))') <(json_get "$site/notes-restored.json" 'json.dumps(sorted(row["title"] for row in doc["data"]["data"]))')
check "keel serve stops on SIGTERM" stop_serve
KEEL_BACKUP_PASSPHRASE=wrong check "restore refuses a wrong passphrase" keel_fails "$site/restore-wrong.err" restore "$site/backup.tar.zst" --with-secret-key
check "the refusal names the passphrase" grep -q "passphrase does not open" "$site/restore-wrong.err"
KEEL_BACKUP_PASSPHRASE=prove-passphrase check "restore with the secret key restores the CLI archive" keel_ok "$site/restore.out" restore "$site/backup.tar.zst" --with-secret-key
check "restore prints the loaded tables, the enabled set and the kept configuration" grep -q "^enabled products: demo -> demo (unchanged)$" "$site/restore.out"
check "restore prints where the secret key landed" grep -q "^secret key: written to $site/lib/secret.key$" "$site/restore.out"
check "restore keeps this host's configuration and puts the archive's copy beside it" cmp -s "$site/etc/config.yaml" "$site/etc/config.yaml.restored"
check "the restore note names the archive and the kept data key" json_is "$site/lib/restore.note" "doc['archive'] == '$site/backup.tar.zst' and doc['data_key'] == 'kept' and doc['requested_by'] == 'keel restore' and doc['products_after'] == ['demo']"
start_serve restored-cli
check "the start consumed the restore note" test ! -e "$site/lib/restore.note"
check "ctl note.list after the CLI restore" keel_ok "$site/notes-restored-cli.json" ctl note.list --out json
check "note.list answers the notes of the CLI backup" cmp -s <(json_get "$site/notes-at-backup.json" 'json.dumps(sorted(row["title"] for row in doc["data"]["data"]))') <(json_get "$site/notes-restored-cli.json" 'json.dumps(sorted(row["title"] for row in doc["data"]["data"]))')
check "keel serve stops on SIGTERM" stop_serve
check "the socket file is removed" socket_is_gone
