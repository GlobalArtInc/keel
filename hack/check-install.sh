#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

keel=${1:-}
problems=0
scripts=(deploy/install.sh deploy/uninstall.sh hack/check-install.sh hack/update-vm.sh hack/backup-vm.sh hack/install-vm.sh hack/host-vm.sh hack/dns-vm.sh)
units=(keel.service keel-privileged.service keel-update.path keel-update.service)
enabled_units=(keel.service keel-privileged.service keel-update.path)

usage() {
  echo "usage: hack/check-install.sh [path/to/keel]" >&2
  echo "  without a path the binary is built with cargo build -p keel-bin" >&2
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

prints_usage() {
  local script=$1 out
  out=$(bash "$script" --help) || return 1
  [[ $out == usage:* ]]
}

lists_subcommand() {
  local help=$1 name=$2
  grep -qE "^  $name( |$)" <<<"$help"
}

lists_flag() {
  local help=$1 flag=$2
  grep -qE "^\s+(-[a-zA-Z], )?$flag( |$)" <<<"$help"
}

function_is_defined() {
  grep -rqF --include='*.rs' "\"$1\"" crates products
}

sample_config_parses() {
  local err
  if err=$("$keel" --config deploy/config-sample.yaml migrate 2>&1 >/dev/null); then
    return 0
  fi
  if grep -qE 'config-sample.yaml|not valid configuration|is unusable|could not read' <<<"$err"; then
    echo "  $err" >&2
    return 1
  fi
}

case "${keel:-}" in
  -h|--help) usage; exit 0 ;;
esac

for script in "${scripts[@]}"; do
  check "bash -n $script" bash -n "$script"
  check "$script is executable" test -x "$script"
done

if command -v shellcheck >/dev/null 2>&1; then
  check "shellcheck" shellcheck -s bash -S warning "${scripts[@]}"
else
  echo "shellcheck is not installed; skipped"
fi

check "install.sh --help prints the usage and exits 0" prints_usage deploy/install.sh
check "uninstall.sh --help prints the usage and exits 0" prints_usage deploy/uninstall.sh
check "the sample config carries every key of the kernel" test "$(grep -cE '^(listen|socket|data_dir|database|tls|trusted_proxies|log):' deploy/config-sample.yaml)" = "7"
check "install.sh needs curl" grep -qE '^\s+need .*\bcurl\b' deploy/install.sh
check "install.sh installs curl when it is absent" grep -qE 'packages\+=\(curl ca-certificates\)' deploy/install.sh
check "install.sh installs openssl when it is absent" grep -qE 'packages\+=\(openssl\)' deploy/install.sh
check "install.sh runs its main when executed or piped, not when sourced" grep -qF 'if [ -z "${BASH_SOURCE[0]:-}" ] || [ "${BASH_SOURCE[0]}" = "$0" ]; then' deploy/install.sh
check "install.sh refuses a shell that is not bash with the bash line" grep -qF 'echo "install.sh runs under bash: curl -fsSL https://dl.globalart.hu/keel/install.sh | bash -s -- --product <id>" >&2' deploy/install.sh
check "install.sh piped into bash reaches its argument parsing" test "$(bash -s -- --product demo --listen nonsense <deploy/install.sh 2>&1 || true)" = 'install.sh: --listen must be <ipv4>:<port> or [<ipv6>]:<port>, not "nonsense"'
check "install.sh piped into bash as a non-root user stops at the root check" test "$(bash -s -- --product demo --yes <deploy/install.sh 2>&1 | tail -n 1 || true)" = 'install.sh: run as root'

