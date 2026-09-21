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
check "$workflow hands each of the two secrets to one step" test "$(grep -c 'secrets.KEEL_RELEASE_KEY\|secrets.KEEL_RELEASE_NEXUS_PASSWORD' "$workflow")" = "2"
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
  "stable/manifest.json" \
  "versions/$beta/$beta_archive" \
  "versions/$beta/$beta_archive.sha256" \
  "versions/$beta/manifest.json" \
  "versions/$version/$archive" \
  "versions/$version/$archive.sha256" \
  "versions/$version/manifest.json" | sort)
check "the two dry runs leave the layout of the design and nothing else" test "$(cd "$out" && find . -type f | sed 's|^\./||' | sort)" = "$expected"
check "stable/manifest.json is the manifest of $version" cmp -s "$out/stable/manifest.json" "$out/versions/$version/manifest.json"
check "beta/manifest.json is the manifest of $beta" cmp -s "$out/beta/manifest.json" "$out/versions/$beta/manifest.json"
for object in "versions/$version/$archive" "versions/$version/$archive.sha256" "versions/$version/manifest.json" "stable/manifest.json"; do
  check "the dry run names the PUT of $object" grep -qF "a publish would PUT https://nexus.globalart.hu/repository/keel-releases/keel/$object" "$tmp/stable.log"
done

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

if command -v sha256sum >/dev/null 2>&1; then
  check "the .sha256 file checks with sha256sum" bash -c "cd '$out/versions/$version' && sha256sum -c --strict --quiet '$archive.sha256'"
else
  check "the .sha256 file checks with shasum" bash -c "cd '$out/versions/$version' && shasum -a 256 -c --strict --quiet '$archive.sha256'"
fi

listing=$(zstd -d -c "$out/versions/$version/$archive" | tar -tvf -)
check "the archive carries keel at its root as an executable" grep -qE '^-rwxr-xr-x .* keel$' <<<"$listing"
check "the archive carries config-sample.yaml" grep -qE ' config-sample.yaml$' <<<"$listing"
for unit in "${units[@]}"; do
  check "the archive carries deploy/systemd/$unit" grep -qE " deploy/systemd/$unit\$" <<<"$listing"
done
check "the archive carries regular files and directories alone" test -z "$(grep -vE '^[-d]' <<<"$listing")"
check "the archive carries no setuid, setgid or sticky bit" test -z "$(grep -E '^.{1,9}[sStT]' <<<"$listing")"
mkdir -p "$tmp/unpacked"
zstd -d -c "$out/versions/$version/$archive" | tar -x -C "$tmp/unpacked"
check "the unpacked keel prints the version of the tag" test "$("$tmp/unpacked/keel" --version)" = "keel $version"

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
check "a publish without the password is refused before anything is packaged" fails_with "KEEL_RELEASE_NEXUS_PASSWORD is not set" env -u KEEL_RELEASE_NEXUS_PASSWORD python3 hack/release.py --tag "keel-v$version" --binary "$keel" --notes-file "$tmp/notes.txt"

if [ "$problems" -gt 0 ]; then
  echo "$problems problem(s)" >&2
  exit 1
fi
echo "the release workflow is checked"
