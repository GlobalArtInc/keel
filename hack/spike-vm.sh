#!/usr/bin/env bash
set -euo pipefail
exec 2>&1

here=/root/keel-spike
binary=/usr/local/bin/keel-spike
unit_name=keel-spike-agent
unit=/etc/systemd/system/$unit_name.service
policy_dir=/etc/keel-spike
runtime_dir=/run/keel-spike
socket=$runtime_dir/agent.sock
marker=$here/.nginx-installed
auto_before=$here/.auto-before
agent_user=keel
spike_user=kd_spike
vhost=/etc/nginx/conf.d/keel-demo-spike.conf
webroot=/var/www/keel-demo-spike
health=http://127.0.0.1:1500/healthz
body="keel spike $(date -u +%Y%m%dT%H%M%SZ) $$"
failures=0
out=

usage() {
  echo "usage: spike-vm.sh [--cleanup]"
  echo "  run as root from $here holding keel-spike, keel-spike-agent.service,"
  echo "  spike.yaml and templates/; proves the host spike on this machine with an nginx"
  echo "  it installs from the distribution; --cleanup removes every trace it left"
}

fail() {
  echo "FAILED: $1"
  failures=$((failures + 1))
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

call() {
  local user=$1 op=$2 args=$3
  runuser -u "$user" -- "$binary" call --socket "$socket" --op "$op" --args "$args" --actor "$user" --product demo
}

answer() {
  out=$(call "$@" 2>&1) || true
  echo "  $out"
}

apply_args() {
  local listen=$1 root=${2:-vhosts} name=${3:-keel-demo-spike.conf}
  printf '{"template":"vhost.conf.j2","vars":{"server_name":"spike.example","listen":"%s","root":"%s"},"dest":{"root":"%s","name":"%s"},"mode":"0644","validate":"nginx.test","reload":"nginx"}' \
    "$listen" "$webroot" "$root" "$name"
}

carries() {
  grep -qF "$1"
}

refused_call() {
  local refusal
  if refusal=$(call "$@" 2>&1); then
    echo "  answered although it should not have: $refusal"
    return 1
  fi
  echo "  $refusal"
}

serves_body() {
  local got polls=0
  while :; do
    polls=$((polls + 1))
    got=$(curl -fsS -H 'Host: spike.example' http://127.0.0.1/ 2>&1) || true
    if [ "$got" = "$body" ]; then
      echo "  $got (poll $polls)"
      return 0
    fi
    if [ "$polls" -ge 20 ]; then
      echo "  $got"
      echo "  not the known body after $polls polls"
      return 1
    fi
    sleep 0.25
  done
}

healthz() {
  local status
  status=$(curl -fsS "$health")
  echo "  $status"
}

agent_runs_as_root() {
  local pid
  pid=$(systemctl show -p MainPID --value "$unit_name")
  [ -n "$pid" ] && [ "$pid" != 0 ] && [ "$(ps -o user= -p "$pid")" = root ]
}

wait_for_socket() {
  local left=40
  while [ "$left" -gt 0 ]; do
    [ -S "$socket" ] && return 0
    sleep 0.25
    left=$((left - 1))
  done
  echo "  no socket at $socket after 10 s"
  journalctl -u "$unit_name" --no-pager | tail -20
  return 1
}

journal() {
  journalctl -u "$unit_name" _SYSTEMD_INVOCATION_ID="$(systemctl show -p InvocationID --value "$unit_name")" --no-pager -o cat
}

refusal_in_journal() {
  journal | grep -F "refused peer_uid=$(id -u nobody) admitted_uid=$(id -u "$agent_user")"
}

digest() {
  sha256sum "$vhost" | cut -d' ' -f1
}

installed_packages() {
  apt-mark showauto | sort
}

cleanup() {
  if [ ! -e "$marker" ]; then
    echo "no $marker: this script did not install nginx here and does not purge it"
    exit 1
  fi
  systemctl disable --now "$unit_name" 2>/dev/null || true
  rm -f "$unit"
  systemctl daemon-reload
  rm -rf "$runtime_dir" "$policy_dir"
  if id -u "$spike_user" >/dev/null 2>&1; then
    userdel "$spike_user"
  fi
  rm -f "$vhost"
  rm -rf "$webroot"
  local packages=() package
  while read -r package; do
    if dpkg -s "$package" >/dev/null 2>&1; then
      packages+=("$package")
    fi
  done < "$marker"
  if [ "${#packages[@]}" -gt 0 ]; then
    echo "purging what the proof installed: ${packages[*]}"
    DEBIAN_FRONTEND=noninteractive apt-get purge -y -q "${packages[@]}"
  fi
  rm -rf /etc/nginx
  rm -f "$binary"
  cd /
  rm -rf "$here"
  echo "probe: id $spike_user"
  id "$spike_user" || true
  echo "probe: systemctl status $unit_name"
  systemctl status "$unit_name" --no-pager || true
  echo "probe: dpkg -l nginx"
  dpkg -l nginx || true
  echo "probe: paths"
  ls -ld "$socket" "$runtime_dir" "$policy_dir" "$vhost" "$webroot" /etc/nginx "$binary" "$here" 2>&1 || true
  echo "the spike is cleaned up"
}

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  --cleanup|"") ;;
  *) usage; exit 2 ;;
