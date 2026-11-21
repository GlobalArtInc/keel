#!/usr/bin/env bash
set -euo pipefail

OPT=/opt/keel
LIB=/var/lib/keel
REQUEST=$LIB/update.request
CERT=$LIB/tls/cert.pem
CONFIG=/etc/keel/config.yaml
CONFIG_BACKUP=$CONFIG.update-vm
DROP_IN_DIR=/run/systemd/system/keel-update.service.d
DROP_IN=$DROP_IN_DIR/update-vm.conf
WAIT=${KEEL_UPDATE_VM_WAIT:-180}
CHANNEL_PORT=${KEEL_UPDATE_VM_CHANNEL_PORT:-8099}
stage=""

usage() {
  cat <<EOF
usage: hack/update-vm.sh prepare <keel binary or tar.zst> <out dir> [--version <ver>] [--installed <ver>]
       hack/update-vm.sh run <dir> [--listen <addr:port>]
       hack/update-vm.sh kernel <dir> [--listen <addr:port>]
       hack/update-vm.sh cleanup <dir>

Proves the update path on a virtual machine that carries an installation made by
deploy/install.sh. "prepare" runs where cargo is (the development machine): it
generates a rehearsal key pair, packages the given binary as a release of
<ver> (default: the installed version with its patch number raised by one) with
the unit files of deploy/systemd, signs its manifest and the manifest of the
installed version, and writes a channel directory under <out dir>. Copy that
directory to the machine and run "run" there as root: it writes update.request
as the user keel, lets keel-update.path start keel-update.service with a
drop-in that points the oneshot at the copied channel and the rehearsal key,
waits for bin/current to switch and /readyz to answer, then rolls back with
"keel privileged update rollback" and checks again. "kernel" is the same
update requested through the kernel instead of by hand: it serves the copied
channel over HTTP on 127.0.0.1:$CHANNEL_PORT, points the update section of
$CONFIG at it (keeping a backup) and restarts keel.service, then runs
"keel update check" and "keel update apply --wait" as the user keel, which
drain the tasks, take the pre-update dump under $LIB/backups and write
update.request for the path unit; it waits for the switch and the restart,
reads the new version from /api/v1/boot and rolls back with the privileged
command line, since the rehearsal channel carries no archive of the installed
version for a kernel-side rollback. "cleanup" removes the drop-in, the
rehearsal version, the channel server and the config change.
EOF
}

say() {
  printf '%s\n' "$*"
}

