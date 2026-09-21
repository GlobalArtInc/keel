#!/usr/bin/env bash
set -Eeuo pipefail

OPT=/opt/keel
ETC=/etc/keel
LIB=/var/lib/keel
RUN=/run/keel
UNIT_DIR=/etc/systemd/system
UNITS=(keel.service keel-privileged.service keel-update.path keel-update.service)
SOCKETS=("$RUN/agent.sock" "$RUN/core.sock")
LINK=/usr/local/bin/keel
PG_SOCKET_DIR=/run/postgresql

purge=0
keep_backups=0
yes=0
step="parsing the arguments"

usage() {
  cat <<EOF
usage: uninstall.sh [--purge [--keep-backups]] [--yes]

Without --purge: stops and disables keel.service, keel-privileged.service,
keel-update.path and keel-update.service, removes the units, the sockets
$RUN/agent.sock and $RUN/core.sock they left behind, $LINK and $OPT.
The configuration in $ETC (config.yaml and the host agent policy agent.yaml),
the data in $LIB, the unix user keel and the Postgres database keel stay in
place.

With --purge: also removes $ETC, $LIB, $RUN, the unix user and group keel,
and drops the database keel and the role keel from the Postgres server on
$PG_SOCKET_DIR. The Postgres server itself is left installed. The backup
archives under $LIB/backups go with $LIB; take one with "keel backup
--with-secret-key" and copy it elsewhere first, or pass --keep-backups to
leave $LIB/backups in place.

  --purge         remove the data as well
  --keep-backups  with --purge, keep $LIB/backups and remove the rest of $LIB
  --yes           do not ask for confirmation
  -h, --help      print this text

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
      --keep-backups) keep_backups=1; shift ;;
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
    say "  ${UNITS[*]}, $LINK, $OPT, $ETC, $LIB, $RUN,"
    say "  the unix user and group keel, the database keel and the role keel."
    if [ "$keep_backups" -eq 1 ]; then
      say "  $LIB/backups is kept."
    else
      say "  The backup archives under $LIB/backups go with it."
    fi
  else
    say "This removes the keel units and binary from $(hostname):"
    say "  ${UNITS[*]}, $LINK, $OPT."
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
  begin "stopping and disabling ${UNITS[*]}"
  local unit
  if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    for unit in "${UNITS[@]}"; do
      if systemctl list-unit-files "$unit" --no-legend 2>/dev/null | grep -q "^$unit"; then
        systemctl stop "$unit" || true
        systemctl disable "$unit" 2>/dev/null || true
      else
        say "$unit is not installed"
      fi
    done
  else
    say "systemd is not running; the unit files are removed without stopping anything"
  fi
  for unit in "${UNITS[@]}"; do
    if [ -f "$UNIT_DIR/$unit" ]; then
      rm -f "$UNIT_DIR/$unit"
      say "removed $UNIT_DIR/$unit"
    fi
  done
  if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload || true
  fi
  local socket
  for socket in "${SOCKETS[@]}"; do
    if [ -S "$socket" ] || [ -e "$socket" ]; then
      rm -f "$socket"
      say "removed $socket"
    fi
  done
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
  local path entry
  for path in "$ETC" "$RUN"; do
    if [ -e "$path" ]; then
      rm -rf "$path"
      say "removed $path"
    fi
  done
  if [ ! -e "$LIB" ]; then
    return
  fi
  if [ "$keep_backups" -eq 1 ] && [ -d "$LIB/backups" ]; then
    for entry in "$LIB"/* "$LIB"/.[!.]*; do
      if [ -e "$entry" ] && [ "$entry" != "$LIB/backups" ]; then
        rm -rf "$entry"
      fi
    done
    say "removed $LIB except $LIB/backups"
  else
    rm -rf "$LIB"
    say "removed $LIB"
  fi
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
  if [ "$purge" -eq 1 ] && [ "$keep_backups" -eq 1 ]; then
    say "keel is removed from $(hostname) together with its data; the backup archives stay under $LIB/backups. The Postgres server stays installed."
  elif [ "$purge" -eq 1 ]; then
    say "keel is removed from $(hostname) together with its data. The Postgres server stays installed."
  else
    say "keel is removed from $(hostname). Kept: $ETC, $LIB, the unix user keel and the database keel."
    say "Run uninstall.sh --purge to remove them as well."
  fi
}

main() {
  parse_args "$@"
  trap on_error ERR
  if [ "$keep_backups" -eq 1 ] && [ "$purge" -eq 0 ]; then
    fail "--keep-backups goes with --purge; without --purge the data stays in place anyway"
  fi
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
