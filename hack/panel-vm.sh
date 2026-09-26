#!/usr/bin/env bash
set -euo pipefail

ETC=/etc/keel
CONFIG=$ETC/config.yaml
POLICY=$ETC/agent.yaml
KEEL=/opt/keel/bin/current
UNIT=keel.service
AGENT_UNIT=keel-privileged.service
WWW=/var/www
WAIT=${KEEL_PANEL_VM_WAIT:-60}
STAMP=$(date +%s)
NAME=vm$STAMP
USER_NAME=hp_$NAME
HOME_DIR=$WWW/keel-panel-$NAME
NOLOGIN=/usr/sbin/nologin
LOGIN_SHELL=/bin/bash

usage() {
  cat <<EOF
usage: hack/panel-vm.sh install <keel binary or tar.zst>
       hack/panel-vm.sh run

Proves the hosting users of the panel bundle on a virtual machine, run as
root from a checkout of the repository.

"install" prepares $WWW, then runs deploy/install.sh --product panel
--binary <archive> --yes and prints the setup link.

"run" proves the running installation over the control socket as the user
keel. It creates a user account and in it the hosting user $NAME, waits for
it to turn active and finds the operating-system account $USER_NAME with a
uid inside the uid_range of $POLICY, the home $HOME_DIR owned by it and no
login shell. It grants the shell and takes it back, suspends the account
and finds $USER_NAME locked and expired, resumes it and finds it open
again, then deletes the account and finds $USER_NAME and its home gone,
with every change in the journal of $AGENT_UNIT.
EOF
}

say() {
  printf '%s\n' "$*"
}

fail() {
  say "panel-vm.sh: $1" >&2
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

install_keel() {
  local archive=${1:-}
  [ -n "$archive" ] || { usage >&2; fail "the archive is required"; }
  [ -f "$archive" ] || fail "$archive is not a file"
  need_root
  cd "$(dirname "$0")/.."
  [ -f deploy/install.sh ] || fail "deploy/install.sh is missing; run from a checkout of the repository"
  install -d -m 0755 -o root -g root "$WWW"
  local log
  log=$(mktemp)
  if ! bash deploy/install.sh --product panel --binary "$archive" --yes 2>&1 | tee "$log"; then
    fail "deploy/install.sh failed; its output is in $log"
  fi
  check "$UNIT is active" systemctl is-active --quiet "$UNIT"
  check "$AGENT_UNIT is active" systemctl is-active --quiet "$AGENT_UNIT"
  check "the installer printed the setup link" grep -qF "setup link:   https://" "$log"
  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "keel is installed with panel; run: hack/panel-vm.sh run"
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

range_bound() {
  sed -nE "s/^  uid_range: \{ from: ([0-9]+), to: ([0-9]+) \}$/\\$1/p" "$POLICY"
}

passwd_field() {
  getent passwd "$USER_NAME" | cut -d: -f"$1"
}

shadow_field() {
  getent shadow "$USER_NAME" | cut -d: -f"$1"
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

state_is() {
  [ "$(call hosting.user.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "$2" ]
}

inside_range() {
  local uid from to
  uid=$(passwd_field 3)
  from=$(range_bound 1)
  to=$(range_bound 2)
  say "  $USER_NAME has uid $uid; the range is $from to $to"
  [ -n "$uid" ] && [ -n "$from" ] && [ -n "$to" ] && [ "$uid" -ge "$from" ] && [ "$uid" -le "$to" ]
}

home_owned() {
  [ "$(passwd_field 6)" = "$HOME_DIR" ] && [ -d "$HOME_DIR" ] && [ "$(stat -c %u "$HOME_DIR")" = "$(passwd_field 3)" ]
}

shell_is() {
  [ "$(passwd_field 7)" = "$1" ]
}

locked() {
  [ "$(shadow_field 8)" = "1" ] && [ "$(shadow_field 2 | cut -c1)" = "!" ]
}

open_again() {
  [ -z "$(shadow_field 8)" ]
}

gone() {
  ! getent passwd "$USER_NAME" >/dev/null && [ ! -e "$HOME_DIR" ]
}

agent_logged() {
  journalctl -u "$AGENT_UNIT" --since "$1" --no-pager | grep -qF -- "$2"
}

task_done() {
  [ "$(call task.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "done" ]
}

run_proof() {
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  [ -d "$WWW" ] || fail "$WWW is missing; run install first"
  local since account id task
  since=$(date -u '+%Y-%m-%d %H:%M:%S')

  say "--- a hosting user"
  account=$(call keel.account.create kind=user | field data.id) || fail "keel ctl keel.account.create failed"
  say "  the account $account"
  id=$(call hosting.user.create "name=$NAME" "account=$account" | field data.id) || fail "keel ctl hosting.user.create failed"
  check "the hosting user $NAME turns active" eventually state_is "$id" active
  check "$USER_NAME is an account of the host" getent passwd "$USER_NAME"
  check "the uid of $USER_NAME is inside the uid range of the policy" inside_range
  check "the home $HOME_DIR belongs to $USER_NAME" home_owned
  check "$USER_NAME has no login shell" shell_is "$NOLOGIN"

  say "--- the shell"
  check "keel ctl hosting.user.edit grants the shell" call hosting.user.edit "id=$id" shell=true
  check "$USER_NAME gets $LOGIN_SHELL" eventually shell_is "$LOGIN_SHELL"
  check "keel ctl hosting.user.edit takes the shell back" call hosting.user.edit "id=$id" shell=false
  check "$USER_NAME is back to $NOLOGIN" eventually shell_is "$NOLOGIN"

  say "--- suspend and resume"
  check "keel ctl keel.account.suspend suspends $account" call keel.account.suspend "id=$account"
  check "the operating-system account is locked and expired while its account is suspended" eventually locked
  check "the hosting user is suspended" state_is "$id" suspended
  check "keel ctl keel.account.resume resumes $account" call keel.account.resume "id=$account"
  check "$USER_NAME no longer expires once its account is resumed" eventually open_again
  check "the hosting user is active again" state_is "$id" active

  say "--- the account deleted"
  task=$(call keel.account.delete "id=$account" | field data.task.id) || task=""
  check "keel ctl keel.account.delete queues a task" test -n "$task"
  check "the deletion task is done" eventually task_done "$task"
  check "$USER_NAME and its home are gone" eventually gone
  check "the agent journal carries host.user.create" agent_logged "$since" host.user.create
  check "the agent journal carries host.user.modify" agent_logged "$since" host.user.modify
  check "the agent journal carries host.user.delete" agent_logged "$since" host.user.delete

  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "panel's hosting users are proven on this machine: an account inside the range with its home, the shell, the lock of a suspended account and the removal with the account"
}

case ${1:-} in
  -h | --help) usage ;;
  install) shift; install_keel "$@" ;;
  run) shift; run_proof "$@" ;;
  *) usage >&2; exit 2 ;;
esac
