#!/usr/bin/env bash
set -euo pipefail

ETC=/etc/keel
LIB=/var/lib/keel
CONFIG=$ETC/config.yaml
CERT=$LIB/tls/cert.pem
KEEL=/opt/keel/bin/current
TEMPLATES=$ETC/templates
OVERRIDE=$TEMPLATES/bind/zone.j2
UNIT=keel.service
AGENT_UNIT=keel-privileged.service
NAMED_UNIT=named.service
ZONES_DIR=/etc/bind/zones
INCLUDE_DIR=/etc/bind/keel
INDEX=$INCLUDE_DIR/keel-dns-zones.conf
NAMED_LOCAL=/etc/bind/named.conf.local
INCLUDE_LINE="include \"$INDEX\";"
WAIT=${KEEL_DNS_VM_WAIT:-60}
STAMP=$(date +%s)
LOGIN=dnsvm$STAMP
CLI_ZONE=cli-$STAMP.keel.test
TOKEN_ZONE=token-$STAMP.keel.test
WWW=192.0.2.10
ROLLED_BACK=192.0.2.20
AGAIN=192.0.2.21

usage() {
  cat <<EOF
usage: hack/dns-vm.sh install <keel binary or tar.zst>
       hack/dns-vm.sh run [--browser-zone <name>]

Proves the dns bundle on a virtual machine with bind, run as root from a
checkout of the repository.

"install" installs bind9 from the distribution when named is missing,
prepares $ZONES_DIR and $INCLUDE_DIR, an empty $INDEX and the line
$INCLUDE_LINE in $NAMED_LOCAL, then runs deploy/install.sh --product dns
--binary <archive> --yes and prints the setup link. Open it, create the
administrator and, to prove the browser too, a zone on the Zones screen.

"run" proves the running installation. As the user keel over the control
socket it creates a user account and a principal $LOGIN in it, a zone over
"keel ctl dns.zone.create --as" and one with a kat_ API token over HTTPS,
waits for both to turn active and asks named for them with dig @127.0.0.1;
--browser-zone names the zone created in the browser, checked the same way.
It adds a record and sees it answered, then places a broken bind/zone.j2
under $TEMPLATES: the next record makes named-checkconf fail, the agent
rolls the zone file back, the zone turns to error, and named keeps
answering the previous serial without a restart while dig polls it the
whole time. Without the override the next record publishes the zone again
with a higher serial. The zones it created are deleted at the end.
EOF
}

say() {
  printf '%s\n' "$*"
}

