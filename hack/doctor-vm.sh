#!/usr/bin/env bash
set -euo pipefail

ETC=/etc/keel
LIB=/var/lib/keel
CONFIG=$ETC/config.yaml
KEEL=/opt/keel/bin/current
OUT=${1:-/tmp/keel-doctor-vm}

usage() {
  cat <<USAGE
usage: hack/doctor-vm.sh [out directory]

Proves the doctor and the support bundle on a virtual machine that carries an
installation made by deploy/install.sh, run as root. It runs "keel doctor" as
the user keel against the real units, directories and database and expects
exit 0 with every line ok; runs it once more after chmod 0644 on secret.key
and expects exit 1 naming the key, then restores the mode; runs "keel
support-bundle" as keel, unpacks the archive under the out directory and
checks that neither the database password of config.yaml nor any token or
password appears in it and that the log ring, the ledgers, the audit tail and
the readiness answer are there. The transcript is what the exit checklist
records.
USAGE
}

say() {
  printf '%s\n' "$*"
}

fail() {
  say "doctor-vm.sh: $1" >&2
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

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
esac

[ "$(id -u)" -eq 0 ] || fail "run as root"
[ -x "$KEEL" ] || fail "$KEEL is not the installed binary"
for tool in runuser zstd tar python3; do
  command -v "$tool" >/dev/null || fail "$tool is not installed"
done
mkdir -p "$OUT"
rm -rf "$OUT/support"
mkdir -p "$OUT/support"

say "--- keel doctor as keel"
if as_keel doctor >"$OUT/doctor.out" 2>"$OUT/doctor.err"; then
  say "ok: doctor exits 0"
else
  say "FAILED: doctor exits $? (see $OUT/doctor.out and $OUT/doctor.err)"
  failures=$((failures + 1))
fi
cat "$OUT/doctor.out"
check "doctor prints fourteen lines and none fails" test "$(grep -cE '^(ok|warn|fail) ' "$OUT/doctor.out")" = "14" -a "$(grep -c '^fail ' "$OUT/doctor.out")" = "0"
check "the units are installed, enabled and active" grep -qE '^ok +units .*are enabled and active' "$OUT/doctor.out"
check "the data directory is owned by keel" grep -qE "^ok +data_dir +$LIB is owned by keel" "$OUT/doctor.out"
check "bin/current points at the running version" grep -qE '^ok +install_dir ' "$OUT/doctor.out"
check "the database is reached over the unix socket as keel" grep -qE '^ok +database .*over the unix socket as keel' "$OUT/doctor.out"
check "the agent is not in use or reachable" grep -qE '^ok +agent ' "$OUT/doctor.out"
as_keel doctor --out json >"$OUT/doctor.json" || true
check "doctor --out json names the version" python3 -c 'import json,sys; doc=json.load(open(sys.argv[1])); sys.exit(0 if doc["version"] and len(doc["checks"]) == 14 else 1)' "$OUT/doctor.json"

say "--- keel doctor with secret.key opened to the group"
chmod 0644 "$LIB/secret.key"
if as_keel doctor >"$OUT/doctor-open.out" 2>&1; then
  say "FAILED: doctor exits 0 with an open secret.key"
  failures=$((failures + 1))
else
  say "ok: doctor exits 1 with an open secret.key"
fi
chmod 0600 "$LIB/secret.key"
check "the failing line names secret.key and its mode" grep -qE '^fail +secret_key .*secret.key has mode 0644' "$OUT/doctor-open.out"
check "doctor is green again after the mode is restored" as_keel doctor >/dev/null

say "--- keel support-bundle as keel"
rm -f "$OUT/support.tar.zst"
as_keel support-bundle --out "$OUT/support.tar.zst" >"$OUT/support.out" || fail "support-bundle failed: $(cat "$OUT/support.out")"
cat "$OUT/support.out"
chown root:root "$OUT/support.tar.zst"
zstd -d -c "$OUT/support.tar.zst" | tar -x -C "$OUT/support"
check "the bundle carries the configuration, the policy, the logs, the ledgers, the audit tail, the problems, the readiness answer, the doctor report and the manifest" test -f "$OUT/support/config.yaml" -a -f "$OUT/support/agent.yaml" -a -f "$OUT/support/logs.jsonl" -a -f "$OUT/support/ledgers.json" -a -f "$OUT/support/audit.jsonl" -a -f "$OUT/support/problems.json" -a -f "$OUT/support/readiness.json" -a -f "$OUT/support/doctor.txt" -a -f "$OUT/support/manifest.json"
db_password=$(python3 -c 'import re,sys; text=open(sys.argv[1]).read(); m=re.search(r"^database:\s*\"?([^\"\n]+)", text, re.M); from urllib.parse import urlparse; print((urlparse(m.group(1)).password or "") if m else "")' "$CONFIG")
if [ -n "$db_password" ] && [ "$db_password" != '${KEEL_DB_PASS}' ]; then
  check "the bundle carries no database password" test -z "$(grep -rl -- ":$db_password@" "$OUT/support")"
else
  say "skip: config.yaml carries no literal database password"
fi
check "the bundle carries no session, token or login key" test -z "$(grep -rlE -- '(ks|kat|kok)_[0-9a-f]{64}' "$OUT/support")"
check "the audit tail masks every password" python3 -c 'import json,sys; rows=[json.loads(l) for l in open(sys.argv[1])]; sys.exit(0 if all(r["params_redacted"].get("password", "***") == "***" for r in rows) else 1)' "$OUT/support/audit.jsonl"
check "the log ring is not empty" test -s "$OUT/support/logs.jsonl"
check "the readiness answer is ready" python3 -c 'import json,sys; doc=json.load(open(sys.argv[1])); sys.exit(0 if doc.get("status") == 200 and doc["body"]["status"] == "ready" else 1)' "$OUT/support/readiness.json"
check "the doctor report inside the bundle has no failure" grep -qE '^[0-9]+ ok, [0-9]+ warn, 0 fail; keel ' "$OUT/support/doctor.txt"

if [ "$failures" -ne 0 ]; then
  say "$failures check(s) failed; the transcript is under $OUT"
  exit 1
fi
say "the doctor and the support bundle are proven on this machine; the transcript is under $OUT"
