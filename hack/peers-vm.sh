#!/usr/bin/env bash
set -euo pipefail

ETC=/etc/keel
LIB=/var/lib/keel
CONFIG=$ETC/config.yaml
CERT=$LIB/tls/cert.pem
KEEL=/opt/keel/bin/current
UNIT=keel.service
AGENT_UNIT=keel-privileged.service
NAMED_UNIT=named.service
ZONES_DIR=/etc/bind/zones
INCLUDE_DIR=/etc/bind/keel
INDEX=$INCLUDE_DIR/keel-dns-zones.conf
NAMED_LOCAL=/etc/bind/named.conf.local
INCLUDE_LINE="include \"$INDEX\";"
ANCHORS=/usr/local/share/ca-certificates
STATE=/root/keel-peers-vm.state
PORT=${KEEL_PEERS_VM_PORT:-1500}
WAIT=${KEEL_PEERS_VM_WAIT:-120}
STAMP=$(date +%s)
SCOPES='["dns.zone.*","keel.principal.upsert","auth.key.handover"]'
EVENTS='["dns.zone.*","svc.*"]'
HOOK=/api/v1/x/peer/hook

usage() {
  cat <<EOF
usage: hack/peers-vm.sh install dns <keel binary or tar.zst> <address>
       hack/peers-vm.sh install panel <keel binary or tar.zst> <address>
       hack/peers-vm.sh trust <neighbour url> <neighbour certificate>
       hack/peers-vm.sh code
       hack/peers-vm.sh pair <dns url> <pair code> <taken zone>
       hack/peers-vm.sh open
       hack/peers-vm.sh check <domain> <origin> <account>
       hack/peers-vm.sh revoked

Proves the "Done when" of M6 on two virtual machines, the dns machine and the
panel machine, each run as root from a checkout of the repository. Each step
prints the command of the next one with the values it carries over; the state a
machine keeps between its own steps is $STATE.

"install dns" on the dns machine installs bind9 with its tools, includes
$INDEX from $NAMED_LOCAL and runs deploy/install.sh --product dns
--listen <address>:$PORT --binary <archive> --yes; "install panel" on the panel
machine runs deploy/install.sh --product panel the same way. Both give the
installation the public address https://<address>:$PORT, which pairing sends
the neighbour, and find the generated certificate naming <address>.

"trust" runs on each machine with the address and the certificate
($CERT) of the other one: it adds the certificate to the trust store of the
system under $ANCHORS, which the installation reads when it calls a
neighbour, finds the neighbour's /readyz and handshake answering over TLS
without a pinned certificate, and restarts $UNIT.

"code" on the dns machine creates the zone taken-<stamp>.test, which a site on
the panel machine cannot get, and issues the pair code a panel needs:
$SCOPES at the level reseller, on behalf of customers.

"pair" on the panel machine pairs by that code over the CLI, subscribing to
$EVENTS, creates a user account with a principal, a hosting user and a site
with a DNS zone, finds the site active without a zone error, finds a site on
the taken zone refused with dns_peer.error.exists on the field domain and not
committed, waits for the event dns.zone.created of the zone, signed by the dns
machine, to land once in the inbox and once on the bus, and prints the
one-time URL of hosting.site.open_zone to open in a browser, where the zone
list of the dns machine opens as the customer of the site. "open" prints a
fresh one-time URL when the first one has expired.

"check" on the dns machine, after the URL was opened, finds the zone active in
the account of the customer the panel machine named by its origin, named
answering it authoritatively, the creation and the one-time key in the audit
trail as the customer with the link as actor and the key spent, and the
delivery of the zone's event delivered. It marks that delivery failed, as if
its answer had been lost, and retries it with webhook.delivery.retry, so the
same event goes out again under a fresh signature. It calls svc.dns.zones.open
twice with one Idempotency-Key over HTTPS with the API token of an
administrator, finds the second answer replayed and one service in the
customer's account, waits for the delivery of its svc.service.opened to the
panel machine to read delivered, since deleting the link drops a delivery not
yet sent, finds the revoked token answering 401, and revokes the link with
peer.delete.

"revoked" on the panel machine finds the zone's event still once in the inbox
and on the bus after the repeat, svc.service.opened arrived once, and opening
the zone refused with the 401 of the dns machine relayed as peer.
EOF
}

say() {
  printf '%s\n' "$*"
}

fail() {
  say "peers-vm.sh: $1" >&2
  exit 1
}

