#!/usr/bin/env bash
set -Eeuo pipefail

OPT=/opt/keel
ETC=/etc/keel
LIB=/var/lib/keel
RUN=/run/keel
UNIT_DIR=/etc/systemd/system
UNITS=(keel.service keel-privileged.service keel-update.path keel-update.service)
ENABLED_UNITS=(keel.service keel-privileged.service keel-update.path)
LINK=/usr/local/bin/keel
CERT=$LIB/tls/cert.pem
DEFAULT_LISTEN=127.0.0.1:1500
DEFAULT_ADMIN="admin"
ROOT_ACCOUNT=00000000000000000000000000
PG_SOCKET_DIR=/run/postgresql
PG_MIN_VERSION=150000
READY_TIMEOUT=${KEEL_INSTALL_READY_TIMEOUT:-90}

here=$(cd "$(dirname "$0")" && pwd)
products=""
binary=""
listen=$DEFAULT_LISTEN
admin=$DEFAULT_ADMIN
yes=0
step="parsing the arguments"
tmp=""
unit_installed=0
opt_installed=0
link_installed=0
service_started=0
touched=0
db_created=0
install_curl=0

usage() {
  cat <<EOF
usage: install.sh --product <id>[,<id>] --binary <path|url> [options]

Installs keel on a clean Debian or Ubuntu host: Postgres from the distribution
when no server answers on the unix socket, the role and database keel, the unix
user keel, $OPT, $ETC, $LIB, the systemd units keel.service,
keel-privileged.service, keel-update.path and keel-update.service, the
enabled products and the first administrator.

  --product <id>[,<id>]   product bundles to enable (required), for example demo
  --binary <path|url>     the keel binary, the tar.zst artifact of the Build
                          workflow, or an https URL of either; a .sha256 file
                          next to it is verified. Required until M4: it stands
                          in for the release channel, and a missing --binary
                          will then mean the channel's current version
  --listen <addr:port>    listen address written to $ETC/config.yaml
                          (default $DEFAULT_LISTEN, the kernel's own default)
  --admin <login>         login of the first administrator (default $DEFAULT_ADMIN)
  --yes                   do not ask for confirmation
  -h, --help              print this text

The administrator's password is generated, printed once and never written to
disk. Run as root.
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
  say "install.sh: $1" >&2
  exit 1
}

on_error() {
  local code=$?
  trap - ERR
  if [ "$BASH_SUBSHELL" -eq 0 ]; then
    say "install.sh: failed during: $step (exit $code)" >&2
  fi
  exit "$code"
}

on_exit() {
  local code=$?
  trap - EXIT
  if [ "$code" -ne 0 ]; then
    if [ "$service_started" -eq 1 ]; then
      systemctl stop "${UNITS[@]}" >/dev/null 2>&1 || true
    fi
    if [ "$unit_installed" -eq 1 ]; then
      systemctl disable "${ENABLED_UNITS[@]}" >/dev/null 2>&1 || true
      local unit
      for unit in "${UNITS[@]}"; do
        rm -f "$UNIT_DIR/$unit"
      done
      systemctl daemon-reload >/dev/null 2>&1 || true
      say "the units ${UNITS[*]} were disabled and removed" >&2
    fi
    if [ "$link_installed" -eq 1 ]; then
      rm -f "$LINK"
    fi
    if [ "$opt_installed" -eq 1 ]; then
      rm -rf "$OPT"
      say "$OPT was removed" >&2
    fi
    if [ "$touched" -eq 1 ]; then
      local -a left=()
      [ -e "$ETC" ] && left+=("$ETC")
      [ -e "$LIB" ] && left+=("$LIB")
      getent passwd keel >/dev/null 2>&1 && left+=("the unix user keel")
      [ "$db_created" -eq 1 ] && left+=("the database and role keel")
      if [ "${#left[@]}" -gt 0 ]; then
        say "install.sh: the installation did not complete; left in place: $(IFS=,; printf '%s' "${left[*]}" | sed 's/,/, /g')" >&2
        say "install.sh: run $here/uninstall.sh --purge to clear them before trying again." >&2
      else
        say "install.sh: the installation did not complete; nothing of keel is left on the host" >&2
      fi
    fi
  fi
  if [ -n "$tmp" ] && [ -d "$tmp" ]; then
    rm -rf "$tmp"
  fi
  exit "$code"
}

need() {
  local tool
  for tool in "$@"; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      fail "$tool is not installed and is needed for this step"
    fi
  done
}

in_container() {
  if command -v systemd-detect-virt >/dev/null 2>&1 && systemd-detect-virt --container --quiet; then
    return 0
  fi
  if [ -f /.dockerenv ]; then
    return 0
  fi
  if grep -qE '(docker|lxc|containerd|kubepods|podman|libpod)' /proc/1/cgroup 2>/dev/null; then
    return 0
  fi
  return 1
}

parse_args() {
  while [ $# -gt 0 ]; do
    case $1 in
      --product) [ $# -ge 2 ] || fail "--product needs a value"; products=$2; shift 2 ;;
      --product=*) products=${1#*=}; shift ;;
      --binary) [ $# -ge 2 ] || fail "--binary needs a value"; binary=$2; shift 2 ;;
      --binary=*) binary=${1#*=}; shift ;;
      --listen) [ $# -ge 2 ] || fail "--listen needs a value"; listen=$2; shift 2 ;;
      --listen=*) listen=${1#*=}; shift ;;
      --admin) [ $# -ge 2 ] || fail "--admin needs a value"; admin=$2; shift 2 ;;
      --admin=*) admin=${1#*=}; shift ;;
      --yes|-y) yes=1; shift ;;
      -h|--help) usage; exit 0 ;;
      *) usage >&2; fail "unknown argument: $1" ;;
    esac
  done
  if [ -z "$products" ]; then
    usage >&2
    fail "--product is required"
  fi
  if [ -z "$binary" ]; then
    usage >&2
    fail "--binary is required"
  fi
  IFS=, read -r -a product_ids <<<"$products"
  local id
  for id in "${product_ids[@]}"; do
    if ! [[ $id =~ ^[a-z][a-z0-9]*$ ]]; then
      fail "the product id \"$id\" is not a lowercase identifier"
    fi
  done
  if ! [[ $listen =~ ^(\[[0-9a-fA-F:.]+\]|[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+):([0-9]{1,5})$ ]]; then
    fail "--listen must be <ipv4>:<port> or [<ipv6>]:<port>, not \"$listen\""
  fi
  local port=${BASH_REMATCH[2]}
  if [ "$port" -lt 1 ] || [ "$port" -gt 65535 ]; then
    fail "--listen names the port $port, which is not between 1 and 65535"
  fi
  if ! [[ $admin =~ ^[A-Za-z0-9._@-]{1,64}$ ]]; then
    fail "--admin must be 1 to 64 characters of letters, digits, . _ @ -"
  fi
  if [[ $admin =~ ^(true|false|null|-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?)$ ]]; then
    fail "--admin \"$admin\" would be read by keel ctl as a JSON number or keyword, not as a login"
  fi
  case $binary in
    https://*) ;;
    http://*) fail "--binary accepts https URLs only" ;;
    /*) ;;
    *) binary=$PWD/$binary ;;
  esac
}

listen_host() {
  local value=${listen%:*}
  value=${value#[}
  value=${value%]}
  printf '%s\n' "$value"
}

listen_port() {
  printf '%s\n' "${listen##*:}"
}

probe_host() {
  local host
  host=$(listen_host)
  case $host in
    0.0.0.0) printf '127.0.0.1\n' ;;
    ::) printf '::1\n' ;;
    *) printf '%s\n' "$host" ;;
  esac
}

shown_host() {
  local host
  host=$(listen_host)
  case $host in
    0.0.0.0|::)
      local first
      first=$(hostname -I 2>/dev/null | awk '{print $1}')
      printf '%s\n' "${first:-127.0.0.1}"
      ;;
    *:*) printf '[%s]\n' "$host" ;;
    *) printf '%s\n' "$host" ;;
  esac
}

listen_unspecified() {
  case $(listen_host) in
    0.0.0.0|::) return 0 ;;
    *) return 1 ;;
  esac
}

check_host() {
  begin "checking the host"
  if [ "$(id -u)" -ne 0 ]; then
    fail "run as root"
  fi
  if in_container; then
    if [ "${KEEL_INSTALL_ALLOW_CONTAINER:-0}" = "1" ]; then
      say "warning: this looks like a container; continuing because KEEL_INSTALL_ALLOW_CONTAINER=1 is set"
    else
      fail "this host is a container; keel is installed on a clean server or virtual machine only"
    fi
  fi
  if [ -e "$OPT" ] || [ -e "$LIB" ]; then
    fail "$OPT or $LIB already exists; this host carries an installation. Use \"keel product enable <id>\" to add a product, or uninstall.sh --purge to start over"
  fi
  if ! command -v apt-get >/dev/null 2>&1; then
    fail "apt-get is not available; the installer supports Debian and Ubuntu only"
  fi
  if [ ! -d /run/systemd/system ] || ! command -v systemctl >/dev/null 2>&1; then
    fail "systemd is not the running init; the installer needs systemd"
  fi
  if [ "$(uname -s)" != "Linux" ]; then
    fail "this is not a Linux host"
  fi
  if ! command -v curl >/dev/null 2>&1; then
    install_curl=1
  fi
  need tar runuser useradd groupadd getent sha256sum install hostname awk
  local name
  name=$(sed -nE 's/^PRETTY_NAME="?([^"]*)"?$/\1/p' /etc/os-release 2>/dev/null || true)
  say "host: ${name:-unknown}, $(uname -m), systemd $(systemctl --version | head -n 1 | awk '{print $2}')"
}

confirm() {
  say ""
  say "This will install keel on $(hostname):"
  say "  products:   $products"
  say "  binary:     $binary"
  say "  listen:     $listen"
  say "  admin:      $admin"
  say "  paths:      $OPT $ETC $LIB $RUN"
  say "  postgres:   from the distribution unless a server answers on $PG_SOCKET_DIR"
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

pg_clusters_exist() {
  command -v pg_lsclusters >/dev/null 2>&1 && [ "$(pg_lsclusters --no-header 2>/dev/null | wc -l)" -gt 0 ]
}

apt_install() {
  begin "installing packages: $*"
  touched=1
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -q
  apt-get install -y -q --no-install-recommends "$@"
}

ensure_curl() {
  if [ "$install_curl" -eq 1 ]; then
    apt_install curl ca-certificates
  fi
  need curl
}

fetch_binary() {
  begin "fetching the binary"
  tmp=$(mktemp -d /tmp/keel-install.XXXXXX)
  local source name
  case $binary in
    https://*)
      name=${binary##*/}
      say "downloading $binary"
      curl -fsSL --proto '=https' --proto-redir '=https' -o "$tmp/$name" "$binary"
      if curl -fsSL --proto '=https' --proto-redir '=https' -o "$tmp/$name.sha256" "$binary.sha256" 2>/dev/null; then
        say "downloaded $binary.sha256"
      else
        rm -f "$tmp/$name.sha256"
      fi
      source=$tmp/$name
      ;;
    *)
      if [ ! -f "$binary" ]; then
        fail "$binary is not a file"
      fi
      name=${binary##*/}
      source=$binary
      ;;
  esac
  if [ -f "$source.sha256" ]; then
    begin "verifying the checksum of $name"
    (cd "$(dirname "$source")" && sha256sum -c --strict "$name.sha256")
  else
    say "no $name.sha256 next to $binary; the checksum is not verified"
  fi
  begin "unpacking the release under $OPT/staging"
  touched=1
  install -d -m 0755 -o root -g root "$OPT" "$OPT/staging"
  opt_installed=1
  local magic
  magic=$(head -c 4 "$source" | od -An -c | tr -d ' \n')
  if [ "$magic" = "177ELF" ]; then
    install -m 0755 -o root -g root "$source" "$staged"
  else
    if ! command -v zstd >/dev/null 2>&1; then
      apt_install zstd
      begin "unpacking the release under $OPT/staging"
    fi
    if ! zstd -d -c "$source" | tar -x -C "$OPT/staging" 2>/dev/null; then
      fail "$name is neither an ELF binary nor a tar.zst archive"
    fi
    if [ ! -f "$staged" ]; then
      fail "$name does not carry keel at the root of the archive"
    fi
    chown -R root:root "$OPT/staging"
    chmod -R u=rwX,go=rX "$OPT/staging"
    chmod 0755 "$staged"
  fi
  if ! version=$("$staged" --version 2>&1); then
    fail "the binary does not run on this host ($(uname -m)): $version"
  fi
  version=${version#keel }
  if [ -z "$version" ]; then
    fail "the binary did not print its version"
  fi
  say "keel $version"
}

ensure_postgres() {
  begin "checking for a Postgres server on $PG_SOCKET_DIR"
  local port
  if port=$(pg_socket_port); then
    say "a server answers on $PG_SOCKET_DIR/.s.PGSQL.$port"
  elif pg_clusters_exist; then
    pg_lsclusters >&2 || true
    fail "a Postgres cluster exists on this host but no server answers on $PG_SOCKET_DIR; start it (systemctl start postgresql) and run the installer again"
  else
    apt_install postgresql
    begin "starting the Postgres server"
    systemctl enable --now postgresql.service
    begin "waiting for the Postgres server"
    local waited=0
    until port=$(pg_socket_port); do
      if [ "$waited" -ge 60 ]; then
        fail "no server answered on $PG_SOCKET_DIR within 60 s after installing postgresql"
      fi
      sleep 1
      waited=$((waited + 1))
    done
    say "postgresql is installed and answers on $PG_SOCKET_DIR/.s.PGSQL.$port"
  fi
  pg_port=$port
  need psql
  begin "checking the Postgres version"
  local number
  number=$(pg_sql "SHOW server_version_num")
  if [ "$number" -lt "$PG_MIN_VERSION" ]; then
    fail "the Postgres server is version $number; keel needs 15 or newer"
  fi
  say "server version $(pg_sql "SHOW server_version")"
}

pg_sql() {
  (cd / && runuser -u postgres -- env -i PATH=/usr/bin:/bin HOME=/var/lib/postgresql psql -X -U postgres -h "$PG_SOCKET_DIR" -p "$pg_port" -d postgres -qAt -v ON_ERROR_STOP=1 -c "$1")
}

ensure_role_and_database() {
  begin "creating the Postgres role keel and the database keel"
  if [ "$(pg_sql "SELECT 1 FROM pg_roles WHERE rolname = 'keel'")" = "1" ]; then
    say "the role keel exists"
  else
    touched=1
    db_created=1
    pg_sql "CREATE ROLE keel LOGIN" >/dev/null
    say "created the role keel"
  fi
  if [ "$(pg_sql "SELECT 1 FROM pg_database WHERE datname = 'keel'")" = "1" ]; then
    say "the database keel exists"
  else
    touched=1
    db_created=1
    pg_sql "CREATE DATABASE keel OWNER keel" >/dev/null
    say "created the database keel owned by keel"
  fi
}

ensure_user() {
  begin "creating the unix user and group keel"
  if getent group keel >/dev/null; then
    say "the group keel exists"
  else
    touched=1
    groupadd --system keel
    say "created the group keel"
  fi
  if getent passwd keel >/dev/null; then
    say "the user keel exists"
  else
    touched=1
    useradd --system --gid keel --home-dir "$LIB" --no-create-home --shell /usr/sbin/nologin --comment "keel" keel
    say "created the user keel"
  fi
}

check_peer_auth() {
  begin "checking peer authentication as keel"
  local answer
  if ! answer=$(cd / && runuser -u keel -- env -i PATH=/usr/bin:/bin HOME="$LIB" psql -X -U keel -h "$PG_SOCKET_DIR" -p "$pg_port" -d keel -qAt -c "SELECT current_user" 2>&1); then
    say "$answer" >&2
    fail "the unix user keel cannot connect to the database keel over $PG_SOCKET_DIR; pg_hba.conf must allow peer authentication for local connections"
  fi
  say "connected as $answer"
}

release_file() {
  local name=$1 fallback=$2
  if [ -f "$OPT/versions/$version/$name" ]; then
    say "$name comes with the release"
    return 0
  fi
  if [ ! -f "$fallback" ]; then
    return 1
  fi
  install -D -m 0644 -o root -g root "$fallback" "$OPT/versions/$version/$name"
  say "the release does not carry $name; taken from $fallback"
}

install_files() {
  begin "installing the release under $OPT/versions/$version"
  install -d -m 0755 -o root -g root "$OPT/versions" "$OPT/bin"
  mv "$OPT/staging" "$OPT/versions/$version"
  ln -sfn "$OPT/versions/$version/keel" "$OPT/bin/current"
  release_file config-sample.yaml "$here/config-sample.yaml" || say "no config-sample.yaml in the release or next to install.sh; none installed"
  if [ -L "$LINK" ] || [ ! -e "$LINK" ]; then
    ln -sfn "$OPT/bin/current" "$LINK"
    link_installed=1
  else
    say "$LINK exists and is not a symlink; left alone"
  fi
  begin "creating $ETC and $LIB"
  install -d -m 0750 -o root -g keel "$ETC" "$ETC/templates"
  install -d -m 0700 -o keel -g keel "$LIB" "$LIB/tls" "$LIB/backups" "$LIB/plugins" "$LIB/tasks"
  begin "writing $ETC/config.yaml"
  {
    if [ "$listen" != "$DEFAULT_LISTEN" ]; then
      printf 'listen: "%s"\n' "$listen"
    fi
    if [ "$pg_port" != "5432" ]; then
      printf 'database: "postgres:///keel?host=%s&port=%s"\n' "$PG_SOCKET_DIR" "$pg_port"
    fi
  } >"$ETC/config.yaml"
  chown root:keel "$ETC/config.yaml"
  chmod 0640 "$ETC/config.yaml"
  if [ -s "$ETC/config.yaml" ]; then
    sed 's/^/  /' "$ETC/config.yaml"
  else
    say "  (empty: every key keeps the kernel's default)"
  fi
}

install_units() {
  begin "installing the units ${UNITS[*]} from $OPT/versions/$version/deploy/systemd"
  local unit
  for unit in "${UNITS[@]}"; do
    if ! release_file "deploy/systemd/$unit" "$here/systemd/$unit"; then
      fail "the release does not carry deploy/systemd/$unit and $here/systemd/$unit is missing; copy the whole deploy/ directory next to install.sh"
    fi
    install -m 0644 -o root -g root "$OPT/versions/$version/deploy/systemd/$unit" "$UNIT_DIR/$unit"
  done
  unit_installed=1
  systemctl daemon-reload
  systemctl enable "${ENABLED_UNITS[@]}"
}

run_keel() {
  (cd "$LIB" && runuser -u keel -- env -i HOME="$LIB" USER=keel LOGNAME=keel PATH=/usr/sbin:/usr/bin:/sbin:/bin "$OPT/bin/current" "$@")
}

prepare_database() {
  begin "running the kernel migrations as keel"
  run_keel migrate
  local id
  for id in "${product_ids[@]}"; do
    begin "enabling the product $id"
    run_keel product enable "$id"
  done
  begin "running the migrations of the enabled products as keel"
  run_keel migrate
}

ready_status() {
  local host=$1 port=$2
  if [ ! -f "$CERT" ]; then
    printf 'no-certificate\n'
    return
  fi
  case $host in
    *:*) host="[$host]" ;;
  esac
  curl -s -o /dev/null -w '%{http_code}' --connect-timeout 5 --max-time 10 --cacert "$CERT" "https://$host:$port/readyz" 2>/dev/null || true
  printf '\n'
}

