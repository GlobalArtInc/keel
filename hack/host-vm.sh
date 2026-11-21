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
PKG_MARKER=/root/.keel-host-vm-hello
POLICY_BACKUP=$ETC/agent.yaml.host-vm
PROOF_USER=hp_proof
PROOF_PACKAGE=hello
NOTES_DIR=/srv/keel-demo
DIGEST_FILE=keel-demo-digest.txt
DIGEST_TEMPLATE=note/digest.j2
CRON_DIR=/etc/cron.d
CRON_FILE=keel-dns-proof
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
binary of this step, built with the demo bundle, and python3 for the client.
It keeps the installed policy as $POLICY_BACKUP, writes a proof
policy with the same uid_range (named.check, the notes root $NOTES_DIR,
the check note.digest, an exec_allow entry keel.version, a packages
allowlist of $PROOF_PACKAGE and of the three names $PROOF_PACKAGE-, $PROOF_PACKAGE+ and
$PROOF_PACKAGE. that the refusal checks send, and a demo section granted the policy class
note) and restarts $UNIT; --cleanup puts
the installed policy back. It installs bind9 from the distribution when no
named is present and records that for --cleanup. As the user keel it sends frames of
the agent protocol to $SOCKET: an operating-system user is created
inside the uid range of the policy and runs a program through
"keel agent-exec" with its own group alone, no variable of the caller and /
as its directory, refused once its primary group lies outside the range; a zone is rendered from a template override under
$TEMPLATES, validated with named-checkconf, reloaded into named and
answered by dig; the same apply answers changed: false; a broken template is
refused with host.error.validate_failed while the file stays byte-identical
and named keeps answering; an operation outside file_roots is refused with
host.error.policy and appears in journalctl -u $UNIT; the user is
removed again. The rest of the catalogue follows: host.exec runs the
keel.version entry and refuses a binary outside exec_allow; host.pkg
installs and removes $PROOF_PACKAGE through apt-get and reports the
change (left alone when it was installed before), refusing a trailing -
or + that apt-get reads as the other action and never reading a name as
a pattern; host.cron writes $CRON_DIR/$CRON_FILE as root with mode 0644
running the keel.version entry and removes it, refusing a job as root or
outside exec_allow; host.quota lists the notes root, or answers
host.error.unavailable without setquota; the compiled demo template
$DIGEST_TEMPLATE renders under $NOTES_DIR, an override under
$TEMPLATES wins over it and the compiled text returns when the
override is gone; the demo agent operation note.digest.stat reads the
digest under its policy class, is refused for a product without that class
and appears in the journal with class=note. With the demo bundle enabled,
keel serve loads the host module the bundle names, and keel ctl
keel.doctor.run answers the agent as in use and reachable from the kernel's
own account. --cleanup removes the zone, the
template, the user, the notes root, the cron drop-in, the override, the
$PROOF_PACKAGE and bind9 the script installed, and restores the policy. The
transcript is what the exit checklist records.
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

carries_either() {
  carries "$1" || carries "$2"
}

not_installed() {
  ! dpkg-query -W -f='${db:Status-Abbrev}' "$1" 2>/dev/null | grep -q '^ii'
}

digest_is() {
  [ "$(cat "$NOTES_DIR/$DIGEST_FILE" 2>/dev/null)" = "$1" ]
}

served_agent_ok() {
  python3 -c '
import json, sys
doc = json.loads(sys.argv[1])
rows = [row for row in doc["data"]["data"] if row["check"] == "agent"]
sys.exit(0 if rows and rows[0]["status"] == "ok" and "in use and reachable" in rows[0]["message"] else 1)
' "$1"
}

wait_for_socket() {
  local polls=0
  while [ ! -S "$SOCKET" ] || ! runuser -u keel -- python3 -c "$CLIENT" "$SOCKET" '{"id":0}' >/dev/null 2>&1; do
    polls=$((polls + 1))
    [ "$polls" -lt 100 ] || fail "$UNIT does not answer on $SOCKET after the restart"
    sleep 0.2
  done
}