check "install.sh embeds one release key line" test "$(grep -cE "^RELEASE_KEY='ed25519 [A-Za-z0-9+/]{43}= [^']*'$" deploy/install.sh)" = "1"
check "the embedded release key is the committed public key" test "$(sed -nE "s/^RELEASE_KEY='(.*)'$/\1/p" deploy/install.sh)" = "$(head -n 1 crates/release/keys/release.pub)"
check "install.sh fetches the channel head or a version's manifest" grep -qF 'manifest_url="${channel_url}$channel/manifest.json"' deploy/install.sh
check "install.sh verifies the manifest signature with openssl over the raw payload" grep -qF 'openssl pkeyutl -verify -pubin -inkey "$work/key.pem" -rawin -in "$work/payload" -sigfile "$work/signature"' deploy/install.sh
check "install.sh verifies the signature before anything is unpacked" test "$(grep -nE 'verifying the Ed25519 signature of the release manifest' deploy/install.sh | cut -d: -f1)" -lt "$(grep -nF 'unpack "$tmp/$name" "$name"' deploy/install.sh | cut -d: -f1)"
check "install.sh unpacks nothing when the signature fails" grep -qF 'is not signed by the release key; nothing was unpacked' deploy/install.sh
check "install.sh checks the archive against the sha256 of the verified manifest" grep -qF 'printf '"'"'%s  %s\n'"'"' "$sha256" "$name" | sha256sum -c --strict --quiet' deploy/install.sh
check "install.sh keeps the signed manifest beside the installed version" grep -qF 'install -m 0644 -o root -g root "$tmp/manifest.json" "$OPT/staging/manifest.json"' deploy/install.sh
check "install.sh allots the uid range before the policy is written" test "$(grep -nE '^  allot_uid_range$' deploy/install.sh | cut -d: -f1)" -lt "$(grep -nE '^  install_policy$' deploy/install.sh | cut -d: -f1)"
check "install.sh writes the uid range under uid_range of the agent policy" grep -qF 'uid_range: { from: $uid_from, to: $uid_to }' deploy/install.sh
check "install.sh installs the agent policy from the release" grep -qF 'release_file deploy/agent.yaml "$here/agent.yaml"' deploy/install.sh
check "install.sh keeps an existing agent policy" grep -qF 'exists from an earlier installation; kept as it is' deploy/install.sh
check "deploy/agent.yaml carries one uid_range line under policy" test "$(grep -cE '^  uid_range: \{ from: [0-9]+, to: [0-9]+ \}$' deploy/agent.yaml)" = "1"
check "deploy/agent.yaml starts with the policy key" test "$(head -n 1 deploy/agent.yaml)" = "policy:"
check "deploy/agent.yaml names the check of the zones on named" grep -qF 'named.check: ["named-checkconf", "-z"]' deploy/agent.yaml
check "host-vm.sh expects that check in the installed policy" grep -qF 'named.check' hack/host-vm.sh
for product in bill dns panel; do
  check "deploy/agent.yaml carries the section of $product" grep -qE "^    $product: \{ .*drop_in_prefix: keel-$product- \}$" deploy/agent.yaml
done
check "install.sh waits for the readiness notification of keel.service" grep -qF 'did not report READY=1 to the service manager' deploy/install.sh
check "install.sh reads the setup key from the journal" grep -qF "grep -oE '/setup\?key=kok_[0-9a-f]{64}'" deploy/install.sh
check "install.sh prints the setup link" grep -qF 'say "  setup link:   https://$host:$port/setup?key=$key"' deploy/install.sh
check "install.sh creates no administrator over the socket" test "$(grep -cE 'run_keel ctl|keel\.principal\.create|auth\.password\.set|--admin' deploy/install.sh)" = "0"
check "uninstall.sh removes the agent and control sockets" grep -qF 'SOCKETS=("$RUN/agent.sock" "$RUN/core.sock")' deploy/uninstall.sh
check "the deploy README names the channel flags" grep -qE -- '--channel-url' deploy/README.md
check "the deploy README names the setup link" grep -qF 'setup link' deploy/README.md
check "install.sh waits for /readyz with the generated certificate" grep -qE 'curl .*--cacert "\$CERT" "https://\$host:\$port/readyz"' deploy/install.sh
check "install.sh names the certificate under the data directory" grep -qE '^CERT=\$LIB/tls/cert\.pem$' deploy/install.sh
check "install.sh prints the https address" grep -qE 'say "  URL:          https://\$host:\$port/"' deploy/install.sh
check "install.sh prints the certificate fingerprint" grep -qE 'SHA-256 \$fingerprint' deploy/install.sh
check "install.sh shows the API example over https" grep -qE "curl -sS --cacert \\\$CERT -X POST https://" deploy/install.sh
check "install.sh names the certificate's subjects when listen is unspecified" grep -qE 'names:.*\$name, localhost, 127\.0\.0\.1 and ::1' deploy/install.sh
check "install.sh pins the API example to a certificate name when listen is unspecified" grep -qE 'curl -sS --cacert \$CERT --resolve \\"\$name:\$port:\$host\\" -X POST https://\$name:\$port/' deploy/install.sh
check "install.sh prints no plain address" test "$(grep -cE 'http://\$host' deploy/install.sh)" = "0"

