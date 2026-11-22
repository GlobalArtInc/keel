#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

keel=${1:-}
problems=0
scripts=(deploy/install.sh deploy/uninstall.sh hack/check-install.sh hack/update-vm.sh hack/backup-vm.sh hack/install-vm.sh hack/host-vm.sh hack/dns-vm.sh hack/panel-vm.sh hack/peers-vm.sh)
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
check "deploy/agent.yaml names the check of the server blocks on nginx" grep -qF 'nginx.test: ["nginx", "-t"]' deploy/agent.yaml
check "deploy/agent.yaml carries the certificate and challenge roots of the panel" grep -qF 'certs: /etc/ssl/keel, acme: /var/lib/keel-panel-acme' deploy/agent.yaml
check "deploy/agent.yaml carries the database dump root of the panel" grep -qF 'dumps: /var/lib/keel-panel-dumps' deploy/agent.yaml
check "deploy/agent.yaml lets mariadb.exec run the mariadb client and dump" grep -qF '{ mariadb: [mariadb, --batch, --sandbox, --skip-column-names] }, { mariadb-dump: [mariadb-dump, ' deploy/agent.yaml
check "deploy/agent.yaml carries the passwd-file and maildir roots of the panel" grep -qF 'mail: /etc/postfix/keel, dovecot: /etc/dovecot/keel, maildirs: /var/lib/keel-panel-mail' deploy/agent.yaml
check "deploy/agent.yaml names the checks of postfix and dovecot" grep -qF 'postfix.check: ["postfix", "check"], dovecot.check: ["doveconf", "-n"]' deploy/agent.yaml
check "deploy/agent.yaml lets postmap build the three virtual maps" grep -qF '{ postmap: [postmap, "hash:/etc/postfix/keel/keel-panel-domains", "hash:/etc/postfix/keel/keel-panel-mailboxes", "hash:/etc/postfix/keel/keel-panel-aliases"] }' deploy/agent.yaml
check "deploy/agent.yaml lets the maildirs be archived to standard output" grep -qF '{ mail.archive: [tar, --create, --file=-, --directory=/var/lib/keel-panel-mail, .] }' deploy/agent.yaml
check "deploy/agent.yaml carries the user file and the per-account files of vsftpd" grep -qF 'ftp: /etc/vsftpd/keel, ftp-users: /etc/vsftpd/keel-users' deploy/agent.yaml
check "deploy/agent.yaml carries the transfer root the file manager stages uploads and downloads in" grep -qF 'transfer: /var/lib/keel-panel-transfer' deploy/agent.yaml
check "deploy/agent.yaml names the check of vsftpd" grep -qF 'vsftpd.check: ["vsftpd", "/etc/vsftpd.conf", "-v"]' deploy/agent.yaml
check "deploy/agent.yaml names the per-account files of vsftpd after the accounts of the panel" grep -qF 'panel: { user_prefix: hp_, account_roots: [ftp-users], classes: { backup: { archives: backups, homes: www, maildirs: maildirs, dumps: dumps } }, drop_in_prefix: keel-panel- }' deploy/agent.yaml
check "deploy/agent.yaml carries the archive root of the site backups" grep -qF 'backups: /var/lib/keel-panel-backups }' deploy/agent.yaml
check "deploy/agent.yaml carries no plugins section while the release carries no plugin host" bash -c '! grep -q "^[[:space:]]*plugins:" deploy/agent.yaml'
for version in 8.2 8.3 8.4; do
  check "deploy/agent.yaml carries the pool root of php$version-fpm" grep -qF "pools-$version: /etc/php/$version/fpm/pool.d" deploy/agent.yaml
  check "deploy/agent.yaml names the check of php$version-fpm" grep -qF "php$version-fpm.test: [\"php-fpm$version\", \"-t\"]" deploy/agent.yaml
