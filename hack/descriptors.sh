#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

timeout=${KEEL_PROVE_TIMEOUT:-30}
keel=${1:-}
dev=${2:-}
admin_url=${KEEL_TEST_DATABASE_URL:-}
dump=contracts/dump
openapi=contracts/openapi.json
frontend=frontend
database=""
site=""
serve_pid=""
serve_log=""
admin_login=root
reseller_login=res1
user_login=alice

usage() {
  echo "usage: hack/descriptors.sh [path/to/keel] [path/to/keel-dev]" >&2
  echo "  KEEL_TEST_DATABASE_URL must point at a Postgres superuser connection;" >&2
  echo "  a binary that is not given is built with cargo build;" >&2
  echo "  KEEL_UPDATE_DUMP=1 rewrites $dump and $openapi instead of checking them" >&2
}

fail() {
  echo "FAILED: $1" >&2
  exit 1
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
    echo "the descriptor dump, the OpenAPI document and the custom-views share are green"
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

secret() {
  python3 -c 'import secrets; print(secrets.token_hex(16))'
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

start_serve() {
  serve_log="$site/serve.log"
  "$keel" --config "$site/config.yaml" serve >"$serve_log" 2>&1 &
  serve_pid=$!
  local waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if ! kill -0 "$serve_pid" 2>/dev/null; then
      sed 's/^/  /' "$serve_log" >&2
      serve_pid=""
      fail "keel serve exited before it was ready"
    fi
    if [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/readyz" 2>/dev/null)" = "200" ]; then
      echo "keel serve is ready on 127.0.0.1:$port"
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  sed 's/^/  /' "$serve_log" >&2
  fail "keel serve did not answer /readyz within ${timeout}s"
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

create_principal() {
  local out=$1 login=$2 level=$3 account=$4 password=$5
  keel_ok "$out" ctl keel.principal.create "login=$login" "level=$level" "account=$account"
  local id
  id=$(json_get "$out" 'doc["data"]["id"]')
  keel_ok "$out.password" ctl auth.password.set "principal=$id" "password=$password"
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
if [ -z "$dev" ]; then
  cargo build -p keel-dev
  dev="${CARGO_TARGET_DIR:-target}/debug/keel-dev"
fi
case "$keel" in /*) ;; *) keel="$PWD/$keel" ;; esac
case "$dev" in /*) ;; *) dev="$PWD/$dev" ;; esac
for binary in "$keel" "$dev"; do
  if [ ! -x "$binary" ]; then
    fail "$binary is not an executable binary"
  fi
done

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

tmp=${TMPDIR:-/tmp}
site=$(mktemp -d "${tmp%/}/keel-descriptors.XXXXXX")
database="keel_test_descriptors_$(date +%s)_$$"
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
echo "site $site, binary $keel, developer CLI $dev"

keel_ok "$site/enable.out" product enable demo
start_serve

root_account=00000000000000000000000000
admin_password=$(secret)
reseller_password=$(secret)
user_password=$(secret)
keel_ok "$site/reseller-account.json" ctl keel.account.create kind=reseller
reseller_account=$(json_get "$site/reseller-account.json" 'doc["data"]["id"]')
keel_ok "$site/user-account.json" ctl keel.account.create kind=user "parent=$reseller_account"
user_account=$(json_get "$site/user-account.json" 'doc["data"]["id"]')
create_principal "$site/admin.json" "$admin_login" admin "$root_account" "$admin_password"
create_principal "$site/reseller.json" "$reseller_login" reseller "$reseller_account" "$reseller_password"
create_principal "$site/user.json" "$user_login" user "$user_account" "$user_password"
echo "created the admin $admin_login in the root account, the reseller $reseller_login in $reseller_account and the user $user_login in $user_account"

url="http://127.0.0.1:$port"
logins=(
  --as "admin=$admin_login:$admin_password"
  --as "reseller=$reseller_login:$reseller_password"
  --as "user=$user_login:$user_password"
)
if [ -n "${KEEL_UPDATE_DUMP:-}" ]; then
  echo "--- keel-dev descriptors dump"
  "$dev" descriptors dump --url "$url" --out "$dump" "${logins[@]}"
else
  echo "--- keel-dev descriptors check"
  "$dev" descriptors check --url "$url" --dir "$dump" "${logins[@]}"
fi

stop_serve || fail "keel serve did not stop cleanly"

if [ -n "${KEEL_UPDATE_DUMP:-}" ]; then
  echo "--- keel-dev openapi"
  "$dev" openapi --dump "$dump" --out "$openapi"
else
  echo "--- keel-dev openapi --check"
  "$dev" openapi --dump "$dump" --out "$openapi" --check
fi

echo "--- keel-dev views share"
"$dev" views share --dump "$dump" --frontend "$frontend"