unit_value() {
  local unit=$1 key=$2
  sed -nE "s|^$key=(.*)$|\1|p" "deploy/systemd/$unit"
}

for unit in "${units[@]}"; do
  check "deploy/systemd/$unit exists" test -f "deploy/systemd/$unit"
  check "install.sh installs $unit" grep -qE "^UNITS=\(.*\b$unit\b" deploy/install.sh
  check "uninstall.sh removes $unit" grep -qE "^UNITS=\(.*\b$unit\b" deploy/uninstall.sh
  check "$unit has a [Unit] section" grep -qxF "[Unit]" "deploy/systemd/$unit"
done
for unit in "${enabled_units[@]}"; do
  check "install.sh enables $unit" grep -qE "^ENABLED_UNITS=\(.*\b$unit\b" deploy/install.sh
  check "$unit carries an install section" grep -qxF "WantedBy=multi-user.target" "deploy/systemd/$unit"
done
check "install.sh enables the listed units" grep -qF 'systemctl enable "${ENABLED_UNITS[@]}"' deploy/install.sh
check "install.sh installs every listed unit into the unit directory" grep -qF 'install -m 0644 -o root -g root "$OPT/versions/$version/deploy/systemd/$unit" "$UNIT_DIR/$unit"' deploy/install.sh
check "install.sh starts the privileged unit and the update trigger" grep -qF 'systemctl start keel-privileged.service keel-update.path' deploy/install.sh
check "keel-update.service is started by the path unit alone" test -z "$(unit_value keel-update.service WantedBy)"
check "keel.service runs as keel" test "$(unit_value keel.service User)" = "keel"
check "keel.service is Type=notify" test "$(unit_value keel.service Type)" = "notify"
check "keel.service is watched every 60 s" test "$(unit_value keel.service WatchdogSec)" = "60"
check "keel.service takes notifications from the main process" test "$(unit_value keel.service NotifyAccess)" = "main"
check "the other units are not Type=notify" test -z "$(grep -l '^Type=notify$' deploy/systemd/keel-*.service 2>/dev/null)"
check "keel.service restarts on the kernel's restart exit status" test "$(unit_value keel.service RestartForceExitStatus)" = "75"
check "keel.service counts the restart exit status as a clean exit" test "$(unit_value keel.service SuccessExitStatus)" = "75"
check "keel-privileged.service runs as root" test "$(unit_value keel-privileged.service User)" = "root"
check "keel-update.service runs as root" test "$(unit_value keel-update.service User)" = "root"
check "keel-update.service is a oneshot" test "$(unit_value keel-update.service Type)" = "oneshot"
check "keel-update.path watches the update request" test "$(unit_value keel-update.path PathExists)" = "/var/lib/keel/update.request"
check "keel-update.path starts keel-update.service" test "$(unit_value keel-update.path Unit)" = "keel-update.service"
check "keel-privileged.service runs privileged with the socket and the policy" test "$(unit_value keel-privileged.service ExecStart)" = "/opt/keel/bin/current privileged --socket /run/keel/agent.sock --policy /etc/keel/agent.yaml"
check "keel-update.service runs the update from the request" test "$(unit_value keel-update.service ExecStart)" = "/opt/keel/bin/current privileged update --from-request"
check "keel.service alone owns the runtime directory" test "$(grep -l '^RuntimeDirectory=keel$' deploy/systemd/* | tr '\n' ' ')" = "deploy/systemd/keel.service "
check "keel.service keeps the runtime directory and the agent socket across its restarts" test "$(unit_value keel.service RuntimeDirectoryPreserve)" = "restart"
check "host-vm.sh --help prints the usage and exits 0" prints_usage hack/host-vm.sh
check "host-vm.sh proves the refusal outside file_roots in the journal" grep -qF 'check "the refusal is in the journal with its request id"' hack/host-vm.sh
check "host-vm.sh proves the rollback of a broken template without downtime" grep -qF 'check "$NAMED_UNIT was not restarted"' hack/host-vm.sh
check "host-vm.sh proves the user inside the uid range" grep -qF 'check "the uid $uid is inside the uid range of the policy"' hack/host-vm.sh
check "keel.service alone drops privileges with NoNewPrivileges" test "$(grep -l '^NoNewPrivileges=yes$' deploy/systemd/* | tr '\n' ' ')" = "deploy/systemd/keel.service "
check "the kernel unit is the only one that is not root" test "$(grep -L '^User=root$' deploy/systemd/*.service | tr '\n' ' ')" = "deploy/systemd/keel.service "
check "update-vm.sh --help prints the usage and exits 0" prints_usage hack/update-vm.sh
check "backup-vm.sh --help prints the usage and exits 0" prints_usage hack/backup-vm.sh
check "install-vm.sh --help prints the usage and exits 0" prints_usage hack/install-vm.sh
check "dns-vm.sh --help prints the usage and exits 0" prints_usage hack/dns-vm.sh
check "dns-vm.sh installs through install.sh with dns from a local archive" grep -qF 'bash deploy/install.sh --product dns --binary "$archive" --yes' hack/dns-vm.sh
check "dns-vm.sh asks named for the zones with dig" grep -qF 'check "named answers $CLI_ZONE authoritatively" served "$CLI_ZONE"' hack/dns-vm.sh
check "dns-vm.sh proves the rollback of a broken template without downtime" grep -qF 'check "named answered every poll during the broken apply"' hack/dns-vm.sh
check "install-vm.sh runs the installer through curl and bash from the served channel" grep -qF 'curl -fsSL "http://127.0.0.1:$CHANNEL_PORT/install.sh" | bash -s -- --product "$product" --channel-url "http://127.0.0.1:$CHANNEL_PORT/"' hack/install-vm.sh
check "install-vm.sh checks that root runs keel privileged alone" grep -qF 'check "root runs keel privileged and nothing else of keel"' hack/install-vm.sh
check "install-vm.sh follows the setup link" grep -qF 'check "keel.setup.admin with the key creates the first administrator"' hack/install-vm.sh
for flag in --with-secret-key --out; do
  check "backup-vm.sh exercises keel backup $flag" grep -qE -- "backup .*$flag" hack/backup-vm.sh
