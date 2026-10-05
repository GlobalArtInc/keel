#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
. hack/agent.sh

timeout=${KEEL_PROVE_TIMEOUT:-30}
keel=${1:-}
conformance=${2:-}
admin_url=${KEEL_TEST_DATABASE_URL:-}
read -r -a compositions <<<"${KEEL_CONFORMANCE_COMPOSITIONS:-dns panel dns,panel}"
first_party=(dns panel)
scheme=https
cacert=""
database=""
site=""
serve_pid=""
serve_log=""
admin_login=root
user_login=alice
guest_per_minute=600

usage() {
  echo "usage: hack/conformance.sh [path/to/keel] [path/to/keel-conformance]" >&2
  echo "  KEEL_TEST_DATABASE_URL must point at a Postgres superuser connection;" >&2
  echo "  a binary that is not given is built with cargo build;" >&2
  echo "  KEEL_CONFORMANCE_COMPOSITIONS lists the product sets to run against," >&2
  echo "  each a comma-separated set, by default \"dns panel dns,panel\"" >&2
}

fail() {
  echo "FAILED: $1" >&2
  exit 1
}

teardown() {
  local code=0
  if [ -n "$serve_pid" ]; then
    echo "stopping keel serve ($serve_pid)"
    if ! stop_serve; then
      code=1
    fi
  fi
  if ! stop_agent; then
    code=1
  fi
  if [ -n "$database" ]; then
    if psql "$admin_url" -qAt -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS \"$database\" WITH (FORCE)" >/dev/null; then
      echo "dropped database $database"
    else
      echo "could not drop database $database" >&2
      code=1
    fi
    database=""
  fi
  if [ -n "$site" ] && [ -d "$site" ]; then
    rm -rf "$site"
    echo "removed $site"
  fi
  site=""
  return "$code"
}

cleanup() {
  local code=$?
  trap - EXIT
  if ! teardown; then
    code=1
  fi
  if [ "$code" -eq 0 ]; then
    echo "the conformance suite is green against ${compositions[*]}"
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
    if ready; then
      echo "keel serve is ready on $scheme://127.0.0.1:$port"
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

composed() {
  local products=$1 product=$2
  case ",$products," in *",$product,"*) return 0 ;; *) return 1 ;; esac
}

run_composition() {
  local products=$1 index=$2
  local product tmp
  local -a enabled
  echo "=== the composition $products"
  tmp=${TMPDIR:-/tmp}
  site=$(mktemp -d "${tmp%/}/keel-conformance.XXXXXX")
  database="keel_test_conformance_$(date +%s)_$$_$index"
  port=$(free_port)
  psql "$admin_url" -qAt -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$database\" TEMPLATE template1" >/dev/null
  echo "created database $database"
  mkdir -p "$site/run" "$site/lib"
  cacert=""
  if [ "$scheme" = https ]; then
    cacert="$site/lib/tls/cert.pem"
  fi
  cat >"$site/config.yaml" <<EOF
listen: 127.0.0.1:$port
socket: $site/run/core.sock
data_dir: $site/lib
database: $(database_url "$admin_url" "$database")
log: { level: info, format: text }
host: { socket: $site/agent/agent.sock }
dns: { zones_dir: $site/agent/zones, include_dir: $site/agent/named }
hosting: { transfer_dir: $site/agent/transfer }
EOF
  echo "site $site, binary $keel, suite $conformance"
  start_agent "$keel" "$site/agent" || fail "keel privileged did not start beside the kernel"

  IFS=, read -r -a enabled <<<"$products"
  for product in "${enabled[@]}"; do
    keel_ok "$site/enable-$product.out" product enable "$product"
  done
  keel_ok "$site/features.out" product features
  for product in "${first_party[@]}"; do
    if composed "$products" "$product"; then
      grep -q "^$product\." "$site/features.out" \
        || fail "keel product features names no feature of $product, which the composition $products enables"
    elif grep -q "^$product\." "$site/features.out"; then
      fail "keel product features names a feature of $product, which the composition $products leaves disabled"
    fi
  done
  echo "keel product features names the features of the enabled products and of no other"
  start_serve

  local root_account=00000000000000000000000000
  local admin_password user_password admin_id account_id user_id
  admin_password=$(secret)
  user_password=$(secret)
  keel_ok "$site/admin.json" ctl keel.principal.create "login=$admin_login" level=admin "account=$root_account"
  admin_id=$(json_get "$site/admin.json" 'doc["data"]["id"]')
  keel_ok "$site/admin-password.json" ctl auth.password.set "principal=$admin_id" "password=$admin_password"
  keel_ok "$site/account.json" ctl keel.account.create kind=user
  account_id=$(json_get "$site/account.json" 'doc["data"]["id"]')
  keel_ok "$site/user.json" ctl keel.principal.create "login=$user_login" level=user "account=$account_id"
  user_id=$(json_get "$site/user.json" 'doc["data"]["id"]')
  keel_ok "$site/user-password.json" ctl auth.password.set "principal=$user_id" "password=$user_password"
  echo "created the admin $admin_login and the user $user_login in account $account_id"
  keel_ok "$site/private-targets.json" ctl settings.set key=webhook.private_targets value=true
  echo "allowed webhook deliveries to private targets for the suite's listener"
  keel_ok "$site/shell-url.json" ctl settings.set key=auth.shell_url "value=$scheme://127.0.0.1:$port"
  echo "gave the installation its public address $scheme://127.0.0.1:$port, which pairing sends a neighbour"
  keel_ok "$site/rate.json" ctl settings.set key=auth.rate.guest_per_minute "value=$guest_per_minute"
  echo "raised the guest rate to $guest_per_minute calls a minute for the suite's logins and pairings"

  echo "--- keel-conformance against $products"
  local args=(--url "$scheme://127.0.0.1:$port" --admin-login "$admin_login" --user-login "$user_login")
  if [ -n "$cacert" ]; then
    args+=(--cacert "$cacert")
  fi
  KEEL_CONFORMANCE_ADMIN_PASSWORD="$admin_password" "$conformance" "${args[@]}"

  stop_serve || fail "keel serve did not stop cleanly"
  teardown || fail "the composition $products was not torn down cleanly"
  echo "=== the composition $products is green"
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
if [ "${#compositions[@]}" -eq 0 ]; then
  usage
  echo "KEEL_CONFORMANCE_COMPOSITIONS names no composition" >&2
  exit 2
fi
for products in "${compositions[@]}"; do
  if ! [[ "$products" =~ ^[a-z][a-z0-9_]*(,[a-z][a-z0-9_]*)*$ ]]; then
    usage
    echo "the composition $products is not a comma-separated set of product ids" >&2
    exit 2
  fi
done

if [ -z "$keel" ]; then
  cargo build -p keel-bin
  keel="${CARGO_TARGET_DIR:-target}/debug/keel"
fi
if [ -z "$conformance" ]; then
  cargo build -p keel-conformance
  conformance="${CARGO_TARGET_DIR:-target}/debug/keel-conformance"
fi
case "$keel" in /*) ;; *) keel="$PWD/$keel" ;; esac
case "$conformance" in /*) ;; *) conformance="$PWD/$conformance" ;; esac
for binary in "$keel" "$conformance"; do
  if [ ! -x "$binary" ]; then
    fail "$binary is not an executable binary"
  fi
done

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

index=0
for products in "${compositions[@]}"; do
  index=$((index + 1))
  run_composition "$products" "$index"
done
