#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: hack/check-size.sh <binary> [<cap file>]" >&2
  exit 2
fi
binary=$1
cap_file=${2:-"$(dirname "$0")/size-cap"}

if [ ! -f "$binary" ]; then
  echo "$binary is not a file" >&2
  exit 2
fi
cap=$(tr -d '[:space:]' < "$cap_file")
case "$cap" in
  '' | *[!0-9]*)
    echo "$cap_file does not hold a number of bytes" >&2
    exit 2
    ;;
esac

size=$(wc -c < "$binary" | tr -d '[:space:]')
megabytes() {
  awk -v bytes="$1" 'BEGIN { printf "%.1f", bytes / 1000000 }'
}

echo "$binary: $size bytes ($(megabytes "$size") MB) against the cap of $cap bytes ($(megabytes "$cap") MB) in $cap_file"
if [ "$size" -gt "$cap" ]; then
  echo "the binary is $((size - cap)) bytes above the cap" >&2
  exit 1
fi
echo "headroom: $((cap - size)) bytes ($(megabytes $((cap - size))) MB)"
