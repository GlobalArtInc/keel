#!/usr/bin/env bash
if [ -z "${BASH_VERSION:-}" ]; then
  echo "install.sh runs under bash: curl -fsSL https://dl.globalart.hu/keel/install.sh | bash -s -- --product <id>" >&2
  exit 2
fi
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
AGENT_POLICY=$ETC/agent.yaml
DEFAULT_LISTEN=127.0.0.1:1500
DEFAULT_CHANNEL=stable
DEFAULT_CHANNEL_URL=https://dl.globalart.hu/keel/
ARCHIVE_TARGET=x86_64-linux-musl
RELEASE_KEY='ed25519 b3maEuYryi/YLJRRc810KxRsMEbIdsizi55QlKy0+cg= keel release key 2026-09-21'
ED25519_SPKI_PREFIX='\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00'
UID_RANGE_START=100000
UID_RANGE_SIZE=65536
UID_RANGE_CEILING=4000000000
SUBID_FILES=(/etc/subuid /etc/subgid)
PG_SOCKET_DIR=/run/postgresql
PG_MIN_VERSION=150000
READY_TIMEOUT=${KEEL_INSTALL_READY_TIMEOUT:-90}
MANIFEST_MAX_BYTES=1048576
PANEL_PACKAGES=(nginx php-fpm mariadb-server mariadb-client postfix dovecot-core dovecot-imapd dovecot-lmtpd vsftpd libpam-pwdfile)
PANEL_UNITS=(nginx.service mariadb.service postfix.service dovecot.service vsftpd.service)
PANEL_PHP_UNITS='php*-fpm.service'
PANEL_WWW=/var/www
PANEL_CERTS=/etc/ssl/keel
PANEL_CHALLENGES=/var/lib/keel-panel-acme
PANEL_DUMPS=/var/lib/keel-panel-dumps
PANEL_BACKUPS=/var/lib/keel-panel-backups
PANEL_TRANSFER=/var/lib/keel-panel-transfer
PANEL_UNIT_DROP_IN_DIR=$UNIT_DIR/keel.service.d
PANEL_UNIT_DROP_IN=$PANEL_UNIT_DROP_IN_DIR/keel-panel.conf
PANEL_MAIL_MAPS=/etc/postfix/keel
PANEL_MAIL_MAP_NAMES=(domains mailboxes aliases)
PANEL_MAIL_PASSWD_DIR=/etc/dovecot/keel
PANEL_MAIL_PASSWD=$PANEL_MAIL_PASSWD_DIR/keel-panel-passwd
PANEL_MAILDIRS=/var/lib/keel-panel-mail
PANEL_MAIL_USER=vmail
PANEL_MAIL_DROP_IN=/etc/dovecot/conf.d/90-keel-panel.conf
PANEL_FTP_CONF=/etc/vsftpd.conf
PANEL_FTP_PAM=keel-panel-ftp
PANEL_PAM_DIR=/etc/pam.d
PANEL_FTP_DIR=/etc/vsftpd/keel
PANEL_FTP_PASSWD=$PANEL_FTP_DIR/keel-panel-passwd
PANEL_FTP_USERS=/etc/vsftpd/keel-users
NOLOGIN=/usr/sbin/nologin

here=$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)
products=""
binary=""
channel=$DEFAULT_CHANNEL
channel_url=$DEFAULT_CHANNEL_URL
channel_url_unsafe='[^][A-Za-z0-9._~:/@%+=,;-]'
insecure_rehearsal_channel=0
requested_version=""
release_key_file=""
listen=$DEFAULT_LISTEN
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
install_openssl=0
policy_written=0
products_prepared=0

