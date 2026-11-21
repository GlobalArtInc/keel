#!/usr/bin/env bash
set -euo pipefail

OPT=/opt/keel
ETC=/etc/keel
LIB=/var/lib/keel
RUN=/run/keel
CERT=$LIB/tls/cert.pem
POLICY=$ETC/agent.yaml
KEEL=$OPT/bin/current
CHANNEL_PORT=${KEEL_INSTALL_VM_CHANNEL_PORT:-8098}
WATCHDOG_WAIT=${KEEL_INSTALL_VM_WATCHDOG_WAIT:-40}
PASSWORD=${KEEL_INSTALL_VM_PASSWORD:-correct-horse-battery-staple}
stage=""
server_pid=""

usage() {
  cat <<EOF
usage: hack/install-vm.sh prepare <keel binary or tar.zst> <out dir> [--version <ver>]
       hack/install-vm.sh run <dir> [--product <id>] [--listen <addr:port>]
       hack/install-vm.sh cleanup <dir>

Proves deploy/install.sh from the signed channel on a fresh virtual machine.
"prepare" runs where cargo is (the development machine): it generates a
rehearsal key pair, packages the given musl binary as a release with
hack/release.py --dry-run, which embeds the rehearsal public key into the
install.sh it publishes at the channel root, and writes the channel under
<out dir>/channel. Copy the directory to a clean machine and run "run" there
as root: it serves the channel over HTTP on 127.0.0.1:$CHANNEL_PORT with
python3, runs "curl | bash -s -- --product <id>" against it exactly as an
operator would against https://dl.globalart.hu/keel/, and checks the three
units, that root runs "keel privileged" alone among the processes of keel,
the uid range written to $POLICY, the signed manifest kept beside the version,
the Type=notify unit with a fed watchdog, and the setup link the installer
printed, which it follows with curl to create the first administrator and to
show the function gone afterwards. "cleanup" stops the channel server and
runs uninstall.sh --purge from the installed release.
EOF
}

say() {
  printf '%s\n' "$*"
}

fail() {
  say "install-vm.sh: $1" >&2
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

prepare() {
  local source=${1:-} out=${2:-}
  shift 2 || true
  local version=""
  while [ $# -gt 0 ]; do
    case $1 in
      --version) version=$2; shift 2 ;;
      *) usage >&2; fail "unknown argument: $1" ;;
    esac
  done
  [ -n "$source" ] && [ -n "$out" ] || { usage >&2; fail "prepare needs the binary and the output directory"; }
  [ -f "$source" ] || fail "$source is not a file"
  cd "$(dirname "$0")/.."
  stage=$(mktemp -d)
  trap 'rm -rf "$stage"' EXIT
  local binary
  case $source in
    *.tar.zst)
      mkdir -p "$stage/release"
      zstd -d -c "$source" | tar -x -C "$stage/release"
      binary=$stage/release/keel
      ;;
    *) binary=$source ;;
  esac
  [ -f "$binary" ] || fail "$source carries no keel at its root"
  if [ -z "$version" ]; then
    version=$(sed -nE 's/^version = "(.*)"$/\1/p' Cargo.toml | head -n 1)
  fi
  mkdir -p "$out"
  if [ ! -f "$out/keys/release.key" ]; then
    cargo run -q -p keel-release -- keygen --out "$out/keys" --comment "install rehearsal" >/dev/null
  fi
  printf 'Install rehearsal of %s.\n' "$version" >"$stage/notes.txt"
  rm -rf "$out/channel"
  KEEL_RELEASE_KEY=$(cat "$out/keys/release.key") python3 hack/release.py --tag "keel-v$version" --binary "$binary" \
    --notes-file "$stage/notes.txt" --release-key-file "$out/keys/release.pub" --verify-key "$out/keys/release.pub" \
    --zstd-level 19 --dry-run --out "$out/channel" >/dev/null
  printf '%s\n' "$version" >"$out/version"
  cp "$0" "$out/install-vm.sh"
  say "prepared $out: channel for $version signed with the rehearsal key $out/keys/release.pub"
  say "copy it to a clean machine: scp -r $out root@<host>:/root/keel-install && ssh root@<host> /root/keel-install/install-vm.sh run /root/keel-install"
}

serve_channel() {
  local dir=$1
  (cd "$dir/channel" && exec python3 -m http.server "$CHANNEL_PORT" --bind 127.0.0.1 >"$dir/channel.log" 2>&1) &
  server_pid=$!
  printf '%s\n' "$server_pid" >"$dir/channel.pid"
  local waited=0
  while [ "$waited" -lt 30 ]; do
    if curl -sf "http://127.0.0.1:$CHANNEL_PORT/stable/manifest.json" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  fail "the channel server did not answer on 127.0.0.1:$CHANNEL_PORT"
}