start_service() {
  begin "starting keel-privileged.service and keel-update.path"
  systemctl start keel-privileged.service keel-update.path
  service_started=1
  begin "starting keel.service"
  systemctl start keel.service
  local host port waited status
  host=$(probe_host)
  port=$(listen_port)
  begin "waiting for /readyz on https://$listen with the certificate $CERT"
  waited=0
  while :; do
    if ! systemctl is-active --quiet keel.service; then
      journalctl -u keel.service --no-pager -n 40 >&2 || true
      fail "keel.service stopped before it was ready; see journalctl -u keel"
    fi
    status=$(ready_status "$host" "$port")
    if [ "$status" = "200" ]; then
      say "ready after ${waited}s"
      return
    fi
    if [ "$waited" -ge "$READY_TIMEOUT" ]; then
      journalctl -u keel.service --no-pager -n 40 >&2 || true
      fail "/readyz did not answer 200 within ${READY_TIMEOUT}s (last status: ${status:-none}); see journalctl -u keel"
    fi
    sleep 1
    waited=$((waited + 1))
  done
}

generate_password() {
  local candidate
  while :; do
    if command -v openssl >/dev/null 2>&1; then
      candidate=$(openssl rand -base64 96 | tr -dc 'A-Za-z0-9' | head -c 24)
    else
      candidate=$(head -c 1024 /dev/urandom | tr -dc 'A-Za-z0-9' | head -c 24)
    fi
    if [ "${#candidate}" -ne 24 ]; then
      continue
    fi
    if ! printf '%s' "$candidate" | grep -q '[a-df-zA-DF-Z]'; then
      continue
    fi
    if ! printf '%s' "$candidate" | grep -q '[0-9]'; then
      continue
    fi
    printf '%s\n' "$candidate"
    return
  done
}