fail() {
  say "dns-vm.sh: $1" >&2
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

need_root() {
  [ "$(id -u)" -eq 0 ] || fail "run as root"
}

named_present() {
  command -v named >/dev/null 2>&1 && command -v named-checkconf >/dev/null 2>&1 && command -v dig >/dev/null 2>&1
}

install_bind() {
  if named_present; then
    say "named, named-checkconf and dig are present"
  else
    say "installing bind9 and its tools from the distribution"
    DEBIAN_FRONTEND=noninteractive apt-get install -y -q bind9 bind9-utils bind9-dnsutils
  fi
  install -d -m 0755 -o root -g bind "$ZONES_DIR" "$INCLUDE_DIR"
  if [ ! -e "$INDEX" ]; then
    install -m 0644 -o root -g root /dev/null "$INDEX"
  fi
  if ! grep -qxF "$INCLUDE_LINE" "$NAMED_LOCAL"; then
    printf '%s\n' "$INCLUDE_LINE" >>"$NAMED_LOCAL"
  fi
  local out
  out=$(named-checkconf -z 2>&1) || fail "named-checkconf -z refuses the configuration with $INDEX included: $out"
  systemctl enable --now "$NAMED_UNIT"
  systemctl reload "$NAMED_UNIT"
}

install_keel() {
  local archive=${1:-}
  [ -n "$archive" ] || { usage >&2; fail "the archive is required"; }
  [ -f "$archive" ] || fail "$archive is not a file"
  need_root
  cd "$(dirname "$0")/.."
  [ -f deploy/install.sh ] || fail "deploy/install.sh is missing; run from a checkout of the repository"
  install_bind
  local log
  log=$(mktemp)
  if ! bash deploy/install.sh --product dns --binary "$archive" --yes 2>&1 | tee "$log"; then
    fail "deploy/install.sh failed; its output is in $log"
  fi
  check "$UNIT is active" systemctl is-active --quiet "$UNIT"
  check "$AGENT_UNIT is active" systemctl is-active --quiet "$AGENT_UNIT"
  check "$NAMED_UNIT is active" systemctl is-active --quiet "$NAMED_UNIT"
  check "the installer printed the setup link" grep -qF "setup link:   https://" "$log"
  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "keel is installed with dns; open the setup link, create the administrator and a zone on the Zones screen, then run: hack/dns-vm.sh run --browser-zone <that zone>"
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

listen_of() {
  local listen
  listen=$(sed -nE 's/^listen:[[:space:]]*"?([^"]+)"?[[:space:]]*$/\1/p' "$CONFIG" | head -n 1)
  printf '%s\n' "${listen:-127.0.0.1:1500}"
}

api_base() {
  local listen host port
  listen=$(listen_of)
  host=${listen%:*}
  port=${listen##*:}
  case "$host" in 0.0.0.0 | "[::]" | "") host=127.0.0.1 ;; esac
  printf 'https://%s:%s\n' "$host" "$port"
}

zone_state() {
  local id=$1
  shift
  call dns.zone.get "$@" "id=$id" | field data.state
}

wait_state() {
  local id=$1 expected=$2 waited=0 state=""
  shift 2
  while [ "$waited" -lt "$((WAIT * 4))" ]; do
    state=$(zone_state "$id" "$@" 2>/dev/null || true)
    if [ "$state" = "$expected" ]; then
      return 0
    fi
    sleep 0.25
    waited=$((waited + 1))
  done
  say "  the zone $id is $state, not $expected, after ${WAIT}s"
  return 1
}

authoritative() {
  dig @127.0.0.1 "$1" SOA +norecurse +time=1 +tries=1 2>/dev/null | grep -qE '^;; flags: [a-z ]*\baa\b'
}

serial_of() {
  dig @127.0.0.1 "$1" SOA +short +time=1 +tries=1 2>/dev/null | awk '{print $3}'
}

answers() {
  local name=$1 expected=$2 got polls=0
  while :; do
    polls=$((polls + 1))
    got=$(dig @127.0.0.1 "$name" A +short +time=1 +tries=1 2>/dev/null || true)
    if [ "$got" = "$expected" ]; then
      say "  $name -> $got"
      return 0
    fi
    if [ "$polls" -ge 20 ]; then
      say "  $name -> '$got', not '$expected'"
      return 1
    fi
    sleep 0.25
  done
}

unanswered() {
  [ -z "$(dig @127.0.0.1 "$1" A +short +time=1 +tries=1 2>/dev/null || true)" ]
}

digest() {
  sha256sum "$ZONES_DIR/keel-dns-$1.zone" | cut -d' ' -f1
}

named_pid() {
  systemctl show -p MainPID --value "$NAMED_UNIT"
}

named_restarts() {
  systemctl show -p NRestarts --value "$NAMED_UNIT"
}

served() {
  local zone=$1
  test -f "$ZONES_DIR/keel-dns-$zone.zone" \
    && test -f "$INCLUDE_DIR/keel-dns-$zone.conf" \
    && grep -qF "$INCLUDE_DIR/keel-dns-$zone.conf" "$INDEX" \
    && authoritative "$zone"
}

gone() {
  local zone=$1 waited=0
  while [ "$waited" -lt "$((WAIT * 4))" ]; do
    if [ ! -e "$ZONES_DIR/keel-dns-$zone.zone" ] && [ ! -e "$INCLUDE_DIR/keel-dns-$zone.conf" ] \
      && ! grep -qF "keel-dns-$zone.conf" "$INDEX" && ! authoritative "$zone"; then
      return 0
    fi
    sleep 0.25
    waited=$((waited + 1))
  done
  return 1
}

watch_named() {
  local zone=$1 flag=$2 log=$3
  while [ -e "$flag" ]; do
    if [ -n "$(dig @127.0.0.1 "$zone" SOA +short +time=1 +tries=1 2>/dev/null || true)" ]; then
      printf 'up\n' >>"$log"
    else
      printf 'down\n' >>"$log"
    fi
    sleep 0.1
  done
}

agent_logged() {
  local since=$1 text=$2
  journalctl -u "$AGENT_UNIT" --since "$since" --no-pager | grep -qF -- "$text"
}

answered_throughout() {
  local polls=$1
  grep -qx up "$polls" && ! grep -qx down "$polls"
}

break_template() {
  install -d -m 0755 -o root -g root "$TEMPLATES/bind"
  cat >"$OVERRIDE" <<'EOF'
$ORIGIN {{ name }}.
$TTL {{ minimum }}
@ IN SOA {{ nameserver }}. {{ hostmaster }}. ( {{ serial }} {{ refresh }} {{ retry }} {{ expire }} {{ minimum }} )
@ IN NS {{ nameserver }}.
@ IN BROKEN on purpose
EOF
  chmod 0644 "$OVERRIDE"
}

create_principal() {
  local account
  account=$(call keel.account.create kind=user | field data.id) || return 1
  call keel.principal.create "login=$LOGIN" level=user "account=$account" >/dev/null || return 1
  say "  the principal $LOGIN in the account $account"
}

create_with_token() {
  local token header body id
  token=$(call auth.token.create --as "$LOGIN" name=dns-vm | field data.token) || return 1
  [[ $token == kat_* ]] || return 1
  header=$(mktemp)
  body=$(mktemp)
  chmod 0600 "$header"
  printf 'authorization: Bearer %s\n' "$token" >"$header"
  if ! curl -sS --cacert "$CERT" -H @"$header" -H 'content-type: application/json' \
    -d "{\"name\":\"$TOKEN_ZONE\"}" "$(api_base)/api/v1/call/dns.zone.create" >"$body"; then
    rm -f "$header" "$body"
    return 1
  fi
  rm -f "$header"
  id=$(field data.id <"$body") || { say "  $(cat "$body")" >&2; rm -f "$body"; return 1; }
  rm -f "$body"
  printf '%s\n' "$id"
}

prove_browser_zone() {
  local zone=$1 id
  id=$(call dns.zone.list "search=$zone" | python3 -c '
import json, sys
rows = [row for row in json.load(sys.stdin)["data"]["data"] if row["name"] == sys.argv[1]]
print(rows[0]["id"] if rows else "")
' "$zone")
  check "the zone $zone the browser created is listed" test -n "$id"
  [ -n "$id" ] || return 0
  check "the browser's zone is active" wait_state "$id" active
  check "named answers the browser's zone authoritatively" served "$zone"
  say "  $zone SOA serial $(serial_of "$zone")"
}

run_proof() {
  local browser_zone=""
  while [ $# -gt 0 ]; do
    case $1 in
      --browser-zone)
        [ $# -ge 2 ] && [ -n "$2" ] || fail "--browser-zone needs the name of the zone"
        browser_zone=$2
        shift 2
        ;;
      *) usage >&2; fail "unknown argument $1" ;;
    esac
  done
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  systemctl is-active --quiet "$NAMED_UNIT" || fail "$NAMED_UNIT is not running"
  named_present || fail "named, named-checkconf and dig are needed; run install first"
  grep -qxF "$INCLUDE_LINE" "$NAMED_LOCAL" || fail "$NAMED_LOCAL does not include $INDEX; run install first"

  if [ -n "$browser_zone" ]; then
    say "--- the zone of the browser"
    prove_browser_zone "$browser_zone"
  fi

  say "--- a zone over the CLI and one with a token"
  check "keel ctl creates the principal $LOGIN" create_principal
  local cli_id token_id
  cli_id=$(call dns.zone.create --as "$LOGIN" "name=$CLI_ZONE" | field data.id) || cli_id=""
  check "keel ctl dns.zone.create --as $LOGIN creates $CLI_ZONE" test -n "$cli_id"
  token_id=$(create_with_token) || token_id=""
  check "a kat_ token of $LOGIN creates $TOKEN_ZONE over HTTPS" test -n "$token_id"
  if [ -z "$cli_id" ] || [ -z "$token_id" ]; then
    fail "$failures check(s) failed before the zones existed"
  fi
  check "$CLI_ZONE turns active" wait_state "$cli_id" active --as "$LOGIN"
  check "$TOKEN_ZONE turns active" wait_state "$token_id" active --as "$LOGIN"
  check "named answers $CLI_ZONE authoritatively" served "$CLI_ZONE"
  check "named answers $TOKEN_ZONE authoritatively" served "$TOKEN_ZONE"

  say "--- a record"
  check "keel ctl dns.record.create adds www.$CLI_ZONE" call dns.record.create --as "$LOGIN" "zone=$cli_id" type=A name=www "value=$WWW"
  check "$CLI_ZONE turns active again" wait_state "$cli_id" active --as "$LOGIN"
  check "named answers www.$CLI_ZONE" answers "www.$CLI_ZONE" "$WWW"
  local serial before pid restarts
  serial=$(serial_of "$CLI_ZONE")
  say "  $CLI_ZONE SOA serial $serial"

  say "--- a broken template"
  before=$(digest "$CLI_ZONE")
  pid=$(named_pid)
  restarts=$(named_restarts)
  local since flag polls
  since=$(date -u '+%Y-%m-%d %H:%M:%S')
  flag=$(mktemp)
  polls=$(mktemp)
  watch_named "$CLI_ZONE" "$flag" "$polls" &
  local watcher=$!
  break_template
  check "keel ctl dns.record.create adds rollback.$CLI_ZONE under the broken template" call dns.record.create --as "$LOGIN" "zone=$cli_id" type=A name=rollback "value=$ROLLED_BACK"
  check "$CLI_ZONE turns to error" wait_state "$cli_id" error --as "$LOGIN"
  sleep 1
  rm -f "$flag"
  wait "$watcher" || true
  say "  the zone's error: $(call dns.zone.get --as "$LOGIN" "id=$cli_id" | field data.error)"
  check "the agent logged host.error.validate_failed" agent_logged "$since" host.error.validate_failed
  check "the zone file is byte-identical to the one before" test "$(digest "$CLI_ZONE")" = "$before"
  check "named still serves serial $serial" test "$(serial_of "$CLI_ZONE")" = "$serial"
  check "named still answers www.$CLI_ZONE" answers "www.$CLI_ZONE" "$WWW"
  check "named does not answer the rolled-back rollback.$CLI_ZONE" unanswered "rollback.$CLI_ZONE"
  check "$NAMED_UNIT is still active" systemctl is-active --quiet "$NAMED_UNIT"
  check "$NAMED_UNIT kept its process" test "$(named_pid)" = "$pid"
  check "$NAMED_UNIT was not restarted" test "$(named_restarts)" = "$restarts"
  say "  dig polled $CLI_ZONE $(grep -c . "$polls") times during the broken apply, $(grep -cx down "$polls" || true) without an answer"
  check "named answered every poll during the broken apply" answered_throughout "$polls"
  rm -f "$polls"

  say "--- the template restored"
  rm -f "$OVERRIDE"
  check "keel ctl dns.record.create adds again.$CLI_ZONE without the override" call dns.record.create --as "$LOGIN" "zone=$cli_id" type=A name=again "value=$AGAIN"
  check "$CLI_ZONE turns active again" wait_state "$cli_id" active --as "$LOGIN"
  check "named answers again.$CLI_ZONE" answers "again.$CLI_ZONE" "$AGAIN"
  check "named answers rollback.$CLI_ZONE now" answers "rollback.$CLI_ZONE" "$ROLLED_BACK"
  local after
  after=$(serial_of "$CLI_ZONE")
  say "  $CLI_ZONE SOA serial $after"
  check "the serial rose above $serial" test "${after:-0}" -gt "$serial"

  say "--- cleanup"
  check "keel ctl dns.zone.delete removes both zones" call dns.zone.delete --as "$LOGIN" "ids=[\"$cli_id\",\"$token_id\"]"
  check "the files and includes of $CLI_ZONE are gone and named no longer serves it" gone "$CLI_ZONE"
  check "the files and includes of $TOKEN_ZONE are gone and named no longer serves it" gone "$TOKEN_ZONE"
  check "named-checkconf -z passes" named-checkconf -z
  check "$NAMED_UNIT is still active" systemctl is-active --quiet "$NAMED_UNIT"

  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "dns is proven on this machine: zones from the CLI and a token served by named, and a broken template rolled back while named kept answering"
}

case ${1:-} in
  -h | --help) usage ;;
  install) shift; install_keel "$@" ;;
  run) shift; run_proof "$@" ;;
  *) usage >&2; exit 2 ;;
esac