done
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
refuses_plain_http() {
  local out
  if out=$(bash deploy/install.sh --product demo --channel-url http://127.0.0.1:9/ </dev/null 2>&1); then
    return 1
  fi
  grep -qF 'is plain http; a release channel is read over https' <<<"$out" && grep -qF -- '--insecure-rehearsal-channel' <<<"$out"
}

admits_rehearsal_http() {
  local out
  out=$(bash deploy/install.sh --product demo --channel-url http://127.0.0.1:9/ --insecure-rehearsal-channel </dev/null 2>&1 || true)
  grep -qF 'warning: the channel http://127.0.0.1:9/ is reached over plain http' <<<"$out" && ! grep -qF 'is plain http; a release channel' <<<"$out"
}

check "install.sh refuses a plain http channel without --insecure-rehearsal-channel" refuses_plain_http
check "install.sh admits a plain http channel with --insecure-rehearsal-channel and warns" admits_rehearsal_http
rendered_config() {
  bash -c 'set -Eeuo pipefail; source deploy/install.sh; channel_url=$1; insecure_rehearsal_channel=$2; render_config' bash "$@"
}

refuses_odd_rehearsal_url() {
  local out
  if out=$(bash deploy/install.sh --product demo --channel-url "$1" --insecure-rehearsal-channel </dev/null 2>&1); then
    return 1
  fi
  grep -qF 'carries a character that is not written into' <<<"$out"
}

check "install.sh writes the channel and the rehearsal setting into config.yaml for a plain http channel named a rehearsal" test "$(rendered_config http://127.0.0.1:8098/ 1)" = "$(printf 'update:\n  channel_url: "http://127.0.0.1:8098/"\n  insecure_rehearsal_channel: true')"
check "install.sh writes nothing about the rehearsal for an https channel even with the flag" test -z "$(rendered_config https://dl.example.org/keel/ 1)"
check "install.sh writes nothing about the rehearsal without the flag" test -z "$(rendered_config https://dl.globalart.hu/keel/ 0)"
check "install.sh refuses a rehearsal channel address with a double quote" refuses_odd_rehearsal_url 'http://a"b/'
check "install.sh refuses a rehearsal channel address with a backslash" refuses_odd_rehearsal_url 'http://a\b/'
check "install.sh refuses a rehearsal channel address with a single quote" refuses_odd_rehearsal_url "http://a'b/"
check "install.sh refuses a rehearsal channel address with a space" refuses_odd_rehearsal_url 'http://a b/'
check "install.sh refuses a rehearsal channel address with a dollar sign" refuses_odd_rehearsal_url 'http://a$b/'
check "the deploy README names the rehearsal flag and the setting" test "$(grep -cF -e '--insecure-rehearsal-channel' -e 'insecure_rehearsal_channel' deploy/README.md)" -ge 2
check "install-vm.sh names its plain http channel as a rehearsal" grep -qF -- '--channel-url "http://127.0.0.1:$CHANNEL_PORT/" --insecure-rehearsal-channel' hack/install-vm.sh
check "update-vm.sh names its plain http channel as a rehearsal for the kernel and the root side" test "$(grep -cF -e 'insecure_rehearsal_channel: true' -e '--insecure-rehearsal-channel --release-key' hack/update-vm.sh)" = "2"
check "update-vm.sh signs the head of its rehearsal channel with an expiry" grep -qF -- '--expires-at' hack/update-vm.sh

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
check "panel-vm.sh --help prints the usage and exits 0" prints_usage hack/panel-vm.sh
check "panel-vm.sh installs through install.sh with panel from a local archive" grep -qF 'bash deploy/install.sh --product panel --binary "$archive" --yes' hack/panel-vm.sh
check "panel-vm.sh proves the lock of a suspended account" grep -qF 'check "the operating-system account is locked and expired while its account is suspended"' hack/panel-vm.sh
check "panel-vm.sh proves the removal with the account" grep -qF 'check "$USER_NAME and its home are gone"' hack/panel-vm.sh
check "panel-vm.sh proves nginx serving an issued certificate" grep -qF 'check "nginx serves the issued certificate"' hack/panel-vm.sh
check "panel-vm.sh proves a database user reaching its database" grep -qF 'check "$DATABASE_USER creates a table in $DATABASE with its password"' hack/panel-vm.sh
check "panel-vm.sh proves an FTP session chrooted to its site" grep -qF 'check "/ of the session is $FTP_CHROOT with nothing of the host above it"' hack/panel-vm.sh
check "panel-vm.sh proves a site restored from its backup" grep -qF 'check "the index of the site is back after the restore, owned by $USER_NAME"' hack/panel-vm.sh
check "panel-vm.sh proves an archive changed by keel is not restored" grep -qF 'check "the restore of an archive keel changed fails"' hack/panel-vm.sh
check "panel-vm.sh proves the mariadb client of the agent runs sandboxed" grep -qF 'check "the mariadb client refuses a shell escape under --sandbox"' hack/panel-vm.sh
check "panel-vm.sh leaves the packages of panel to install.sh" test "$(grep -cE 'apt-get install .*\b(nginx|php-fpm|mariadb-server|postfix|dovecot-core|vsftpd)\b' hack/panel-vm.sh)" = "0"
check "panel-vm.sh finds every package of panel installed" grep -qF 'check "the package $package is installed" package_installed "$package"' hack/panel-vm.sh
check "panel-vm.sh finds every unit of panel running" grep -qF 'check "$unit is enabled and running" unit_running "$unit"' hack/panel-vm.sh
check "panel-vm.sh takes the Let's Encrypt staging directory unless --production is given" grep -qF 'ACME_DIRECTORY=$PRODUCTION' hack/panel-vm.sh
check "panel-vm.sh names --production in its usage" grep -qE '^       hack/panel-vm.sh ssl <domain> \[--production\]$' hack/panel-vm.sh
check "peers-vm.sh --help prints the usage and exits 0" prints_usage hack/peers-vm.sh
check "peers-vm.sh installs through install.sh listening on the address the other machine reaches" grep -qF 'bash deploy/install.sh --product "$product" --listen "$address:$PORT" --binary "$archive" --yes' hack/peers-vm.sh
check "peers-vm.sh trusts the neighbour's certificate through the trust store of the system" grep -qF 'update-ca-certificates' hack/peers-vm.sh
check "peers-vm.sh pairs by code over the CLI" grep -qF 'call peer.create "url=\"$url\"" "code=\"$code\""' hack/peers-vm.sh
check "peers-vm.sh lets the dns machine deliver its events to the address of the panel machine" grep -qF 'called settings.set key=webhook.private_targets value=true' hack/peers-vm.sh
check "peers-vm.sh proves a site's zone in the account of its customer on the other machine" grep -qF 'check "the zone belongs to the account of the customer the panel machine named by its origin"' hack/peers-vm.sh
check "peers-vm.sh proves the repeat of a signed event received once" grep -qF 'check "the event of the zone is still once in the inbox and on the bus after its repeat"' hack/peers-vm.sh
check "peers-vm.sh proves one service for two opens with one key" grep -qF 'check "the dns machine holds one service of that external id, in the customer'"'"'s account"' hack/peers-vm.sh
check "peers-vm.sh proves a revoked token answering 401" grep -qF 'check "the revoked API token answers 401"' hack/peers-vm.sh
check "peers-vm.sh makes the archive path absolute before it changes into the checkout" test "$(grep -nF 'archive=$(realpath -- "$archive")' hack/peers-vm.sh | cut -d: -f1)" -lt "$(grep -nF 'cd "$(dirname "$0")/.."' hack/peers-vm.sh | cut -d: -f1)"
check "peers-vm.sh finds svc.service.opened delivered to the panel machine before it deletes the link" test "$(grep -nF 'check "svc.service.opened of the service went to the hook of the panel machine and was delivered before the link goes"' hack/peers-vm.sh | cut -d: -f1)" -lt "$(grep -nF 'called peer.delete' hack/peers-vm.sh | cut -d: -f1)"
check "install.sh prepares the products after the policy and before the units" test "$(grep -nE '^  install_policy$' deploy/install.sh | cut -d: -f1)" -lt "$(grep -nE '^  prepare_products$' deploy/install.sh | cut -d: -f1)" -a "$(grep -nE '^  prepare_products$' deploy/install.sh | cut -d: -f1)" -lt "$(grep -nE '^  install_units$' deploy/install.sh | cut -d: -f1)"
check "install.sh installs the packages of panel through the package manager" grep -qF '  apt_install "${packages[@]}"' deploy/install.sh
for root in /var/www /etc/ssl/keel /var/lib/keel-panel-acme /var/lib/keel-panel-dumps /var/lib/keel-panel-backups /var/lib/keel-panel-transfer /etc/postfix/keel /etc/dovecot/keel /var/lib/keel-panel-mail /etc/vsftpd/keel /etc/vsftpd/keel-users; do
  check "install.sh prepares the root $root the policy names" test "$(grep -cF -- ": $root" deploy/agent.yaml)" -ge 1 -a "$(grep -cE "^PANEL_[A-Z_]+=$root$" deploy/install.sh)" = "1"
done
check "install.sh checks vsftpd.conf the way vsftpd.check of the policy does" grep -qF 'PANEL_FTP_CONF=/etc/vsftpd.conf' deploy/install.sh
check "install.sh names the passwd-file the panel writes into the dovecot root" grep -qF 'PANEL_MAIL_PASSWD=$PANEL_MAIL_PASSWD_DIR/keel-panel-passwd' deploy/install.sh
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
  now=$(date +%s)
  cargo run -q -p keel-release -- sign --key "$rehearsal/keys/release.key" --version 0.1.0 --archive "$rehearsal/archive" \
    --min-upgrade-from 0.1.0 --contract 1 --notes "install rehearsal" --expires-at $((now + 3600)) --out "$rehearsal/head-fresh.json" >/dev/null
  cargo run -q -p keel-release -- sign --key "$rehearsal/keys/release.key" --version 0.1.0 --archive "$rehearsal/archive" \
    --min-upgrade-from 0.1.0 --contract 1 --notes "install rehearsal" --expires-at 1 --out "$rehearsal/head-expired.json" >/dev/null
  printf '{\n  "version": "0.1.0",\n  "expires_at": "%s"\n}\n' $((now + 3600)) >"$rehearsal/payload-text-expiry.json"
  printf '{\n  "version": "0.1.0",\n  "expires_at": %s.5\n}\n' $((now + 3600)) >"$rehearsal/payload-float-expiry.json"
  for kind in fresh expired version; do
    mkdir -p "$rehearsal/channel-$kind/stable" "$rehearsal/channel-$kind/versions/0.1.0"
    : >"$rehearsal/channel-$kind/versions/0.1.0/keel-0.1.0-x86_64-linux-musl.tar.zst"
  done
  cp "$rehearsal/head-fresh.json" "$rehearsal/channel-fresh/stable/manifest.json"
  cp "$rehearsal/head-expired.json" "$rehearsal/channel-expired/stable/manifest.json"
  cp "$rehearsal/manifest.json" "$rehearsal/channel-version/stable/manifest.json"
  for kind in fresh expired version; do
    cp "$rehearsal/manifest.json" "$rehearsal/channel-$kind/versions/0.1.0/manifest.json"
  done
  mkdir -p "$rehearsal/stub-empty" "$rehearsal/stub-garbage" "$rehearsal/stub-failing"
  printf '#!/bin/sh\nexit 0\n' >"$rehearsal/stub-empty/date"
  printf '#!/bin/sh\necho not-a-number\n' >"$rehearsal/stub-garbage/date"
  printf '#!/bin/sh\nexit 1\n' >"$rehearsal/stub-failing/date"
  chmod +x "$rehearsal/stub-empty/date" "$rehearsal/stub-garbage/date" "$rehearsal/stub-failing/date"
  printf '{\n  "version": "0.1.0",\n  "expires_at": 9999999999999999999\n}\n' >"$rehearsal/payload-huge-expiry.json"
  fetched_from() {
    local kind=$1 requested=${2:-} stubs=${3:-}
    PATH="${stubs:+$stubs:}$(dirname "$ossl"):$PATH" bash -c '
set -Eeuo pipefail
source deploy/install.sh
channel_dir=$1
RELEASE_KEY=$(head -n 1 "$2")
requested_version=$3
channel_url=http://rehearsal.invalid/
channel=stable
tmp=$(mktemp -d)
trap '"'"'rm -rf "$tmp"'"'"' EXIT
download() { cp "$channel_dir/${1#"$channel_url"}" "$2"; }
sha256sum() { cat >/dev/null; }
install() { :; }
unpack() { printf "unpacked\n"; }
fetch_from_channel
' bash "$rehearsal/channel-$kind" "$rehearsal/keys/release.pub" "$requested" 2>&1
  }
  fetch_reaches_unpack() {
    local out
    out=$(fetched_from "$@") && grep -qxF 'unpacked' <<<"$out"
  }
  fetch_refused_with() {
    local pattern=$1 out
    shift
    if out=$(fetched_from "$@"); then
      return 1
    fi
    grep -qE -- "$pattern" <<<"$out" && ! grep -qxF 'unpacked' <<<"$out"
  }
  freshness() {
    sourced "$ossl" check_head_fresh "$1" "$2" stable 2>&1
  }
  is_fresh() {
    freshness "$@" >/dev/null
  }
  refused_with() {
    local pattern=$1 out
    shift
    if out=$(freshness "$@"); then
      return 1
    fi
    grep -qE -- "$pattern" <<<"$out"
  }
  check "install.sh accepts a head signed with an expiry in the future" verifies "$ossl" "$rehearsal/head-fresh.json" "$rehearsal/keys/release.pub" "$rehearsal/payload-fresh.json"
  check "install.sh reads the expires_at of the verified head" test "$(sourced "$ossl" json_number "$rehearsal/payload-fresh.json" expires_at)" = "$((now + 3600))"
  check "install.sh finds no expires_at in the manifest of a version" test -z "$(sourced "$ossl" json_number "$rehearsal/payload.json" expires_at)"
  check "install.sh takes a head that expires after now" is_fresh "$rehearsal/payload-fresh.json" "$now"
  check "install.sh refuses a head that expires at this very second" refused_with 'expired at' "$rehearsal/payload-fresh.json" "$((now + 3600))"
  check "install.sh names the expiry of a head past it and says to try later or use another channel" refused_with 'expired at .*Try again later.*another channel' "$rehearsal/payload-fresh.json" "$((now + 7200))"
  check "install.sh refuses a head without an expires_at" refused_with 'carries no usable expires_at' "$rehearsal/payload.json" "$now"
  check "install.sh refuses an expires_at that is a string" refused_with 'carries no usable expires_at' "$rehearsal/payload-text-expiry.json" "$now"
  check "install.sh refuses an expires_at that is not a whole number" refused_with 'carries no usable expires_at' "$rehearsal/payload-float-expiry.json" "$now"
  check "install.sh refuses an expires_at too large to compare" refused_with 'carries no usable expires_at' "$rehearsal/payload-huge-expiry.json" "$now"
  check "install.sh refuses a head when the clock gives nothing" refused_with 'clock of this host' "$rehearsal/payload-fresh.json" ""
  check "install.sh refuses a head when the clock does not give a number" refused_with 'clock of this host' "$rehearsal/payload-fresh.json" "not-a-number"
  check "install.sh refuses a head when the clock gives a negative number" refused_with 'clock of this host' "$rehearsal/payload-fresh.json" "-5"
  check "install.sh reads a fresh head through to the unpacking" fetch_reaches_unpack fresh
  check "install.sh refuses an expired head before it unpacks anything" fetch_refused_with 'expired at' expired
  check "install.sh refuses the manifest of a version served in the place of the head before it unpacks anything" fetch_refused_with 'carries no usable expires_at' version
  check "install.sh refuses a fresh head when the date command prints nothing" fetch_refused_with 'clock of this host' fresh "" "$rehearsal/stub-empty"
  check "install.sh refuses a fresh head when the date command prints no number" fetch_refused_with 'clock of this host' fresh "" "$rehearsal/stub-garbage"
  check "install.sh refuses a fresh head when the date command fails" fetch_refused_with 'clock of this host' fresh "" "$rehearsal/stub-failing"
  check "install.sh installs a version of the channel although its head is expired" fetch_reaches_unpack expired 0.1.0
  check "install.sh installs a version of the channel although the clock gives no number" fetch_reaches_unpack version 0.1.0 "$rehearsal/stub-garbage"
  printf 'keel:100000:65536\n' >"$rehearsal/subuid"
  check "install.sh skips uid ranges taken by a subordinate range or an account" allots "$ossl" "$rehearsal/subuid" "231072 296607"
  printf '' >"$rehearsal/empty-subuid"
  check "install.sh allots the first range when nothing claims it" allots "$ossl" "$rehearsal/empty-subuid" "100000 165535"
else
  echo "no OpenSSL 1.1.1 or newer on this host; the signature check of install.sh is not exercised here"
fi

panel_packages=(nginx php-fpm mariadb-server mariadb-client postfix dovecot-core dovecot-imapd dovecot-lmtpd vsftpd libpam-pwdfile)
panel_units=(nginx.service mariadb.service postfix.service dovecot.service vsftpd.service php8.4-fpm.service)

packages_of() {
  bash -c 'set -Eeuo pipefail; source deploy/install.sh; product_packages "$1"' bash "$1" | tr '\n' ' '
}

prepare_panel_under() {
  local dir=$1 product=${2:-panel}
  bash -c '
set -Eeuo pipefail
source deploy/install.sh
dir=$1
calls=$dir/calls
: >"$calls"
record() { printf "%s\n" "$*" >>"$calls"; }
PANEL_WWW=$dir/www
PANEL_CERTS=$dir/certs
PANEL_CHALLENGES=$dir/acme
PANEL_DUMPS=$dir/dumps
PANEL_BACKUPS=$dir/backups
PANEL_TRANSFER=$dir/transfer
PANEL_UNIT_DROP_IN_DIR=$dir/keel.service.d
PANEL_UNIT_DROP_IN=$PANEL_UNIT_DROP_IN_DIR/keel-panel.conf
PANEL_MAIL_MAPS=$dir/mail
PANEL_MAIL_PASSWD_DIR=$dir/dovecot
PANEL_MAIL_PASSWD=$PANEL_MAIL_PASSWD_DIR/keel-panel-passwd
PANEL_MAILDIRS=$dir/maildirs
PANEL_MAIL_DROP_IN=$dir/90-keel-panel.conf
PANEL_FTP_CONF=$dir/vsftpd.conf
PANEL_PAM_DIR=$dir/pam.d
PANEL_FTP_DIR=$dir/ftp
PANEL_FTP_PASSWD=$PANEL_FTP_DIR/keel-panel-passwd
PANEL_FTP_USERS=$dir/ftp-users
mkdir -p "$PANEL_PAM_DIR"
printf "listen=NO\n" >"$PANEL_FTP_CONF"
apt_install() { record apt_install "$@"; }
debconf-set-selections() { cat >"$dir/debconf"; }
getent() { return 2; }
useradd() { record useradd "$@"; }
postmap() { record postmap "$@"; }
postconf() { record postconf "$@"; }
postfix() { record postfix "$@"; }
doveconf() { record doveconf "$@"; }
vsftpd() { record vsftpd "$@"; }
systemctl() {
  record systemctl "$@"
  if [ "$1" = list-unit-files ]; then
    printf "php8.4-fpm.service enabled enabled\n"
  fi
}
install() {
  record install "$@"
  local directories=0
  local -a paths=()
  while [ $# -gt 0 ]; do
    case $1 in
      -d) directories=1; shift ;;
      -m|-o|-g) shift 2 ;;
      *) paths+=("$1"); shift ;;
    esac
  done
  if [ "$directories" -eq 1 ]; then
    mkdir -p "${paths[@]}"
  else
    cp "${paths[0]}" "${paths[1]}"
  fi
}
product_ids=("$2")
prepare_products >/dev/null
' bash "$dir" "$product"
}

