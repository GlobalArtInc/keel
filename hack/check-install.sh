#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

keel=${1:-}
problems=0
scripts=(deploy/install.sh deploy/uninstall.sh hack/check-install.sh)

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

unit_subcommand=$(sed -nE 's|^ExecStart=/opt/keel/bin/current ([a-z]+).*|\1|p' deploy/systemd/keel.service)
check "keel.service starts a subcommand" test -n "$unit_subcommand"
check "the unit's subcommand $unit_subcommand is in keel --help" lists_subcommand "$help" "$unit_subcommand"

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