create_admin() {
  begin "creating the administrator $admin"
  local created id
  if ! created=$(run_keel ctl keel.principal.create "login=$admin" level=admin "account=$ROOT_ACCOUNT" 2>&1); then
    say "$created" >&2
    fail "keel.principal.create refused the administrator"
  fi
  id=$(printf '%s\n' "$created" | grep -oE '"id": *"[0-7][0-9A-HJKMNP-TV-Z]{25}"' | head -n 1 | grep -oE '[0-7][0-9A-HJKMNP-TV-Z]{25}') || id=""
  if [ -z "$id" ]; then
    say "$created" >&2
    fail "keel.principal.create answered without an id"
  fi
  password=$(generate_password)
  local answer
  if ! answer=$(run_keel ctl auth.password.set "principal=$id" "password=$password" 2>&1); then
    say "$answer" >&2
    fail "auth.password.set refused the generated password"
  fi
  admin_id=$id
}

certificate_fingerprint() {
  local fingerprint=""
  fingerprint=$(journalctl -u keel.service --no-pager -o cat 2>/dev/null | grep -oE '([0-9A-F]{2}:){31}[0-9A-F]{2}' | tail -n 1) || true
  if [ -z "$fingerprint" ] && command -v openssl >/dev/null 2>&1 && [ -f "$CERT" ]; then
    fingerprint=$(openssl x509 -in "$CERT" -noout -fingerprint -sha256 2>/dev/null | sed 's/^.*=//') || true
  fi
  printf '%s\n' "${fingerprint:-(not found; see journalctl -u keel for the line \"tls certificate\")}"
}

