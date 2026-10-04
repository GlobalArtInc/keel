#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
. hack/agent.sh

timeout=${KEEL_PROVE_TIMEOUT:-30}
keel=${1:-}
admin_url=${KEEL_TEST_DATABASE_URL:-}
scheme=https
cacert=""
smtp_url=${KEEL_TEST_SMTP_URL:-}
smtp_api=${KEEL_TEST_SMTP_API:-}
frontend=frontend
database=""
site=""
serve_pid=""
serve_log=""
catcher=""
runtime=""
catcher_image=axllent/mailpit:v1.31.1
admin_login=root
reseller_login=res1
user_login=alice
polled_tag=@polled
setup_tag=@setup
mail_tag=@mail
panel_tag=@panel
hosting_user=files
config_domain=config.example.org
config_directive="client_max_body_size 64m;"
wizard_domain=wizard.e2e.test
hosting_range="{ from: 100000, to: 100009 }"
reset_login=bob
reset_address=bob@example.org
guest_per_minute=120
setup_link=""
dns_zone=cli.e2e.test
dns_zones=(browser.e2e.test "$dns_zone" token.e2e.test)
peers_tag=@peers
peer_site=peer.e2e.test
peer_scopes='["dns.zone.*","keel.principal.upsert","auth.key.handover"]'
neighbour=""
neighbour_database=""
neighbour_port=""
neighbour_pid=""
neighbour_log=""
relay_pid=""
relay_port=""