done
check "backup-vm.sh exercises keel restore --with-secret-key" grep -qE -- 'restore "\$staged" --with-secret-key' hack/backup-vm.sh

if [ -z "$keel" ]; then
  cargo build -p keel-bin
  keel="${CARGO_TARGET_DIR:-target}/debug/keel"
fi
if [ ! -x "$keel" ]; then
  echo "$keel is not an executable binary" >&2
  exit 2
fi

help=$("$keel" --help)
product_help=$("$keel" product --help)
ctl_help=$("$keel" ctl --help)
update_help=$("$keel" update --help)

check "the subcommand update is in keel --help" lists_subcommand "$help" update
for subcommand in backup restore; do
  check "the subcommand $subcommand is in keel --help" lists_subcommand "$help" "$subcommand"
  check "the flag --with-secret-key of $subcommand is in keel $subcommand --help" lists_flag "$("$keel" "$subcommand" --help)" --with-secret-key
done
check "backup inspect is in keel backup --help" lists_subcommand "$("$keel" backup --help)" inspect
check "the flag --out of backup is in keel backup --help" lists_flag "$("$keel" backup --help)" --out
check "uninstall.sh --keep-backups is documented in its usage" grep -qE '^\s+--keep-backups\s' deploy/uninstall.sh
check "uninstall.sh keeps the backup directory when asked" grep -qF 'if [ "$keep_backups" -eq 1 ] && [ -d "$LIB/backups" ]; then' deploy/uninstall.sh
check "the deploy README names uninstall.sh --keep-backups" grep -qF -- '--purge --keep-backups' deploy/README.md
for step in check apply rollback; do
  check "update $step is in keel update --help" lists_subcommand "$update_help" "$step"
  check "update-vm.sh or the README exercises keel update $step" grep -qE "update $step\b" hack/update-vm.sh deploy/README.md
