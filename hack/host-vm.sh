#!/usr/bin/env bash
set -euo pipefail

ETC=/etc/keel
RUN=/run/keel
KEEL=/opt/keel/bin/current
POLICY=$ETC/agent.yaml
TEMPLATES=$ETC/templates
SOCKET=$RUN/agent.sock
UNIT=keel-privileged.service
NAMED_UNIT=named.service
ZONE=keel-proof.test
ZONE_FILE=keel-dns-$ZONE.zone
ZONES_DIR=/etc/bind/zones
NAMED_LOCAL=/etc/bind/named.conf.local
MARKER=/root/.keel-host-vm-bind
PROOF_USER=hp_proof
BEGIN="// keel host-vm proof begin"
END="// keel host-vm proof end"
CLIENT='
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sys.argv[1])
s.sendall((sys.argv[2] + "\n").encode())
print(s.makefile("r").readline().rstrip("\n"))
'

usage() {
  cat <<EOF
usage: hack/host-vm.sh [--cleanup]

Proves the host agent on a virtual machine that carries an installation made
by deploy/install.sh, run as root. It expects $UNIT running the
binary of this step, python3 for the client, and named.check in the checks of
$POLICY; it installs bind9 from the distribution when no named is
present and records that for --cleanup. As the user keel it sends frames of
the agent protocol to $SOCKET: an operating-system user is created
inside the uid range of the policy and runs a program through
"keel agent-exec"; a zone is rendered from a template override under
$TEMPLATES, validated with named-checkconf, reloaded into named and
answered by dig; the same apply answers changed: false; a broken template is
refused with host.error.validate_failed while the file stays byte-identical
and named keeps answering; an operation outside file_roots is refused with
host.error.policy and appears in journalctl -u $UNIT; the user is
removed again. --cleanup removes the zone, the template, the user and the
bind9 the script installed. The transcript is what the exit checklist records.
EOF
}

say() {
  printf '%s\n' "$*"
}

fail() {
  say "host-vm.sh: $1" >&2
  exit 1
}

failures=0
next_id=1
answer=

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

call() {
  local product=$1 op=$2 args=$3
  local id=$next_id
  next_id=$((next_id + 1))
  local frame
  frame=$(printf '{"id":%s,"op":"%s","actor":{"login":"host-vm","level":"owner"},"request_id":"host-vm-%s","product":"%s","args":%s}' "$id" "$op" "$id" "$product" "$args")
  answer=$(runuser -u keel -- python3 -c "$CLIENT" "$SOCKET" "$frame" 2>&1) || true
  say "  $op -> $answer"
}

carries() {
  grep -qF -- "$1" <<<"$answer"
}

zone_args() {
  local serial=$1 value=$2
  printf '{"template":"bind/zone.j2","vars":{"origin":"%s","serial":%s,"records":[{"name":"ns1","type":"A","value":"192.0.2.1"},{"name":"www","type":"A","value":"%s"}]},"dest":{"root":"zones","name":"%s"},"mode":"0640","owner":"bind","validate":"named.check","reload":"named"}' \
    "$ZONE" "$serial" "$value" "$ZONE_FILE"
}

digest() {
  sha256sum "$ZONES_DIR/$ZONE_FILE" | cut -d' ' -f1
}

answers_www() {
  local got polls=0
  while :; do
    polls=$((polls + 1))
    got=$(dig @127.0.0.1 "www.$ZONE" A +short +time=1 +tries=1 2>/dev/null || true)
    if [ "$got" = "$1" ]; then
      say "  www.$ZONE -> $got (poll $polls)"
      return 0
    fi
    if [ "$polls" -ge 20 ]; then
      say "  www.$ZONE -> '$got', not $1 after $polls polls"
      return 1
    fi
    sleep 0.25
  done
}

serial_is() {
  local got
  got=$(dig @127.0.0.1 "$ZONE" SOA +short +time=1 +tries=1 2>/dev/null | awk '{print $3}')
  say "  SOA serial $got"
  [ "$got" = "$1" ]
}

named_restarts() {
  systemctl show -p NRestarts --value "$NAMED_UNIT"
}

unit_runs_as_root() {
  local pid
  pid=$(systemctl show -p MainPID --value "$UNIT")
  [ -n "$pid" ] && [ "$pid" != 0 ] && [ "$(ps -o user= -p "$pid")" = root ]
}

journal_since() {
  journalctl -u "$UNIT" --since "$1" --no-pager -o cat
}

policy_range() {
  sed -nE 's/^  uid_range: \{ from: ([0-9]+), to: ([0-9]+) \}$/\1 \2/p' "$POLICY" | head -n 1
}

uid_in_range() {
  local uid=$1 from to
  read -r from to <<<"$(policy_range)"
  [ -n "$from" ] && [ "$uid" -ge "$from" ] && [ "$uid" -le "$to" ]
}