recorded() {
  grep -qxF -- "$2" "$1/calls"
}

recorded_before() {
  local first second
  first=$(grep -nxF -- "$2" "$1/calls" | head -n 1 | cut -d: -f1)
  second=$(grep -nxF -- "$3" "$1/calls" | head -n 1 | cut -d: -f1)
  [ -n "$first" ] && [ -n "$second" ] && [ "$first" -lt "$second" ]
}

prepared=$(mktemp -d)
trap 'rm -rf "$prepared"; [ -z "${rehearsal:-}" ] || rm -rf "$rehearsal"' EXIT
check "install.sh names the packages of panel" test "$(packages_of panel)" = "${panel_packages[*]} "
check "install.sh installs no package for dns" test -z "$(packages_of dns)"
if prepare_panel_under "$prepared"; then
  check "install.sh --product panel installs the packages of panel in one apt_install" recorded "$prepared" "apt_install ${panel_packages[*]}"
  check "install.sh answers the mailer question of postfix before the packages" grep -qF 'postfix postfix/main_mailer_type select Internet Site' "$prepared/debconf"
  check "install.sh enables the units of panel with the php-fpm unit found" recorded "$prepared" "systemctl enable ${panel_units[*]}"
  check "install.sh starts the units of panel after their configuration" recorded "$prepared" "systemctl restart ${panel_units[*]}"
  for unit in "${panel_units[@]}"; do
    check "install.sh finds $unit running" recorded "$prepared" "systemctl is-active --quiet $unit"
  done
  check "install.sh installs the packages before it enables the units" recorded_before "$prepared" "apt_install ${panel_packages[*]}" "systemctl enable ${panel_units[*]}"
  check "install.sh gives the transfer root to keel alone" recorded "$prepared" "install -d -m 0700 -o keel -g keel $prepared/transfer"
  check "install.sh gives the certificate root to root alone" recorded "$prepared" "install -d -m 0700 -o root -g root $prepared/certs"
  check "install.sh creates the mail user vmail without a login" recorded "$prepared" "useradd --system --home-dir $prepared/maildirs --no-create-home --shell /usr/sbin/nologin vmail"
  for map in domains mailboxes aliases; do
    check "install.sh builds the empty map keel-panel-$map" recorded "$prepared" "postmap hash:$prepared/mail/keel-panel-$map"
  done
  check "install.sh points postfix at the virtual mailboxes over LMTP" recorded "$prepared" "postconf -e virtual_mailbox_domains = hash:$prepared/mail/keel-panel-domains virtual_mailbox_maps = hash:$prepared/mail/keel-panel-mailboxes virtual_alias_maps = hash:$prepared/mail/keel-panel-aliases virtual_transport = lmtp:unix:private/dovecot-lmtp"
  check "install.sh points dovecot at the passwd-file of the panel" grep -qxF "  passwd_file_path = $prepared/dovecot/keel-panel-passwd" "$prepared/90-keel-panel.conf"
  check "install.sh points the INBOX of the panel mailboxes at the maildir root" grep -qxF "mail_inbox_path = ~/Maildir" "$prepared/90-keel-panel.conf"
  check "install.sh keeps the domain of the address for the lmtp lookups of the panel mailboxes" test "$(sed -n '/^protocol lmtp {$/,/^}$/p' "$prepared/90-keel-panel.conf")" = "$(printf 'protocol lmtp {\n  auth_username_format = %%{user | lower}\n}')"
  check "install.sh checks dovecot and postfix before it starts them" recorded_before "$prepared" "doveconf -n" "systemctl restart ${panel_units[*]}"
  check "install.sh points vsftpd at the per-account files of the panel" grep -qxF "user_config_dir=$prepared/ftp-users" "$prepared/vsftpd.conf"
  check "install.sh authenticates FTP through its PAM service" grep -qxF "pam_service_name=keel-panel-ftp" "$prepared/vsftpd.conf"
  check "the PAM service reads the user file of the panel" grep -qxF "auth required pam_pwdfile.so pwdfile=$prepared/ftp/keel-panel-passwd" "$prepared/pam.d/keel-panel-ftp"
  check "install.sh keeps the previous vsftpd.conf" grep -qxF "listen=NO" "$prepared/vsftpd.conf.orig"
  check "install.sh checks vsftpd.conf before it starts vsftpd" recorded_before "$prepared" "vsftpd $prepared/vsftpd.conf -v" "systemctl restart ${panel_units[*]}"
  drop_in=$prepared/keel.service.d/keel-panel.conf
  check "install.sh creates the drop-in directory of keel.service for root" recorded "$prepared" "install -d -m 0755 -o root -g root $prepared/keel.service.d"
  check "install.sh creates the drop-in of keel.service with mode 0644 for root" recorded "$prepared" "install -m 0644 -o root -g root /dev/null $drop_in"
  check "the drop-in of keel.service is a Service section naming the transfer root alone" test "$(cat "$drop_in")" = "$(printf '[Service]\nReadWritePaths=%s/transfer' "$prepared")"
  check "install.sh writes the drop-in of keel.service after the transfer root it names" recorded_before "$prepared" "install -d -m 0700 -o keel -g keel $prepared/transfer" "install -m 0644 -o root -g root /dev/null $drop_in"