done
for flag in --wait --out; do
  check "the flag $flag of update apply is in keel update apply --help" lists_flag "$("$keel" update apply --help)" "$flag"
done

for unit in deploy/systemd/*.service; do
  line=$(sed -nE 's|^ExecStart=/opt/keel/bin/current (.*)$|\1|p' "$unit")
  check "$unit starts a subcommand of the binary" test -n "$line"
  read -r -a words <<<"$line"
  path=()
  for word in "${words[@]}"; do
    case $word in
      --*) break ;;
      *) path+=("$word") ;;
    esac
  done
  parent_help=$help
  for ((index = 0; index < ${#path[@]}; index++)); do
    check "the subcommand ${path[*]:0:index+1} of $unit is in keel --help" lists_subcommand "$parent_help" "${path[index]}"
    parent_help=$("$keel" "${path[@]:0:index+1}" --help)
  done
  for word in "${words[@]}"; do
    case $word in
      --*) check "the flag $word of $unit is in keel ${path[*]} --help" lists_flag "$parent_help" "$word" ;;
    esac
  done
done

calls=$(grep -oE 'run_keel [a-z]+( [a-z.]+)?' deploy/install.sh | sed 's/^run_keel //' | sort -u) || true
check "install.sh calls the binary" test -n "$calls"
while read -r subcommand argument; do
  check "the subcommand $subcommand is in keel --help" lists_subcommand "$help" "$subcommand"
  case $subcommand in
    product)
      check "product $argument is in keel product --help" lists_subcommand "$product_help" "$argument"
      ;;
    ctl)
      check "the function $argument is defined in the workspace" function_is_defined "$argument"
      ;;
  esac
done <<<"$calls"

ctl_flags=$(grep -ohE '(run_keel|keel) ctl [a-z.]+.*' deploy/install.sh deploy/README.md | grep -oE -- '--[a-z]+' | sort -u) || true
for flag in $ctl_flags; do
  check "the ctl flag $flag is in keel ctl --help" lists_flag "$ctl_help" "$flag"
done

readme_functions=$(grep -oE 'keel ctl [a-z]+\.[a-z.]+' deploy/README.md | awk '{print $3}' | sort -u) || true
for func in $readme_functions; do
  check "the README's function $func is defined in the workspace" function_is_defined "$func"
done

check "the sample config parses" sample_config_parses

find_openssl() {
  local candidate
  for candidate in openssl /opt/homebrew/opt/openssl@3/bin/openssl /usr/local/opt/openssl@3/bin/openssl; do
    if command -v "$candidate" >/dev/null 2>&1; then
      case $("$candidate" version 2>/dev/null) in
        OpenSSL\ 1.1.*|OpenSSL\ 3*|OpenSSL\ [4-9]*) command -v "$candidate"; return 0 ;;
      esac
    fi
  done
  return 1
}

sourced() {
  local ossl=$1
  shift
  PATH="$(dirname "$ossl"):$PATH" bash -c 'set -Eeuo pipefail; source deploy/install.sh; tmp=$(mktemp -d); trap '"'"'rm -rf "$tmp"'"'"' EXIT; "$@"' bash "$@"
}

verifies() {
  local ossl=$1 manifest=$2 key=$3 out=$4
  sourced "$ossl" verify_signed "$manifest" "$(head -n 1 "$key")" "$out" 2>/dev/null
}

refuses() {
  ! verifies "$@"
}

allots() {
  local ossl=$1 subuid=$2 expected=$3
  local got
  got=$(PATH="$(dirname "$ossl"):$PATH" bash -c 'set -Eeuo pipefail; source deploy/install.sh; SUBID_FILES=("$1"); getent() { printf "root:x:0:0::/root:/bin/sh\nheld:x:165536:1::/:/bin/sh\n"; }; allot_uid_range >/dev/null; printf "%s %s\n" "$uid_from" "$uid_to"' bash "$subuid")
  [ "$got" = "$expected" ]
}

tamper() {
  local field=$1 source=$2 target=$3
  sed -E -e "s/^(  \"$field\": \")Z/\\1Y/" -e t -e "s/^(  \"$field\": \")./\\1Z/" "$source" >"$target"
}

differs() {
  local signed=$1
  shift
  local tampered
  for tampered in "$@"; do
    if cmp -s "$signed" "$tampered"; then
      return 1
    fi
  done
}

if ossl=$(find_openssl); then
  echo "the signature check of install.sh is exercised with $ossl"
  rehearsal=$(mktemp -d)
  trap 'rm -rf "$rehearsal"' EXIT
  cargo run -q -p keel-release -- keygen --out "$rehearsal/keys" --comment "install rehearsal" >/dev/null
  cargo run -q -p keel-release -- keygen --out "$rehearsal/other" --comment "another key" >/dev/null
  printf 'archive' >"$rehearsal/archive"
  cargo run -q -p keel-release -- sign --key "$rehearsal/keys/release.key" --version 0.1.0 --archive "$rehearsal/archive" \
    --min-upgrade-from 0.1.0 --contract 1 --notes "install rehearsal" --out "$rehearsal/manifest.json" >/dev/null
  tamper signature "$rehearsal/manifest.json" "$rehearsal/signature-tampered.json"
  tamper payload "$rehearsal/manifest.json" "$rehearsal/payload-tampered.json"
  printf '{"version":"0.1.0"}\n' >"$rehearsal/unsigned.json"
  check "install.sh accepts a manifest signed by the given key and yields its payload" verifies "$ossl" "$rehearsal/manifest.json" "$rehearsal/keys/release.pub" "$rehearsal/payload.json"
  check "the yielded payload names the version and the sha256 of the archive" test "$(sourced "$ossl" json_string "$rehearsal/payload.json" version) $(sourced "$ossl" json_string "$rehearsal/payload.json" sha256)" = "0.1.0 $(python3 -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$rehearsal/archive")"
  check "install.sh refuses the manifest under another key" refuses "$ossl" "$rehearsal/manifest.json" "$rehearsal/other/release.pub" "$rehearsal/refused.json"
  check "install.sh refuses the manifest under the embedded key" refuses "$ossl" "$rehearsal/manifest.json" crates/release/keys/release.pub "$rehearsal/refused.json"
  check "the tampered signature and payload differ from the signed manifest" differs "$rehearsal/manifest.json" "$rehearsal/signature-tampered.json" "$rehearsal/payload-tampered.json"
  check "install.sh refuses a tampered signature" refuses "$ossl" "$rehearsal/signature-tampered.json" "$rehearsal/keys/release.pub" "$rehearsal/refused.json"
  check "install.sh refuses a tampered payload" refuses "$ossl" "$rehearsal/payload-tampered.json" "$rehearsal/keys/release.pub" "$rehearsal/refused.json"
  check "install.sh refuses a manifest that is not a signed envelope" refuses "$ossl" "$rehearsal/unsigned.json" "$rehearsal/keys/release.pub" "$rehearsal/refused.json"
  check "a refused manifest yields no payload" test ! -e "$rehearsal/refused.json"
  printf 'keel:100000:65536\n' >"$rehearsal/subuid"
  check "install.sh skips uid ranges taken by a subordinate range or an account" allots "$ossl" "$rehearsal/subuid" "231072 296607"
  printf '' >"$rehearsal/empty-subuid"
  check "install.sh allots the first range when nothing claims it" allots "$ossl" "$rehearsal/empty-subuid" "100000 165535"
else
  echo "no OpenSSL 1.1.1 or newer on this host; the signature check of install.sh is not exercised here"
fi

if [ "$problems" -gt 0 ]; then
  echo "$problems problem(s)" >&2
  exit 1
fi
echo "the installer is checked"
