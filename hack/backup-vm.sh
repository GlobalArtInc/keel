#!/usr/bin/env bash
set -euo pipefail

ETC=/etc/keel
LIB=/var/lib/keel
CONFIG=$ETC/config.yaml
CERT=$LIB/tls/cert.pem
KEEL=/opt/keel/bin/current
WAIT=${KEEL_BACKUP_VM_WAIT:-120}

usage() {
  cat <<EOF
usage: hack/backup-vm.sh take <out archive>
       hack/backup-vm.sh restore <archive>

Proves backup and restore on virtual machines that carry an installation made
by deploy/install.sh, run as root with KEEL_BACKUP_PASSPHRASE in the
environment. "take" runs on the first machine while the service runs: it takes
the backup as the user keel with "keel backup --with-secret-key", lists the
manifest with "keel backup inspect" and copies the archive to <out archive>
for the transfer. "restore" runs on the second machine, installed with the
same product: it stops keel.service, restores the archive as the user keel
with "keel restore --with-secret-key", copies what the restore could not write
into $ETC (agent.yaml and the template overrides, root's files) from the
tree the restore left under $LIB/backups, keeps this machine's config.yaml,
starts the service, waits for /readyz and checks that the journal names the
restore and the enabled set it changed. The second factor of a user enrolled
on the first machine is then checked by hand: log in on the second machine
with the same authenticator.
EOF
}

say() {
  printf '%s\n' "$*"
}

fail() {
  say "backup-vm.sh: $1" >&2
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
  runuser -u keel -- env KEEL_BACKUP_PASSPHRASE="$KEEL_BACKUP_PASSPHRASE" "$KEEL" --config "$CONFIG" "$@"
}

inspect_into() {
  local archive=$1 out=$2
  as_keel backup inspect "$archive" > "$out"
}

need_root() {
  [ "$(id -u)" -eq 0 ] || fail "run as root"
  [ -n "${KEEL_BACKUP_PASSPHRASE:-}" ] || fail "KEEL_BACKUP_PASSPHRASE is not set"
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary"
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

take() {
  local out=${1:-}
  [ -n "$out" ] || { usage >&2; fail "take needs the output path"; }
  need_root
  systemctl is-active --quiet keel.service || fail "keel.service is not running; take the backup on a running installation"
  local archive
  archive="$LIB/backups/rehearsal-$(date -u +%Y%m%dT%H%M%SZ).tar.zst"
  check "keel backup --with-secret-key as the user keel" as_keel backup --out "$archive" --with-secret-key
  check "the archive is owned by keel with mode 0600" test "$(stat -c '%U %a' "$archive")" = "keel 600"
  local manifest
  manifest=$(mktemp)
  check "keel backup inspect prints the manifest" inspect_into "$archive" "$manifest"
  check "the manifest carries the sealed secret key" grep -q '"secretKey": true' "$manifest"
  rm -f "$manifest"
  install -m 0600 "$archive" "$out"
  say "the archive is at $out; copy it to the second machine and run: KEEL_BACKUP_PASSPHRASE=... hack/backup-vm.sh restore <archive>"
  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
}

restore() {
  local archive=${1:-}
  [ -n "$archive" ] || { usage >&2; fail "restore needs the archive"; }
  need_root
  [ -f "$archive" ] || fail "$archive is not a file"
  local staged
  staged="$LIB/backups/$(basename "$archive")"
  if [ "$archive" != "$staged" ]; then
    install -o keel -g keel -m 0600 "$archive" "$staged"
  fi
  systemctl stop keel.service
  check "keel.service is stopped" bash -c '! systemctl is-active --quiet keel.service'
  check "keel restore --with-secret-key as the user keel" as_keel restore "$staged" --with-secret-key
  local stem=${staged%.tar.zst}
  local fallback="$stem.etc"
  if [ -d "$fallback" ]; then
    say "the restore left the archive's $ETC tree at $fallback; copying root's files"
    if [ -f "$fallback/agent.yaml" ]; then
      install -m 0640 -o root -g keel "$fallback/agent.yaml" "$ETC/agent.yaml"
    fi
    if [ -d "$fallback/templates" ]; then
      mkdir -p "$ETC/templates"
      cp -a "$fallback/templates/." "$ETC/templates/"
      chown -R root:keel "$ETC/templates"
    fi
    if [ -f "$fallback/config.yaml" ]; then
      install -m 0640 -o root -g keel "$fallback/config.yaml" "$CONFIG.restored"
      say "the archive's config.yaml is at $CONFIG.restored; this machine's $CONFIG is kept"
    fi
  fi
  check "secret.key is owned by keel with mode 0600" test "$(stat -c '%U %a' "$LIB/secret.key")" = "keel 600"
  check "the restore note is written for the next start" test -f "$LIB/restore.note"
  local since
  since=$(date -u '+%Y-%m-%d %H:%M:%S')
  systemctl start keel.service
  check "/readyz answers within ${WAIT}s" wait_ready
  check "the journal names the restore and the enabled set" bash -c "journalctl -u keel.service --since '$since' --no-pager | grep -q 'restored from a backup'"
  check "the restore note is consumed" test ! -e "$LIB/restore.note"
  check "keel product list as keel answers the restored bundles" as_keel product list
  say "log in on this machine with a user enrolled on the first machine and confirm the same authenticator code is accepted"
  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
}

case ${1:-} in
  take) shift; take "$@" ;;
  restore) shift; restore "$@" ;;
  -h|--help) usage ;;
  *) usage >&2; fail "unknown stage: ${1:-}" ;;
esac