fail() {
  say "update-vm.sh: $1" >&2
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

bump_patch() {
  local version=$1
  local major=${version%%.*}
  local rest=${version#*.}
  local minor=${rest%%.*}
  local patch=${rest#*.}
  patch=${patch%%[^0-9]*}
  printf '%s.%s.%s\n' "$major" "$minor" "$((patch + 1))"
}

prepare() {
  local source=${1:-} out=${2:-}
  shift 2 || true
  local version="" installed=""
  while [ $# -gt 0 ]; do
    case $1 in
      --version) version=$2; shift 2 ;;
      --installed) installed=$2; shift 2 ;;
      *) usage >&2; fail "unknown argument: $1" ;;
    esac
  done
  [ -n "$source" ] && [ -n "$out" ] || { usage >&2; fail "prepare needs the binary and the output directory"; }
  [ -f "$source" ] || fail "$source is not a file"
  cd "$(dirname "$0")/.."
  stage=$(mktemp -d)
  trap 'rm -rf "$stage"' EXIT
  mkdir -p "$stage/release/deploy/systemd"
  case $source in
    *.tar.zst) zstd -d -c "$source" | tar -x -C "$stage/release" ;;
    *) install -m 0755 "$source" "$stage/release/keel" ;;
  esac
  [ -f "$stage/release/keel" ] || fail "$source carries no keel at its root"
  cp deploy/systemd/*.service deploy/systemd/*.path "$stage/release/deploy/systemd/"
  if [ -z "$installed" ]; then
    installed=$("$stage/release/keel" --version | awk '{print $2}') || fail "the binary does not run here; pass --installed <ver>"
  fi
  if [ -z "$version" ]; then
    version=$(bump_patch "$installed")
  fi
  mkdir -p "$out/keys" "$out/channel/stable" "$out/channel/versions/$version" "$out/manifests"
  if [ ! -f "$out/keys/release.key" ]; then
    cargo run -q -p keel-release -- keygen --out "$out/keys" --comment "update rehearsal" >/dev/null
  fi
  local archive="$out/channel/versions/$version/keel-$version-x86_64-linux-musl.tar.zst"
  tar -C "$stage/release" -cf - keel deploy | zstd -19 -q -o "$archive" -f
  (cd "$(dirname "$archive")" && sha256sum "$(basename "$archive")" > "$(basename "$archive").sha256")
  cargo run -q -p keel-release -- sign --key "$out/keys/release.key" --version "$version" --archive "$archive" \
    --min-upgrade-from "$installed" --contract 1 --notes "update rehearsal of $version over $installed" \
    --out "$out/channel/versions/$version/manifest.json" >/dev/null
  cargo run -q -p keel-release -- sign --key "$out/keys/release.key" --version "$version" --archive "$archive" \
    --min-upgrade-from "$installed" --contract 1 --notes "update rehearsal of $version over $installed" \
    --expires-at "$(($(date +%s) + 30 * 86400))" --out "$out/channel/stable/manifest.json" >/dev/null
  cargo run -q -p keel-release -- sign --key "$out/keys/release.key" --version "$installed" --archive "$archive" \
    --min-upgrade-from 0.0.1 --contract 1 --notes "manifest of the installed version for the rehearsal" \
    --out "$out/manifests/$installed.json" >/dev/null
  mkdir -p "$out/channel/versions/$installed"
  cp "$out/manifests/$installed.json" "$out/channel/versions/$installed/manifest.json"
  printf '%s\n' "$installed" > "$out/installed"
  printf '%s\n' "$version" > "$out/version"
  say "prepared $out: channel for $version over $installed, rehearsal key $out/keys/release.pub"
  say "copy it to the machine: scp -r $out root@<host>:/root/keel-update && ssh root@<host> /root/keel-update/update-vm.sh run /root/keel-update"
  cp "$0" "$out/update-vm.sh"
}

ready_status() {
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
  curl -s -o /dev/null -w '%{http_code}' --connect-timeout 5 --max-time 10 --cacert "$CERT" "https://$host:$port/readyz" 2>/dev/null || true
}

wait_ready() {
  local listen=$1 waited=0
  while [ "$waited" -lt "$WAIT" ]; do
    if [ "$(ready_status "$listen")" = "200" ]; then
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  return 1
}

wait_current() {
  local expected=$1 waited=0
  while [ "$waited" -lt "$WAIT" ]; do
    if [ "$(readlink "$OPT/bin/current")" = "$expected" ]; then
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  return 1
}

privileged_update() {
  local dir=$1
  shift
  "$OPT/bin/current" privileged update --channel-url "file://$dir/channel" --release-key "$dir/keys/release.pub" "$@"
}

run() {
  local dir=${1:-}
  shift || true
  local listen=127.0.0.1:1500
  while [ $# -gt 0 ]; do
    case $1 in
      --listen) listen=$2; shift 2 ;;
      *) usage >&2; fail "unknown argument: $1" ;;
    esac
  done
  [ -n "$dir" ] || { usage >&2; fail "run needs the prepared directory"; }
  [ "$(id -u)" -eq 0 ] || fail "run as root"
  [ -d /run/systemd/system ] || fail "systemd is not the running init"
  [ -L "$OPT/bin/current" ] || fail "$OPT/bin/current is missing; install keel first"
  local installed version
  installed=$(cat "$dir/installed")
  version=$(cat "$dir/version")
  local before
  before=$(readlink "$OPT/bin/current")
  say "installed $installed at $before; the rehearsal installs $version from $dir/channel"
  if [ ! -f "$OPT/versions/$installed/manifest.json" ]; then
    install -m 0644 -o root -g root "$dir/manifests/$installed.json" "$OPT/versions/$installed/manifest.json"
    say "placed the manifest of $installed"
  fi
  check "keel-privileged.service is active" systemctl is-active --quiet keel-privileged.service
  check "keel-update.path is active" systemctl is-active --quiet keel-update.path
  check "keel.service is active" systemctl is-active --quiet keel.service
  check "no process other than the privileged code path runs the binary as root" test -z "$(ps -eo user:20,args | awk -v bin="$OPT/versions/$installed/keel" '$1 == "root" && index($0, bin) && !index($0, "privileged")')"
  mkdir -p "$DROP_IN_DIR"
  cat >"$DROP_IN" <<EOF
[Service]
ExecStart=
ExecStart=$OPT/bin/current privileged update --from-request --channel-url file://$dir/channel --release-key $dir/keys/release.pub
EOF
  systemctl daemon-reload
  say "wrote $DROP_IN pointing the oneshot at the rehearsal channel"
  runuser -u keel -- sh -c "printf '%s' '{\"version\":\"$version\",\"channel\":\"stable\",\"requested_by\":\"update-vm\"}' > $REQUEST.tmp && mv $REQUEST.tmp $REQUEST"
  say "wrote $REQUEST as keel"
  check "keel-update.path started the update and bin/current switched to $version" wait_current "$OPT/versions/$version/keel"
  check "the request was consumed" test ! -e "$REQUEST"
  check "keel.service is ready after the update" wait_ready "$listen"
  check "keel-privileged.service is active after the update" systemctl is-active --quiet keel-privileged.service
  check "bin/previous points at $installed" test "$(readlink "$OPT/bin/previous")" = "$before"
  check "the unit files were refreshed from the release" cmp -s "$OPT/versions/$version/deploy/systemd/keel.service" /etc/systemd/system/keel.service
  say "--- journal of keel-update.service"
  journalctl -u keel-update.service --no-pager -n 30 -o cat || true
  say "--- rollback"
  check "keel privileged update rollback exits 0" privileged_update "$dir" rollback
  check "bin/current points at $installed again" test "$(readlink "$OPT/bin/current")" = "$before"
  check "keel.service is ready after the rollback" wait_ready "$listen"
  check "a second rollback returns to $version" privileged_update "$dir" rollback
  check "bin/current points at $version" test "$(readlink "$OPT/bin/current")" = "$OPT/versions/$version/keel"
  check "keel.service is ready again" wait_ready "$listen"
  check "rollback to $installed for the cleanup exits 0" privileged_update "$dir" rollback
  check "keel.service is ready on $installed" wait_ready "$listen"
  rm -f "$DROP_IN"
  systemctl daemon-reload
  say "removed $DROP_IN"
  if [ "$failures" -gt 0 ]; then
    fail "$failures check(s) failed; run cleanup $dir once the machine is looked at"
  fi
  say "the update path is proven on $(hostname); run \"$0 cleanup $dir\" to remove the rehearsal version"
}

boot_version() {
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
  curl -s --connect-timeout 5 --max-time 10 --cacert "$CERT" "https://$host:$port/api/v1/boot" 2>/dev/null \
    | python3 -c 'import json, sys; print(json.load(sys.stdin)["keel"]["version"])' 2>/dev/null || true
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

as_keel_to() {
  local out=$1
  shift
  runuser -u keel -- "$OPT/bin/current" "$@" > "$out"
}

start_channel_server() {
  local dir=$1
  if [ -f "$dir/channel.pid" ] && kill -0 "$(cat "$dir/channel.pid")" 2>/dev/null; then
    return 0
  fi
  nohup python3 -m http.server "$CHANNEL_PORT" --bind 127.0.0.1 --directory "$dir/channel" >"$dir/channel.log" 2>&1 &
  printf '%s\n' "$!" > "$dir/channel.pid"
  local waited=0
  while [ "$waited" -lt 30 ]; do
    if curl -sf -o /dev/null "http://127.0.0.1:$CHANNEL_PORT/stable/manifest.json"; then
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  return 1
}

stop_channel_server() {
  local dir=$1
  if [ -f "$dir/channel.pid" ]; then
    kill "$(cat "$dir/channel.pid")" 2>/dev/null || true
    rm -f "$dir/channel.pid"
  fi
}

point_kernel_at_channel() {
  local edited
  [ -f "$CONFIG_BACKUP" ] || cp -p "$CONFIG" "$CONFIG_BACKUP"
  edited=$(mktemp)
  awk '/^update:/ { skip = 1; next } skip && (/^[[:space:]]/ || /^$/) { next } { skip = 0; print }' "$CONFIG" >"$edited"
  printf 'update: { channel_url: http://127.0.0.1:%s/, insecure_rehearsal_channel: true }\n' "$CHANNEL_PORT" >>"$edited"
  cat "$edited" >"$CONFIG"
  rm -f "$edited"
  systemctl restart keel.service
  say "pointed the update section of $CONFIG at the rehearsal channel and restarted keel.service"
}

restore_config() {
  if [ -f "$CONFIG_BACKUP" ]; then
    mv "$CONFIG_BACKUP" "$CONFIG"
    systemctl restart keel.service || true
    say "restored $CONFIG and restarted keel.service"
  fi
}

kernel() {
  local dir=${1:-}
  shift || true
  local listen=127.0.0.1:1500
  while [ $# -gt 0 ]; do
    case $1 in
      --listen) listen=$2; shift 2 ;;
      *) usage >&2; fail "unknown argument: $1" ;;
    esac
  done
  [ -n "$dir" ] || { usage >&2; fail "kernel needs the prepared directory"; }
  [ "$(id -u)" -eq 0 ] || fail "run as root"
  [ -d /run/systemd/system ] || fail "systemd is not the running init"
  [ -L "$OPT/bin/current" ] || fail "$OPT/bin/current is missing; install keel first"
  local installed version
  installed=$(cat "$dir/installed")
  version=$(cat "$dir/version")
  local before
  before=$(readlink "$OPT/bin/current")
  say "installed $installed at $before; the kernel requests $version from http://127.0.0.1:$CHANNEL_PORT/"
  if [ ! -f "$OPT/versions/$installed/manifest.json" ]; then
    install -m 0644 -o root -g root "$dir/manifests/$installed.json" "$OPT/versions/$installed/manifest.json"
    say "placed the manifest of $installed"
  fi
  check "the rehearsal channel answers on 127.0.0.1:$CHANNEL_PORT" start_channel_server "$dir"
  point_kernel_at_channel
  check "keel.service is ready with the update section" wait_ready "$listen"
  mkdir -p "$DROP_IN_DIR"
  cat >"$DROP_IN" <<EOF
[Service]
ExecStart=
ExecStart=$OPT/bin/current privileged update --from-request --channel-url http://127.0.0.1:$CHANNEL_PORT/ --insecure-rehearsal-channel --release-key $dir/keys/release.pub
EOF
  systemctl daemon-reload
  say "wrote $DROP_IN pointing the oneshot at the rehearsal channel"
  check "keel update check as keel exits 0" as_keel_to "$dir/check.json" update check --out json
  check "update check sees $version available over $installed and allowed" json_is "$dir/check.json" "doc['ok'] is True and doc['data']['running'] == '$installed' and doc['data']['available'] == '$version' and doc['data']['allowed'] is True and doc['data']['runningContract'] == 1"
  check "keel update apply --wait as keel exits 0" as_keel_to "$dir/apply.json" update apply --wait --out json
  check "the apply task ended done with the request written" json_is "$dir/apply.json" "doc['ok'] is True and doc['data']['state'] == 'done' and doc['data']['result']['to'] == '$version' and doc['data']['result']['requestedBy'] == 'owner'"
  check "the pre-update dump lies under $LIB/backups" test -n "$(find "$LIB/backups" -maxdepth 1 -name "pre-update-$installed-*" -type d 2>/dev/null)"
  check "keel-update.path started the update and bin/current switched to $version" wait_current "$OPT/versions/$version/keel"
  check "the request was consumed" test ! -e "$REQUEST"
  check "keel.service is ready after the update" wait_ready "$listen"
  check "/api/v1/boot answers $version" test "$(boot_version "$listen")" = "$version"
  check "bin/previous points at $installed" test "$(readlink "$OPT/bin/previous")" = "$before"
  say "--- journal of keel-update.service"
  journalctl -u keel-update.service --no-pager -n 30 -o cat || true
  say "--- rollback through the privileged command line"
  check "keel privileged update rollback exits 0" privileged_update "$dir" rollback
  check "keel.service is ready after the rollback" wait_ready "$listen"
  check "/api/v1/boot answers $installed again" test "$(boot_version "$listen")" = "$installed"
  rm -f "$DROP_IN"
  systemctl daemon-reload
  say "removed $DROP_IN"
  if [ "$failures" -gt 0 ]; then
    fail "$failures check(s) failed; run cleanup $dir once the machine is looked at"
  fi
  say "the kernel-side update is proven on $(hostname); run \"$0 cleanup $dir\" to remove the rehearsal version, the channel server and the config change"
}

cleanup() {
  local dir=${1:-}
  [ -n "$dir" ] || { usage >&2; fail "cleanup needs the prepared directory"; }
  [ "$(id -u)" -eq 0 ] || fail "run as root"
  local version installed
  version=$(cat "$dir/version")
  installed=$(cat "$dir/installed")
  if [ "$(readlink "$OPT/bin/current")" = "$OPT/versions/$version/keel" ]; then
    privileged_update "$dir" rollback
  fi
  rm -rf "$OPT/versions/$version"
  rm -f "$DROP_IN" "$OPT/bin/previous"
  systemctl daemon-reload
  stop_channel_server "$dir"
  restore_config
  say "removed the rehearsal version $version and the drop-in; $installed is current"
}

case ${1:-} in
  prepare) shift; prepare "$@" ;;
  run) shift; run "$@" ;;
  kernel) shift; kernel "$@" ;;
  cleanup) shift; cleanup "$@" ;;
  -h|--help|"") usage; [ -n "${1:-}" ] || exit 2; exit 0 ;;
  *) usage >&2; fail "unknown command: $1" ;;
esac