else
  problem "install.sh --product panel did not prepare the host against the recording stubs"
fi
dns_prepared=$(mktemp -d)
if prepare_panel_under "$dns_prepared" dns; then
  check "install.sh --product dns writes no drop-in of keel.service" test ! -e "$dns_prepared/keel.service.d"
  check "install.sh --product dns runs no command at all in its preparation" test ! -s "$dns_prepared/calls"
else
  problem "install.sh --product dns did not prepare the host against the recording stubs"
fi
rm -rf "$dns_prepared"
main_line() {
  grep -nE "^  $1\$" deploy/install.sh | cut -d: -f1
}
check "install.sh writes the drop-in of keel.service before it installs the units and reloads systemd" test "$(main_line prepare_products)" -lt "$(main_line install_units)"
check "install.sh starts keel.service after the units are installed and systemd is reloaded" test "$(main_line install_units)" -lt "$(main_line start_service)"
check "install.sh reloads systemd in the step that installs the units" test -n "$(sed -n '/^install_units() {/,/^}/p' deploy/install.sh | grep -x '  systemctl daemon-reload')"
check "install.sh starts keel.service in one place only" test "$(grep -c 'systemctl start keel.service' deploy/install.sh)" = "1"
check "uninstall.sh removes the drop-in of keel.service" grep -qF 'UNIT_DROP_INS=(keel-panel.conf)' deploy/uninstall.sh
check "the deploy README gives the operator the drop-in and the reload" test "$(grep -cF -e 'keel.service.d/keel-panel.conf' -e 'systemctl daemon-reload' deploy/README.md)" -ge 3
check "panel-vm.sh install proves the transfer root among the ReadWritePaths of the unit" grep -qF 'among the ReadWritePaths of $UNIT' hack/panel-vm.sh

if [ "$problems" -gt 0 ]; then
  echo "$problems problem(s)" >&2
  exit 1
fi
echo "the installer is checked"
