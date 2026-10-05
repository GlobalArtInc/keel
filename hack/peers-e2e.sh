#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
. hack/agent.sh

timeout=${KEEL_PROVE_TIMEOUT:-30}
keel=${1:-}
admin_url=${KEEL_TEST_DATABASE_URL:-}
frontend=frontend
work=""
panel=""
dns=""
panel_database=""
dns_database=""
panel_port=""
dns_port=""
panel_pid=""
dns_pid=""
panel_agent=""
dns_agent=""
to_dns_pid=""
to_dns_port=""
to_panel_pid=""
to_panel_port=""
root_account=00000000000000000000000000
admin_login=root
reseller_login=res1
user_login=alice
hosting_user=files
hosting_range="{ from: 100000, to: 100009 }"
peers_tag=@peers
peer_site=peer.e2e.test
taken_zone=taken.e2e.test
peer_scopes='["dns.zone.*","keel.principal.upsert","auth.key.handover"]'
peer_events='["dns.zone.*","svc.*"]'
service=peers-e2e-zones
service_key=peers-e2e-open-1
zone_limit=5
hook_path=/api/v1/x/peer/hook

usage() {
  echo "usage: hack/peers-e2e.sh [path/to/keel] [playwright arguments...]" >&2
  echo "  KEEL_TEST_DATABASE_URL must point at a Postgres superuser connection;" >&2
  echo "  a binary that is not given is built with cargo build;" >&2
  echo "  the arguments after the binary are passed to pnpm e2e" >&2
}

fail() {
  echo "FAILED: $1" >&2
  exit 1
}

stop_relay() {
  local pid=$1 name=$2
  [ -n "$pid" ] || return 0
  kill -TERM "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  echo "stopped $name"
}

stop_agent_of() {
  local pid=$1
  [ -n "$pid" ] || return 0
  agent_pid=$pid
  stop_agent
}