report() {
  local host port name fingerprint
  host=$(shown_host)
  port=$(listen_port)
  name=$(hostname)
  fingerprint=$(certificate_fingerprint)
  say ""
  say "keel $version is installed on $(hostname)"
  say ""
  say "  products:     $products"
  if [ "$link_installed" -eq 1 ]; then
    say "  binary:       $OPT/versions/$version/keel (also as $LINK)"
  else
    say "  binary:       $OPT/versions/$version/keel"
  fi
  if [ -f "$OPT/versions/$version/config-sample.yaml" ]; then
    say "  config:       $ETC/config.yaml (sample: $OPT/versions/$version/config-sample.yaml)"
  else
    say "  config:       $ETC/config.yaml"
  fi
  say "  data:         $LIB"
  say "  socket:       $RUN/core.sock"
  say "  units:        keel.service, keel-privileged.service and keel-update.path (enabled, running);"
  say "                keel-update.service runs once per update request"
  say "  postgres:     database keel, role keel, peer authentication on $PG_SOCKET_DIR port $pg_port"
  say ""
  say "  URL:          https://$host:$port/"
  if listen_unspecified; then
    say "  names:        $name, localhost, 127.0.0.1 and ::1 are the names the certificate carries; a browser"
    say "                reaching the host by another address, $host included, warns about the name as well"
  fi
  say "  certificate:  $CERT (self-signed, SHA-256 $fingerprint)"
  say "  login:        $admin"
  say "  password:     $password"
  say "  principal:    $admin_id"
  say ""
  say "The password is shown once and is stored nowhere by the installer."
  say "The URL serves the panel over TLS with the self-signed certificate above; a browser warns once"
  say "and shows the fingerprint, which must match the one printed here. A reverse proxy in front"
  say "trusts $CERT and is named in trusted_proxies of $ETC/config.yaml."
  say "The same login over the API:"
  if listen_unspecified; then
    say "  curl -sS --cacert $CERT --resolve \"$name:$port:$host\" -X POST https://$name:$port/api/v1/call/auth.login \\"
  else
    say "  curl -sS --cacert $CERT -X POST https://$host:$port/api/v1/call/auth.login \\"
  fi
  say "    -H 'Content-Type: application/json' -d '{\"login\":\"$admin\",\"password\":\"<the password above>\"}'"
  say ""
  say "Checks:"
  say "  systemctl status keel keel-privileged keel-update.path"
  say "  journalctl -u keel -f"
  say "  runuser -u keel -- keel ctl keel.account.list --out table"
  say "  runuser -u keel -- keel product list"
  say ""
  say "An operator's certificate goes into tls.cert and tls.key of $ETC/config.yaml; ACME, the host policy"
  say "of the privileged unit, the update button and the setup link arrive with later milestones (see deploy/README.md)."
}

main() {
  parse_args "$@"
  trap on_error ERR
  trap on_exit EXIT
  check_host
  confirm
  ensure_curl
  fetch_binary
  ensure_postgres
  ensure_role_and_database
  ensure_user
  check_peer_auth
  install_files
  install_units
  prepare_database
  start_service
  create_admin
  report
}

version=""
staged=$OPT/staging/keel
pg_port=5432
password=""
admin_id=""
product_ids=()
main "$@"