failures=0

check() {
  local name=$1
  shift
  if "$@"; then
    say "ok: $name"
  else
    say "FAILED: $name"
    failures=$((failures + 1))
  fi
}

finish() {
  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "$1"
}

need_root() {
  [ "$(id -u)" -eq 0 ] || fail "run as root"
}

need_keel() {
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
}

remember() {
  local key=$1 value=$2
  touch "$STATE"
  chmod 0600 "$STATE"
  grep -v "^$key=" "$STATE" >"$STATE.new" || true
  printf '%s=%s\n' "$key" "$value" >>"$STATE.new"
  mv "$STATE.new" "$STATE"
}

recall() {
  local value
  value=$(sed -n "s/^$1=//p" "$STATE" 2>/dev/null | tail -n 1)
  [ -n "$value" ] || fail "$STATE holds no $1; run the earlier steps on this machine first"
  printf '%s\n' "$value"
}

as_keel() {
  runuser -u keel -- "$KEEL" --config "$CONFIG" "$@"
}

field() {
  python3 -c '
import json, sys
doc = json.load(sys.stdin)
value = doc
for key in sys.argv[1].split("."):
    value = value[int(key)] if isinstance(value, list) else value[key]
print(value)
' "$1"
}

call() {
  local out
  out=$(as_keel ctl "$@" --out json) || {
    say "  keel ctl $1 answered: $out" >&2
    return 1
  }
  printf '%s\n' "$out"
}

called() {
  call "$@" >/dev/null
}

refused_with() {
  local code=$1 key=$2 path=$3
  shift 3
  local out
  if out=$(as_keel ctl "$@" --out json 2>&1); then
    say "  keel ctl $1 was accepted: $out"
    return 1
  fi
  say "  keel ctl $1 answered: $out"
  python3 -c '
import json, sys
doc = json.loads(sys.stdin.read())
error = doc["error"]
code, key, path = sys.argv[1:4]
assert error["code"] == code, error
if path:
    assert [(field["path"], field["key"]) for field in error.get("fields", [])] == [(path, key)], error
else:
    assert error["key"] == key, error
' "$code" "$key" "$path" <<<"$out"
}

eventually() {
  local waited=0
  while [ "$waited" -lt "$((WAIT * 4))" ]; do
    if "$@" 2>/dev/null; then
      return 0
    fi
    sleep 0.25
    waited=$((waited + 1))
  done
  "$@"
}

database_url() {
  local url
  url=$(sed -nE 's/^database:[[:space:]]*"?([^"]+)"?[[:space:]]*$/\1/p' "$CONFIG" | head -n 1)
  printf '%s\n' "${url:-postgres:///keel}"
}

sql() {
  (cd / && runuser -u keel -- psql -X -qAt -v ON_ERROR_STOP=1 "$(database_url)" -c "$1")
}

sql_is() {
  local expected=$1 query=$2 actual
  actual=$(sql "$query") || return 1
  if [ "$actual" != "$expected" ]; then
    say "  the database answered '$actual', expected '$expected'" >&2
    return 1
  fi
}

listen_of() {
  local listen
  listen=$(sed -nE 's/^listen:[[:space:]]*"?([^"]+)"?[[:space:]]*$/\1/p' "$CONFIG" | head -n 1)
  printf '%s\n' "${listen:-127.0.0.1:1500}"
}

own_url() {
  local listen
  listen=$(listen_of)
  printf 'https://%s\n' "$listen"
}

ready() {
  curl -sf --cacert "$CERT" "$(own_url)/readyz" >/dev/null 2>&1
}

names_address() {
  openssl x509 -in "$CERT" -noout -ext subjectAltName 2>/dev/null | grep -F "IP Address:$1" >/dev/null
}

install_bind() {
  DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends bind9 bind9-utils bind9-dnsutils
  install -d -m 0755 -o root -g bind "$ZONES_DIR" "$INCLUDE_DIR"
  [ -e "$INDEX" ] || install -m 0644 -o root -g root /dev/null "$INDEX"
  grep -qxF "$INCLUDE_LINE" "$NAMED_LOCAL" || printf '%s\n' "$INCLUDE_LINE" >>"$NAMED_LOCAL"
  local out
  out=$(named-checkconf -z 2>&1) || fail "named-checkconf -z refuses the configuration with $INDEX included: $out"
  systemctl enable --now "$NAMED_UNIT"
  systemctl reload "$NAMED_UNIT"
}