cleanup() {
  local code=$?
  trap - EXIT
  stop_relay "$to_dns_pid" "the relay to the dns installation"
  to_dns_pid=""
  stop_relay "$to_panel_pid" "the relay to the panel installation"
  to_panel_pid=""
  if [ -n "$panel_pid" ]; then
    echo "stopping the panel installation ($panel_pid)"
    if ! stop_panel; then
      code=1
    fi
  fi
  if [ -n "$dns_pid" ]; then
    echo "stopping the dns installation ($dns_pid)"
    if ! stop_dns; then
      code=1
    fi
  fi
  if ! stop_agent_of "$panel_agent"; then
    code=1
  fi
  panel_agent=""
  if ! stop_agent_of "$dns_agent"; then
    code=1
  fi
  dns_agent=""
  for name in "$panel_database" "$dns_database"; do
    [ -n "$name" ] || continue
    if psql "$admin_url" -qAt -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS \"$name\" WITH (FORCE)" >/dev/null; then
      echo "dropped database $name"
    else
      echo "could not drop database $name" >&2
      code=1
    fi
  done
  if [ -n "$work" ] && [ -d "$work" ]; then
    rm -rf "$work"
    echo "removed $work"
  fi
  if [ "$code" -eq 0 ]; then
    echo "two installations are green end to end"
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

json_is() {
  local file=$1 expression=$2
  python3 - "$file" "$expression" <<'EOF'
import json, sys
with open(sys.argv[1]) as handle:
    doc = json.load(handle)
if not eval(sys.argv[2], {"doc": doc}):
    print("  " + json.dumps(doc)[:600], file=sys.stderr)
    sys.exit(1)
EOF
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

eventually() {
  local waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if "$@" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  "$@"
}

lower() {
  tr '[:upper:]' '[:lower:]' <<<"$1"
}

run_panel() {
  "$keel" --config "$panel/config.yaml" "$@"
}

run_dns() {
  "$keel" --config "$dns/config.yaml" "$@"
}

panel_ok() {
  local out=$1
  shift
  if ! run_panel "$@" >"$out" 2>"$panel/stderr"; then
    cat "$out" "$panel/stderr" >&2
    return 1
  fi
}

dns_ok() {
  local out=$1
  shift
  if ! run_dns "$@" >"$out" 2>"$dns/stderr"; then
    cat "$out" "$dns/stderr" >&2
    return 1
  fi
}

panel_refused() {
  local out=$1
  shift
  if run_panel "$@" >"$panel/stdout" 2>"$out"; then
    echo "  keel ctl $2 was accepted" >&2
    cat "$panel/stdout" >&2
    return 1
  fi
}

panel_sql() {
  psql "$(database_url "$admin_url" "$panel_database")" -qAt -v ON_ERROR_STOP=1 -c "$1"
}

dns_sql() {
  psql "$(database_url "$admin_url" "$dns_database")" -qAt -v ON_ERROR_STOP=1 -c "$1"
}

sql_is() {
  local side=$1 expected=$2 query=$3 actual
  actual=$("${side}_sql" "$query") || return 1
  if [ "$actual" != "$expected" ]; then
    echo "  the $side database answered '$actual', expected '$expected'" >&2
    return 1
  fi
}

answered_https() {
  local certificate=$1 port=$2
  [ -f "$certificate" ] \
    && [ "$(curl --cacert "$certificate" -s -o /dev/null -w '%{http_code}' "https://127.0.0.1:$port/readyz" 2>/dev/null)" = "200" ]
}

panel_ready() {
  answered_https "$panel/lib/tls/cert.pem" "$panel_port"
}

dns_ready() {
  answered_https "$dns/lib/tls/cert.pem" "$dns_port"
}

to_dns_ready() {
  [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$to_dns_port/readyz" 2>/dev/null)" = "200" ]
}

to_panel_ready() {
  [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$to_panel_port/readyz" 2>/dev/null)" = "200" ]
}

await_ready() {
  local pid=$1 log=$2 name=$3 probe=$4
  local waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if ! kill -0 "$pid" 2>/dev/null; then
      sed 's/^/  /' "$log" >&2
      fail "$name exited before it was ready"
    fi
    if "$probe"; then
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  sed 's/^/  /' "$log" >&2
  fail "$name did not answer /readyz within ${timeout}s"
}

stop_process() {
  local pid=$1 log=$2 name=$3
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
    echo "$name did not stop on SIGTERM within ${timeout}s; sending SIGKILL" >&2
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    return 1
  fi
  local status=0
  wait "$pid" || status=$?
  if [ "$status" -ne 0 ]; then
    echo "$name exited with $status" >&2
    sed 's/^/  /' "$log" >&2
    return 1
  fi
}

start_panel() {
  "$keel" --config "$panel/config.yaml" serve >"$panel/serve.log" 2>&1 &
  panel_pid=$!
  await_ready "$panel_pid" "$panel/serve.log" "the panel installation" panel_ready
  echo "the panel installation is ready on https://127.0.0.1:$panel_port"
}

stop_panel() {
  [ -n "$panel_pid" ] || return 0
  local pid=$panel_pid
  panel_pid=""
  stop_process "$pid" "$panel/serve.log" "the panel installation"
}

start_dns() {
  "$keel" --config "$dns/config.yaml" serve >"$dns/serve.log" 2>&1 &
  dns_pid=$!
  await_ready "$dns_pid" "$dns/serve.log" "the dns installation" dns_ready
  echo "the dns installation is ready on https://127.0.0.1:$dns_port"
}

stop_dns() {
  [ -n "$dns_pid" ] || return 0
  local pid=$dns_pid
  dns_pid=""
  stop_process "$pid" "$dns/serve.log" "the dns installation"
}

relay() {
  local listen=$1 target=$2 certificate=$3 log=$4 mode=$5
  node -e '
const http = require("node:http");
const https = require("node:https");
const { appendFileSync, readFileSync } = require("node:fs");
const [listen, target, certificate, log, mode, hook] = process.argv.slice(1);
const ca = readFileSync(certificate);
let dropped = mode !== "drop-first-hook";
const credential = (header) => /^Bearer (k[a-z]+)_/.exec(header ?? "")?.[1] ?? null;
const record = (entry) => appendFileSync(log, `${JSON.stringify(entry)}\n`);
http
  .createServer((request, response) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => {
      const body = Buffer.concat(chunks);
      const delivery = request.method === "POST" && request.url === hook;
      const headers = { ...request.headers, host: `127.0.0.1:${target}` };
      const upstream = https.request(
        { host: "127.0.0.1", port: Number(target), path: request.url, method: request.method, headers, ca },
        (answer) => {
          const parts = [];
          answer.on("data", (part) => parts.push(part));
          answer.on("end", () => {
            const reply = Buffer.concat(parts);
            const drop = delivery && !dropped;
            dropped = dropped || drop;
            const status = drop ? 500 : (answer.statusCode ?? 502);
            const entry = {
              method: request.method,
              path: request.url,
              status: answer.statusCode,
              answered: status,
              credential: credential(request.headers.authorization),
            };
            if (delivery) {
              entry.eventId = request.headers["x-keel-event-id"] ?? null;
              entry.peer = request.headers["x-keel-peer"] ?? null;
              entry.signature = request.headers["x-keel-signature"] ?? null;
              entry.body = body.toString("base64");
              entry.reply = reply.toString();
            }
            record(entry);
            if (drop) {
              response.writeHead(500, { "content-type": "text/plain" });
              response.end("the relay dropped the answer of the first delivery");
              return;
            }
            const out = { ...answer.headers, "content-length": String(reply.length) };
            delete out["transfer-encoding"];
            response.writeHead(status, out);
            response.end(reply);
          });
        },
      );
      upstream.on("error", (error) => {
        record({ method: request.method, path: request.url, error: String(error) });
        response.writeHead(502, { "content-type": "text/plain" });
        response.end(String(error));
      });
      upstream.end(body);
    });
  })
  .listen(Number(listen), "127.0.0.1");
' "$listen" "$target" "$certificate" "$log" "$mode" "$hook_path" >"$log.out" 2>&1 &
}

start_relays() {
  : >"$work/to-dns.jsonl"
  relay "$to_dns_port" "$dns_port" "$dns/lib/tls/cert.pem" "$work/to-dns.jsonl" pass
  to_dns_pid=$!
  await_ready "$to_dns_pid" "$work/to-dns.jsonl.out" "the relay to the dns installation" to_dns_ready
  echo "the relay to the dns installation listens on http://127.0.0.1:$to_dns_port"
  : >"$work/to-panel.jsonl"
  relay "$to_panel_port" "$panel_port" "$panel/lib/tls/cert.pem" "$work/to-panel.jsonl" drop-first-hook
  to_panel_pid=$!
  await_ready "$to_panel_pid" "$work/to-panel.jsonl.out" "the relay to the panel installation" to_panel_ready
  echo "the relay to the panel installation listens on http://127.0.0.1:$to_panel_port and drops the answer of the first delivery"
}

deliveries() {
  python3 - "$work/to-panel.jsonl" "$hook_path" <<'EOF'
import base64, json, sys
with open(sys.argv[1]) as handle:
    rows = [json.loads(line) for line in handle if line.strip()]
deliveries = []
for row in rows:
    if row.get("method") != "POST" or row.get("path") != sys.argv[2]:
        continue
    signature = dict(part.split("=", 1) for part in (row.get("signature") or "").split(",") if "=" in part)
    row["t"] = int(signature.get("t", "0"))
    row["v1"] = signature.get("v1")
    row["event"] = json.loads(base64.b64decode(row["body"]))
    row["answer"] = json.loads(row["reply"]) if row.get("reply") else None
    deliveries.append(row)
print(json.dumps(deliveries))
EOF
}

deliveries_are() {
  local expression=$1
  deliveries >"$work/deliveries.json" && json_is "$work/deliveries.json" "$expression"
}

peer_token_answers() {
  local mark=$1 expression=$2
  python3 - "$work/to-dns.jsonl" "$mark" "$expression" <<'EOF'
import json, sys
with open(sys.argv[1]) as handle:
    rows = [json.loads(line) for line in handle if line.strip()]
mark = int(sys.argv[2])
before = [row["status"] for row in rows[:mark] if row.get("credential") == "kpt"]
after = [row["status"] for row in rows[mark:] if row.get("credential") == "kpt"]
if not eval(sys.argv[3], {"before": before, "after": after}):
    print(f"  before {before}, after {after}", file=sys.stderr)
    sys.exit(1)
EOF
}

instance_of() {
  local certificate=$1 port=$2
  curl --cacert "$certificate" -s "https://127.0.0.1:$port/api/v1/instance"
}

handshake_names() {
  local certificate=$1 port=$2 product=$3 out=$4
  instance_of "$certificate" "$port" >"$out" \
    && json_is "$out" "doc['products'] == ['$product'] and doc['publicKey'].startswith('ed25519:') and 'peers' in doc['features']"
}

create_principal() {
  local side=$1 out=$2 login=$3 level=$4 account=$5 password=$6 id
  "${side}_ok" "$out" ctl keel.principal.create "login=$login" "level=$level" "account=$account" --out json || return 1
  id=$(json_get "$out" 'doc["data"]["id"]')
  "${side}_ok" "$out.password" ctl auth.password.set "principal=$id" "password=$password"
}

hosting_user_active() {
  panel_ok "$panel/hosting-user-state.json" ctl hosting.user.get "id=$1" --out json \
    && json_is "$panel/hosting-user-state.json" "doc['data']['state'] == 'active'"
}

zone_on_dns() {
  dns_ok "$dns/zones.json" ctl dns.zone.list "search=$peer_site" --out json \
    && json_is "$dns/zones.json" "[(row['name'], row['state']) for row in doc['data']['data']] == [('$peer_site', 'active')]" \
    && test -f "$dns/agent/zones/keel-dns-$peer_site.zone" \
    && grep -qF "keel-dns-$peer_site.conf" "$dns/agent/named/keel-dns-zones.conf"
}

dns_delivery_is() {
  local event=$1 expression=$2
  dns_ok "$dns/delivery.json" ctl webhook.delivery.list "search=$event" --out json \
    && json_is "$dns/delivery.json" "$expression"
}

panel_received_once() {
  local event=$1 kind=$2
  sql_is panel "1 true" "SELECT count(*) || ' ' || coalesce(bool_and(processed_at IS NOT NULL), false) FROM keel.peer_inbox WHERE event_id = '$event'" \
    && sql_is panel "$kind $dns_instance" "SELECT string_agg(type || ' ' || instance_id, ',') FROM keel.keel_outbox WHERE id = '$event'"
}

replay_delivery() {
  local index=$1 tamper=$2 out=$3 expected=$4 status
  python3 - "$work/deliveries.json" "$index" "$tamper" "$work/replay.body" "$work/replay.headers" <<'EOF'
import base64, json, sys
with open(sys.argv[1]) as handle:
    delivery = json.load(handle)[int(sys.argv[2])]
body = base64.b64decode(delivery["body"])
if sys.argv[3] == "tamper":
    body = body + b" "
with open(sys.argv[4], "wb") as handle:
    handle.write(body)
with open(sys.argv[5], "w") as handle:
    handle.write("Content-Type: application/json\n")
    handle.write(f"X-Keel-Event-Id: {delivery['eventId']}\n")
    handle.write(f"X-Keel-Peer: {delivery['peer']}\n")
    handle.write(f"X-Keel-Signature: {delivery['signature']}\n")
EOF
  status=$(curl --cacert "$panel/lib/tls/cert.pem" -s -o "$out" -w '%{http_code}' -X POST \
    -H @"$work/replay.headers" --data-binary @"$work/replay.body" "https://127.0.0.1:$panel_port$hook_path") || true
  if [ "$status" != "$expected" ]; then
    echo "  the replayed delivery answered $status, expected $expected" >&2
    cat "$out" >&2
    return 1
  fi
}

dns_http() {
  local out=$1 headers=$2 path=$3 body=$4
  shift 4
  local args=(--cacert "$dns/lib/tls/cert.pem" -s -o "$out" -D "$headers" -w '%{http_code}'
    -H 'Content-Type: application/json' -H @"$work/billing.header")
  local extra
  for extra in "$@"; do
    args+=(-H "$extra")
  done
  curl "${args[@]}" --data "$body" "https://127.0.0.1:$dns_port$path"
}

http_answered() {
  local expected=$1 out=$2 headers=$3 path=$4 body=$5
  shift 5
  local status
  status=$(dns_http "$out" "$headers" "$path" "$body" "$@") || true
  if [ "$status" != "$expected" ]; then
    echo "  POST $path answered $status, expected $expected" >&2
    cat "$out" >&2
    return 1
  fi
}

replayed() {
  grep -qi '^x-keel-idempotent-replay:' "$1"
}

not_replayed() {
  [ -s "$1" ] && ! replayed "$1"
}

if [ -z "$admin_url" ]; then
  usage
  echo "KEEL_TEST_DATABASE_URL is not set" >&2
  exit 2
fi
for tool in psql curl python3 pnpm node; do
  if ! command -v "$tool" >/dev/null; then
    echo "$tool is not installed" >&2
    exit 2
  fi
done

if [ -z "$keel" ]; then
  cargo build -p keel-bin
  keel="${CARGO_TARGET_DIR:-target}/debug/keel"
else
  shift
fi
case "$keel" in /*) ;; *) keel="$PWD/$keel" ;; esac
if [ ! -x "$keel" ]; then
  fail "$keel is not an executable binary"
fi

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

tmp=${TMPDIR:-/tmp}
work=$(mktemp -d "${tmp%/}/keel-peers-e2e.XXXXXX")
panel="$work/panel"
dns="$work/dns"
stamp="$(date +%s)_$$"
panel_database="keel_test_peers_panel_$stamp"
dns_database="keel_test_peers_dns_$stamp"
panel_port=$(free_port)
dns_port=$(free_port)
to_dns_port=$(free_port)
to_panel_port=$(free_port)
for name in "$panel_database" "$dns_database"; do
  psql "$admin_url" -qAt -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$name\" TEMPLATE template1" >/dev/null
  echo "created database $name"
done
mkdir -p "$panel/run" "$panel/lib" "$dns/run" "$dns/lib"
cat >"$panel/config.yaml" <<EOF
listen: 127.0.0.1:$panel_port
socket: $panel/run/core.sock
data_dir: $panel/lib
database: $(database_url "$admin_url" "$panel_database")
log: { level: info, format: text }
host: { socket: $panel/agent/agent.sock }
hosting: { transfer_dir: $panel/agent/transfer }
EOF
cat >"$dns/config.yaml" <<EOF
listen: 127.0.0.1:$dns_port
socket: $dns/run/core.sock
data_dir: $dns/lib
database: $(database_url "$admin_url" "$dns_database")
log: { level: info, format: text }
host: { socket: $dns/agent/agent.sock }
dns: { zones_dir: $dns/agent/zones, include_dir: $dns/agent/named }
EOF
echo "work $work, binary $keel"
start_agent "$keel" "$dns/agent" || fail "keel privileged did not start beside the dns installation"
dns_agent=$agent_pid
start_agent "$keel" "$panel/agent" "$hosting_range" || fail "keel privileged did not start beside the panel installation"
panel_agent=$agent_pid

echo "--- two installations, dns alone and panel alone"
check "the dns installation enables dns alone" dns_ok "$dns/enable.out" product enable dns
check "the panel installation enables panel alone" panel_ok "$panel/enable.out" product enable panel
start_dns
start_panel
check "the handshake of the dns installation names dns alone and its instance key" handshake_names "$dns/lib/tls/cert.pem" "$dns_port" dns "$dns/instance.json"
check "the handshake of the panel installation names panel alone and its instance key" handshake_names "$panel/lib/tls/cert.pem" "$panel_port" panel "$panel/instance.json"
dns_instance=$(json_get "$dns/instance.json" 'doc["instanceId"]')
panel_instance=$(json_get "$panel/instance.json" 'doc["instanceId"]')
start_relays
dns_url="https://localhost:$dns_port"
check "the dns installation takes its public address $dns_url" dns_ok "$dns/shell-url.json" ctl settings.set key=auth.shell_url "value=$dns_url"
check "the panel installation takes the relay to it as its public address, which pairing sends the dns installation" panel_ok "$panel/shell-url.json" ctl settings.set key=auth.shell_url "value=http://127.0.0.1:$to_panel_port"

admin_password=$(secret)
reseller_password=$(secret)
user_password=$(secret)
dns_admin_password=$(secret)
check "the panel installation gets its administrator $admin_login" create_principal panel "$panel/admin.json" "$admin_login" admin "$root_account" "$admin_password"
panel_ok "$panel/reseller-account.json" ctl keel.account.create kind=reseller --out json
reseller_account=$(json_get "$panel/reseller-account.json" 'doc["data"]["id"]')
panel_ok "$panel/user-account.json" ctl keel.account.create kind=user "parent=$reseller_account" --out json
user_account=$(json_get "$panel/user-account.json" 'doc["data"]["id"]')
check "the panel installation gets the reseller $reseller_login" create_principal panel "$panel/reseller.json" "$reseller_login" reseller "$reseller_account" "$reseller_password"
check "the panel installation gets the customer $user_login in $user_account" create_principal panel "$panel/user.json" "$user_login" user "$user_account" "$user_password"
check "ctl hosting.user.create --as $user_login creates the hosting user $hosting_user" panel_ok "$panel/hosting-user.json" ctl hosting.user.create --as "$user_login" "name=$hosting_user" --out json
hosting_user_id=$(json_get "$panel/hosting-user.json" 'doc["data"]["id"]')
check "the hosting user $hosting_user turns active" eventually hosting_user_active "$hosting_user_id"
check "the dns installation gets the administrator $admin_login a billing installation would call as" create_principal dns "$dns/admin.json" "$admin_login" admin "$root_account" "$dns_admin_password"
check "ctl dns.zone.create on the dns installation creates $taken_zone before the pairing" dns_ok "$dns/taken.json" ctl dns.zone.create "name=$taken_zone" --out json

echo "--- pairing by code over the CLI"
check "ctl peer.pair_code on the dns installation grants the zones, the customer and the handover" dns_ok "$dns/code.json" ctl peer.pair_code "scopes=$peer_scopes" level=reseller onBehalf=true --out json
pair_code=$(json_get "$dns/code.json" 'doc["data"]["code"]')
check "the pair code reads PAIR-XXXX-XXXX" grep -qE '^PAIR-[0-9A-Z]{4}-[0-9A-Z]{4}$' <<<"$pair_code"
check "ctl peer.create on the panel installation pairs it with the dns installation by the code" panel_ok "$panel/peer.json" ctl peer.create "url=http://127.0.0.1:$to_dns_port" "code=$pair_code" "subscribe=$peer_events" --out json
check "peer.create answers the link to the dns installation with the scopes it was granted" json_is "$panel/peer.json" "doc['data']['instanceId'] == '$dns_instance' and doc['data']['products'] == ['dns'] and doc['data']['status'] == 'active' and doc['data']['granted'] == {'functions': $peer_scopes, 'level': 'reseller', 'onBehalf': True}"
check "the dns installation lists one active link to the panel installation at the relay" dns_ok "$dns/peers.json" ctl peer.list --out json
check "the link of the dns installation names the panel installation and its address" json_is "$dns/peers.json" "[(row['instanceId'], row['products'], row['status'], row['url']) for row in doc['data']['data']] == [('$panel_instance', ['panel'], 'active', 'http://127.0.0.1:$to_panel_port')]"
link=$(json_get "$dns/peers.json" 'doc["data"]["data"][0]["id"]')
link_principal=$(dns_sql "SELECT principal_id FROM keel.peer_product WHERE id = '$link'")
link_login="peer-$(lower "$link")"
customer_login=$(lower "panel-$panel_instance-$user_account")
check "the dns installation subscribed the hook of the panel installation to the events it asked for" sql_is dns "http://127.0.0.1:$to_panel_port$hook_path {dns.zone.*,svc.*}" "SELECT url || ' ' || events::text FROM keel.webhook_subscription WHERE peer_id = '$link'"
check "the code is spent" sql_is dns 0 "SELECT count(*) FROM keel.peer_pair_code"

echo "--- a site on the panel installation and its zone on the dns installation"
check "ctl hosting.site.create --as $user_login creates $peer_site with a zone" panel_ok "$panel/site.json" ctl hosting.site.create --as "$user_login" "user=$hosting_user_id" "domain=$peer_site" dnsZone=true --out json
site_id=$(json_get "$panel/site.json" 'doc["data"]["id"]')
check "the zone $peer_site lands on the dns installation, active with its zone file and include" eventually zone_on_dns
check "the zone belongs to the account of the customer the panel installation named by its origin" sql_is dns "panel:$panel_instance $user_account" "SELECT a.origin || ' ' || a.external_id FROM keel.dns_zone z JOIN keel.keel_account a ON a.id = z.account_id WHERE z.name = '$peer_site'"
check "the dns installation audits the zone as created by the customer with the link as actor" sql_is dns "$customer_login $link_login peer:$link committed" "SELECT string_agg(principal_login || ' ' || coalesce(actor_login, '-') || ' ' || via || ' ' || status, ',') FROM keel.audit_call WHERE function = 'dns.zone.create' AND principal_login = '$customer_login'"
check "the site carries no zone error" sql_is panel "" "SELECT coalesce(zone_error, '') FROM keel.hosting_site WHERE id = '$site_id'"
zone_account=$(dns_sql "SELECT account_id FROM keel.dns_zone WHERE name = '$peer_site'")

echo "--- the event of the zone arrives signed, and its repeat creates no duplicate"
check "the first delivery to the panel installation carries the zone's event, signed by the dns installation" eventually deliveries_are "len(doc) >= 1 and doc[0]['peer'] == '$dns_instance' and doc[0]['t'] > 0 and len(doc[0]['v1'] or '') == 64 and (doc[0]['event']['type'], doc[0]['event']['data']['name'], doc[0]['event']['id'], doc[0]['event']['instanceId']) == ('dns.zone.created', '$peer_site', doc[0]['eventId'], '$dns_instance')"
check "the panel installation took the first delivery, and the relay answered the dns installation 500 in its place" deliveries_are "(doc[0]['status'], doc[0]['answer']['data'], doc[0]['answered']) == (200, {'eventId': doc[0]['eventId'], 'duplicate': False}, 500)"
event=$(json_get "$work/deliveries.json" 'doc[0]["eventId"]')
check "the dns installation reads the delivery whose answer the relay dropped as failed at attempt 1 with 500" eventually dns_delivery_is "$event" "[(row['state'], row['attempt'], row['lastStatus']) for row in doc['data']['data']] == [('failed', 1, 500)]"
delivery=$(json_get "$dns/delivery.json" 'doc["data"]["data"][0]["id"]')
check "the panel installation holds the event once in its inbox and once on its bus under the dns instance" eventually panel_received_once "$event" dns.zone.created
sleep 1.1
check "ctl webhook.delivery.retry on the dns installation sends the delivery again" dns_ok "$dns/retry.json" ctl webhook.delivery.retry "id=$delivery" --out json
check "the dns installation reads the delivery as delivered at attempt 2" eventually dns_delivery_is "$event" "[(row['state'], row['attempt'], row['lastStatus']) for row in doc['data']['data']] == [('delivered', 2, 200)]"
check "the repeat carried the same event under a later signature and the panel installation answered it as a duplicate" deliveries_are "len(doc) >= 2 and doc[1]['eventId'] == '$event' and doc[1]['body'] == doc[0]['body'] and doc[1]['t'] > doc[0]['t'] and doc[1]['v1'] != doc[0]['v1'] and (doc[1]['status'], doc[1]['answer']['data'], doc[1]['answered']) == (200, {'eventId': '$event', 'duplicate': True}, 200)"
check "the panel installation still holds the event once in its inbox and once on its bus" panel_received_once "$event" dns.zone.created
check "a copy of the delivery whose body differs by a space is refused as unsigned" replay_delivery 0 tamper "$work/tampered.json" 401
check "the refusal names the signature" json_is "$work/tampered.json" "doc['ok'] is False and doc['error']['key'] == 'peer.error.signature'"
check "the copy of the delivery replayed byte for byte is answered as a duplicate" replay_delivery 0 exact "$work/replayed.json" 200
check "the replay answers the event as a duplicate" json_is "$work/replayed.json" "doc['data'] == {'eventId': '$event', 'duplicate': True}"
check "the panel installation holds the event once after the copies" panel_received_once "$event" dns.zone.created

echo "--- a DNS error reaches the hosting form as a key on a field"
check "ctl hosting.site.create --as $user_login of $taken_zone with a zone is refused" panel_refused "$panel/taken-site.json" ctl hosting.site.create --as "$user_login" "user=$hosting_user_id" "domain=$taken_zone" dnsZone=true --out json
check "the refusal is peer with the key of dns-peer on the field domain" json_is "$panel/taken-site.json" "doc['error']['code'] == 'peer' and [(field['path'], field['key'], field['code'], field['params']['name']) for field in doc['error']['fields']] == [('domain', 'dns_peer.error.exists', 'dns.zone.error.exists', '$taken_zone')]"
check "no site $taken_zone was committed on the panel installation" sql_is panel 0 "SELECT count(*) FROM keel.hosting_site WHERE domain = '$taken_zone'"

echo "--- pnpm e2e $peers_tag"
spkis=$(node -e '
const { readFileSync } = require("node:fs");
const { X509Certificate, createHash } = require("node:crypto");
for (const file of process.argv.slice(1)) {
  const spki = new X509Certificate(readFileSync(file)).publicKey.export({ type: "spki", format: "der" });
  console.log(createHash("sha256").update(spki).digest("base64"));
}
' "$panel/lib/tls/cert.pem" "$dns/lib/tls/cert.pem" | paste -sd, -)
echo "the browser trusts the certificates with the SPKI $spkis"
(
  cd "$frontend"
  env "KEEL_E2E_URL=https://127.0.0.1:$panel_port" "KEEL_E2E_SPKI=$spkis" "NODE_EXTRA_CA_CERTS=$panel/lib/tls/cert.pem" \
    "KEEL_E2E_ROOT_PASSWORD=$admin_password" "KEEL_E2E_RES1_PASSWORD=$reseller_password" "KEEL_E2E_ALICE_PASSWORD=$user_password" \
    "KEEL_E2E_HOSTING_USER=hp_$hosting_user" "KEEL_E2E_PEER_URL=$dns_url" "KEEL_E2E_PEER_SITE=$peer_site" \
    "KEEL_E2E_PEER_TAKEN=$taken_zone" "KEEL_E2E_PEER_LOGIN=$customer_login" pnpm e2e --grep "$peers_tag" "$@"
)
check "the browser left no site $taken_zone on the panel installation" sql_is panel 0 "SELECT count(*) FROM keel.hosting_site WHERE domain = '$taken_zone'"
check "the dns installation audits the one-time key as the customer with the link as actor" eventually sql_is dns "$customer_login $link_login peer:$link committed" "SELECT string_agg(principal_login || ' ' || coalesce(actor_login, '-') || ' ' || via || ' ' || status, ',') FROM keel.audit_call WHERE function = 'auth.key.handover'"
check "the key was issued by the principal of the link and is spent" sql_is dns "$link_principal true" "SELECT string_agg(issued_by || ' ' || (used_at IS NOT NULL)::text, ',') FROM keel.auth_login_key"

echo "--- svc.dns.zones.open twice with one key"
check "ctl auth.token.create --as $admin_login gives the dns installation's administrator an API token" dns_ok "$dns/token.json" ctl auth.token.create --as "$admin_login" name=billing --out json
install -m 0600 /dev/null "$work/billing.header"
json_get "$dns/token.json" '"Authorization: Bearer " + doc["data"]["token"]' >"$work/billing.header"
billing_token_id=$(json_get "$dns/token.json" 'doc["data"]["id"]')
open_body="{\"externalId\":\"$service\",\"account\":{\"origin\":\"panel:$panel_instance\",\"externalId\":\"$user_account\"},\"params\":{\"zoneLimit\":$zone_limit}}"
check "svc.dns.zones.open with the Idempotency-Key $service_key opens the service" http_answered 200 "$work/open.json" "$work/open.headers" /api/v1/call/svc.dns.zones.open "$open_body" "Idempotency-Key: $service_key"
check "the service is active in the account of the customer and counts the zone the site created" json_is "$work/open.json" "doc['ok'] is True and doc['data']['status'] == 'active' and doc['data']['accountId'] == '$zone_account' and doc['data']['state'] == {'zonesUsed': 1}"
check "the first answer is not a replay" not_replayed "$work/open.headers"
check "svc.dns.zones.open again with the same key and body answers" http_answered 200 "$work/open-again.json" "$work/open-again.headers" /api/v1/call/svc.dns.zones.open "$open_body" "Idempotency-Key: $service_key"
check "the second answer is the first answer replayed, with the same service" json_is "$work/open-again.json" "doc['ok'] is True and doc['data'] == __import__('json').load(open('$work/open.json'))['data']"
check "the second answer carries X-Keel-Idempotent-Replay" replayed "$work/open-again.headers"
check "svc.dns.zones.open of the same external id under another key answers svc.error.exists" http_answered 409 "$work/open-other.json" "$work/open-other.headers" /api/v1/call/svc.dns.zones.open "$open_body" "Idempotency-Key: $service_key-other"
check "the conflict names svc.error.exists" json_is "$work/open-other.json" "doc['ok'] is False and doc['error']['key'] == 'svc.error.exists'"
check "the dns installation holds one service of that external id, in the customer's account" sql_is dns "1 $zone_account" "SELECT count(*) || ' ' || max(account_id) FROM keel.svc_service WHERE kind = 'dns.zones' AND external_id = '$service'"
check "the dns installation emitted svc.service.opened once" sql_is dns 1 "SELECT count(*) FROM keel.keel_outbox WHERE type = 'svc.service.opened'"
check "the account of the customer took the zone limit of the service" sql_is dns "$zone_limit" "SELECT value FROM keel.keel_limit WHERE account_id = '$zone_account' AND key = 'dns.zones'"
opened=$(dns_sql "SELECT id FROM keel.keel_outbox WHERE type = 'svc.service.opened'")
check "the panel installation receives svc.service.opened once, signed through the same hook" eventually panel_received_once "$opened" svc.service.opened

echo "--- revoking a token breaks its calls with 401"
check "the API token of the dns installation's administrator calls svc.kinds" http_answered 200 "$work/kinds.json" "$work/kinds.headers" /api/v1/call/svc.kinds '{}'
check "ctl auth.token.revoke --as $admin_login revokes the API token" dns_ok "$dns/revoke.json" ctl auth.token.revoke --as "$admin_login" "id=$billing_token_id" --out json
check "the revoked API token answers 401" http_answered 401 "$work/revoked.json" "$work/revoked.headers" /api/v1/call/svc.kinds '{}'
check "the refusal is unauthenticated" json_is "$work/revoked.json" "doc['ok'] is False and doc['error']['code'] == 'unauthenticated' and doc['error']['key'] == 'keel.error.unauthenticated'"
mark=$(wc -l <"$work/to-dns.jsonl" | tr -d ' ')
check "ctl peer.delete on the dns installation revokes the link of the panel installation" dns_ok "$dns/peer-delete.json" ctl peer.delete "id=$link" --out json
check "opening the zone of $peer_site from the panel installation is refused" panel_refused "$panel/open-revoked.json" ctl hosting.site.open_zone --as "$user_login" "id=$site_id" --out json
check "the refusal relays the 401 of the dns installation as peer" json_is "$panel/open-revoked.json" "doc['error']['code'] == 'peer' and doc['error']['key'] == 'keel.error.unauthenticated'"
check "the dns installation admitted every call of the peer token before the revocation and answered the revoked one 401" peer_token_answers "$mark" "len(before) > 0 and 401 not in before and after == [401]"

stop_panel || fail "the panel installation did not stop cleanly"
stop_dns || fail "the dns installation did not stop cleanly"
