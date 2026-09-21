#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

keel=${1:-}
problems=0
scripts=(deploy/install.sh deploy/uninstall.sh hack/check-install.sh hack/update-vm.sh hack/backup-vm.sh)
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
check "install.sh installs curl when it is absent" grep -qE 'apt_install curl ca-certificates' deploy/install.sh
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
check "keel-privileged.service runs as root" test "$(unit_value keel-privileged.service User)" = "root"
check "keel-update.service runs as root" test "$(unit_value keel-update.service User)" = "root"
check "keel-update.service is a oneshot" test "$(unit_value keel-update.service Type)" = "oneshot"
check "keel-update.path watches the update request" test "$(unit_value keel-update.path PathExists)" = "/var/lib/keel/update.request"
check "keel-update.path starts keel-update.service" test "$(unit_value keel-update.path Unit)" = "keel-update.service"
check "keel-privileged.service runs privileged with the socket and the policy" test "$(unit_value keel-privileged.service ExecStart)" = "/opt/keel/bin/current privileged --socket /run/keel/agent.sock --policy /etc/keel/agent.yaml"
check "keel-update.service runs the update from the request" test "$(unit_value keel-update.service ExecStart)" = "/opt/keel/bin/current privileged update --from-request"
check "keel.service alone owns the runtime directory" test "$(grep -l '^RuntimeDirectory=keel$' deploy/systemd/* | tr '\n' ' ')" = "deploy/systemd/keel.service "
check "keel.service alone drops privileges with NoNewPrivileges" test "$(grep -l '^NoNewPrivileges=yes$' deploy/systemd/* | tr '\n' ' ')" = "deploy/systemd/keel.service "
check "the kernel unit is the only one that is not root" test "$(grep -L '^User=root$' deploy/systemd/*.service | tr '\n' ' ')" = "deploy/systemd/keel.service "
check "update-vm.sh --help prints the usage and exits 0" prints_usage hack/update-vm.sh
check "backup-vm.sh --help prints the usage and exits 0" prints_usage hack/backup-vm.sh
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

if [ "$problems" -gt 0 ]; then
  echo "$problems problem(s)" >&2
  exit 1
fi
echo "the installer is checked"
