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
WWW=/var/www
WAIT=${KEEL_BRIDGE_VM_WAIT:-120}
STAMP=$(date +%s)
NAME=br$STAMP
DOMAIN=$NAME.test
TAKEN=taken-$NAME.test
REMOTE=remote-$NAME.test

usage() {
  cat <<EOF
usage: hack/bridge-vm.sh install <keel binary or tar.zst>
       hack/bridge-vm.sh run

Proves the bridge of the panel bundle to dns on one virtual machine, run as
root from a checkout of the repository.

"install" installs bind9 with its tools, nginx, php-fpm and curl, includes
$INDEX from $NAMED_LOCAL, prepares $WWW, then runs deploy/install.sh
--product dns,panel --binary <archive> --yes.

"run" proves the running installation over the control socket as the user
keel. It creates a user account, the hosting user $NAME in it and the site
$DOMAIN with a DNS zone, finds the site active, the zone active under the
same account and named answering it authoritatively. It creates the zone
$TAKEN, then finds the site $TAKEN with a zone refused with
dns.zone.error.exists on the field domain and no such site. It finds
keel product disable dns refused while panel reaches dns locally, disables
it with --switch-remote, waits for the restart, finds the site $REMOTE with a
zone refused with dns_peer.error.unpaired and the site without a zone
created, enables dns again, waits for the restart and deletes the account.
EOF
}

say() {
  printf '%s\n' "$*"
}