named_present() {
  command -v named >/dev/null 2>&1 && command -v named-checkconf >/dev/null 2>&1
}

install_bind() {
  if named_present; then
    say "named is present; the proof uses it and --cleanup leaves it"
    command -v dig >/dev/null 2>&1 || fail "dig is needed beside the present named; install bind9-dnsutils"
    return
  fi
  say "installing bind9 and its tools from the distribution"
  DEBIAN_FRONTEND=noninteractive apt-get install -y -q bind9 bind9-utils bind9-dnsutils
  : >"$MARKER"
}

configure_zone() {
  install -d -m 2755 -o root -g bind "$ZONES_DIR"
  if ! grep -qF "$BEGIN" "$NAMED_LOCAL"; then
    {
      printf '%s\n' "$BEGIN"
      printf 'zone "%s" { type master; file "%s/%s"; };\n' "$ZONE" "$ZONES_DIR" "$ZONE_FILE"
      printf '%s\n' "$END"
    } >>"$NAMED_LOCAL"
  fi
  install -d -m 0750 -o root -g keel "$TEMPLATES/bind"
  cat >"$TEMPLATES/bind/zone.j2" <<'EOF'
$ORIGIN {{ origin }}.
$TTL 300
@ IN SOA ns1.{{ origin }}. hostmaster.{{ origin }}. ( {{ serial }} 3600 900 604800 300 )
@ IN NS ns1.{{ origin }}.
{% for record in records %}{{ record.name }} IN {{ record.type }} {{ record.value }}
{% endfor %}
EOF
  chmod 0640 "$TEMPLATES/bind/zone.j2"
  chown root:keel "$TEMPLATES/bind/zone.j2"
}

remove_zone() {
  if [ -f "$NAMED_LOCAL" ] && grep -qF "$BEGIN" "$NAMED_LOCAL"; then
    local kept
    kept=$(awk -v b="$BEGIN" -v e="$END" '$0 == b { skip = 1 } !skip { print } $0 == e { skip = 0 }' "$NAMED_LOCAL")
    printf '%s\n' "$kept" >"$NAMED_LOCAL"
  fi
  rm -f "$ZONES_DIR/$ZONE_FILE" "$TEMPLATES/bind/zone.j2"
  rmdir "$TEMPLATES/bind" 2>/dev/null || true
  if named_present && systemctl is-active --quiet "$NAMED_UNIT"; then
    systemctl reload "$NAMED_UNIT" || true
  fi
}

cleanup() {
  remove_zone
  if id -u "$PROOF_USER" >/dev/null 2>&1; then
    userdel "$PROOF_USER"
  fi
  if [ -e "$MARKER" ]; then
    say "purging the bind9 the proof installed"
    systemctl stop "$NAMED_UNIT" || true
    DEBIAN_FRONTEND=noninteractive apt-get purge -y -q bind9 bind9-utils bind9-dnsutils
    apt-get autoremove -y -q
    rm -rf /etc/bind /var/cache/bind
    rm -f "$MARKER"
  fi
  say "probe: id $PROOF_USER"
  id "$PROOF_USER" || true
  say "probe: zone file and template"
  ls -l "$ZONES_DIR/$ZONE_FILE" "$TEMPLATES/bind/zone.j2" 2>&1 || true
  say "probe: dpkg -l bind9"
  dpkg -l bind9 2>&1 | tail -n 1 || true
  say "the host proof is cleaned up"
}