usage() {
  cat <<EOF
usage: install.sh --product <id>[,<id>] [options]

Installs keel on a clean Debian or Ubuntu host from the signed release channel:
the release manifest's Ed25519 signature is verified with the key embedded in
this script before anything is unpacked, then Postgres from the distribution
when no server answers on the unix socket, the role and database keel, the unix
user keel with the uid range the host agent may manage, $OPT, $ETC, $LIB, the
policy $AGENT_POLICY, the systemd units keel.service, keel-privileged.service,
keel-update.path and keel-update.service, and the enabled products. The first
administrator is created in the browser through the one-time setup link the
service prints at its first start; the installer prints that link.

A product that runs daemons on the host brings them: panel installs
${PANEL_PACKAGES[*]}
from the distribution, points postfix and dovecot at the virtual mailboxes and
vsftpd at the virtual FTP accounts the panel writes, prepares the roots of
$AGENT_POLICY it uses, and enables and starts
${PANEL_UNITS[*]} and $PANEL_PHP_UNITS.

  --product <id>[,<id>]   product bundles to enable (required), for example demo
  --channel <name>        the release channel, stable or beta (default $DEFAULT_CHANNEL)
  --channel-url <url>     the release channel's address, https (default $DEFAULT_CHANNEL_URL)
  --insecure-rehearsal-channel
                          admit a plain http:// --channel-url for a rehearsal channel on a
                          network you trust, and write update.channel_url and
                          update.insecure_rehearsal_channel: true into $ETC/config.yaml;
                          never for a release channel
  --version <ver>         a version of the channel instead of its head
  --release-key <file>    a public key file in place of the embedded one, for a rehearsal channel
  --binary <path|url>     a keel binary or tar.zst artifact of the Build workflow, or an
                          https URL of either, in place of the channel; a .sha256 file next
                          to it is verified, the manifest signature is not
  --listen <addr:port>    listen address written to $ETC/config.yaml
                          (default $DEFAULT_LISTEN, the kernel's own default)
  --yes                   do not ask for confirmation
  -h, --help              print this text

Run as root.
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
      if [ "$products_prepared" -eq 1 ]; then
        say "install.sh: the packages and the host configuration of the products stay installed; uninstall.sh does not remove them and install.sh prepares them again on the next attempt." >&2
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
      --channel) [ $# -ge 2 ] || fail "--channel needs a value"; channel=$2; shift 2 ;;
      --channel=*) channel=${1#*=}; shift ;;
      --channel-url) [ $# -ge 2 ] || fail "--channel-url needs a value"; channel_url=$2; shift 2 ;;
      --channel-url=*) channel_url=${1#*=}; shift ;;
      --insecure-rehearsal-channel) insecure_rehearsal_channel=1; shift ;;
      --version) [ $# -ge 2 ] || fail "--version needs a value"; requested_version=$2; shift 2 ;;
      --version=*) requested_version=${1#*=}; shift ;;
      --release-key) [ $# -ge 2 ] || fail "--release-key needs a value"; release_key_file=$2; shift 2 ;;
      --release-key=*) release_key_file=${1#*=}; shift ;;
      --listen) [ $# -ge 2 ] || fail "--listen needs a value"; listen=$2; shift 2 ;;
      --listen=*) listen=${1#*=}; shift ;;
      --yes|-y) yes=1; shift ;;
      -h|--help) usage; exit 0 ;;
      *) usage >&2; fail "unknown argument: $1" ;;
    esac
  done
  if [ -z "$products" ]; then
    usage >&2
    fail "--product is required"
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
  case $channel in
    stable|beta) ;;
    *) fail "--channel must be stable or beta, not \"$channel\"" ;;
  esac
  case $channel_url in
    https://*) ;;
    http://*)
      if [ "$insecure_rehearsal_channel" -ne 1 ]; then
        fail "--channel-url $channel_url is plain http; a release channel is read over https. A rehearsal channel on a network you trust is named explicitly with --insecure-rehearsal-channel"
      fi
      if [[ $channel_url =~ $channel_url_unsafe ]]; then
        fail "--channel-url $channel_url carries a character that is not written into $ETC/config.yaml; use letters, digits and . _ ~ : / @ % + = , ; - [ ]"
      fi
      say "warning: the channel $channel_url is reached over plain http; the manifest signature still protects the release"
      ;;
    *) fail "--channel-url must be an https:// address, not \"$channel_url\"" ;;
  esac
  channel_url=${channel_url%/}/
  if [ -n "$requested_version" ] && ! [[ $requested_version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    fail "--version must be a semantic version, not \"$requested_version\""
  fi
  if [ -n "$release_key_file" ]; then
    case $release_key_file in
      /*) ;;
      *) release_key_file=$PWD/$release_key_file ;;
    esac
    [ -f "$release_key_file" ] || fail "--release-key $release_key_file is not a file"
    RELEASE_KEY=$(head -n 1 "$release_key_file")
  fi
  if ! [[ $RELEASE_KEY =~ ^ed25519\ [A-Za-z0-9+/]{43}=( .*)?$ ]]; then
    fail "the release key is not \"ed25519 <base64 of 32 bytes> [comment]\""
  fi
  case $binary in
    '') ;;
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
  if [ -z "$binary" ] && [ "$(uname -m)" != "x86_64" ]; then
    fail "the channel publishes $ARCHIVE_TARGET alone; this host is $(uname -m). Pass --binary with a build for it"
  fi
  if ! command -v curl >/dev/null 2>&1; then
    install_curl=1
  fi
  if [ -z "$binary" ] && ! command -v openssl >/dev/null 2>&1; then
    install_openssl=1
  fi
  need tar runuser useradd groupadd getent sha256sum install hostname awk base64 sed
  local name
  name=$(sed -nE 's/^PRETTY_NAME="?([^"]*)"?$/\1/p' /etc/os-release 2>/dev/null || true)
  say "host: ${name:-unknown}, $(uname -m), systemd $(systemctl --version | head -n 1 | awk '{print $2}')"
}

confirm() {
  say ""
  say "This will install keel on $(hostname):"
  say "  products:   $products"
  if [ -n "$binary" ]; then
    say "  binary:     $binary (an override; the manifest signature is not checked)"
  else
    say "  channel:    $channel at $channel_url${requested_version:+, version $requested_version}"
    say "  key:        ${RELEASE_KEY#ed25519 }"
  fi
  say "  listen:     $listen"
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

ensure_tools() {
  local -a packages=()
  if [ "$install_curl" -eq 1 ]; then
    packages+=(curl ca-certificates)
  fi
  if [ "$install_openssl" -eq 1 ]; then
    packages+=(openssl)
  fi
  if [ "${#packages[@]}" -gt 0 ]; then
    apt_install "${packages[@]}"
  fi
  need curl
  if [ -z "$binary" ]; then
    need openssl
    case $(openssl version 2>/dev/null) in
      OpenSSL\ 1.1.*|OpenSSL\ 3*|OpenSSL\ [4-9]*) ;;
      *) fail "openssl is $(openssl version 2>/dev/null); the signature check needs OpenSSL 1.1.1 or newer" ;;
    esac
  fi
}

download() {
  local url=$1 out=$2
  case $url in
    https://*) curl -fsSL --proto '=https' --proto-redir '=https' -o "$out" "$url" ;;
    *) curl -fsSL -o "$out" "$url" ;;
  esac
}

json_string() {
  local file=$1 field=$2
  sed -nE "s/^[[:space:]]*\"$field\"[[:space:]]*:[[:space:]]*\"([^\"]*)\".*$/\1/p" "$file" | head -n 1
}

json_number() {
  local file=$1 field=$2
  sed -nE "s/^[[:space:]]*\"$field\"[[:space:]]*:[[:space:]]*([0-9]+)[[:space:]]*,?[[:space:]]*$/\1/p" "$file" | head -n 1
}

utc_text() {
  date -u -d "@$1" '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null || printf '%s seconds since the Unix epoch\n' "$1"
}

check_head_fresh() {
  local payload=$1 now=$2 channel_name=$3
  local expires_at
  if ! [[ $now =~ ^[0-9]{1,18}$ ]]; then
    fail "the clock of this host gave \"$now\" instead of the time in seconds, so the head of the $channel_name channel cannot be checked for expiry and is refused"
  fi
  expires_at=$(json_number "$payload" expires_at)
  if ! [[ $expires_at =~ ^[0-9]{1,18}$ ]]; then
    fail "the head of the $channel_name channel carries no usable expires_at; a head without an expiry is refused. Try again later, when the channel has been published again, or use another channel with --channel or --channel-url"
  fi
  if ! [ "$expires_at" -gt "$now" ]; then
    fail "the head of the $channel_name channel expired at $(utc_text "$expires_at"); an expired head is refused. Try again later, when the channel has been refreshed, or use another channel with --channel or --channel-url"
  fi
}

verify_signed() {
  local file=$1 key=$2 out=$3
  local work
  work=$(mktemp -d "${tmp:-/tmp}/verify.XXXXXX")
  if [ "$(wc -c <"$file")" -gt "$MANIFEST_MAX_BYTES" ]; then
    rm -rf "$work"
    say "the manifest is larger than $MANIFEST_MAX_BYTES bytes" >&2
    return 1
  fi
  local payload signature
  payload=$(json_string "$file" payload)
  signature=$(json_string "$file" signature)
  if [ -z "$payload" ] || [ -z "$signature" ]; then
    rm -rf "$work"
    say "the manifest is not a signed envelope with payload and signature" >&2
    return 1
  fi
  if ! { printf '%b' "$ED25519_SPKI_PREFIX"; printf '%s' "${key#ed25519 }" | awk '{print $1}' | base64 -d; } >"$work/key.der" 2>/dev/null \
    || ! openssl pkey -pubin -inform DER -in "$work/key.der" -out "$work/key.pem" >/dev/null 2>&1; then
    rm -rf "$work"
    say "the release key does not decode" >&2
    return 1
  fi
  if ! printf '%s' "$payload" | base64 -d >"$work/payload" 2>/dev/null \
    || ! printf '%s' "$signature" | base64 -d >"$work/signature" 2>/dev/null; then
    rm -rf "$work"
    say "the payload or the signature is not base64" >&2
    return 1
  fi
  if ! openssl pkeyutl -verify -pubin -inkey "$work/key.pem" -rawin -in "$work/payload" -sigfile "$work/signature" >/dev/null 2>&1; then
    rm -rf "$work"
    say "the signature does not verify with the release key" >&2
    return 1
  fi
  cp "$work/payload" "$out"
  rm -rf "$work"
}

fetch_release() {
  tmp=$(mktemp -d /tmp/keel-install.XXXXXX)
  if [ -n "$binary" ]; then
    fetch_binary
  else
    fetch_from_channel
  fi
}

fetch_from_channel() {
  local manifest_url
  if [ -n "$requested_version" ]; then
    manifest_url="${channel_url}versions/$requested_version/manifest.json"
  else
    manifest_url="${channel_url}$channel/manifest.json"
  fi
  begin "fetching the release manifest $manifest_url"
  download "$manifest_url" "$tmp/manifest.json"
  begin "verifying the Ed25519 signature of the release manifest"
  if ! verify_signed "$tmp/manifest.json" "$RELEASE_KEY" "$tmp/payload.json"; then
    fail "the release manifest at $manifest_url is not signed by the release key; nothing was unpacked"
  fi
  if [ -z "$requested_version" ]; then
    check_head_fresh "$tmp/payload.json" "$(date +%s)" "$channel"
  fi
  version=$(json_string "$tmp/payload.json" version)
  local sha256
  sha256=$(json_string "$tmp/payload.json" sha256)
  if [ -z "$version" ] || ! [[ $sha256 =~ ^[0-9a-f]{64}$ ]]; then
    fail "the verified manifest names no version or no sha256"
  fi
  if [ -n "$requested_version" ] && [ "$version" != "$requested_version" ]; then
    fail "the manifest of $requested_version names the version $version"
  fi
  say "the manifest is signed by the release key: keel $version, sha256 $sha256"
  local name="keel-$version-$ARCHIVE_TARGET.tar.zst"
  local archive_url="${channel_url}versions/$version/$name"
  begin "downloading $archive_url"
  download "$archive_url" "$tmp/$name"
  begin "verifying the sha256 of $name against the manifest"
  (cd "$tmp" && printf '%s  %s\n' "$sha256" "$name" | sha256sum -c --strict --quiet)
  unpack "$tmp/$name" "$name"
  install -m 0644 -o root -g root "$tmp/manifest.json" "$OPT/staging/manifest.json"
}

fetch_binary() {
  begin "fetching the binary"
  local source name
  case $binary in
    https://*)
      name=${binary##*/}
      say "downloading $binary"
      download "$binary" "$tmp/$name"
      if download "$binary.sha256" "$tmp/$name.sha256" 2>/dev/null; then
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
  unpack "$source" "$name"
}

