#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

keel=${1:-}
problems=0
workflow=.github/workflows/release.yml
channel_file=crates/release/channel.json
units=(keel.service keel-privileged.service keel-update.path keel-update.service)

usage() {
  echo "usage: hack/check-release.sh [path/to/keel]" >&2
  echo "  without a path the binary is built with cargo build -p keel-bin" >&2
  echo "  with KEEL_RELEASE_KEY in the environment the manifests are signed with it and verified" >&2
  echo "  with the compiled release keys; otherwise a rehearsal pair is generated" >&2
}

problem() {
  echo "problem: $1" >&2
  problems=$((problems + 1))
}

check() {
  local name=$1
  shift
  if "$@"; then
    echo "ok: $name"
  else
    problem "$name"
  fi
}

fails_with() {
  local pattern=$1 output
  shift
  if output=$("$@" 2>&1); then
    echo "  succeeded instead of failing" >&2
    return 1
  fi
  if ! grep -qF -- "$pattern" <<<"$output"; then
    echo "  failed for another reason: $output" >&2
    return 1
  fi
}

case "${keel:-}" in
  -h|--help) usage; exit 0 ;;
esac

if [ -z "$keel" ]; then
  cargo build -p keel-bin
  keel="${CARGO_TARGET_DIR:-target}/debug/keel"
fi
if [ ! -x "$keel" ]; then
  echo "$keel is not an executable binary" >&2
  exit 2
fi
for tool in python3 zstd tar; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "$tool is not installed" >&2
    exit 2
  fi
done
keel=$(cd "$(dirname "$keel")" && pwd)/$(basename "$keel")

check "bash -n hack/check-release.sh" bash -n hack/check-release.sh
check "hack/release.py is executable" test -x hack/release.py
check "hack/release.py parses" python3 -c 'import ast, sys; ast.parse(open(sys.argv[1]).read(), sys.argv[1])' hack/release.py
check "hack/release.py --help prints the usage" bash -c 'python3 hack/release.py --help | grep -q "^usage: hack/release.py"'
if command -v shellcheck >/dev/null 2>&1; then
  check "shellcheck" shellcheck -s bash -S warning hack/check-release.sh
else
  echo "shellcheck is not installed; skipped"
fi
if command -v actionlint >/dev/null 2>&1; then
  check "actionlint $workflow" actionlint "$workflow"
else
  echo "actionlint is not installed; the trigger of $workflow is checked by the tests of keel-release"
fi
check "$workflow runs hack/release.py with the tag and the musl binary" grep -qF 'python3 hack/release.py --tag "$GITHUB_REF_NAME" --binary target/x86_64-unknown-linux-musl/release/keel' "$workflow"
check "$workflow hands each of the two secrets to one step" test "$(grep -c 'secrets.KEEL_RELEASE_KEY\|secrets.KEEL_RELEASE_ASTRUM_PASSWORD' "$workflow")" = "2"
refresh_workflow=.github/workflows/refresh-channel.yml
check "$refresh_workflow runs hack/release.py --refresh for both channels with the keel-release it built" test "$(grep -cE 'run: python3 hack/release.py --refresh (stable|beta) --keel-release target/debug/keel-release$' "$refresh_workflow")" = "2"
check "$refresh_workflow builds keel-release in a step that holds no secret" test "$(sed -n '/- name: Build keel-release/,/- name: Refresh the stable head/p' "$refresh_workflow" | grep -c 'secrets\.\|env:')" = "0"
check "$refresh_workflow and $workflow share one concurrency group" test "$(grep -h -A1 '^concurrency:' "$refresh_workflow" "$workflow" | grep -c 'group: release-channel$')" = "2"
check "$refresh_workflow hands each of the two secrets to both steps" test "$(grep -c 'secrets.KEEL_RELEASE_KEY\|secrets.KEEL_RELEASE_ASTRUM_PASSWORD' "$refresh_workflow")" = "4"
check "$refresh_workflow runs on a schedule and by hand" grep -qE '^  workflow_dispatch:$' "$refresh_workflow"
if command -v actionlint >/dev/null 2>&1; then
  check "actionlint $refresh_workflow" actionlint "$refresh_workflow"
fi
check "$workflow refuses a tag without an annotation" grep -qF 'git cat-file -t "$GITHUB_REF_NAME"' "$workflow"
check "$workflow checks the binary against the tag" grep -qF 'expected="keel ${GITHUB_REF_NAME#keel-v}"' "$workflow"