stop_channel() {
  local dir=$1
  if [ -f "$dir/channel.pid" ]; then
    kill "$(cat "$dir/channel.pid")" 2>/dev/null || true
    rm -f "$dir/channel.pid"
  fi
}

listen_parts() {
  local listen=$1
  local host=${listen%:*} port=${listen##*:}
  host=${host#[}
  host=${host%]}
  case $host in
    0.0.0.0) host=127.0.0.1 ;;
    ::) host=::1 ;;
  esac
  case $host in
    *:*) host="[$host]" ;;
  esac
  printf '%s %s\n' "$host" "$port"
}

api() {
  local host=$1 port=$2 func=$3 body=$4 out=$5
  curl -s -o "$out" -w '%{http_code}' --cacert "$CERT" -X POST -H 'Content-Type: application/json' --data "$body" "https://$host:$port/api/v1/call/$func" 2>/dev/null || true
}

json_has() {
  local file=$1 expression=$2
  python3 - "$file" "$expression" <<'EOF'
import json, sys
with open(sys.argv[1]) as handle:
    doc = json.load(handle)
sys.exit(0 if eval(sys.argv[2], {"doc": doc}) else 1)
EOF
}

root_processes_of_keel() {
  ps -eo user:32,args | awk '$1 == "root" && $2 ~ /(^|\/)keel$/ { $1 = ""; print }' | sed 's/^ *//'
}