usage() {
  echo "usage: hack/e2e.sh [path/to/keel] [playwright arguments...]" >&2
  echo "  KEEL_TEST_DATABASE_URL must point at a Postgres superuser connection;" >&2
  echo "  KEEL_TEST_SMTP_URL and KEEL_TEST_SMTP_API name a running mail catcher;" >&2
  echo "  without both the script starts the $catcher_image container through" >&2
  echo "  KEEL_CONTAINER_RUNTIME, podman when it is on the path and docker otherwise;" >&2
  echo "  a binary that is not given is built with cargo build;" >&2
  echo "  the arguments after the binary are passed to pnpm e2e" >&2
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
  if [ -n "$relay_pid" ]; then
    kill -TERM "$relay_pid" 2>/dev/null || true
    wait "$relay_pid" 2>/dev/null || true
    relay_pid=""
    echo "stopped the relay to the neighbour"
  fi
  if [ -n "$neighbour_pid" ]; then
    echo "stopping the neighbour ($neighbour_pid)"
    if ! stop_neighbour; then
      code=1
    fi
  fi
  if ! stop_agent; then
    code=1
  fi
  for name in "$database" "$neighbour_database"; do
    [ -n "$name" ] || continue
    if psql "$admin_url" -qAt -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS \"$name\" WITH (FORCE)" >/dev/null; then
      echo "dropped database $name"
    else
      echo "could not drop database $name" >&2
      code=1
    fi
  done
  if [ -n "$catcher" ]; then
    if "$runtime" rm -f "$catcher" >/dev/null 2>&1; then
      echo "removed the mail catcher $catcher"
    else
      echo "could not remove the mail catcher $catcher" >&2
      code=1
    fi
  fi
  if [ -n "$site" ] && [ -d "$site" ]; then
    rm -rf "$site"
    echo "removed $site"
  fi
  if [ "$code" -eq 0 ]; then
    echo "the browser suite is green"
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
    print("  " + json.dumps(doc)[:400], file=sys.stderr)
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

start_catcher() {
  local smtp_port api_port waited
  smtp_port=$(free_port)
  api_port=$(free_port)
  catcher="keel-e2e-mailpit-$$"
  if ! "$runtime" run -d --rm --name "$catcher" -p "$smtp_port:1025" -p "$api_port:8025" "$catcher_image" >/dev/null; then
    catcher=""
    fail "$runtime could not start the $catcher_image container"
  fi
  smtp_url="smtp://127.0.0.1:$smtp_port"
  smtp_api="http://127.0.0.1:$api_port"
  waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if curl -sf "$smtp_api/readyz" >/dev/null 2>&1; then
      echo "the mail catcher $catcher is ready at $smtp_api"
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  fail "the mail catcher did not answer $smtp_api/readyz within ${timeout}s"
}

mail_arrives() {
  local out=$1 needle=$2 waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if curl -sf "$smtp_api/api/v1/messages?limit=200" >"$out" 2>/dev/null \
      && json_is "$out" "any('$needle' in message['Subject'] for message in doc['messages'])" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  echo "  no message whose subject names $needle within ${timeout}s" >&2
  return 1
}

problem_listed() {
  local waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if keel_ok "$site/problems.json" ctl notify.problem.list --out json \
      && json_is "$site/problems.json" "doc['ok'] is True and [(row['key'], row['subject'], row['params']['count'], row['open']) for row in doc['data']['data']] == [('note.overdue', '$user_account', 1, True)]" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  cat "$site/problems.json" >&2
  return 1
}

zones_served() {
  local waited=0 zone
  local expected
  expected=$(printf "'%s', " "${dns_zones[@]}")
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if keel_ok "$site/zones.json" ctl dns.zone.list --as "$user_login" --out json \
      && json_is "$site/zones.json" "doc['ok'] is True and [(row['name'], row['state'], row['served']) for row in doc['data']['data']] == [(name, 'active', True) for name in ($expected)]" 2>/dev/null; then
      for zone in "${dns_zones[@]}"; do
        if [ ! -f "$site/agent/zones/keel-dns-$zone.zone" ] || ! grep -qF "keel-dns-$zone.conf" "$site/agent/named/keel-dns-zones.conf"; then
          echo "  the agent did not write the zone file and the include of $zone" >&2
          return 1
        fi
      done
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  cat "$site/zones.json" >&2
  return 1
}

deliveries_sent() {
  local count=$1 waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if keel_ok "$site/deliveries.json" ctl notify.delivery.list --out json \
      && json_is "$site/deliveries.json" "doc['ok'] is True and doc['data']['total'] == $count and all((row['channel'], row['messageKey'], row['state'], row['attempt']) == ('catcher', 'notify.problem.raised', 'sent', 1) for row in doc['data']['data'])" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  cat "$site/deliveries.json" >&2
  return 1
}

hosting_user_active() {
  local id=$1 waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if keel_ok "$site/hosting-user-state.json" ctl hosting.user.get "id=$id" --out json \
      && json_is "$site/hosting-user-state.json" "doc['ok'] is True and doc['data']['state'] == 'active'" 2>/dev/null; then
      test -d "$site/agent/www/keel-panel-$hosting_user"
      return
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  cat "$site/hosting-user-state.json" >&2
  return 1
}

site_active() {
  local id=$1 domain=$2 waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if keel_ok "$site/site-state.json" ctl hosting.site.get "id=$id" --out json \
      && json_is "$site/site-state.json" "doc['ok'] is True and doc['data']['state'] == 'active'" 2>/dev/null; then
      test -f "$site/agent/vhosts/keel-panel-$domain.conf"
      return
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  cat "$site/site-state.json" >&2
  return 1
}

wizard_site_with_zone() {
  local waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if keel_ok "$site/wizard-sites.json" ctl hosting.site.list --as "$user_login" "search=$wizard_domain" --out json \
      && keel_ok "$site/wizard-zones.json" ctl dns.zone.list --as "$user_login" "search=$wizard_domain" --out json \
      && json_is "$site/wizard-sites.json" "[(row['domain'], row['state']) for row in doc['data']['data']] == [('$wizard_domain', 'active')]" 2>/dev/null \
      && json_is "$site/wizard-zones.json" "[(row['name'], row['state'], row['served']) for row in doc['data']['data']] == [('$wizard_domain', 'active', True)]" 2>/dev/null; then
      test "$(sql "SELECT account_id FROM keel.hosting_site WHERE domain = '$wizard_domain'")" = "$user_account" \
        && test "$(sql "SELECT account_id FROM keel.dns_zone WHERE name = '$wizard_domain'")" = "$user_account" \
        && test -f "$site/agent/vhosts/keel-panel-$wizard_domain.conf" \
        && test -f "$site/agent/zones/keel-dns-$wizard_domain.zone" \
        && grep -qF "keel-dns-$wizard_domain.conf" "$site/agent/named/keel-dns-zones.conf"
      return
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  cat "$site/wizard-sites.json" "$site/wizard-zones.json" >&2
  return 1
}

files_ran_as() {
  local uid=$1 calls="$site/agent/keel.calls"
  [ -n "$uid" ] && [ -s "$calls" ] || return 1
  grep -q " agent-home --home .* write " "$calls" && grep -q " agent-home --home .* read " "$calls" \
    && ! grep -qv "^agent-exec --uid $uid " "$calls"
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

neighbour_ready() {
  local certificate="$neighbour/lib/tls/cert.pem"
  [ -f "$certificate" ] \
    && [ "$(curl --cacert "$certificate" -s -o /dev/null -w '%{http_code}' "https://127.0.0.1:$neighbour_port/readyz" 2>/dev/null)" = "200" ]
}

relay_ready() {
  [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$relay_port/readyz" 2>/dev/null)" = "200" ]
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
    [ -n "$log" ] && sed 's/^/  /' "$log" >&2
    return 1
  fi
}

spki_of() {
  node -e '
const { readFileSync } = require("node:fs");
const { X509Certificate, createHash } = require("node:crypto");
const certificate = new X509Certificate(readFileSync(process.argv[1]));
const spki = certificate.publicKey.export({ type: "spki", format: "der" });
process.stdout.write(createHash("sha256").update(spki).digest("base64"));
' "$1"
}

start_serve() {
  serve_log="$site/serve.log"
  "$keel" --config "$site/config.yaml" serve >"$serve_log" 2>&1 &
  serve_pid=$!
  await_ready "$serve_pid" "$serve_log" "keel serve" ready
  echo "keel serve is ready on $scheme://127.0.0.1:$port"
}

stop_serve() {
  if [ -z "$serve_pid" ]; then
    return 0
  fi
  local pid=$serve_pid
  serve_pid=""
  stop_process "$pid" "$serve_log" "keel serve"
}

start_neighbour() {
  neighbour_log="$neighbour/serve.log"
  "$keel" --config "$neighbour/config.yaml" serve >"$neighbour_log" 2>&1 &
  neighbour_pid=$!
  await_ready "$neighbour_pid" "$neighbour_log" "the neighbour" neighbour_ready
  echo "the neighbour is ready on https://127.0.0.1:$neighbour_port"
}

stop_neighbour() {
  if [ -z "$neighbour_pid" ]; then
    return 0
  fi
  local pid=$neighbour_pid
  neighbour_pid=""
  stop_process "$pid" "$neighbour_log" "the neighbour"
}

start_relay() {
  relay_port=$(free_port)
  node -e '
const http = require("node:http");
const https = require("node:https");
const { readFileSync } = require("node:fs");
const [listen, target, certificate] = process.argv.slice(1);
const ca = readFileSync(certificate);
http
  .createServer((request, response) => {
    const headers = { ...request.headers, host: `127.0.0.1:${target}` };
    const upstream = https.request(
      { host: "127.0.0.1", port: Number(target), path: request.url, method: request.method, headers, ca },
      (answer) => {
        response.writeHead(answer.statusCode ?? 502, answer.headers);
        answer.pipe(response);
      },
    );
    upstream.on("error", (error) => {
      response.writeHead(502, { "content-type": "text/plain" });
      response.end(String(error));
    });
    request.pipe(upstream);
  })
  .listen(Number(listen), "127.0.0.1");
' "$relay_port" "$neighbour_port" "$neighbour/lib/tls/cert.pem" >"$neighbour/relay.log" 2>&1 &
  relay_pid=$!
  await_ready "$relay_pid" "$neighbour/relay.log" "the relay to the neighbour" relay_ready
  echo "the relay to the neighbour listens on http://127.0.0.1:$relay_port"
}

run_neighbour() {
  "$keel" --config "$neighbour/config.yaml" "$@"
}

neighbour_ok() {
  local out=$1
  shift
  if ! run_neighbour "$@" >"$out" 2>"$neighbour/stderr"; then
    cat "$neighbour/stderr" >&2
    return 1
  fi
}

neighbour_sql() {
  psql "$(database_url "$admin_url" "$neighbour_database")" -qAt -v ON_ERROR_STOP=1 -c "$1"
}

peer_zone_served() {
  local waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if neighbour_ok "$neighbour/zones.json" ctl dns.zone.list "search=$peer_site" --out json \
      && json_is "$neighbour/zones.json" "doc['ok'] is True and [row['name'] for row in doc['data']['data']] == ['$peer_site']" 2>/dev/null; then
      test "$(neighbour_sql "SELECT a.origin || ' ' || a.external_id FROM keel.dns_zone z JOIN keel.keel_account a ON a.id = z.account_id WHERE z.name = '$peer_site'")" = "panel:$instance $user_account"
      return
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  cat "$neighbour/zones.json" >&2
  return 1
}

handover_audited() {
  local expected=$1 waited=0
  while [ "$waited" -lt "$((timeout * 10))" ]; do
    if [ "$(neighbour_sql "SELECT principal_login || ' ' || coalesce(actor_login, '-') || ' ' || via || ' ' || status FROM keel.audit_call WHERE function = 'auth.key.handover'")" = "$expected" ]; then
      return 0
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  neighbour_sql "SELECT function, principal_login, actor_login, via, status FROM keel.audit_call WHERE function LIKE 'auth.key.%'" >&2
  return 1
}

http_status_is() {
  local expected=$1 method=$2 path=$3 body=${4:-}
  local status
  status=$(curl_tls -s -o "$site/http.json" -w '%{http_code}' -X "$method" -H 'Content-Type: application/json' --data "$body" "$scheme://127.0.0.1:$port$path" 2>/dev/null) || true
  if [ "$status" != "$expected" ]; then
    echo "  $method $path answered $status, expected $expected" >&2
    cat "$site/http.json" >&2
    return 1
  fi
}

sql() {
  psql "$(database_url "$admin_url" "$database")" -qAt -v ON_ERROR_STOP=1 -c "$1"
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
for tool in psql curl python3 pnpm node; do
  if ! command -v "$tool" >/dev/null; then
    echo "$tool is not installed" >&2
    exit 2
  fi
done
if [ -z "$smtp_url" ] || [ -z "$smtp_api" ]; then
  runtime=${KEEL_CONTAINER_RUNTIME:-}
  if [ -z "$runtime" ]; then
    if command -v podman >/dev/null; then
      runtime=podman
    else
      runtime=docker
    fi
  fi
  if ! command -v "$runtime" >/dev/null; then
    echo "KEEL_TEST_SMTP_URL and KEEL_TEST_SMTP_API are not set and the container runtime $runtime is not installed" >&2
    exit 2
  fi
fi

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
site=$(mktemp -d "${tmp%/}/keel-e2e.XXXXXX")
database="keel_test_e2e_$(date +%s)_$$"
port=$(free_port)
psql "$admin_url" -qAt -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$database\" TEMPLATE template1" >/dev/null
echo "created database $database"
mkdir -p "$site/run" "$site/lib"
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
echo "site $site, binary $keel"
start_agent "$keel" "$site/agent" "$hosting_range" || fail "keel privileged did not start beside the kernel"

if [ -n "$runtime" ]; then
  start_catcher
else
  echo "using the mail catcher at $smtp_api"
fi
smtp_host=${smtp_url#smtp://}
smtp_host=${smtp_host%%/*}
smtp_port=${smtp_host##*:}
smtp_host=${smtp_host%:*}

keel_ok "$site/enable.out" product enable demo
keel_ok "$site/enable-dns.out" product enable dns
start_serve
keel_ok "$site/rate.json" ctl settings.set key=auth.rate.guest_per_minute value=$guest_per_minute
keel_ok "$site/shell-url.json" ctl settings.set key=auth.shell_url "value=$scheme://127.0.0.1:$port"

admin_password=$(secret)
reseller_password=$(secret)
user_password=$(secret)
reset_password=$(secret)
e2e_env=(
  "KEEL_E2E_URL=$scheme://127.0.0.1:$port"
  "KEEL_E2E_ROOT_PASSWORD=$admin_password"
  "KEEL_E2E_RES1_PASSWORD=$reseller_password"
  "KEEL_E2E_ALICE_PASSWORD=$user_password"
)
if [ -n "$cacert" ]; then
  spki=$(spki_of "$cacert")
  echo "the browser suite trusts the certificate with the SPKI $spki"
  e2e_env+=("KEEL_E2E_SPKI=$spki" "NODE_EXTRA_CA_CERTS=$cacert")
fi

echo "--- the setup link"
setup_link=$(grep -oE "https://127\.0\.0\.1:$port/setup\?key=kok_[0-9a-f]+" "$serve_log" | head -n 1) || true
check "the start without an administrator logs the setup link" test -n "$setup_link"
check "the setup key is thirty-two random bytes as hex" grep -qE '^https://.*key=kok_[0-9a-f]{64}$' <<<"$setup_link"
echo "--- pnpm e2e $setup_tag"
(
  cd "$frontend"
  env "${e2e_env[@]}" "KEEL_E2E_SETUP_URL=$setup_link" pnpm e2e --grep "$setup_tag"
)
check "ctl keel.principal.list finds the administrator the link created" keel_ok "$site/setup-admin.json" ctl keel.principal.list "search=$admin_login" --out json
check "the administrator is an admin of the root account" json_is "$site/setup-admin.json" "doc['ok'] is True and [(row['login'], row['level'], row['accountId']) for row in doc['data']['data']] == [('$admin_login', 'admin', '00000000000000000000000000')]"
check "the setup function is gone after the first administrator" http_status_is 404 POST "/api/v1/call/keel.setup.admin" '{"key":"kok_used","login":"second","password":"correct-horse-battery"}'

keel_ok "$site/reseller-account.json" ctl keel.account.create kind=reseller
reseller_account=$(json_get "$site/reseller-account.json" 'doc["data"]["id"]')
keel_ok "$site/user-account.json" ctl keel.account.create kind=user "parent=$reseller_account"
user_account=$(json_get "$site/user-account.json" 'doc["data"]["id"]')
create_principal "$site/reseller.json" "$reseller_login" reseller "$reseller_account" "$reseller_password"
create_principal "$site/user.json" "$user_login" user "$user_account" "$user_password"
echo "created the admin $admin_login through the setup link, the reseller $reseller_login in $reseller_account and the user $user_login in $user_account"
check "ctl dns.zone.create --as alice creates the zone $dns_zone" keel_ok "$site/dns-zone.json" ctl dns.zone.create --as "$user_login" "name=$dns_zone"
check "dns.zone.create answers the zone in alice's account" json_is "$site/dns-zone.json" "doc['ok'] is True and doc['data']['name'] == '$dns_zone' and doc['data']['accountId'] == '$user_account'"
e2e_env+=("KEEL_E2E_DNS_ZONE=$dns_zone")

echo "--- pnpm e2e"
(
  cd "$frontend"
  env "${e2e_env[@]}" pnpm e2e --grep-invert "$polled_tag|$setup_tag|$mail_tag|$panel_tag|$peers_tag" "$@"
)

echo "--- the zones of the browser, the CLI and the token"
check "the three zones are active and the agent wrote their files and includes" zones_served

echo "--- the poll job, the problem and the mail"
check "ctl notify.channel.set creates the email channel to the catcher" keel_ok "$site/channel.json" ctl notify.channel.set kind=email name=catcher enabled=true "email={\"host\":\"$smtp_host\",\"port\":$smtp_port,\"security\":\"none\",\"from\":\"keel@example.org\",\"to\":\"ops@example.org\"}"
check "notify.channel.set answers the enabled channel" json_is "$site/channel.json" 'doc["ok"] is True and doc["data"]["kind"] == "email" and doc["data"]["enabled"] is True and doc["data"]["email"]["hasPassword"] is False'
check "ctl note.edit --as alice creates an overdue todo" keel_ok "$site/overdue.json" ctl note.edit --as "$user_login" "title=renew the certificate" kind=todo due=2020-01-01
check "note.edit answers the todo" json_is "$site/overdue.json" 'doc["ok"] is True and doc["data"]["kind"] == "todo" and doc["data"]["due"] == "2020-01-01"'
check "ctl cron.job.run id=notify.problems.poll" keel_ok "$site/poll.json" ctl cron.job.run id=notify.problems.poll
check "cron.job.run answers the started run" json_is "$site/poll.json" 'doc["ok"] is True and doc["data"]["id"] == "notify.problems.poll"'
check "notify.problem.list shows note.overdue for alice's account" problem_listed
check "the catcher lists the mail whose subject names note.overdue" mail_arrives "$site/mail.json" "note.overdue"
check "the mail is addressed from and to the channel's addresses" json_is "$site/mail.json" "any(message['Subject'] == 'Problem note.overdue raised for $user_account' and message['From']['Address'] == 'keel@example.org' and message['To'][0]['Address'] == 'ops@example.org' for message in doc['messages'])"
check "notify.delivery.list shows a sent delivery per notified principal" deliveries_sent 2

echo "--- pnpm e2e $polled_tag"
(
  cd "$frontend"
  env "${e2e_env[@]}" pnpm e2e --grep "$polled_tag"
)

echo "--- pnpm e2e $mail_tag"
create_principal "$site/reset-user.json" "$reset_login" user "$user_account" "$reset_password"
reset_id=$(json_get "$site/reset-user.json" 'doc["data"]["id"]')
check "ctl auth.address.set gives $reset_login the confirmed address $reset_address" keel_ok "$site/reset-address.json" ctl auth.address.set "principal=$reset_id" "value=$reset_address"
check "auth.address.set answers the confirmed address" json_is "$site/reset-address.json" "doc['ok'] is True and doc['data']['value'] == '$reset_address' and doc['data']['verified'] is True"
(
  cd "$frontend"
  env "${e2e_env[@]}" "KEEL_E2E_SMTP_API=$smtp_api" "KEEL_E2E_RESET_LOGIN=$reset_login" \
    "KEEL_E2E_RESET_PASSWORD=$reset_password" "KEEL_E2E_RESET_ADDRESS=$reset_address" \
    pnpm e2e --grep "$mail_tag"
)
check "the reset left one used reset token of $reset_login" test "$(sql "SELECT count(*) FROM keel.auth_mail_token WHERE purpose = 'reset' AND used_at IS NOT NULL AND principal_id = '$reset_id'")" = "1"

echo "--- pnpm e2e $panel_tag"
stop_serve || fail "keel serve did not stop cleanly before panel was enabled"
keel_ok "$site/enable-panel.out" product enable panel
start_serve
check "ctl hosting.user.create --as alice creates the hosting user $hosting_user" keel_ok "$site/hosting-user.json" ctl hosting.user.create --as "$user_login" "name=$hosting_user"
hosting_user_id=$(json_get "$site/hosting-user.json" 'doc["data"]["id"]')
check "the hosting user $hosting_user turns active with its home" hosting_user_active "$hosting_user_id"
check "ctl hosting.site.create --as alice creates the site $config_domain" keel_ok "$site/config-site.json" ctl hosting.site.create --as "$user_login" "user=$hosting_user_id" "domain=$config_domain"
config_site_id=$(json_get "$site/config-site.json" 'doc["data"]["id"]')
check "the site $config_domain turns active with its server block" site_active "$config_site_id" "$config_domain"
(
  cd "$frontend"
  env "${e2e_env[@]}" "KEEL_E2E_HOSTING_USER=hp_$hosting_user" "KEEL_E2E_SITE=$config_domain" \
    "KEEL_E2E_SITE_DIRECTIVE=$config_directive" "KEEL_E2E_WIZARD_SITE=$wizard_domain" pnpm e2e --grep "$panel_tag"
)
check "the site $wizard_domain of the wizard and its zone are active in alice's account, with the server block, the zone file and its include" wizard_site_with_zone
check "the zone $dns_zone dns refused left no site behind" test "$(sql "SELECT count(*) FROM keel.hosting_site WHERE domain = '$dns_zone'")" = "0"
check "the zone $dns_zone stays the one alice created over the CLI" test "$(sql "SELECT count(*) FROM keel.dns_zone WHERE name = '$dns_zone'")" = "1"
check "the server block of $config_domain carries the directive the configuration editor saved" grep -qF "    $config_directive" "$site/agent/vhosts/keel-panel-$config_domain.conf"
check "the refused directive never stayed in the server block of $config_domain" test "$(grep -c keel_e2e_refused "$site/agent/vhosts/keel-panel-$config_domain.conf")" = 0
check "ctl hosting.site.config.get answers the saved override of $config_domain" keel_ok "$site/config-get.json" ctl hosting.site.config.get "id=$config_site_id" --out json
check "the saved override is the directive alone" json_is "$site/config-get.json" "doc['data']['server'] == '$config_directive'"
check "every file operation ran the worker through agent-exec under the uid of hp_$hosting_user" files_ran_as "$(awk -F: -v k="hp_$hosting_user" '$1 == k {print $3}' "$site/agent/passwd")"
check "no staged upload or download is left in the transfer root" test -z "$(ls -A "$site/agent/transfer")"

echo "--- pnpm e2e $peers_tag"
stop_serve || fail "keel serve did not stop cleanly before dns moved to a neighbour"
check "product disable dns --switch-remote moves panel to dns-peer" keel_ok "$site/disable-dns.out" product disable dns --switch-remote
neighbour="$site/neighbour"
neighbour_database="keel_test_e2e_neighbour_$(date +%s)_$$"
neighbour_port=$(free_port)
psql "$admin_url" -qAt -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$neighbour_database\" TEMPLATE template1" >/dev/null
echo "created database $neighbour_database"
mkdir -p "$neighbour/run" "$neighbour/lib"
cat >"$neighbour/config.yaml" <<EOF
listen: 127.0.0.1:$neighbour_port
socket: $neighbour/run/core.sock
data_dir: $neighbour/lib
database: $(database_url "$admin_url" "$neighbour_database")
log: { level: info, format: text }
host: { socket: $site/agent/agent.sock }
dns: { zones_dir: $site/agent/zones, include_dir: $site/agent/named }
EOF
check "the neighbour enables dns alone" neighbour_ok "$neighbour/enable-dns.out" product enable dns
start_neighbour
neighbour_url="https://localhost:$neighbour_port"
check "ctl settings.set gives the neighbour its public address $neighbour_url" neighbour_ok "$neighbour/shell-url.json" ctl settings.set key=auth.shell_url "value=$neighbour_url"
start_relay
start_serve
check "ctl peer.pair_code on the neighbour grants the zones, the customer and the handover" neighbour_ok "$neighbour/code.json" ctl peer.pair_code "scopes=$peer_scopes" level=reseller onBehalf=true --out json
pair_code=$(json_get "$neighbour/code.json" 'doc["data"]["code"]')
check "ctl peer.create pairs the panel installation with the neighbour" keel_ok "$site/peer.json" ctl peer.create "url=http://127.0.0.1:$relay_port" "code=$pair_code" --out json
link=$(neighbour_sql "SELECT id FROM keel.peer_product")
peer_principal=$(neighbour_sql "SELECT principal_id FROM keel.peer_product")
instance=$(curl_tls -s "$scheme://127.0.0.1:$port/api/v1/instance" | python3 -c 'import json, sys; print(json.load(sys.stdin)["instanceId"])')
peer_login=$(echo "panel-$instance-$user_account" | tr '[:upper:]' '[:lower:]')
check "ctl hosting.site.create --as alice creates $peer_site with its zone on the neighbour" keel_ok "$site/peer-site.json" ctl hosting.site.create --as "$user_login" "user=$hosting_user_id" "domain=$peer_site" dnsZone=true
check "the zone $peer_site lands on the neighbour in the account of alice's customer" peer_zone_served
neighbour_spki=$(spki_of "$neighbour/lib/tls/cert.pem")
(
  cd "$frontend"
  env "${e2e_env[@]}" "KEEL_E2E_SPKI=$spki,$neighbour_spki" "KEEL_E2E_PEER_URL=$neighbour_url" \
    "KEEL_E2E_PEER_SITE=$peer_site" "KEEL_E2E_PEER_LOGIN=$peer_login" pnpm e2e --grep "$peers_tag"
)
check "the neighbour audits the key as $peer_login with its peer as actor" handover_audited "$peer_login peer-$(echo "$link" | tr '[:upper:]' '[:lower:]') peer:$link committed"
check "the key was issued by the principal of the link and is spent" test "$(neighbour_sql "SELECT issued_by || ' ' || (used_at IS NOT NULL)::text FROM keel.auth_login_key")" = "$peer_principal true"

stop_serve || fail "keel serve did not stop cleanly"