use_proof_policy() {
  local from to
  read -r from to <<<"$(policy_range)"
  if [ ! -e "$POLICY_BACKUP" ]; then
    cp -p "$POLICY" "$POLICY_BACKUP"
  fi
  install -d -m 0755 -o root -g root "$NOTES_DIR"
  cat >"$POLICY.new" <<EOF
policy:
  uid_range: { from: $from, to: $to }
  file_roots: { zones: $ZONES_DIR, vhosts: /etc/nginx/conf.d, www: /var/www, notes: $NOTES_DIR }
  services: [named, nginx, php8.3-fpm]
  checks: { nginx.test: ["nginx", "-t"], named.check: ["named-checkconf", "-z"], note.digest: ["grep", "-q", "Notes", "$NOTES_DIR/$DIGEST_FILE"] }
  exec_allow: [certbot, quotatool, { keel.version: ["$KEEL", "--version"] }]
  packages: { allow: ["$PROOF_PACKAGE", "$PROOF_PACKAGE-", "$PROOF_PACKAGE+", "$PROOF_PACKAGE."] }
  products:
    bill: { drop_in_prefix: keel-bill- }
    demo: { user_prefix: kd_, drop_in_prefix: keel-demo-, classes: { note: { root: notes } } }
    dns: { drop_in_prefix: keel-dns- }
    panel: { user_prefix: hp_, drop_in_prefix: keel-panel- }
EOF
  chown --reference="$POLICY_BACKUP" "$POLICY.new"
  chmod --reference="$POLICY_BACKUP" "$POLICY.new"
  mv -f "$POLICY.new" "$POLICY"
  systemctl restart "$UNIT"
  wait_for_socket
  say "the proof policy is in place; $POLICY_BACKUP keeps the installed one"
}

restore_policy() {
  if [ -e "$POLICY_BACKUP" ]; then
    mv -f "$POLICY_BACKUP" "$POLICY"
    systemctl restart "$UNIT" || true
    say "the installed policy is back in $POLICY"
  fi
}