run() {
  local dir=${1:-}
  shift || true
  local product=demo listen=127.0.0.1:1500
  while [ $# -gt 0 ]; do
    case $1 in
      --product) product=$2; shift 2 ;;
      --listen) listen=$2; shift 2 ;;
      *) usage >&2; fail "unknown argument: $1" ;;
    esac
  done
  [ -n "$dir" ] || { usage >&2; fail "run needs the prepared directory"; }
  [ "$(id -u)" -eq 0 ] || fail "run as root"
  [ -d /run/systemd/system ] || fail "systemd is not the running init"
  [ ! -e "$OPT" ] && [ ! -e "$LIB" ] || fail "$OPT or $LIB exists; the rehearsal needs a clean machine"
  [ -f "$dir/channel/install.sh" ] || fail "$dir/channel/install.sh is missing; run prepare first"
  for tool in curl python3 ps awk; do
    command -v "$tool" >/dev/null 2>&1 || fail "$tool is not installed"
  done
  local version
  version=$(cat "$dir/version")
  serve_channel "$dir"
  say "the channel of $version is served on http://127.0.0.1:$CHANNEL_PORT/"

  local installed=0
  check "curl | sh refuses a plain sh with the bash line" test "$(curl -fsSL "http://127.0.0.1:$CHANNEL_PORT/install.sh" | sh -s -- --product "$product" 2>&1 || true)" = "install.sh runs under bash: curl -fsSL https://dl.globalart.hu/keel/install.sh | bash -s -- --product <id>"
  if curl -fsSL "http://127.0.0.1:$CHANNEL_PORT/install.sh" | bash -s -- --product "$product" --channel-url "http://127.0.0.1:$CHANNEL_PORT/" --insecure-rehearsal-channel --listen "$listen" --yes >"$dir/install.log" 2>&1; then
    installed=1
  fi
  cat "$dir/install.log"
  check "curl | bash -s -- --product $product installs from the served channel" test "$installed" = "1"
  check "the installer said the manifest is signed by the release key" grep -q "the manifest is signed by the release key: keel $version" "$dir/install.log"
  check "the installer printed the setup link" grep -qE 'setup link:   https://[^ ]+/setup\?key=kok_[0-9a-f]{64}' "$dir/install.log"
  for unit in keel.service keel-privileged.service keel-update.path; do
    check "$unit is active" systemctl is-active --quiet "$unit"
    check "$unit is enabled" systemctl is-enabled --quiet "$unit"
  done
  check "keel.service is Type=notify" test "$(systemctl show keel.service -p Type --value)" = "notify"
  check "keel.service has a 60 s watchdog" test "$(systemctl show keel.service -p WatchdogUSec --value)" = "1min"
  check "bin/current points at the installed version" test "$(readlink "$KEEL")" = "$OPT/versions/$version/keel"
  check "the signed manifest is kept beside the version" cmp -s "$OPT/versions/$version/manifest.json" "$dir/channel/versions/$version/manifest.json"
  check "the installed deploy directory carries the published install.sh" cmp -s "$OPT/versions/$version/deploy/install.sh" "$dir/channel/install.sh"
  check "root runs keel privileged and nothing else of keel" test "$(root_processes_of_keel)" = "$KEEL privileged --socket $RUN/agent.sock --policy $POLICY"
  check "keel serve runs as the user keel" test "$(ps -eo user:32,args | awk '$2 ~ /(^|\/)keel$/ && $3 == "serve" { print $1 }')" = "keel"
  check "$POLICY carries the allotted uid range" grep -qE '^  uid_range: \{ from: [0-9]+, to: [0-9]+ \}$' "$POLICY"
  check "$POLICY carries the sections of bill, dns and panel" test "$(grep -cE '^    (bill|dns|panel): ' "$POLICY")" = "3"
  check "$POLICY is readable by root and the group keel alone" test "$(stat -c '%a %U %G' "$POLICY")" = "640 root keel"
  runuser -u keel -- "$KEEL" ctl keel.principal.list --out json >"$dir/principals-before.json"
  check "no administrator was created over the socket" json_has "$dir/principals-before.json" 'doc["ok"] is True and doc["data"]["total"] == 0'

  read -r host port <<<"$(listen_parts "$listen")"
  local key
  key=$(journalctl -u keel.service --no-pager -o cat | grep -oE '/setup\?key=kok_[0-9a-f]{64}' | tail -n 1 | sed 's/^.*key=//') || true
  check "the journal carries the setup key" test -n "$key"
  check "keel.setup.admin with a wrong key is 401" test "$(api "$host" "$port" keel.setup.admin "{\"key\":\"${key%?}\",\"login\":\"first\",\"password\":\"$PASSWORD\"}" "$dir/setup-wrong.json")" = "401"
  check "keel.setup.admin with the key creates the first administrator" test "$(api "$host" "$port" keel.setup.admin "{\"key\":\"$key\",\"login\":\"first\",\"password\":\"$PASSWORD\"}" "$dir/setup.json")" = "200"
  check "the setup answers the login" json_has "$dir/setup.json" 'doc["ok"] is True and doc["data"]["login"] == "first"'
  check "keel.setup.admin again is 404" test "$(api "$host" "$port" keel.setup.admin "{\"key\":\"$key\",\"login\":\"second\",\"password\":\"$PASSWORD\"}" "$dir/setup-again.json")" = "404"
  check "auth.login as first is 200" test "$(api "$host" "$port" auth.login "{\"login\":\"first\",\"password\":\"$PASSWORD\"}" "$dir/login.json")" = "200"
  check "the login answers the authenticated administrator" json_has "$dir/login.json" 'doc["ok"] is True and doc["data"]["state"] == "authenticated" and doc["data"]["principal"]["level"] == "admin"'
  runuser -u keel -- "$KEEL" ctl keel.principal.list search=first --out json >"$dir/principals.json"
  check "the administrator is in the root account with the admin level" json_has "$dir/principals.json" 'doc["ok"] is True and [(row["login"], row["level"], row["accountId"]) for row in doc["data"]["data"]] == [("first", "admin", "00000000000000000000000000")]'

  say "waiting ${WATCHDOG_WAIT}s for the watchdog to be fed"
  sleep "$WATCHDOG_WAIT"
  check "keel.service is still active after the watchdog interval" systemctl is-active --quiet keel.service
  check "the service manager saw a watchdog ping" test "$(systemctl show keel.service -p WatchdogTimestampMonotonic --value)" != "0"
  check "keel.service was not restarted by the watchdog" test "$(systemctl show keel.service -p NRestarts --value)" = "0"
  check "keel doctor as keel passes" runuser -u keel -- "$KEEL" doctor
  say "the setup link is used; the administrator first signs in with the rehearsal password"
  if [ "$failures" -gt 0 ]; then
    say "$failures check(s) failed; the install log is $dir/install.log"
    exit 1
  fi
  say "the installer is proven on $(hostname); run install-vm.sh cleanup $dir to remove the installation"
}

cleanup() {
  local dir=${1:-}
  [ -n "$dir" ] || { usage >&2; fail "cleanup needs the prepared directory"; }
  [ "$(id -u)" -eq 0 ] || fail "run as root"
  stop_channel "$dir"
  local script
  script=$(ls -d "$OPT"/versions/*/deploy/uninstall.sh 2>/dev/null | head -n 1) || true
  if [ -z "$script" ]; then
    mkdir -p "$dir/unpacked"
    local archive
    archive=$(ls "$dir"/channel/versions/*/keel-*.tar.zst | head -n 1)
    zstd -d -c "$archive" | tar -x -C "$dir/unpacked" deploy/uninstall.sh
    script=$dir/unpacked/deploy/uninstall.sh
  fi
  bash "$script" --purge --yes
  say "the rehearsal installation is removed"
}

case ${1:-} in
  prepare) shift; prepare "$@" ;;
  run) shift; run "$@" ;;
  cleanup) shift; cleanup "$@" ;;
  -h|--help) usage ;;
  *) usage >&2; fail "unknown command: ${1:-}" ;;
esac