fail() {
  say "bridge-vm.sh: $1" >&2
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

install_keel() {
  local archive=${1:-}
  [ -n "$archive" ] || { usage >&2; fail "the archive is required"; }
  [ -f "$archive" ] || fail "$archive is not a file"
  need_root
  cd "$(dirname "$0")/.."
  [ -f deploy/install.sh ] || fail "deploy/install.sh is missing; run from a checkout of the repository"
  DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends bind9 bind9-utils bind9-dnsutils nginx php-fpm curl
  install -d -m 0755 -o root -g bind "$ZONES_DIR" "$INCLUDE_DIR"
  [ -e "$INDEX" ] || install -m 0644 -o root -g root /dev/null "$INDEX"
  grep -qxF "$INCLUDE_LINE" "$NAMED_LOCAL" || printf '%s\n' "$INCLUDE_LINE" >>"$NAMED_LOCAL"
  local out
  out=$(named-checkconf -z 2>&1) || fail "named-checkconf -z refuses the configuration with $INDEX included: $out"
  systemctl enable --now "$NAMED_UNIT"
  systemctl reload "$NAMED_UNIT"
  install -d -m 0755 -o root -g root "$WWW"
  local log
  log=$(mktemp)
  if ! bash deploy/install.sh --product dns,panel --binary "$archive" --yes 2>&1 | tee "$log"; then
    fail "deploy/install.sh failed; its output is in $log"
  fi
  check "$UNIT is active" systemctl is-active --quiet "$UNIT"
  check "$AGENT_UNIT is active" systemctl is-active --quiet "$AGENT_UNIT"
  check "$NAMED_UNIT is active" systemctl is-active --quiet "$NAMED_UNIT"
  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "keel is installed with dns and panel; run: hack/bridge-vm.sh run"
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

refused_with() {
  local key=$1 path=$2
  shift 2
  local out
  if out=$(as_keel ctl "$@" --out json 2>&1); then
    say "  keel ctl $1 was accepted: $out"
    return 1
  fi
  say "  keel ctl $1 answered: $out"
  grep -qF "\"$key\"" <<<"$out" && { [ -z "$path" ] || grep -qF "\"path\":\"$path\"" <<<"$out"; }
}

eventually() {
  local waited=0
  while [ "$waited" -lt "$((WAIT * 4))" ]; do
    if "$@"; then
      return 0
    fi
    sleep 0.25
    waited=$((waited + 1))
  done
  return 1
}

listen_of() {
  local listen
  listen=$(sed -nE 's/^listen:[[:space:]]*"?([^"]+)"?[[:space:]]*$/\1/p' "$CONFIG" | head -n 1)
  printf '%s\n' "${listen:-127.0.0.1:1500}"
}

ready() {
  local listen host port
  listen=$(listen_of)
  host=${listen%:*}
  port=${listen##*:}
  [ "$host" = "0.0.0.0" ] && host=127.0.0.1
  curl -sf --cacert "$CERT" "https://$host:$port/readyz" >/dev/null 2>&1
}

restarts() {
  systemctl show -p NRestarts --value "$UNIT"
}

main_pid() {
  systemctl show -p MainPID --value "$UNIT"
}

restarted() {
  [ "$(restarts)" -gt "$1" ] && [ "$(main_pid)" != "$2" ] && [ "$(main_pid)" != "0" ] && ready
}

switch() {
  local before pid out
  before=$(restarts)
  pid=$(main_pid)
  out=$(as_keel product "$@" 2>&1) || {
    say "  keel product $* answered: $out"
    return 1
  }
  say "  keel product $* answered: $out"
  eventually restarted "$before" "$pid"
}

user_active() {
  [ "$(call hosting.user.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "active" ]
}

site_active() {
  [ "$(call hosting.site.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "active" ]
}

zone_row() {
  call dns.zone.list "search=\"$1\"" 2>/dev/null | field "data.data.0.$2" 2>/dev/null
}

zone_active() {
  [ "$(zone_row "$1" state)" = "active" ]
}

same_account() {
  local site=$1 zone=$2
  local site_account zone_account
  site_account=$(call hosting.site.get "id=$site" | field data.accountId)
  zone_account=$(zone_row "$zone" accountId)
  say "  the site is in $site_account, the zone in $zone_account"
  [ -n "$site_account" ] && [ "$site_account" = "$zone_account" ]
}

authoritative() {
  dig @127.0.0.1 "$1" SOA +norecurse +time=1 +tries=1 2>/dev/null | grep -E '^;; flags: [a-z ]*\baa\b' >/dev/null
}

no_site() {
  [ "$(call hosting.site.list "search=\"$1\"" | field data.total)" = "0" ]
}

bridged_refusal() {
  local out
  if out=$(as_keel product disable dns 2>&1); then
    say "  keel product disable dns was accepted: $out"
    return 1
  fi
  say "  keel product disable dns answered: $out"
  grep -qF "keel.product.error.bridged" <<<"$out" || grep -qF -- "--switch-remote" <<<"$out"
}

run_proof() {
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  systemctl is-active --quiet "$NAMED_UNIT" || fail "$NAMED_UNIT is not running"
  named_present || fail "named, named-checkconf and dig are needed; run install first"
  local account user site plain
  account=$(call keel.account.create kind=user | field data.id) || fail "keel ctl keel.account.create failed"
  user=$(call hosting.user.create "name=$NAME" "account=$account" | field data.id) || fail "keel ctl hosting.user.create failed"
  check "the hosting user $NAME turns active" eventually user_active "$user"

  site=$(call hosting.site.create "user=$user" "domain=\"$DOMAIN\"" dnsZone=true | field data.id) || fail "keel ctl hosting.site.create with a zone failed"
  check "the site $DOMAIN turns active" eventually site_active "$site"
  check "the zone $DOMAIN turns active" eventually zone_active "$DOMAIN"
  check "the site and its zone belong to the same account" same_account "$site" "$DOMAIN"
  check "named answers $DOMAIN authoritatively" eventually authoritative "$DOMAIN"

  check "keel ctl dns.zone.create creates $TAKEN" call dns.zone.create "name=\"$TAKEN\"" "account=$account"
  check "a site on the taken zone $TAKEN is refused on the field domain" refused_with dns.zone.error.exists domain hosting.site.create "user=$user" "domain=\"$TAKEN\"" dnsZone=true
  check "no site $TAKEN was committed" no_site "$TAKEN"

  check "keel product disable dns is refused while panel reaches dns locally" bridged_refusal
  check "keel product disable dns --switch-remote restarts the service" switch disable dns --switch-remote
  check "a site with a zone answers dns_peer.error.unpaired" refused_with dns_peer.error.unpaired "" hosting.site.create "user=$user" "domain=\"$REMOTE\"" dnsZone=true
  plain=$(call hosting.site.create "user=$user" "domain=\"$REMOTE\"" | field data.id) || plain=""
  check "a site without a zone is still created" test -n "$plain"

  check "keel product enable dns restarts the service" switch enable dns
  check "keel ctl keel.account.delete queues the deletion of $account" call keel.account.delete "id=$account"
  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "the bridge of panel to dns is proven on this host"
}

case "${1:-}" in
  install) shift; install_keel "$@" ;;
  run) run_proof ;;
  -h | --help) usage ;;
  *) usage >&2; exit 2 ;;
esac