version=$(sed -nE 's/^version = "(.*)"$/\1/p' Cargo.toml | head -n 1)
check "Cargo.toml names the workspace version" test -n "$version"
core=${version%%-*}
patch=${core##*.}
beta="${core%.*}.$((patch + 1))-beta.1"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
out=$tmp/channel
mkdir -p "$out"
printf 'Release %s of the rehearsal.\n\nWith a second paragraph.\n' "$version" > "$tmp/notes.txt"

rehearsal_pub=""
if [ -n "${KEEL_RELEASE_KEY:-}" ]; then
  echo "KEEL_RELEASE_KEY is set; the manifests are verified with the compiled release keys"
else
  cargo run -q -p keel-release -- keygen --out "$tmp/keys" --comment "release rehearsal" >/dev/null
  rehearsal_pub=$tmp/keys/release.pub
  KEEL_RELEASE_KEY=$(cat "$tmp/keys/release.key")
  export KEEL_RELEASE_KEY
  echo "KEEL_RELEASE_KEY is not set; a rehearsal pair verifies the manifests"
fi

dry_run() {
  if [ -n "$rehearsal_pub" ]; then
    python3 hack/release.py --binary "$keel" --notes-file "$tmp/notes.txt" --zstd-level 3 --dry-run --out "$out" --verify-key "$rehearsal_pub" "$@"
  else
    python3 hack/release.py --binary "$keel" --notes-file "$tmp/notes.txt" --zstd-level 3 --dry-run --out "$out" "$@"
  fi
}

verify() {
  if [ -n "$rehearsal_pub" ]; then
    cargo run -q -p keel-release -- verify "$1" --key "$rehearsal_pub"
  else
    cargo run -q -p keel-release -- verify "$1"
  fi
}

verify_quietly() {
  verify "$1" >/dev/null
}

verify_head() {
  if [ -n "$rehearsal_pub" ]; then
    cargo run -q -p keel-release -- verify "$1" --head --key "$rehearsal_pub"
  else
    cargo run -q -p keel-release -- verify "$1" --head
  fi
}

verify_head_quietly() {
  verify_head "$1" >/dev/null
}

refresh_dry_run() {
  local channel=$1 dir=$2
  shift 2
  if [ -n "$rehearsal_pub" ]; then
    python3 hack/release.py --refresh "$channel" --dry-run --out "$dir" --verify-key "$rehearsal_pub" "$@"
  else
    python3 hack/release.py --refresh "$channel" --dry-run --out "$dir" "$@"
  fi
}

payload_text() {
  python3 -c 'import base64, json, sys; sys.stdout.write(base64.b64decode(json.load(open(sys.argv[1]))["payload"]).decode())' "$1"
}

expiry_of() {
  payload_text "$1" | python3 -c 'import json, sys; print(json.load(sys.stdin).get("expires_at", ""))'
}

without_expiry() {
  payload_text "$1" | sed -E 's/"expires_at": [0-9]+/"expires_at": N/'
}

same_but_expiry() {
  python3 -c '
import base64, json, sys
def load(path):
    document = json.loads(base64.b64decode(json.load(open(path))["payload"]))
    document.pop("expires_at", None)
    return document
sys.exit(0 if load(sys.argv[1]) == load(sys.argv[2]) else 1)
' "$1" "$2"
}

tampered_copy() {
  python3 -c '
import base64, json, sys
envelope = json.load(open(sys.argv[1]))
payload = bytearray(base64.b64decode(envelope["payload"]))
position = payload.index(b"1")
payload[position] = ord("2")
envelope["payload"] = base64.b64encode(bytes(payload)).decode()
json.dump(envelope, open(sys.argv[2], "w"))
' "$1" "$2"
}

within() {
  local value=$1 low=$2 high=$3
  [ "$value" -ge "$low" ] && [ "$value" -le "$high" ]
}

field() {
  python3 -c 'import json, sys; value = json.load(open(sys.argv[1]))[sys.argv[2]]; print(value if isinstance(value, str) else json.dumps(value))' "$tmp/payload.json" "$1"
}

channel_value() {
  python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$channel_file" "$1"
}

sha256_hex() {
  python3 -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$1"
}

dry_run_stable() {
  dry_run --tag "keel-v$version" > "$tmp/stable.log"
}

check "the dry run of keel-v$version writes the stable channel" dry_run_stable
check "the dry run of keel-v$beta writes the beta channel" dry_run --tag "keel-v$beta"
archive="keel-$version-x86_64-linux-musl.tar.zst"
beta_archive="keel-$beta-x86_64-linux-musl.tar.zst"
expected=$(printf '%s\n' \
  "beta/manifest.json" \
  "install.sh" \
  "stable/manifest.json" \
  "versions/$beta/$beta_archive" \
  "versions/$beta/$beta_archive.sha256" \
  "versions/$beta/manifest.json" \
  "versions/$version/$archive" \
  "versions/$version/$archive.sha256" \
  "versions/$version/manifest.json" | sort)
check "the two dry runs leave the layout of the design plus install.sh at the root and nothing else" test "$(cd "$out" && find . -type f | sed 's|^\./||' | sort)" = "$expected"
check "stable/manifest.json carries the manifest of $version and an expires_at" same_but_expiry "$out/stable/manifest.json" "$out/versions/$version/manifest.json"
check "beta/manifest.json carries the manifest of $beta and an expires_at" same_but_expiry "$out/beta/manifest.json" "$out/versions/$beta/manifest.json"
check "versions/$version/manifest.json carries no expires_at" test -z "$(expiry_of "$out/versions/$version/manifest.json")"
check "versions/$beta/manifest.json carries no expires_at" test -z "$(expiry_of "$out/versions/$beta/manifest.json")"
published=$(date +%s)
for head in stable beta; do
  check "$head/manifest.json expires thirty days after it was published" within "$(expiry_of "$out/$head/manifest.json")" "$((published + 29 * 86400))" "$((published + 31 * 86400))"
done
for object in "versions/$version/$archive" "versions/$version/$archive.sha256" "versions/$version/manifest.json" "stable/manifest.json" "install.sh"; do
  check "the dry run names the PUT of $object" grep -qF "a publish would PUT https://astrum.globalart.hu/repository/keel-releases/keel/$object" "$tmp/stable.log"
done
published_key=$(sed -nE "s/^RELEASE_KEY='(.*)'$/\1/p" "$out/install.sh")
check "the published install.sh carries the committed release key" test "$published_key" = "$(head -n 1 crates/release/keys/release.pub)"
check "the published install.sh differs from deploy/install.sh in the key line alone" cmp -s <(grep -v '^RELEASE_KEY=' deploy/install.sh) <(grep -v '^RELEASE_KEY=' "$out/install.sh")
check "the published install.sh parses" bash -n "$out/install.sh"

if payload=$(verify "$out/versions/$version/manifest.json"); then
  echo "ok: keel-release verify accepts versions/$version/manifest.json"
  sed '$d' <<<"$payload" > "$tmp/payload.json"
  check "the verified manifest names $version" test "$(field version)" = "$version"
  check "the verified manifest carries the sha256 of the archive" test "$(field sha256)" = "$(sha256_hex "$out/versions/$version/$archive")"
  check "the verified manifest carries the contract of $channel_file" test "$(field contract)" = "$(channel_value contract)"
  check "the verified manifest carries min_upgrade_from of $channel_file" test "$(field min_upgrade_from)" = "$(channel_value min_upgrade_from)"
  check "the verified manifest carries the notes" test "$(field notes)" = "$(cat "$tmp/notes.txt")"
else
  problem "keel-release verify accepts versions/$version/manifest.json"
fi
check "keel-release verify accepts beta/manifest.json" verify_quietly "$out/beta/manifest.json"
check "keel-release verify --head accepts stable/manifest.json" verify_head_quietly "$out/stable/manifest.json"
check "keel-release verify --head accepts beta/manifest.json" verify_head_quietly "$out/beta/manifest.json"
check "keel-release verify --head refuses the manifest of a version, which carries no expires_at" fails_with "carries no expires_at" verify_head "$out/versions/$version/manifest.json"

refresh_dir=$tmp/refresh
mkdir -p "$refresh_dir/stable"
cp "$out/stable/manifest.json" "$refresh_dir/stable/manifest.json"
cp "$out/stable/manifest.json" "$tmp/head-before.json"
refreshed_stable() {
  refresh_dry_run stable "$refresh_dir" --expires-days 90 > "$tmp/refresh.log"
}
check "the dry run of --refresh stable signs the head again" refreshed_stable
refreshed=$refresh_dir/stable/manifest.json
check "the refreshed head verifies as a head" verify_head_quietly "$refreshed"
check "the refreshed head differs from the head it came from" bash -c '! cmp -s "$1" "$2"' bash "$refreshed" "$tmp/head-before.json"
check "the refreshed head expires later than the head it came from" test "$(expiry_of "$refreshed")" -gt "$(expiry_of "$tmp/head-before.json")"
check "the refreshed head expires ninety days after the refresh" within "$(expiry_of "$refreshed")" "$(($(date +%s) + 89 * 86400))" "$(($(date +%s) + 91 * 86400))"
check "the refreshed head differs from its source in the expires_at number alone, byte for byte" cmp -s <(without_expiry "$refreshed") <(without_expiry "$tmp/head-before.json")
check "the refreshed head still carries the manifest of $version" same_but_expiry "$refreshed" "$out/versions/$version/manifest.json"
check "the refresh writes the head and nothing else" test "$(cd "$refresh_dir" && find . -type f | sed 's|^\./||' | sort)" = "stable/manifest.json"
check "the dry run names the PUT of the refreshed head" grep -qF "a publish would PUT https://astrum.globalart.hu/repository/keel-releases/keel/stable/manifest.json" "$tmp/refresh.log"
mkdir -p "$tmp/tampered/stable"
tampered_copy "$out/stable/manifest.json" "$tmp/tampered/stable/manifest.json"
cp "$tmp/tampered/stable/manifest.json" "$tmp/tampered-before.json"
check "a refresh of a head whose payload was changed is rejected" fails_with "the signature matches none of the trusted keys" refresh_dry_run stable "$tmp/tampered"
check "a rejected refresh writes nothing" test "$(cd "$tmp/tampered" && find . -type f | sed 's|^\./||' | sort)" = "stable/manifest.json"
check "a rejected refresh leaves the head as it was" cmp -s "$tmp/tampered-before.json" "$tmp/tampered/stable/manifest.json"
mkdir -p "$tmp/headless"
refresh_without_head() {
  refresh_dry_run beta "$tmp/headless" > "$tmp/headless.log"
}
check "a refresh of a beta channel without a head reports it and succeeds" refresh_without_head
check "the report names the missing head" grep -qF "the beta channel has no head to refresh" "$tmp/headless.log"
check "a refresh of a stable channel without a head fails" fails_with "the stable channel has no head to refresh" refresh_dry_run stable "$tmp/headless"
check "a refresh of a channel without a head writes nothing" test -z "$(find "$tmp/headless" -type f)"
check "--refresh refuses a tag" fails_with "does not belong to --refresh" refresh_dry_run stable "$refresh_dir" --tag "keel-v$version"
check "--expires-days of zero is refused" fails_with "--expires-days is between 1 and 365" refresh_dry_run stable "$refresh_dir" --expires-days 0
check "--expires-days above a year is refused" fails_with "--expires-days is between 1 and 365" refresh_dry_run stable "$refresh_dir" --expires-days 366
check "a refresh without the password is refused before the channel is read" fails_with "KEEL_RELEASE_ASTRUM_PASSWORD is not set" env -u KEEL_RELEASE_ASTRUM_PASSWORD python3 hack/release.py --refresh stable

if command -v sha256sum >/dev/null 2>&1; then
  check "the .sha256 file checks with sha256sum" bash -c "cd '$out/versions/$version' && sha256sum -c --strict --quiet '$archive.sha256'"
else
  check "the .sha256 file checks with shasum" bash -c "cd '$out/versions/$version' && shasum -a 256 -c --strict --quiet '$archive.sha256'"
fi

listing=$(zstd -d -c "$out/versions/$version/$archive" | tar -tvf -)
check "the archive carries keel at its root as an executable" grep -qE '^-rwxr-xr-x .* keel$' <<<"$listing"
check "the archive carries config-sample.yaml" grep -qE ' config-sample.yaml$' <<<"$listing"
check "the archive carries deploy/install.sh as an executable" grep -qE '^-rwxr-xr-x .* deploy/install.sh$' <<<"$listing"
check "the archive carries deploy/uninstall.sh as an executable" grep -qE '^-rwxr-xr-x .* deploy/uninstall.sh$' <<<"$listing"
check "the archive carries the policy template deploy/agent.yaml" grep -qE '^-rw-r--r-- .* deploy/agent.yaml$' <<<"$listing"
for unit in "${units[@]}"; do
  check "the archive carries deploy/systemd/$unit" grep -qE " deploy/systemd/$unit\$" <<<"$listing"
done
check "the archive carries regular files and directories alone" test -z "$(grep -vE '^[-d]' <<<"$listing")"
check "the archive carries no setuid, setgid or sticky bit" test -z "$(grep -E '^.{1,9}[sStT]' <<<"$listing")"
mkdir -p "$tmp/unpacked"
zstd -d -c "$out/versions/$version/$archive" | tar -x -C "$tmp/unpacked"
check "the unpacked keel prints the version of the tag" test "$("$tmp/unpacked/keel" --version)" = "keel $version"
check "the unpacked install.sh is the published one" cmp -s "$tmp/unpacked/deploy/install.sh" "$out/install.sh"
check "the unpacked agent.yaml is the repository's template" cmp -s "$tmp/unpacked/deploy/agent.yaml" deploy/agent.yaml

tree=$tmp/tree
mkdir -p "$tree/opt/versions/$version" "$tree/opt/bin" "$tree/units" "$tree/stub"
cp "$keel" "$tree/opt/versions/$version/keel"
ln -s "$tree/opt/versions/$version/keel" "$tree/opt/bin/current"
printf '#!/bin/sh\nprintf "%%s\\n" "$*" >> "%s/stub/calls"\n' "$tree" > "$tree/stub/systemctl"
chmod +x "$tree/stub/systemctl"
printf '{"version":"%s","channel":"beta","requested_by":"check-release"}\n' "$beta" > "$tree/update.request"
install_from_request() {
  if [ -n "$rehearsal_pub" ]; then
    "$keel" privileged update --from-request --request "$tree/update.request" --channel-url "$out" \
      --opt "$tree/opt" --unit-dir "$tree/units" --systemctl "$tree/stub/systemctl" --release-key "$rehearsal_pub"
  else
    "$keel" privileged update --from-request --request "$tree/update.request" --channel-url "$out" \
      --opt "$tree/opt" --unit-dir "$tree/units" --systemctl "$tree/stub/systemctl"
  fi
}
check "the root runner installs the dry run's beta release over $version" install_from_request
check "bin/current points at the installed beta" test "$(readlink "$tree/opt/bin/current")" = "$tree/opt/versions/$beta/keel"
check "the installed version carries its signed manifest" cmp -s "$tree/opt/versions/$beta/manifest.json" "$out/versions/$beta/manifest.json"
check "the unit files were refreshed from the release" test "$(ls "$tree/units" | sort | tr '\n' ' ')" = "keel-privileged.service keel-update.path keel-update.service keel.service "
check "systemctl was told to reload and restart the two units" test "$(cat "$tree/stub/calls")" = "$(printf 'daemon-reload\nrestart keel-privileged.service\nrestart keel.service')"

check "a second dry run of $version refuses to publish over it" fails_with "is published, --replace publishes over it" dry_run --tag "keel-v$version"
check "a second dry run of $version with --replace publishes over it" dry_run --tag "keel-v$version" --replace
printf '{"contract": 1, "min_upgrade_from": "0.0.1"}\n' > "$tmp/low-floor.json"
check "a dry run below the stable head refuses to move it backwards" fails_with "would move it backwards" dry_run --tag keel-v0.0.1 --channel-file "$tmp/low-floor.json"
check "a dry run whose floor is above its version is refused" fails_with "would admit nothing" dry_run --tag keel-v0.0.1
check "a tag without the prefix is refused" fails_with "does not start with keel-v" dry_run --tag v0.1.0
check "a tag that is not a semantic version is refused" fails_with "does not name a semantic version" dry_run --tag keel-v1.0
check "a publish without the password is refused before anything is packaged" fails_with "KEEL_RELEASE_ASTRUM_PASSWORD is not set" env -u KEEL_RELEASE_ASTRUM_PASSWORD python3 hack/release.py --tag "keel-v$version" --binary "$keel" --notes-file "$tmp/notes.txt"

if [ "$problems" -gt 0 ]; then
  echo "$problems problem(s)" >&2
  exit 1
fi
echo "the release workflow is checked"
