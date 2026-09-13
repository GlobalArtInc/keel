#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

timeout=${KEEL_PROVE_TIMEOUT:-30}
keel=${1:-}
admin_url=${KEEL_TEST_DATABASE_URL:-}
database=""
site=""
serve_pid=""
serve_log=""

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
print(eval(sys.argv[2], {"doc": doc}))
EOF
}

header_of() {
  local file=$1 name=$2
  grep -i "^$name:" "$file" | head -n 1 | cut -d' ' -f2- | tr -d '\r'
}

run_keel() {
  "$keel" --config "$site/config.yaml" "$@"
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

http() {
  local out=$1 method=$2 path=$3 body=${4:-} cookie=${5:-}
  local status
  local -a headers=(-H 'Content-Type: application/json')
  if [ -n "$cookie" ]; then
    headers+=(-H "Cookie: $cookie")
  fi
  if [ -n "$body" ]; then
    status=$(curl -sS -o "$out" -D "$out.headers" -w '%{http_code}' -X "$method" "${headers[@]}" --data "$body" "http://127.0.0.1:$port$path")
  else
    status=$(curl -sS -o "$out" -D "$out.headers" -w '%{http_code}' -X "$method" "${headers[@]}" "http://127.0.0.1:$port$path")
  fi
  echo "$status"
}

http_is() {
  local out=$1 expected=$2 method=$3 path=$4 body=${5:-} cookie=${6:-}
  local status
  status=$(http "$out" "$method" "$path" "$body" "$cookie")
  if [ "$status" != "$expected" ]; then
    echo "  $method $path answered $status, expected $expected" >&2
    sed 's/^/  /' "$out" >&2
    return 1
  fi
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
  "$keel" --config "$site/config.yaml" serve >"$serve_log" 2>&1 &
  serve_pid=$!
  local waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if ! kill -0 "$serve_pid" 2>/dev/null; then
      sed 's/^/  /' "$serve_log" >&2
      serve_pid=""
      fail "keel serve ($label) exited before it was ready"
    fi
    if [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/readyz" 2>/dev/null)" = "200" ]; then
      echo "ok: keel serve ($label) is ready on 127.0.0.1:$port"
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

socket_is_gone() {
  [ ! -e "$site/run/core.sock" ]
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
mkdir -p "$site/run" "$site/lib"
cat >"$site/config.yaml" <<EOF
listen: 127.0.0.1:$port
socket: $site/run/core.sock
data_dir: $site/lib
database: $(database_url "$admin_url" "$database")
log: { level: info, format: text }
EOF
echo "site $site, binary $keel $version"

echo "--- run 1: the demo bundle enabled"
check "product enable demo" keel_ok "$site/enable.out" product enable demo
check "product enable demo prints demo: enabled" stdout_is "$site/enable.out" "demo: enabled"
check "product list" keel_ok "$site/list.out" product list
check "product list shows demo enabled" stdout_has_line "$site/list.out" "$(printf 'demo\tenabled\t%s' "$version")"
start_serve demo

check "ctl keel.account.create kind=user" keel_ok "$site/account.json" ctl keel.account.create kind=user
check "keel.account.create answers ok with an id" json_is "$site/account.json" 'doc["ok"] is True and re.fullmatch("[0-7][0-9A-HJKMNP-TV-Z]{25}", doc["data"]["id"])'
check "GET /healthz is 200" http_is "$site/healthz.json" 200 GET /healthz
check "healthz reports ok" json_is "$site/healthz.json" 'doc == {"status": "ok"}'
check "GET /api/v1/boot is 200" http_is "$site/boot.json" 200 GET /api/v1/boot
check "boot lists the demo product" json_is "$site/boot.json" 'any(p["id"] == "demo" for p in doc["products"])'
check "boot carries the demo brand" json_is "$site/boot.json" 'doc["brand"]["name"] == "product.demo.brand" and doc["brand"]["hue"] == 150 and doc["brand"]["chroma"] == 0.1'
check "boot names the keel version" json_is "$site/boot.json" "doc['keel']['version'] == '$version' and doc['keel']['api'] == 1"
check "boot describes the demo product with its brand" json_is "$site/boot.json" 'doc["products"][0] == {"id": "demo", "name": "product.demo.brand", "edition": None, "brand": {"name": "product.demo.brand", "logo": "/brand/demo/logo.svg", "hue": 150, "chroma": 0.1}}'
check "ctl note.edit title=x" keel_ok "$site/edit.json" ctl note.edit title=x
check "note.edit answers the saved note" json_is "$site/edit.json" 'doc["ok"] is True and doc["data"]["title"] == "x" and doc["meta"]["principal"]["login"] == "owner"'
check "ctl note.list --out json" keel_ok "$site/notes.json" ctl note.list --out json
check "note.list answers the page with the note" json_is "$site/notes.json" 'doc["ok"] is True and [row["title"] for row in doc["data"]["data"]] == ["x"]'
check "POST /api/v1/call/note.list without a credential is 401" http_is "$site/call.json" 401 POST /api/v1/call/note.list '{}'
check "the call answers the unauthenticated envelope" json_is "$site/call.json" 'doc["ok"] is False and doc["error"]["code"] == "unauthenticated" and doc["error"]["key"] == "keel.error.unauthenticated"'
account_id=$(json_get "$site/account.json" 'doc["data"]["id"]')
check "ctl keel.principal.create login=alice" keel_ok "$site/principal.json" ctl keel.principal.create login=alice level=user "account=$account_id"
check "keel.principal.create answers ok with an id" json_is "$site/principal.json" 'doc["ok"] is True and re.fullmatch("[0-7][0-9A-HJKMNP-TV-Z]{25}", doc["data"]["id"])'
principal_id=$(json_get "$site/principal.json" 'doc["data"]["id"]')
check "ctl auth.password.set for alice" keel_ok "$site/password.json" ctl auth.password.set "principal=$principal_id" password=correct-horse
check "auth.password.set answers the principal" json_is "$site/password.json" "doc['ok'] is True and doc['data']['principal'] == '$principal_id'"
check "ctl note.edit title=hers in the account of alice" keel_ok "$site/hers.json" ctl note.edit title=hers "account=$account_id"
check "POST /api/v1/call/auth.login with a wrong password is 401" http_is "$site/wrong.json" 401 POST /api/v1/call/auth.login '{"login":"alice","password":"wrong"}'
check "the wrong password answers auth.error.credentials" json_is "$site/wrong.json" 'doc["ok"] is False and doc["error"]["code"] == "unauthenticated" and doc["error"]["key"] == "auth.error.credentials"'
check "POST /api/v1/call/auth.login is 200" http_is "$site/login.json" 200 POST /api/v1/call/auth.login '{"login":"alice","password":"correct-horse"}'
check "auth.login answers the authenticated state for alice" json_is "$site/login.json" 'doc["ok"] is True and doc["data"]["state"] == "authenticated" and doc["data"]["principal"]["login"] == "alice"'
cookie=$(session_cookie_of "$site/login.json.headers") || fail "auth.login sets the session cookie"
check "auth.login sets the session cookie" test -n "$cookie"
check "the cookie carries the fixed attributes" grep -qi '^set-cookie: __Host-keel_session=ks_[0-9a-f]*; Path=/; Secure; HttpOnly; SameSite=Lax\r\{0,1\}$' "$site/login.json.headers"
check "GET /api/v1/boot with the cookie is 200" http_is "$site/boot-alice.json" 200 GET /api/v1/boot '' "$cookie"
check "boot names alice as the principal" json_is "$site/boot-alice.json" 'doc["principal"]["login"] == "alice" and doc["principal"]["level"] == "user"'
check "POST /api/v1/call/note.list with the cookie is 200" http_is "$site/notes-alice.json" 200 POST /api/v1/call/note.list '{}' "$cookie"
check "note.list answers the page of alice" json_is "$site/notes-alice.json" 'doc["ok"] is True and [row["title"] for row in doc["data"]["data"]] == ["hers"] and doc["meta"]["principal"]["login"] == "alice"'
check "POST /api/v1/call/auth.logout with the cookie is 200" http_is "$site/logout.json" 200 POST /api/v1/call/auth.logout '{}' "$cookie"
check "auth.logout clears the cookie" grep -qi '^set-cookie: __Host-keel_session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0\r\{0,1\}$' "$site/logout.json.headers"
check "POST /api/v1/call/note.list with the revoked cookie is 401" http_is "$site/revoked.json" 401 POST /api/v1/call/note.list '{}' "$cookie"
check "keel serve stops on SIGTERM" stop_serve
check "the socket file is removed" socket_is_gone

echo "--- run 2: no bundle enabled"
check "product disable demo" keel_ok "$site/disable.out" product disable demo
check "product disable demo prints demo: disabled" stdout_is "$site/disable.out" "demo: disabled"
start_serve bare
check "ctl keel.account.list" keel_ok "$site/accounts.json" ctl keel.account.list
check "keel.account.list answers the accounts" json_is "$site/accounts.json" 'doc["ok"] is True and len(doc["data"]["data"]) >= 2'
check "ctl note.list is refused" keel_fails "$site/orphan.json" ctl note.list
check "note.list answers not_found" json_is "$site/orphan.json" 'doc["ok"] is False and doc["error"]["code"] == "not_found"'
check "keel serve stops on SIGTERM" stop_serve
check "the socket file is removed" socket_is_gone