esac

if [ "$(id -u)" != 0 ]; then
  echo "run as root"
  exit 1
fi

script_dir=$(cd "$(dirname "$0")" && pwd -P)
if [ "$script_dir" != "$here" ]; then
  echo "run from $here, not $script_dir"
  exit 1
fi

if [ "${1:-}" = --cleanup ]; then
  cleanup
  exit 0
fi

for needed in keel-spike keel-spike-agent.service spike.yaml templates/vhost.conf.j2; do
  if [ ! -e "$here/$needed" ]; then
    echo "$here/$needed is missing"
    exit 1
  fi
done
if dpkg -s nginx >/dev/null 2>&1; then
  echo "nginx is already installed on this machine; the proof installs and purges its own"
  exit 1
fi
if ! id -u "$agent_user" >/dev/null 2>&1; then
  echo "no user $agent_user on this machine; the proof runs the client as it"
  exit 1
fi

check "keel.service answers $health before" healthz

installed_packages > "$auto_before"
DEBIAN_FRONTEND=noninteractive apt-get install -y -q nginx
{ echo nginx; comm -13 "$auto_before" <(installed_packages); } > "$marker"
rm -f "$auto_before"
echo "the proof installed: $(tr '\n' ' ' < "$marker")"
check "nginx is installed from the distribution" dpkg -s nginx

mkdir -p "$webroot"
printf '%s' "$body" > "$webroot/index.html"
check "the web root carries the known body" test -f "$webroot/index.html"

install -m 0755 "$here/keel-spike" "$binary"
mkdir -p "$policy_dir/templates"
install -m 0644 "$here/spike.yaml" "$policy_dir/spike.yaml"
install -m 0644 "$here/templates/vhost.conf.j2" "$policy_dir/templates/vhost.conf.j2"
sed "s/@UID@/$(id -u "$agent_user")/" "$here/keel-spike-agent.service" > "$unit"
systemctl daemon-reload
systemctl restart "$unit_name"
check "the agent listens on $socket" wait_for_socket
check "the agent runs as root" agent_runs_as_root
check "the socket admits every uid by its mode" test "$(stat -c %a "$socket")" = 666

check "a call as nobody is refused" refused_call nobody host.user.create "{\"name\":\"$spike_user\"}"
check "the refusal is in the journal" refusal_in_journal

answer "$agent_user" host.user.create "{\"name\":\"$spike_user\"}"
check "host.user.create answers created: true" carries '"created":true' <<<"$out"
check "id -u $spike_user works" id -u "$spike_user"

answer "$agent_user" host.template.apply "$(apply_args 80)"
check "host.template.apply answers changed: true" carries '"changed":true' <<<"$out"
check "the vhost is written" test -f "$vhost"
check "nginx -t passes" nginx -t
check "the vhost answers the known body" serves_body

answer "$agent_user" host.template.apply "$(apply_args 80)"
check "the same apply answers changed: false" carries '"changed":false' <<<"$out"

before=$(digest)
answer "$agent_user" host.template.apply "$(apply_args eighty)"
check "an apply with listen eighty answers validate_failed" carries '"key":"host.error.validate_failed"' <<<"$out"
check "the detail carries nginx's message" carries '[emerg]' <<<"$out"
check "the detail names the host eighty" carries 'host not found in \"eighty\"' <<<"$out"
check "the vhost is byte-identical to the previous one" test "$(digest)" = "$before"
check "the vhost still answers the known body" serves_body

answer "$agent_user" host.template.apply "$(apply_args 80 etc)"
check "dest.root etc answers policy" carries '"key":"host.error.policy"' <<<"$out"
answer "$agent_user" host.template.apply "$(apply_args 80 vhosts spike.conf)"
check "name spike.conf answers policy" carries '"key":"host.error.policy"' <<<"$out"

check "keel.service answers $health after" healthz

echo "journal of $unit_name:"
journal | grep -E '^(listening|refused|audit) ' || true

if [ "$failures" -gt 0 ]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "the spike is proven; run $0 --cleanup to remove its traces"