unpack() {
  local source=$1 name=$2
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
  local reported
  if ! reported=$("$staged" --version 2>&1); then
    fail "the binary does not run on this host ($(uname -m)): $reported"
  fi
  reported=${reported#keel }
  if [ -z "$reported" ]; then
    fail "the binary did not print its version"
  fi
  if [ -n "$version" ] && [ "$reported" != "$version" ]; then
    fail "the manifest names keel $version but the binary prints $reported"
  fi
  version=$reported
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

uid_range_free() {
  local from=$1 to=$2 file
  if getent passwd | awk -F: -v from="$from" -v to="$to" '$3 >= from && $3 <= to { found = 1 } END { exit !found }'; then
    return 1
  fi
  for file in "${SUBID_FILES[@]}"; do
    [ -f "$file" ] || continue
    if awk -F: -v from="$from" -v to="$to" '$2 != "" && $3 != "" { start = $2 + 0; end = $2 + $3 - 1; if (start <= to && end >= from) found = 1 } END { exit !found }' "$file"; then
      return 1
    fi
  done
  return 0
}

allot_uid_range() {
  begin "allotting the uid range the host agent may manage"
  local from=$UID_RANGE_START to
  while :; do
    to=$((from + UID_RANGE_SIZE - 1))
    if [ "$to" -gt "$UID_RANGE_CEILING" ]; then
      fail "no free range of $UID_RANGE_SIZE uids between $UID_RANGE_START and $UID_RANGE_CEILING"
    fi
    if uid_range_free "$from" "$to"; then
      break
    fi
    say "the range $from-$to is taken by an account or a subordinate range; trying the next"
    from=$((from + UID_RANGE_SIZE))
  done
  uid_from=$from
  uid_to=$to
  say "the installation manages uids $uid_from to $uid_to"
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

render_config() {
  if [ "$listen" != "$DEFAULT_LISTEN" ]; then
    printf 'listen: "%s"\n' "$listen"
  fi
  if [ "$pg_port" != "5432" ]; then
    printf 'database: "postgres:///keel?host=%s&port=%s"\n' "$PG_SOCKET_DIR" "$pg_port"
  fi
  if [ "$insecure_rehearsal_channel" -eq 1 ]; then
    case $channel_url in
      http://*) printf 'update:\n  channel_url: "%s"\n  insecure_rehearsal_channel: true\n' "$channel_url" ;;
    esac
  fi
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
  render_config >"$ETC/config.yaml"
  chown root:keel "$ETC/config.yaml"
  chmod 0640 "$ETC/config.yaml"
  if [ -s "$ETC/config.yaml" ]; then
    sed 's/^/  /' "$ETC/config.yaml"
  else
    say "  (empty: every key keeps the kernel's default)"
  fi
}

install_policy() {
  begin "writing the host agent policy $AGENT_POLICY with uid_range $uid_from to $uid_to"
  if [ -f "$AGENT_POLICY" ]; then
    say "$AGENT_POLICY exists from an earlier installation; kept as it is, the allotted uid range is not written"
    return
  fi
  if ! release_file deploy/agent.yaml "$here/agent.yaml"; then
    fail "the release does not carry deploy/agent.yaml and $here/agent.yaml is missing; copy the whole deploy/ directory next to install.sh"
  fi
  local template="$OPT/versions/$version/deploy/agent.yaml"
  if [ "$(grep -cE '^  uid_range:' "$template")" != "1" ]; then
    fail "$template does not carry exactly one uid_range line under policy"
  fi
  sed -E "s/^  uid_range:.*$/  uid_range: { from: $uid_from, to: $uid_to }/" "$template" >"$tmp/agent.yaml"
  install -m 0640 -o root -g keel "$tmp/agent.yaml" "$AGENT_POLICY"
  policy_written=1
  sed 's/^/  /' "$AGENT_POLICY"
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

product_packages() {
  case $1 in
    panel) printf '%s\n' "${PANEL_PACKAGES[@]}" ;;
  esac
}

prepare_products() {
  local id
  for id in "${product_ids[@]}"; do
    case $id in
      panel) prepare_panel ;;
    esac
  done
}

prepare_panel() {
  local -a packages=()
  local package
  while IFS= read -r package; do
    packages+=("$package")
  done < <(product_packages panel)
  need debconf-set-selections
  printf '%s\n' "postfix postfix/main_mailer_type select Internet Site" "postfix postfix/mailname string $(hostname -f 2>/dev/null || hostname)" | debconf-set-selections
  products_prepared=1
  apt_install "${packages[@]}"
  begin "preparing the roots of panel"
  install -d -m 0755 -o root -g root "$PANEL_WWW" "$PANEL_CHALLENGES"
  install -d -m 0711 -o root -g root "$PANEL_DUMPS" "$PANEL_BACKUPS"
  install -d -m 0700 -o root -g root "$PANEL_CERTS"
  install -d -m 0700 -o keel -g keel "$PANEL_TRANSFER"
  prepare_panel_unit
  prepare_panel_mail
  prepare_panel_ftp
  start_panel_units
}

prepare_panel_unit() {
  begin "letting keel.service write the transfer root of panel"
  install -d -m 0755 -o root -g root "$PANEL_UNIT_DROP_IN_DIR"
  install -m 0644 -o root -g root /dev/null "$PANEL_UNIT_DROP_IN"
  printf '%s\n' "[Service]" "ReadWritePaths=$PANEL_TRANSFER" >"$PANEL_UNIT_DROP_IN"
}

prepare_panel_mail() {
  begin "pointing postfix and dovecot at the mailboxes of panel"
  need postconf postmap doveconf postfix
  if ! getent passwd "$PANEL_MAIL_USER" >/dev/null; then
    useradd --system --home-dir "$PANEL_MAILDIRS" --no-create-home --shell "$NOLOGIN" "$PANEL_MAIL_USER"
  fi
  install -d -m 0755 -o root -g root "$PANEL_MAIL_MAPS"
  install -d -m 0750 -o root -g dovecot "$PANEL_MAIL_PASSWD_DIR"
  install -d -m 0700 -o "$PANEL_MAIL_USER" -g "$PANEL_MAIL_USER" "$PANEL_MAILDIRS"
  local map
  for map in "${PANEL_MAIL_MAP_NAMES[@]}"; do
    [ -f "$PANEL_MAIL_MAPS/keel-panel-$map" ] || install -m 0644 -o root -g root /dev/null "$PANEL_MAIL_MAPS/keel-panel-$map"
    postmap "hash:$PANEL_MAIL_MAPS/keel-panel-$map"
  done
  [ -f "$PANEL_MAIL_PASSWD" ] || install -m 0640 -o dovecot -g dovecot /dev/null "$PANEL_MAIL_PASSWD"
  postconf -e \
    "virtual_mailbox_domains = hash:$PANEL_MAIL_MAPS/keel-panel-domains" \
    "virtual_mailbox_maps = hash:$PANEL_MAIL_MAPS/keel-panel-mailboxes" \
    "virtual_alias_maps = hash:$PANEL_MAIL_MAPS/keel-panel-aliases" \
    "virtual_transport = lmtp:unix:private/dovecot-lmtp"
  cat >"$PANEL_MAIL_DROP_IN" <<CONF
mail_driver = maildir
mail_path = ~/Maildir
mail_inbox_path = ~/Maildir
mail_uid = $PANEL_MAIL_USER
mail_gid = $PANEL_MAIL_USER
passdb passwd-file {
  passwd_file_path = $PANEL_MAIL_PASSWD
}
userdb passwd-file {
  passwd_file_path = $PANEL_MAIL_PASSWD
}
mail_plugins {
  quota = yes
}
quota "User quota" {
  driver = count
}
service lmtp {
  unix_listener /var/spool/postfix/private/dovecot-lmtp {
    mode = 0600
    user = postfix
    group = postfix
  }
}
protocol lmtp {
  auth_username_format = %{user | lower}
}
CONF
  chmod 0644 "$PANEL_MAIL_DROP_IN"
  doveconf -n >/dev/null || fail "doveconf -n refuses $PANEL_MAIL_DROP_IN; panel needs dovecot 2.4, the one of Debian 13"
  postfix check || fail "postfix check refuses the virtual maps under $PANEL_MAIL_MAPS"
}

prepare_panel_ftp() {
  begin "pointing vsftpd at the FTP accounts of panel"
  need vsftpd
  install -d -m 0700 -o root -g root "$PANEL_FTP_DIR" "$PANEL_FTP_USERS"
  [ -f "$PANEL_FTP_PASSWD" ] || install -m 0600 -o root -g root /dev/null "$PANEL_FTP_PASSWD"
  if [ -f "$PANEL_FTP_CONF" ] && [ ! -f "$PANEL_FTP_CONF.orig" ]; then
    cp -p "$PANEL_FTP_CONF" "$PANEL_FTP_CONF.orig"
  fi
  cat >"$PANEL_PAM_DIR/$PANEL_FTP_PAM" <<CONF
auth required pam_pwdfile.so pwdfile=$PANEL_FTP_PASSWD
account required pam_permit.so
CONF
  cat >"$PANEL_FTP_CONF" <<CONF
listen=YES
listen_ipv6=NO
anonymous_enable=NO
local_enable=YES
write_enable=YES
local_umask=022
guest_enable=YES
guest_username=nobody
virtual_use_local_privs=YES
pam_service_name=$PANEL_FTP_PAM
user_config_dir=$PANEL_FTP_USERS
chroot_local_user=YES
allow_writeable_chroot=YES
secure_chroot_dir=/var/run/vsftpd/empty
hide_ids=YES
CONF
  chmod 0644 "$PANEL_PAM_DIR/$PANEL_FTP_PAM" "$PANEL_FTP_CONF"
  vsftpd "$PANEL_FTP_CONF" -v </dev/null >/dev/null || fail "vsftpd refuses $PANEL_FTP_CONF"
}

start_panel_units() {
  local -a php=() units down=()
  local found
  while IFS= read -r found; do
    php+=("$found")
  done < <(systemctl list-unit-files --type=service --no-legend "$PANEL_PHP_UNITS" | awk '{print $1}')
  if [ "${#php[@]}" -eq 0 ]; then
    fail "php-fpm installed no unit matching $PANEL_PHP_UNITS"
  fi
  units=("${PANEL_UNITS[@]}" "${php[@]}")
  begin "enabling and starting ${units[*]}"
  systemctl enable "${units[@]}"
  systemctl restart "${units[@]}"
  local unit
  for unit in "${units[@]}"; do
    systemctl is-active --quiet "$unit" || down+=("$unit")
  done
  if [ "${#down[@]}" -gt 0 ]; then
    fail "${down[*]} did not start; see journalctl -u ${down[0]}"
  fi
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
  begin "starting keel.service and waiting for its readiness notification"
  if ! systemctl start keel.service; then
    journalctl -u keel.service --no-pager -n 40 >&2 || true
    fail "keel.service did not report READY=1 to the service manager; see journalctl -u keel"
  fi
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

certificate_fingerprint() {
  local fingerprint=""
  fingerprint=$(journalctl -u keel.service --no-pager -o cat 2>/dev/null | grep -oE '([0-9A-F]{2}:){31}[0-9A-F]{2}' | tail -n 1) || true
  if [ -z "$fingerprint" ] && command -v openssl >/dev/null 2>&1 && [ -f "$CERT" ]; then
    fingerprint=$(openssl x509 -in "$CERT" -noout -fingerprint -sha256 2>/dev/null | sed 's/^.*=//') || true
  fi
  printf '%s\n' "${fingerprint:-(not found; see journalctl -u keel for the line \"tls certificate\")}"
}

setup_key() {
  local key=""
  local waited=0
  while [ "$waited" -lt 10 ]; do
    key=$(journalctl -u keel.service --no-pager -o cat 2>/dev/null | grep -oE '/setup\?key=kok_[0-9a-f]{64}' | tail -n 1 | sed 's/^.*key=//') || true
    if [ -n "$key" ]; then
      break
    fi
    sleep 1
    waited=$((waited + 1))
  done
  printf '%s\n' "$key"
}

report() {
  local host port name fingerprint key
  host=$(shown_host)
  port=$(listen_port)
  name=$(hostname)
  fingerprint=$(certificate_fingerprint)
  key=$(setup_key)
  say ""
  say "keel $version is installed on $(hostname)"
  say ""
  say "  products:     $products"
  if [ "$link_installed" -eq 1 ]; then
    say "  binary:       $OPT/versions/$version/keel (also as $LINK)"
  else
    say "  binary:       $OPT/versions/$version/keel"
  fi
  if [ -z "$binary" ]; then
    say "  release:      $channel channel at $channel_url, manifest signed by the release key"
  fi
  if [ -f "$OPT/versions/$version/config-sample.yaml" ]; then
    say "  config:       $ETC/config.yaml (sample: $OPT/versions/$version/config-sample.yaml)"
  else
    say "  config:       $ETC/config.yaml"
  fi
  if [ "$policy_written" -eq 1 ]; then
    say "  agent policy: $AGENT_POLICY (uid_range $uid_from to $uid_to)"
  else
    say "  agent policy: $AGENT_POLICY (kept from an earlier installation)"
  fi
  say "  data:         $LIB"
  say "  sockets:      $RUN/core.sock (control), $RUN/agent.sock (host agent)"
  say "  units:        keel.service (Type=notify, watchdog), keel-privileged.service and keel-update.path (enabled, running);"
  say "                keel-update.service runs once per update request"
  say "  postgres:     database keel, role keel, peer authentication on $PG_SOCKET_DIR port $pg_port"
  if [ "$products_prepared" -eq 1 ]; then
    say "  panel:        ${PANEL_UNITS[*]} and $PANEL_PHP_UNITS enabled and running;"
    say "                sites under $PANEL_WWW, certificates under $PANEL_CERTS, mailboxes under $PANEL_MAILDIRS,"
    say "                FTP accounts through the PAM service $PANEL_FTP_PAM ($PANEL_FTP_CONF.orig keeps the previous $PANEL_FTP_CONF)"
  fi
  say ""
  say "  URL:          https://$host:$port/"
  if listen_unspecified; then
    say "  names:        $name, localhost, 127.0.0.1 and ::1 are the names the certificate carries; a browser"
    say "                reaching the host by another address, $host included, warns about the name as well"
  fi
  say "  certificate:  $CERT (self-signed, SHA-256 $fingerprint)"
  if [ -n "$key" ]; then
    say "  setup link:   https://$host:$port/setup?key=$key"
  else
    say "  setup link:   (not found in the journal; journalctl -u keel -o cat | grep setup prints it)"
  fi
  say ""
  say "The setup link creates the first administrator from a login and a password entered in the"
  say "browser. It is valid once and dies with the service: a restart before it is used prints a new"
  say "one in the journal. The URL serves the panel over TLS with the self-signed certificate above;"
  say "a browser warns once and shows the fingerprint, which must match the one printed here. A"
  say "reverse proxy in front trusts $CERT and is named in trusted_proxies of $ETC/config.yaml."
  say "The same login over the API, once the administrator exists:"
  if listen_unspecified; then
    say "  curl -sS --cacert $CERT --resolve \"$name:$port:$host\" -X POST https://$name:$port/api/v1/call/auth.login \\"
  else
    say "  curl -sS --cacert $CERT -X POST https://$host:$port/api/v1/call/auth.login \\"
  fi
  say "    -H 'Content-Type: application/json' -d '{\"login\":\"<the login>\",\"password\":\"<the password>\"}'"
  say ""
  say "Checks:"
  say "  systemctl status keel keel-privileged keel-update.path"
  say "  journalctl -u keel -f"
  say "  ps -eo user,comm | grep keel"
  say "  runuser -u keel -- keel ctl keel.account.list --out table"
  say "  runuser -u keel -- keel product list"
  say "  runuser -u keel -- keel doctor"
  say ""
  say "An operator's certificate goes into tls.cert and tls.key of $ETC/config.yaml; ACME arrives with a"
  say "later milestone. The host policy in $AGENT_POLICY is read by keel-privileged.service when a"
  say "product names the host module (see deploy/README.md)."
}

main() {
  parse_args "$@"
  trap on_error ERR
  trap on_exit EXIT
  check_host
  confirm
  ensure_tools
  fetch_release
  ensure_postgres
  ensure_role_and_database
  ensure_user
  allot_uid_range
  check_peer_auth
  install_files
  install_policy
  prepare_products
  install_units
  prepare_database
  start_service
  report
}

version=""
staged=$OPT/staging/keel
pg_port=5432
uid_from=0
uid_to=0
product_ids=()
if [ -z "${BASH_SOURCE[0]:-}" ] || [ "${BASH_SOURCE[0]}" = "$0" ]; then
  main "$@"
fi