prove_catalogue() {
  call dns host.exec '{"name":"keel.version"}'
  check "host.exec runs the keel.version entry of exec_allow" carries '"output":"keel '
  call dns host.exec '{"name":"sh"}'
  check "host.exec refuses a binary outside exec_allow" carries '"key":"host.error.policy"'

  call dns host.pkg.refresh '{}'
  check "host.pkg.refresh runs apt-get update" carries '"refreshed":true'
  if not_installed "$PROOF_PACKAGE"; then
    : >"$PKG_MARKER"
    call dns host.pkg.install "{\"packages\":[\"$PROOF_PACKAGE\"]}"
    check "host.pkg.install reports $PROOF_PACKAGE as installed" carries "{\"name\":\"$PROOF_PACKAGE\",\"from\":null,\"to\":\""
    check "dpkg knows $PROOF_PACKAGE as installed" dpkg -s "$PROOF_PACKAGE"
    call dns host.pkg.install "{\"packages\":[\"$PROOF_PACKAGE\"]}"
    check "the same install answers changed: false" carries '"changed":false'
    call dns host.pkg.remove "{\"packages\":[\"$PROOF_PACKAGE\"]}"
    check "host.pkg.remove reports $PROOF_PACKAGE as removed" carries '"to":null'
    check "$PROOF_PACKAGE is no longer installed" not_installed "$PROOF_PACKAGE"
    rm -f "$PKG_MARKER"
  else
    say "$PROOF_PACKAGE is installed already; the proof reads it and leaves it"
    call dns host.pkg.get "{\"name\":\"$PROOF_PACKAGE\"}"
    check "host.pkg.get reports the installed version" carries '"version":"'
  fi
  call dns host.pkg.install '{"packages":["-y"]}'
  check "a package name read as an option answers host.error.policy" carries '"key":"host.error.policy"'
  call dns host.pkg.install "{\"packages\":[\"$PROOF_PACKAGE-\"]}"
  check "a trailing - on install answers host.error.policy" carries '"key":"host.error.policy"'
  call dns host.pkg.remove "{\"packages\":[\"$PROOF_PACKAGE+\"]}"
  check "a trailing + on remove answers host.error.policy" carries '"key":"host.error.policy"'
  call dns host.pkg.install "{\"packages\":[\"$PROOF_PACKAGE.\"]}"
  check "apt-get takes $PROOF_PACKAGE. by its name, not as a pattern" carries '"key":"host.error.exec_failed"'

  call dns host.cron.write "{\"name\":\"$CRON_FILE\",\"jobs\":[{\"schedule\":\"17 3 * * *\",\"user\":\"keel\",\"exec\":\"keel.version\"}]}"
  check "host.cron.write answers changed: true" carries '"changed":true'
  check "$CRON_DIR/$CRON_FILE is root:root with mode 644" test "$(stat -c '%U:%G:%a' "$CRON_DIR/$CRON_FILE" 2>/dev/null)" = "root:root:644"
  check "the drop-in runs the job as keel" grep -qxF "17 3 * * * keel '$KEEL' '--version'" "$CRON_DIR/$CRON_FILE"
  call dns host.cron.write "{\"name\":\"$CRON_FILE\",\"jobs\":[{\"schedule\":\"@daily\",\"user\":\"root\",\"exec\":\"keel.version\"}]}"
  check "a job as root answers host.error.policy" carries '"key":"host.error.policy"'
  call dns host.cron.write "{\"name\":\"$CRON_FILE\",\"jobs\":[{\"schedule\":\"@daily\",\"user\":\"keel\",\"exec\":\"id\"}]}"
  check "a job outside exec_allow answers host.error.policy" carries '"key":"host.error.policy"'
  call dns host.cron.list '{}'
  check "host.cron.list names the drop-in" carries "\"$CRON_FILE\""
  call dns host.cron.remove "{\"name\":\"$CRON_FILE\"}"
  check "host.cron.remove answers removed: true" carries '"removed":true'
  check "the drop-in is gone" test ! -e "$CRON_DIR/$CRON_FILE"

  call demo host.quota.list '{"root":"notes"}'
  if command -v setquota >/dev/null 2>&1 && command -v repquota >/dev/null 2>&1; then
    check "host.quota.list answers the report or the refusal of repquota" carries_either '"ok":true' '"key":"host.error.exec_failed"'
  else
    check "without the quota tools host.quota answers host.error.unavailable" carries '"key":"host.error.unavailable"'
  fi

  local digest_args compiled
  digest_args="{\"template\":\"$DIGEST_TEMPLATE\",\"vars\":{\"title\":\"Notes\",\"notes\":[\"a\",\"b\"]},\"dest\":{\"root\":\"notes\",\"name\":\"$DIGEST_FILE\"}}"
  compiled=$(printf 'Notes\n- a\n- b')
  rm -f "$TEMPLATES/$DIGEST_TEMPLATE"
  call demo host.template.apply "$digest_args"
  check "the compiled demo template renders and passes note.digest" carries '"changed":true'
  check "the digest carries the compiled text" digest_is "$compiled"
  install -d -m 0750 -o root -g keel "$TEMPLATES/note"
  printf 'Notes from the override: {{ title }}\n' >"$TEMPLATES/$DIGEST_TEMPLATE"
  call demo host.template.apply "$digest_args"
  check "an override under $TEMPLATES wins over the compiled text" digest_is "Notes from the override: Notes"
  rm -f "$TEMPLATES/$DIGEST_TEMPLATE"
  call demo host.template.apply "$digest_args"
  check "the compiled text returns without the override" digest_is "$compiled"
  call demo note.digest.stat "{\"name\":\"$DIGEST_FILE\"}"
  check "note.digest.stat reads the digest under its policy class" carries '"lines":3'
  call dns note.digest.stat "{\"name\":\"$DIGEST_FILE\"}"
  check "a product without the class note answers host.error.policy" carries '"key":"host.error.policy"'
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
  rm -f "$CRON_DIR/$CRON_FILE" "$TEMPLATES/$DIGEST_TEMPLATE"
  rmdir "$TEMPLATES/note" 2>/dev/null || true
  rm -rf "$NOTES_DIR"
  if [ -e "$PKG_MARKER" ]; then
    say "purging the $PROOF_PACKAGE the proof installed"
    DEBIAN_FRONTEND=noninteractive apt-get purge -y -q "$PROOF_PACKAGE" || true
    rm -f "$PKG_MARKER"
  fi
  restore_policy
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
  say "probe: notes root, cron drop-in and policy backup"
  ls -ld "$NOTES_DIR" "$CRON_DIR/$CRON_FILE" "$POLICY_BACKUP" 2>&1 || true
  say "the host proof is cleaned up"
}