main() {
  [ "$(id -u)" -eq 0 ] || fail "run as root"
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary"
  command -v python3 >/dev/null 2>&1 || fail "python3 is needed for the client"
  [ -f "$POLICY" ] || fail "$POLICY is missing"
  grep -qE 'named\.check: \["named-checkconf", "-z"\]' "$POLICY" || fail "$POLICY has no named.check in its checks; add named.check: [\"named-checkconf\", \"-z\"] and restart $UNIT"
  [ -n "$(policy_range)" ] || fail "$POLICY carries no uid_range"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"

  check "$UNIT runs as root" unit_runs_as_root
  check "the agent listens on $SOCKET" test -S "$SOCKET"
  check "the socket admits every uid by its mode; peer credentials are the gate" test "$(stat -c %a "$SOCKET")" = 666
  check "$RUN keeps other accounts away from the socket" test "$(stat -c '%U:%a' "$RUN")" = "keel:750"

  install_bind
  check "named is installed" named_present
  configure_zone
  check "named-checkconf accepts the zone statement" named-checkconf
  systemctl restart "$NAMED_UNIT"
  check "$NAMED_UNIT is active" systemctl is-active --quiet "$NAMED_UNIT"
  local since restarts
  since=$(date -u '+%Y-%m-%d %H:%M:%S')
  restarts=$(named_restarts)

  call panel host.user.create "{\"name\":\"$PROOF_USER\"}"
  check "host.user.create answers created: true" carries '"created":true'
  local uid
  uid=$(id -u "$PROOF_USER" 2>/dev/null || true)
  check "id -u $PROOF_USER works" test -n "$uid"
  check "the uid $uid is inside the uid range of the policy" uid_in_range "$uid"
  check "the answer carries that uid" carries "\"uid\":$uid"
  call panel host.user.create "{\"name\":\"$PROOF_USER\"}"
  check "the same create answers created: false" carries '"created":false'
  local seen
  seen=$("$KEEL" agent-exec --uid "$uid" --policy "$POLICY" -- /usr/bin/id -u 2>&1 || true)
  say "  keel agent-exec --uid $uid -- /usr/bin/id -u -> $seen"
  check "keel agent-exec runs the program as $PROOF_USER" test "$seen" = "$uid"
  seen=$("$KEEL" agent-exec --uid 0 --policy "$POLICY" -- /usr/bin/id -u 2>&1 || true)
  say "  keel agent-exec --uid 0 -- /usr/bin/id -u -> $seen"
  check "keel agent-exec refuses a uid outside the range" grep -qF 'outside the uid_range' <<<"$seen"

  call dns host.template.apply "$(zone_args 1 192.0.2.10)"
  check "host.template.apply answers changed: true" carries '"changed":true'
  check "the zone file is written" test -f "$ZONES_DIR/$ZONE_FILE"
  check "the zone file is owned by bind with mode 0640" test "$(stat -c '%U:%a' "$ZONES_DIR/$ZONE_FILE")" = "bind:640"
  check "named-checkconf -z passes" named-checkconf -z
  check "named answers www.$ZONE" answers_www 192.0.2.10
  check "named serves serial 1" serial_is 1

  call dns host.template.apply "$(zone_args 1 192.0.2.10)"
  check "the same apply answers changed: false" carries '"changed":false'

  local before
  before=$(digest)
  call dns host.template.apply "$(zone_args 2 not-an-address)"
  check "a broken template answers validate_failed" carries '"key":"host.error.validate_failed"'
  check "the detail carries the check's message" carries "$ZONE"
  check "the zone file is byte-identical to the previous one" test "$(digest)" = "$before"
  check "named still answers www.$ZONE" answers_www 192.0.2.10
  check "named still serves serial 1" serial_is 1
  check "$NAMED_UNIT is still active" systemctl is-active --quiet "$NAMED_UNIT"
  check "$NAMED_UNIT was not restarted" test "$(named_restarts)" = "$restarts"

  call dns host.template.apply "$(zone_args 3 192.0.2.30)"
  check "a corrected template answers changed: true" carries '"changed":true'
  check "named answers the new address" answers_www 192.0.2.30
  check "named serves serial 3" serial_is 3

  local refused_id=$next_id
  call dns host.fs.write "{\"dest\":{\"root\":\"etc\",\"name\":\"keel-dns-passwd\"},\"content\":\"x\"}"
  check "dest.root etc answers host.error.policy" carries '"key":"host.error.policy"'
  call dns host.fs.write "{\"dest\":{\"root\":\"zones\",\"name\":\"../shadow\"},\"content\":\"x\"}"
  check "a name with a separator answers host.error.policy" carries '"key":"host.error.policy"'
  call bill host.service.reload '{"name":"sshd"}'
  check "a unit outside services answers host.error.policy" carries '"key":"host.error.policy"'
  sleep 1
  local journal
  journal=$(journal_since "$since")
  check "the refusal is in the journal with its request id" grep -qE "request_id=host-vm-$refused_id .*outcome=host.error.policy" <<<"$journal"
  check "the accepted apply is in the journal with its request id" grep -qE "op=host.template.apply actor=host-vm level=owner request_id=host-vm-[0-9]+ product=dns .*outcome=ok" <<<"$journal"
  check "the failed check is in the journal" grep -qF "outcome=host.error.validate_failed" <<<"$journal"

  local nobody
  nobody=$(runuser -u nobody -- python3 -c "$CLIENT" "$SOCKET" '{"id":1}' 2>&1 || true)
  say "  as nobody -> $nobody"
  check "a connection as nobody does not reach the agent" grep -qiE 'permission denied|refused|connection' <<<"$nobody"

  call panel host.user.delete "{\"name\":\"$PROOF_USER\"}"
  check "host.user.delete answers removed: true" carries '"removed":true'
  check "id $PROOF_USER fails afterwards" test -z "$(id -u "$PROOF_USER" 2>/dev/null || true)"

  say "journal of $UNIT since $since:"
  grep -E 'keel_host_agent::audit' <<<"$journal" || true

  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "the host agent is proven on this machine; run $0 --cleanup to remove its traces"
}

case "${1:-}" in
  -h|--help) usage ;;
  --cleanup)
    [ "$(id -u)" -eq 0 ] || fail "run as root"
    cleanup
    ;;
  "") main ;;
  *) usage >&2; exit 2 ;;
esac
