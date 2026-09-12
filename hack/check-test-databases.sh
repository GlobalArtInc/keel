#!/usr/bin/env bash
set -euo pipefail

if [ -z "${KEEL_TEST_DATABASE_URL:-}" ]; then
  echo "KEEL_TEST_DATABASE_URL is not set" >&2
  exit 2
fi

leftover=$(psql "$KEEL_TEST_DATABASE_URL" -tAc "SELECT datname FROM pg_database WHERE datname LIKE 'keel\_test\_%'")
if [ -n "$leftover" ]; then
  echo "test databases remain after the run:" >&2
  echo "$leftover" >&2
  exit 1
fi
echo "no test database remains"