main() {
  [ "$(id -u)" -eq 0 ] || fail "run as root"
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary"
  command -v python3 >/dev/null 2>&1 || fail "python3 is needed for the client"
  [ -f "$POLICY" ] || fail "$POLICY is missing"
  [ -n "$(policy_range)" ] || fail "$POLICY carries no uid_range"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  use_proof_policy
  check "the proof policy names named.check" grep -qF 'named.check: ["named-checkconf", "-z"]' "$POLICY"

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
  local gid
  gid=$(id -g "$PROOF_USER" 2>/dev/null || true)
  seen=$("$KEEL" agent-exec --uid "$uid" --policy "$POLICY" -- /usr/bin/id -G 2>&1 || true)
  say "  keel agent-exec --uid $uid -- /usr/bin/id -G -> $seen"
  check "keel agent-exec keeps the account's own group alone" test "$seen" = "$gid"
  seen=$(KEEL_PROOF_SECRET=leaked "$KEEL" agent-exec --uid "$uid" --policy "$POLICY" -- /usr/bin/env 2>&1 || true)
  say "  keel agent-exec --uid $uid -- /usr/bin/env -> $(tr '\n' ' ' <<<"$seen")"
  check "keel agent-exec passes no variable of the caller" test "$(cut -d= -f1 <<<"$seen" | sort | tr '\n' ' ')" = "HOME LC_ALL LOGNAME PATH USER "
  seen=$("$KEEL" agent-exec --uid "$uid" --policy "$POLICY" -- /bin/pwd 2>&1 || true)
  check "keel agent-exec starts the program in /" test "$seen" = "/"
  usermod -g 0 "$PROOF_USER"
  seen=$("$KEEL" agent-exec --uid "$uid" --policy "$POLICY" -- /usr/bin/id -u 2>&1 || true)
  usermod -g "$gid" "$PROOF_USER"
  say "  keel agent-exec --uid $uid with the primary group 0 -> $seen"
  check "keel agent-exec refuses an account whose group is outside the range" grep -qF 'has the group 0, outside the uid_range' <<<"$seen"

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

  prove_catalogue
  sleep 1
  local journal
  journal=$(journal_since "$since")
  check "the refusal is in the journal with its request id" grep -qE "request_id=host-vm-$refused_id .*outcome=host.error.policy" <<<"$journal"
  check "the accepted apply is in the journal with its request id" grep -qE "op=host.template.apply actor=host-vm level=owner request_id=host-vm-[0-9]+ product=dns .*outcome=ok" <<<"$journal"
  check "the failed check is in the journal" grep -qF "outcome=host.error.validate_failed" <<<"$journal"
  check "the demo agent operation is in the journal with its policy class" grep -qE "op=note.digest.stat actor=host-vm level=owner request_id=host-vm-[0-9]+ product=demo class=note .*outcome=ok" <<<"$journal"
  check "the package change is in the journal" grep -qE "op=host.pkg.(install|get) .*outcome=ok" <<<"$journal"

  local nobody
  nobody=$(runuser -u nobody -- python3 -c "$CLIENT" "$SOCKET" '{"id":1}' 2>&1 || true)
  say "  as nobody -> $nobody"
  check "a connection as nobody does not reach the agent" grep -qiE 'permission denied|refused|connection' <<<"$nobody"

  local served
  served=$("$KEEL" ctl keel.doctor.run --out json 2>&1 || true)
  check "keel serve, running as keel with the host module of the demo bundle, reaches the agent" served_agent_ok "$served"

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
