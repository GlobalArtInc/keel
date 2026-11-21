#!/usr/bin/env bash
set -euo pipefail

ETC=/etc/keel
LIB=/var/lib/keel
CONFIG=$ETC/config.yaml
CERT=$LIB/tls/cert.pem
KEEL=/opt/keel/bin/current
UNIT=keel.service
EXIT_STATUS=75
WAIT=${KEEL_PRODUCT_VM_WAIT:-120}

usage() {
  cat <<EOF
usage: hack/product-vm.sh <product id> [<function of the product>]

Proves the product switch on a running installation made by deploy/install.sh,
run as root on the virtual machine with the product enabled. As the user keel
it runs "keel product disable <id>" over the control socket, waits for
$UNIT to exit with status $EXIT_STATUS and to be started again by the service
manager, checks the pre-change dump under $LIB/backups, that the product is
disabled and its function gone while its tables stay orphaned in the doctor's
report, then runs "keel product enable <id>" the same way and checks that the
function answers again and the doctor lists no orphan. The function defaults
to note.list, the demo bundle's list; pass the list function of another bundle.
EOF
}

say() {
  printf '%s\n' "$*"
}

fail() {
  say "product-vm.sh: $1" >&2
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

as_keel() {
  runuser -u keel -- "$KEEL" --config "$CONFIG" "$@"
}

need_root() {
  [ "$(id -u)" -eq 0 ] || fail "run as root"
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running; the switch on a running installation needs the service"
}

listen_of() {
  local listen
  listen=$(sed -nE 's/^listen:[[:space:]]*"?([^"]+)"?[[:space:]]*$/\1/p' "$CONFIG" | head -n 1)
  printf '%s\n' "${listen:-127.0.0.1:1500}"
}

ready() {
  local listen
  listen=$(listen_of)
  local host=${listen%:*}
  local port=${listen##*:}
  [ "$host" = "0.0.0.0" ] && host=127.0.0.1
  curl -sf --cacert "$CERT" "https://$host:$port/readyz" >/dev/null 2>&1
}

wait_ready() {
  local waited=0
  while [ "$waited" -lt "$WAIT" ]; do
    if ready; then
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  return 1
}

restarts() {
  systemctl show -p NRestarts --value "$UNIT"
}

main_pid() {
  systemctl show -p MainPID --value "$UNIT"
}

wait_restarted() {
  local before=$1 pid=$2
  local waited=0
  while [ "$waited" -lt "$WAIT" ]; do
    if [ "$(restarts)" -gt "$before" ] && [ "$(main_pid)" != "$pid" ] && [ "$(main_pid)" != "0" ]; then
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  return 1
}

journal_has() {
  local since=$1 text=$2 lines
  for _ in $(seq 1 15); do
    lines=$(journalctl -u "$UNIT" --since "$since" --no-pager)
    if grep -qF -- "$text" <<<"$lines"; then
      return 0
    fi
    sleep 1
  done
  return 1
}

product_state() {
  local id=$1
  as_keel ctl keel.product.list --out json | python3 -c '
import json, sys
doc = json.load(sys.stdin)
rows = [row for row in doc["data"]["data"] if row["id"] == sys.argv[1]]
print(rows[0]["state"] if rows else "absent")
' "$id"
}

function_answers() {
  as_keel ctl "$1" --out json >/dev/null 2>&1
}

function_gone() {
  ! function_answers "$1"
}

doctor_line() {
  (as_keel doctor 2>/dev/null || true) | grep -E "^[a-z]+ +$1 " || true
}

doctor_status() {
  local id=$1 status=$2
  doctor_line "$id" | grep -qE "^$status "
}

switch() {
  local action=$1 id=$2 expected=$3
  local before pid since out
  before=$(restarts)
  pid=$(main_pid)
  since=$(date '+%Y-%m-%d %H:%M:%S')
  out=$(mktemp)
  if as_keel product "$action" "$id" >"$out" 2>&1; then
    say "ok: keel product $action $id as the user keel"
  else
    say "FAILED: keel product $action $id as the user keel"
    failures=$((failures + 1))
  fi
  say "  $(cat "$out")"
  check "the command names the restart" grep -q "^$id: $expected; the service restarts" "$out"
  rm -f "$out"
  check "$UNIT exits with status $EXIT_STATUS and is started again" wait_restarted "$before" "$pid"
  check "the service manager scheduled the restart" journal_has "$since" "Scheduled restart job"
  check "the kernel logged the requested restart" journal_has "$since" "restart requested"
  check "the exit counts as a clean one" test "$(systemctl show -p Result --value "$UNIT")" = "success"
  check "/readyz answers within ${WAIT}s after the restart" wait_ready
}

main() {
  local id=${1:-}
  local function=${2:-note.list}
  [ -n "$id" ] || { usage >&2; fail "the product id is required"; }
  need_root
  [ "$(product_state "$id")" = "enabled" ] || fail "the product $id is not enabled on this installation"
  check "$function answers before the switch" function_answers "$function"
  local dumps_before
  dumps_before=$(find "$LIB/backups" -maxdepth 1 -type d -name 'pre-restart-*' 2>/dev/null | wc -l)

  switch disable "$id" disabled
  check "the product is disabled" test "$(product_state "$id")" = "disabled"
  check "$function is gone while the product is disabled" function_gone "$function"
  local dump
  dump=$(find "$LIB/backups" -maxdepth 1 -type d -name 'pre-restart-*' | sort | tail -n 1)
  check "a pre-restart dump was written" test "$(find "$LIB/backups" -maxdepth 1 -type d -name 'pre-restart-*' | wc -l)" -gt "$dumps_before"
  check "the dump manifest names the disable" grep -q "\"reason\": \"product disable $id\"" "$dump/manifest.json"
  check "the dump is owned by keel" test "$(stat -c '%U' "$dump")" = "keel"
  check "the doctor lists the orphaned modules" doctor_status orphans warn
  say "doctor: $(doctor_line orphans)"

  switch enable "$id" enabled
  check "the product is enabled again" test "$(product_state "$id")" = "enabled"
  check "$function answers again" function_answers "$function"
  check "the doctor lists no orphan" doctor_status orphans ok
  check "the doctor's migrations check is ok" doctor_status migrations ok
  say "doctor: $(doctor_line migrations)"
  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "the product switch is proven on this machine"
}

case ${1:-} in
  -h|--help) usage ;;
  *) main "$@" ;;
esac
