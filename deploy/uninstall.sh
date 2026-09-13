#!/usr/bin/env bash
set -Eeuo pipefail

OPT=/opt/keel
ETC=/etc/keel
LIB=/var/lib/keel
RUN=/run/keel
UNIT=/etc/systemd/system/keel.service
LINK=/usr/local/bin/keel
PG_SOCKET_DIR=/run/postgresql

purge=0
yes=0
step="parsing the arguments"

usage() {
  cat <<EOF
usage: uninstall.sh [--purge] [--yes]

Without --purge: stops and disables keel.service, removes the unit, $LINK
and $OPT. The configuration in $ETC, the data in $LIB, the unix user
keel and the Postgres database keel stay in place.

With --purge: also removes $ETC, $LIB, $RUN, the unix user and group keel,
and drops the database keel and the role keel from the Postgres server on
$PG_SOCKET_DIR. The Postgres server itself is left installed.

  --purge     remove the data as well
  --yes       do not ask for confirmation
  -h, --help  print this text

Run as root.
EOF
}

say() {
  printf '%s\n' "$*"
}

begin() {
  step=$1
  say "==> $step"
}

fail() {
  say "uninstall.sh: $1" >&2
  exit 1
}

on_error() {
  local code=$?
  trap - ERR
  if [ "$BASH_SUBSHELL" -eq 0 ]; then
    say "uninstall.sh: failed during: $step (exit $code)" >&2
  fi
  exit "$code"
}

parse_args() {
  while [ $# -gt 0 ]; do
    case $1 in
      --purge) purge=1; shift ;;
      --yes|-y) yes=1; shift ;;
      -h|--help) usage; exit 0 ;;
      *) usage >&2; fail "unknown argument: $1" ;;
    esac
  done
}

confirm() {
  say ""
  if [ "$purge" -eq 1 ]; then
    say "This removes keel from $(hostname) together with its data:"
    say "  keel.service, $LINK, $OPT, $ETC, $LIB, $RUN,"
    say "  the unix user and group keel, the database keel and the role keel."
  else
    say "This removes the keel service and binary from $(hostname):"
    say "  keel.service, $LINK, $OPT."
    say "  $ETC, $LIB, the unix user keel and the database keel stay in place."
  fi
  say ""
  if [ "$yes" -eq 1 ]; then
    return
  fi
  if [ ! -t 0 ]; then
    fail "stdin is not a terminal; pass --yes to confirm"
  fi
  local answer
  read -r -p 'Type "yes" to continue: ' answer
  if [ "$answer" != "yes" ]; then
    fail "cancelled"
  fi
}

remove_service() {
  begin "stopping and disabling keel.service"
  if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    if systemctl list-unit-files keel.service --no-legend 2>/dev/null | grep -q '^keel.service'; then
      systemctl disable --now keel.service || true
    else
      say "keel.service is not installed"
    fi
  else
    say "systemd is not running; the unit file is removed without stopping anything"
  fi
  if [ -f "$UNIT" ]; then
    rm -f "$UNIT"
    say "removed $UNIT"
  fi
  if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload || true
  fi
}

remove_binary() {
  begin "removing $LINK and $OPT"
  if [ -L "$LINK" ]; then
    rm -f "$LINK"
    say "removed $LINK"
  fi
  if [ -e "$OPT" ]; then
    rm -rf "$OPT"
    say "removed $OPT"
  fi
}

pg_socket_port() {
  local socket
  for socket in "$PG_SOCKET_DIR"/.s.PGSQL.*; do
    if [ -S "$socket" ]; then
      printf '%s\n' "${socket##*.}"
      return 0
    fi
  done
  return 1
}

pg_sql() {
  (cd / && runuser -u postgres -- env -i PATH=/usr/bin:/bin HOME=/var/lib/postgresql psql -X -U postgres -h "$PG_SOCKET_DIR" -p "$pg_port" -d postgres -qAt -v ON_ERROR_STOP=1 -c "$1")
}

drop_database() {
  begin "dropping the database keel and the role keel"
  if ! pg_port=$(pg_socket_port); then
    say "no Postgres server answers on $PG_SOCKET_DIR; the database keel and the role keel are not dropped"
    return
  fi
  if ! getent passwd postgres >/dev/null || ! command -v psql >/dev/null 2>&1; then
    say "no postgres user or psql on this host; the database keel and the role keel are not dropped"
    return
  fi
  if [ "$(pg_sql "SELECT 1 FROM pg_database WHERE datname = 'keel'")" = "1" ]; then
    pg_sql "DROP DATABASE keel WITH (FORCE)" >/dev/null
    say "dropped the database keel"
  else
    say "the database keel does not exist"
  fi
  if [ "$(pg_sql "SELECT 1 FROM pg_roles WHERE rolname = 'keel'")" = "1" ]; then
    pg_sql "DROP ROLE keel" >/dev/null
    say "dropped the role keel"
  else
    say "the role keel does not exist"
  fi
}

remove_data() {
  begin "removing $ETC, $LIB and $RUN"
  local path
  for path in "$ETC" "$LIB" "$RUN"; do
    if [ -e "$path" ]; then
      rm -rf "$path"
      say "removed $path"
    fi
  done
}

remove_user() {
  begin "removing the unix user and group keel"
  if getent passwd keel >/dev/null; then
    userdel keel
    say "removed the user keel"
  else
    say "the user keel does not exist"
  fi
  if getent group keel >/dev/null; then
    groupdel keel
    say "removed the group keel"
  fi
}

report() {
  say ""
  if [ "$purge" -eq 1 ]; then
    say "keel is removed from $(hostname) together with its data. The Postgres server stays installed."
  else
    say "keel is removed from $(hostname). Kept: $ETC, $LIB, the unix user keel and the database keel."
    say "Run uninstall.sh --purge to remove them as well."
  fi
}

main() {
  parse_args "$@"
  trap on_error ERR
  if [ "$(id -u)" -ne 0 ]; then
    fail "run as root"
  fi
  confirm
  remove_service
  remove_binary
  if [ "$purge" -eq 1 ]; then
    drop_database
    remove_data
    remove_user
  fi
  report
}

pg_port=5432
main "$@"