install_keel() {
  local product=${1:-} archive=${2:-} address=${3:-}
  case "$product" in dns | panel) ;; *) usage >&2; fail "install takes dns or panel" ;; esac
  [ -n "$archive" ] && [ -n "$address" ] || { usage >&2; fail "the archive and the address are required"; }
  [[ $address =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "the address must be the IPv4 address the other machine reaches, not \"$address\""
  [ -f "$archive" ] || fail "$archive is not a file"
  archive=$(realpath -- "$archive")
  need_root
  cd "$(dirname "$0")/.."
  [ -f deploy/install.sh ] || fail "deploy/install.sh is missing; run from a checkout of the repository"
  DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends curl python3 openssl ca-certificates
  if [ "$product" = dns ]; then
    install_bind
  fi
  local log url="https://$address:$PORT"
  log=$(mktemp)
  if ! bash deploy/install.sh --product "$product" --listen "$address:$PORT" --binary "$archive" --yes 2>&1 | tee "$log"; then
    fail "deploy/install.sh failed; its output is in $log"
  fi
  check "$UNIT is active" systemctl is-active --quiet "$UNIT"
  check "$AGENT_UNIT is active" systemctl is-active --quiet "$AGENT_UNIT"
  if [ "$product" = dns ]; then
    check "$NAMED_UNIT is active" systemctl is-active --quiet "$NAMED_UNIT"
  fi
  check "the generated certificate names $address" names_address "$address"
  check "keel ctl settings.set gives the installation its public address $url" called settings.set key=auth.shell_url "value=\"$url\""
  remember product "$product"
  remember url "$url"
  finish "keel is installed with $product at $url; copy $CERT to the other machine and run there: hack/peers-vm.sh trust $url <that file>"
}

handshake_answers() {
  local url=$1
  curl -sf "$url/api/v1/instance" | python3 -c '
import json, sys
doc = json.load(sys.stdin)
assert doc["publicKey"].startswith("ed25519:") and "peers" in doc["features"], doc
print("  " + doc["instanceId"] + " offers " + ",".join(doc["products"]))
'
}

trust() {
  local url=${1:-} certificate=${2:-}
  [ -n "$url" ] && [ -n "$certificate" ] || { usage >&2; fail "the neighbour's url and certificate are required"; }
  [ -f "$certificate" ] || fail "$certificate is not a file"
  need_keel
  local host anchor
  host=${url#https://}
  host=${host%%/*}
  host=${host//[^0-9A-Za-z.-]/-}
  anchor="$ANCHORS/keel-peer-$host.crt"
  openssl x509 -in "$certificate" -noout >/dev/null 2>&1 || fail "$certificate is not a PEM certificate"
  install -m 0644 -o root -g root "$certificate" "$anchor"
  update-ca-certificates >/dev/null
  check "the neighbour's /readyz answers over TLS through the trust store of the system" curl -sf "$url/readyz" -o /dev/null
  check "the neighbour's handshake answers with its instance key" handshake_answers "$url"
  systemctl restart "$UNIT"
  check "$UNIT answers /readyz again after the restart" eventually ready
  finish "this machine trusts $url through $anchor"
}

issue_code() {
  need_keel
  [ "$(recall product)" = dns ] || fail "code runs on the dns machine"
  local taken="taken-$STAMP.test" code
  check "keel ctl settings.set lets the webhook deliveries reach the panel machine at its IP address" called settings.set key=webhook.private_targets value=true
  check "keel ctl dns.zone.create creates $taken, which no site of the panel machine can get" called dns.zone.create "name=\"$taken\""
  code=$(call peer.pair_code "scopes=$SCOPES" level=reseller onBehalf=true | field data.code) || fail "keel ctl peer.pair_code failed"
  check "the pair code reads PAIR-XXXX-XXXX" grep -qE '^PAIR-[0-9A-Z]{4}-[0-9A-Z]{4}$' <<<"$code"
  finish "on the panel machine run: hack/peers-vm.sh pair $(recall url) $code $taken"
}

active() {
  local function=$1 id=$2
  [ "$(call "$function" "id=$id" 2>/dev/null | field data.state 2>/dev/null)" = "active" ]
}

no_zone_error() {
  [ "$(call hosting.site.get "id=$1" | field data.zoneError)" = "None" ]
}

no_site() {
  [ "$(call hosting.site.list "search=\"$1\"" | field data.total)" = "0" ]
}

zone_event() {
  sql "SELECT event_id FROM keel.peer_inbox WHERE event_json->>'type' = 'dns.zone.created' AND event_json->'data'->>'name' = '$1'"
}

zone_arrived() {
  [ -n "$(zone_event "$1")" ]
}

received_once() {
  local event=$1 kind=$2 instance=$3
  sql_is "1 true" "SELECT count(*) || ' ' || coalesce(bool_and(processed_at IS NOT NULL), false) FROM keel.peer_inbox WHERE event_id = '$event'" \
    && sql_is "$kind $instance" "SELECT string_agg(type || ' ' || instance_id, ',') FROM keel.keel_outbox WHERE id = '$event'"
}

own_instance() {
  curl -sf --cacert "$CERT" "$(own_url)/api/v1/instance" | field instanceId
}

lower() {
  tr '[:upper:]' '[:lower:]' <<<"$1"
}

print_open() {
  local opened
  opened=$(call hosting.site.open_zone --as "$(recall login)" "id=$(recall site)" | field data.url) || return 1
  grep -qE "^$(recall neighbour)/login\\?key=kok_[0-9a-f]+$" <<<"$opened" || {
    say "  hosting.site.open_zone answered $opened"
    return 1
  }
  say "  open in a browser within a minute: $opened"
}

pair() {
  local url=${1:-} code=${2:-} taken=${3:-}
  [ -n "$url" ] && [ -n "$code" ] && [ -n "$taken" ] || { usage >&2; fail "the dns url, the pair code and the taken zone are required"; }
  need_keel
  [ "$(recall product)" = panel ] || fail "pair runs on the panel machine"
  local name="pv$STAMP" domain="pv$STAMP.test" login="peersvm$STAMP"
  local linked instance account user site event
  linked=$(call peer.create "url=\"$url\"" "code=\"$code\"" "subscribe=$EVENTS") || fail "keel ctl peer.create did not pair with $url"
  instance=$(field data.instanceId <<<"$linked")
  check "the link to the dns machine $instance is active and offers dns" python3 -c '
import json, sys
doc = json.loads(sys.argv[1])["data"]
assert doc["status"] == "active" and "dns" in doc["products"] and doc["granted"]["onBehalf"], doc
' "$linked"
  remember neighbour "$url"
  remember instance "$instance"
  account=$(call keel.account.create kind=user | field data.id) || fail "keel ctl keel.account.create failed"
  call keel.principal.create "login=$login" level=user "account=$account" >/dev/null || fail "keel ctl keel.principal.create failed"
  user=$(call hosting.user.create --as "$login" "name=$name" | field data.id) || fail "keel ctl hosting.user.create failed"
  check "the hosting user $name turns active" eventually active hosting.user.get "$user"
  site=$(call hosting.site.create --as "$login" "user=$user" "domain=\"$domain\"" dnsZone=true | field data.id) || fail "keel ctl hosting.site.create with a zone failed"
  remember login "$login"
  remember site "$site"
  check "the site $domain turns active" eventually active hosting.site.get "$site"
  check "the site carries no zone error" no_zone_error "$site"
  check "a site on the taken zone $taken is refused with dns_peer.error.exists on the field domain" refused_with peer dns_peer.error.exists domain hosting.site.create --as "$login" "user=$user" "domain=\"$taken\"" dnsZone=true
  check "no site $taken was committed" no_site "$taken"
  check "the event dns.zone.created of $domain arrives signed from the dns machine" eventually zone_arrived "$domain"
  event=$(zone_event "$domain")
  remember event "$event"
  check "the event lands once in the inbox and once on the bus under the dns machine" eventually received_once "$event" dns.zone.created "$instance"
  check "hosting.site.open_zone answers a one-time URL on the dns machine" print_open
  finish "after opening it, on the dns machine run: hack/peers-vm.sh check $domain panel:$(own_instance) $account; a fresh URL: hack/peers-vm.sh open"
}

open_again() {
  need_keel
  [ "$(recall product)" = panel ] || fail "open runs on the panel machine"
  check "hosting.site.open_zone answers a one-time URL on the dns machine" print_open
  finish "the URL lands on the zone list of the dns machine as the customer of the site"
}

zone_active() {
  [ "$(call dns.zone.list "search=\"$1\"" 2>/dev/null | field data.data.0.state 2>/dev/null)" = "active" ]
}

authoritative() {
  dig @127.0.0.1 "$1" SOA +norecurse +time=1 +tries=1 2>/dev/null | grep -E '^;; flags: [a-z ]*\baa\b' >/dev/null
}

trailed() {
  local function=$1 customer=$2 expected=$3
  sql_is "$expected" "SELECT string_agg(DISTINCT principal_login || ' ' || coalesce(actor_login, '-') || ' ' || via || ' ' || status, ',') FROM keel.audit_call WHERE function = '$function' AND principal_login = '$customer'"
}

post_call() {
  local header=$1 path=$2 body=$3 out=$4 answer_headers=$5
  shift 5
  local args=(-s --cacert "$CERT" -o "$out" -D "$answer_headers" -w '%{http_code}' -H @"$header" -H 'content-type: application/json')
  local extra
  for extra in "$@"; do
    args+=(-H "$extra")
  done
  curl "${args[@]}" -d "$body" "$(own_url)$path" || true
}

answered() {
  local expected=$1
  shift
  local status
  status=$(post_call "$@")
  [ "$status" = "$expected" ] || {
    say "  POST $2 answered $status, expected $expected: $(cat "$4")"
    return 1
  }
}

same_data() {
  python3 -c '
import json, sys
first, second = (json.load(open(path))["data"] for path in sys.argv[1:3])
assert first == second and first["status"] == "active", (first, second)
' "$1" "$2"
}

check_dns() {
  local domain=${1:-} origin=${2:-} account=${3:-}
  [ -n "$domain" ] && [ -n "$origin" ] && [ -n "$account" ] || { usage >&2; fail "the domain, the origin and the account are required"; }
  need_keel
  [ "$(recall product)" = dns ] || fail "check runs on the dns machine"
  local peers link instance link_principal customer link_trail event delivery attempt
  local billing token_answer token token_id header work body service
  peers=$(call peer.list) || fail "keel ctl peer.list failed"
  link=$(field data.data.0.id <<<"$peers")
  instance=$(field data.data.0.instanceId <<<"$peers")
  check "the dns machine holds one link, to the panel machine the origin names" test "$(field data.total <<<"$peers") $origin" = "1 panel:$instance"
  link_principal=$(sql "SELECT principal_id FROM keel.peer_product WHERE id = '$link'")
  customer=$(lower "panel-$instance-$account")
  link_trail="$customer peer-$(lower "$link") peer:$link committed"
  check "the zone $domain is active" eventually zone_active "$domain"
  check "the zone belongs to the account of the customer the panel machine named by its origin" sql_is "$origin $account" "SELECT a.origin || ' ' || a.external_id FROM keel.dns_zone z JOIN keel.keel_account a ON a.id = z.account_id WHERE z.name = '$domain'"
  check "named answers $domain authoritatively" eventually authoritative "$domain"
  check "the trail records the creation of the zone as the customer with the link as actor" trailed dns.zone.create "$customer" "$link_trail"
  check "the trail records the one-time key as the customer with the link as actor" trailed auth.key.handover "$customer" "$link_trail"
  check "a key issued by the principal of the link was spent" sql_is "$link_principal true" "SELECT issued_by || ' ' || bool_or(used_at IS NOT NULL)::text FROM keel.auth_login_key WHERE principal_id = (SELECT id FROM keel.keel_principal WHERE login = '$customer') GROUP BY issued_by"

  event=$(sql "SELECT id FROM keel.keel_outbox WHERE type = 'dns.zone.created' AND data_json->>'name' = '$domain'")
  delivery=$(sql "SELECT d.id FROM keel.webhook_delivery d JOIN keel.webhook_subscription s ON s.id = d.subscription_id WHERE s.peer_id = '$link' AND d.event_id = '$event'")
  check "the event of the zone went to the hook of the panel machine and was delivered" eventually sql_is "delivered $HOOK" "SELECT d.state || ' ' || substring(s.url from '/api/.*$') FROM keel.webhook_delivery d JOIN keel.webhook_subscription s ON s.id = d.subscription_id WHERE d.id = '$delivery'"
  attempt=$(sql "SELECT attempt FROM keel.webhook_delivery WHERE id = '$delivery'")
  sql "UPDATE keel.webhook_delivery SET state = 'failed', next_at = now() + interval '1 hour' WHERE id = '$delivery'" >/dev/null
  say "  marked the delivery $delivery failed, as if its answer had been lost"
  sleep 1.1
  check "webhook.delivery.retry sends the event again" called webhook.delivery.retry "id=$delivery"
  check "the repeat is delivered and the panel machine answers it" eventually sql_is "delivered $((attempt + 1)) 200" "SELECT state || ' ' || attempt || ' ' || last_status FROM keel.webhook_delivery WHERE id = '$delivery'"

  work=$(mktemp -d)
  billing="peersvmbilling$STAMP"
  call keel.principal.create "login=$billing" level=admin account=00000000000000000000000000 >/dev/null || fail "keel ctl keel.principal.create of $billing failed"
  token_answer=$(call auth.token.create --as "$billing" name=peers-vm) || fail "keel ctl auth.token.create failed"
  token=$(field data.token <<<"$token_answer")
  token_id=$(field data.id <<<"$token_answer")
  header="$work/header"
  install -m 0600 /dev/null "$header"
  printf 'authorization: Bearer %s\n' "$token" >"$header"
  service="pv-zones-$STAMP"
  body="{\"externalId\":\"$service\",\"account\":{\"origin\":\"$origin\",\"externalId\":\"$account\"},\"params\":{\"zoneLimit\":5}}"
  check "svc.dns.zones.open with an Idempotency-Key opens the service" answered 200 "$header" /api/v1/call/svc.dns.zones.open "$body" "$work/first.json" "$work/first.headers" "Idempotency-Key: $service-open"
  check "svc.dns.zones.open again with the same key answers the first answer replayed" answered 200 "$header" /api/v1/call/svc.dns.zones.open "$body" "$work/second.json" "$work/second.headers" "Idempotency-Key: $service-open"
  check "the second answer carries X-Keel-Idempotent-Replay and the first does not" test "$(grep -ci '^x-keel-idempotent-replay:' "$work/first.headers") $(grep -ci '^x-keel-idempotent-replay:' "$work/second.headers")" = "0 1"
  check "both answers name the same service" same_data "$work/first.json" "$work/second.json"
  check "the dns machine holds one service of that external id, in the customer's account" sql_is "1 $origin $account" "SELECT count(*) || ' ' || max(a.origin) || ' ' || max(a.external_id) FROM keel.svc_service v JOIN keel.keel_account a ON a.id = v.account_id WHERE v.kind = 'dns.zones' AND v.external_id = '$service'"
  check "svc.service.opened of the service went to the hook of the panel machine and was delivered before the link goes" eventually sql_is "delivered $HOOK" "SELECT string_agg(d.state || ' ' || substring(s.url from '/api/.*$'), ',') FROM keel.webhook_delivery d JOIN keel.webhook_subscription s ON s.id = d.subscription_id JOIN keel.keel_outbox o ON o.id = d.event_id WHERE s.peer_id = '$link' AND d.type = 'svc.service.opened' AND o.data_json->>'externalId' = '$service'"
  check "keel ctl auth.token.revoke revokes the API token" called auth.token.revoke --as "$billing" "id=$token_id"
  check "the revoked API token answers 401" answered 401 "$header" /api/v1/call/svc.kinds '{}' "$work/revoked.json" "$work/revoked.headers"
  rm -rf "$work"
  check "keel ctl peer.delete revokes the link of the panel machine" called peer.delete "id=$link"
  finish "on the panel machine run: hack/peers-vm.sh revoked"
}

revoked() {
  need_keel
  [ "$(recall product)" = panel ] || fail "revoked runs on the panel machine"
  local instance
  instance=$(recall instance)
  check "the event of the zone is still once in the inbox and on the bus after its repeat" received_once "$(recall event)" dns.zone.created "$instance"
  check "svc.service.opened of the dns machine arrived once" eventually sql_is 1 "SELECT count(*) FROM keel.keel_outbox WHERE type = 'svc.service.opened' AND instance_id = '$instance'"
  check "opening the zone is refused with the 401 of the dns machine relayed as peer" refused_with peer keel.error.unauthenticated "" hosting.site.open_zone --as "$(recall login)" "id=$(recall site)"
  finish "two installations on two machines are proven end to end"
}

case "${1:-}" in
  install) shift; install_keel "$@" ;;
  trust) shift; trust "$@" ;;
  code) issue_code ;;
  pair) shift; pair "$@" ;;
  open) open_again ;;
  check) shift; check_dns "$@" ;;
  revoked) revoked ;;
  -h | --help) usage ;;
  *) usage >&2; exit 2 ;;
esac
