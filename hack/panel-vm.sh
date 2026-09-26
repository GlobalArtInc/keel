#!/usr/bin/env bash
set -euo pipefail

ETC=/etc/keel
CONFIG=$ETC/config.yaml
POLICY=$ETC/agent.yaml
KEEL=/opt/keel/bin/current
UNIT=keel.service
AGENT_UNIT=keel-privileged.service
WWW=/var/www
WAIT=${KEEL_PANEL_VM_WAIT:-60}
STAMP=$(date +%s)
NAME=vm$STAMP
USER_NAME=hp_$NAME
HOME_DIR=$WWW/keel-panel-$NAME
NOLOGIN=/usr/sbin/nologin
LOGIN_SHELL=/bin/bash
DOMAIN=$NAME.test
ALIAS=www.$DOMAIN
VHOST=/etc/nginx/conf.d/keel-panel-$DOMAIN.conf
DOC_ROOT=$HOME_DIR/$DOMAIN
CERTS=/etc/ssl/keel
CHALLENGES=/var/lib/keel-panel-acme
DUMPS=/var/lib/keel-panel-dumps
BACKUPS=/var/lib/keel/backups
SITE_BACKUPS=/var/lib/keel-panel-backups
SITE_MAILBOX=box@$DOMAIN
DATABASE=${USER_NAME}_wp
DATABASE_USER=${USER_NAME}_app
SIBLING=${USER_NAME}xwp
DATABASE_PASSWORD="it's $STAMP \\ \"quoted\""
STAGING=https://acme-staging-v02.api.letsencrypt.org/directory
ACME_DIRECTORY=${KEEL_PANEL_VM_ACME:-$STAGING}
MAIL_MAPS=/etc/postfix/keel
MAIL_PASSWD_DIR=/etc/dovecot/keel
MAIL_PASSWD=$MAIL_PASSWD_DIR/keel-panel-passwd
MAILDIRS=/var/lib/keel-panel-mail
MAIL_DROP_IN=/etc/dovecot/conf.d/90-keel-panel.conf
MAIL_DOMAIN=mail-$NAME.test
MAILBOX=box@$MAIL_DOMAIN
MAIL_ALIAS=info@$MAIL_DOMAIN
MAIL_PASSWORD=mail-$STAMP-secret
FTP_CONF=/etc/vsftpd.conf
FTP_PAM=/etc/pam.d/keel-panel-ftp
FTP_DIR=/etc/vsftpd/keel
FTP_PASSWD=$FTP_DIR/keel-panel-passwd
FTP_USERS=/etc/vsftpd/keel-users
FTP_NAME=${NAME}up
FTP_LOGIN=hp_$FTP_NAME
FTP_PASSWORD=ftp-$STAMP-secret
FTP_CHROOT=$DOC_ROOT/uploads
LIB=/var/lib/keel
CERT=$LIB/tls/cert.pem
TRANSFER=/var/lib/keel-panel-transfer
FILES_LOGIN=panelvm$STAMP
CONFIG_HEADER="add_header X-Keel-Panel config-$STAMP always;"
CONFIG_POOL="pm.max_children = 3"
FILES_CONTENT="keel-panel-files $STAMP"

usage() {
  cat <<EOF
usage: hack/panel-vm.sh install <keel binary or tar.zst>
       hack/panel-vm.sh run
       hack/panel-vm.sh ssl <domain>
       hack/panel-vm.sh db
       hack/panel-vm.sh mail
       hack/panel-vm.sh ftp
       hack/panel-vm.sh files
       hack/panel-vm.sh backup
       hack/panel-vm.sh config

Proves the hosting users and the sites of the panel bundle on a virtual
machine, run as root from a checkout of the repository.

"install" installs nginx, php-fpm, mariadb, postfix, dovecot, vsftpd,
libpam-pwdfile, curl, openssl and zstd, prepares $WWW, the certificate root
$CERTS, the challenge root $CHALLENGES, the dump root $DUMPS and the site
backup root $SITE_BACKUPS, points postfix
at the virtual maps under $MAIL_MAPS and dovecot, as the system user vmail, at
the passwd-file $MAIL_PASSWD and the maildirs under $MAILDIRS through
$MAIL_DROP_IN, writes $FTP_CONF for virtual users authenticated by the PAM
service $FTP_PAM against the user file $FTP_PASSWD, each mapped and chrooted by
its file under $FTP_USERS, then runs deploy/install.sh --product panel
--binary <archive> --yes and prints the setup link.

"run" proves the running installation over the control socket as the user
keel. It creates a user account and in it the hosting user $NAME, waits for
it to turn active and finds the operating-system account $USER_NAME with a
uid inside the uid_range of $POLICY, the home $HOME_DIR owned by it and no
login shell. It grants the shell and takes it back. It creates the site
$DOMAIN with the alias $ALIAS on the newest PHP of /etc/php, waits for it to
turn active, finds its server block in /etc/nginx/conf.d and its pool in the
pool.d of that PHP, both accepted by nginx -t and php-fpm -t, writes an
index.php into $DOC_ROOT as $USER_NAME and fetches it through nginx. It
suspends the account and finds $USER_NAME locked and expired and the site
answering 403, resumes it and finds both open again, then deletes the
account and finds the server block, the pool, $USER_NAME and its home gone,
with every change in the journal of $AGENT_UNIT.

"ssl <domain>" needs a domain whose address is this machine, with port 80
reachable from the internet. It points the setting
hosting.certificate.directory at $ACME_DIRECTORY (KEEL_PANEL_VM_ACME
overrides it; the default is the Let's Encrypt staging directory), creates
an account, a hosting user and the site <domain> with a Let's Encrypt
certificate, waits for the certificate to turn active, finds the server block
listening on 443 with the pair under $CERTS, the key readable by root alone,
no challenge left in $CHALLENGES and nginx serving <domain> over TLS with the
issued certificate, then deletes the account and finds the pair gone.

"db" proves the databases on the running installation. It creates an
account with the hosting user $NAME, the database $DATABASE and the database
user $DATABASE_USER with a password carrying quotes and a backslash, finds
the database in mariadb and lets $DATABASE_USER create a table in it with
that password. It creates $SIBLING as root, which an unescaped grant on
$DATABASE would match, and finds it closed to the user. It revokes the grant and finds the database closed to the
user, grants it again, suspends the account and finds the login refused,
resumes it and finds it open again. It runs backup.create and finds the dump
of $DATABASE in the archive and $DUMPS empty, then deletes the account and
finds the database and the user dropped, with mariadb.exec in the journal
of $AGENT_UNIT and no password in it.

"mail" proves the mail on the running installation. It creates an account
with the mail domain $MAIL_DOMAIN and the mailbox $MAILBOX with a quota and the
alias $MAIL_ALIAS, finds both in the virtual maps through postmap -q, the
passwd-file owned by dovecot with mode 0640 and an ARGON2ID hash, and
doveadm auth test accepting the password and refusing another. It sends a mail
to the mailbox and one to the alias through sendmail and finds both in the
maildir. It suspends the account and finds the login refused, resumes it and
finds it accepted again. It runs backup.create and finds the maildirs in the
archive and $DUMPS empty, then deletes the account and finds the mailbox gone
from the maps and the passwd-file and its maildir removed.

"ftp" proves the FTP accounts on the running installation. It finds that
vsftpd refuses a configuration with an unknown option, the way vsftpd.check
of $POLICY reads $FTP_CONF. It creates an account with the hosting user
$NAME and the site $DOMAIN, refuses an FTP account whose directory leaves
the site, and creates $FTP_LOGIN chrooted to $FTP_CHROOT. It finds the user
file and the file of $FTP_LOGIN owned by root with mode 0600, logs in over
FTP, finds / of the session to be $FTP_CHROOT with nothing of the host above
it, uploads a file that lands there owned by $USER_NAME and finds another
password refused. It suspends the account and finds the login refused,
resumes it and finds it accepted again, then deletes the account and finds
$FTP_LOGIN gone from the user file and its file under $FTP_USERS removed.

"files" proves the file manager on the running installation. It creates an
account with the hosting user $NAME and, through the file functions over the
control socket, the folder docs in $HOME_DIR and an upload into it, both owned
by $USER_NAME, with nothing left in the transfer root $TRANSFER. It renames the
upload, sets its mode to 0600 and lists the folder, downloads the file over
HTTPS with a kat_ token of the principal $FILES_LOGIN and gets its content
back. It finds a link $USER_NAME plants towards /etc refused as a path leaving
the home, and a mode change of a file root placed in the home refused because
the worker runs as $USER_NAME, then deletes the folder and the account, with
the operations of host.home in the journal of $AGENT_UNIT.

"backup" proves the site backups on the running installation. It creates an
account with the hosting user $NAME, the site $DOMAIN, the database $DATABASE
with a table and a row, the mail domain $DOMAIN with the mailbox
$SITE_MAILBOX and a delivered mail, and in $DOC_ROOT an index, a link to it and
a link to /etc/shadow, all written as $USER_NAME. It runs hosting.backup.create
and finds the archive in $SITE_BACKUPS owned by keel with mode 0600 beside its
digest owned by root with mode 0600, carrying the index, the dump of
$DATABASE and the mail, the link to /etc/shadow as a link and nothing of
/etc/shadow, and $DUMPS empty. It overwrites the index, removes the link,
drops the table and removes the mail, appends one byte to the archive as keel
and finds the restore failing with the index still overwritten, puts the
archive back, runs hosting.backup.restore and finds the index, the link, the
row and the mail back, owned by $USER_NAME and vmail, and the mariadb client
refusing a shell escape under --sandbox. It sets a schedule keeping one archive,
takes a second backup and finds the first archive gone, then deletes the
account and finds the archives of the site removed.

"config" proves the configuration editor on the running installation. It
creates an account with the hosting user $NAME and the site $DOMAIN on the
newest PHP of /etc/php. It saves a server block override with a directive
nginx does not know through hosting.site.config.set and finds it refused with
the output of nginx -t on the field server, the server block unchanged and
nginx answering as before. It saves the override $CONFIG_HEADER and the pool
line $CONFIG_POOL, finds both in the server block and the pool, both accepted by
nginx -t and php-fpm -t, nginx sending the header and
hosting.site.config.get answering the installed server block, then deletes
the account.
EOF
}

say() {
  printf '%s\n' "$*"
}

fail() {
  say "panel-vm.sh: $1" >&2
  exit 1
}

failures=0

check() {
  local name=$1
  shift
  if "$@"; then
    say "ok: $name"
  else
    say "FAILED: $name"
    failures=$((failures + 1))
  fi
}

need_root() {
  [ "$(id -u)" -eq 0 ] || fail "run as root"
}

install_keel() {
  local archive=${1:-}
  [ -n "$archive" ] || { usage >&2; fail "the archive is required"; }
  [ -f "$archive" ] || fail "$archive is not a file"
  need_root
  cd "$(dirname "$0")/.."
  [ -f deploy/install.sh ] || fail "deploy/install.sh is missing; run from a checkout of the repository"
  printf '%s\n' "postfix postfix/main_mailer_type select Internet Site" "postfix postfix/mailname string $(hostname -f)" | debconf-set-selections
  DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends nginx php-fpm mariadb-server mariadb-client postfix dovecot-core dovecot-imapd dovecot-lmtpd vsftpd libpam-pwdfile curl openssl zstd
  install -d -m 0755 -o root -g root "$WWW" "$CHALLENGES"
  install -d -m 0711 -o root -g root "$DUMPS" "$SITE_BACKUPS"
  install -d -m 0700 -o root -g root "$CERTS"
  prepare_mail
  prepare_ftp
  local log
  log=$(mktemp)
  if ! bash deploy/install.sh --product panel --binary "$archive" --yes 2>&1 | tee "$log"; then
    fail "deploy/install.sh failed; its output is in $log"
  fi
  check "$UNIT is active" systemctl is-active --quiet "$UNIT"
  check "$AGENT_UNIT is active" systemctl is-active --quiet "$AGENT_UNIT"
  check "the installer printed the setup link" grep -qF "setup link:   https://" "$log"
  install -d -m 0700 -o keel -g keel "$TRANSFER"
  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "keel is installed with panel; run: hack/panel-vm.sh run"
}

prepare_mail() {
  getent passwd vmail >/dev/null || useradd --system --home-dir "$MAILDIRS" --no-create-home --shell "$NOLOGIN" vmail
  install -d -m 0755 -o root -g root "$MAIL_MAPS"
  install -d -m 0750 -o root -g dovecot "$MAIL_PASSWD_DIR"
  install -d -m 0700 -o vmail -g vmail "$MAILDIRS"
  local map
  for map in domains mailboxes aliases; do
    [ -f "$MAIL_MAPS/keel-panel-$map" ] || install -m 0644 -o root -g root /dev/null "$MAIL_MAPS/keel-panel-$map"
    postmap "hash:$MAIL_MAPS/keel-panel-$map"
  done
  [ -f "$MAIL_PASSWD" ] || install -m 0640 -o dovecot -g dovecot /dev/null "$MAIL_PASSWD"
  postconf -e \
    "virtual_mailbox_domains = hash:$MAIL_MAPS/keel-panel-domains" \
    "virtual_mailbox_maps = hash:$MAIL_MAPS/keel-panel-mailboxes" \
    "virtual_alias_maps = hash:$MAIL_MAPS/keel-panel-aliases" \
    "virtual_transport = lmtp:unix:private/dovecot-lmtp"
  cat >"$MAIL_DROP_IN" <<CONF
mail_driver = maildir
mail_path = ~/Maildir
mail_uid = vmail
mail_gid = vmail
passdb passwd-file {
  passwd_file_path = $MAIL_PASSWD
}
userdb passwd-file {
  passwd_file_path = $MAIL_PASSWD
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
CONF
  doveconf -n >/dev/null || fail "doveconf -n refuses $MAIL_DROP_IN"
  postfix check || fail "postfix check refuses the virtual maps"
  systemctl restart dovecot postfix
}

prepare_ftp() {
  install -d -m 0700 -o root -g root "$FTP_DIR" "$FTP_USERS"
  [ -f "$FTP_PASSWD" ] || install -m 0600 -o root -g root /dev/null "$FTP_PASSWD"
  [ -f "$FTP_CONF.orig" ] || cp -p "$FTP_CONF" "$FTP_CONF.orig"
  cat >"$FTP_PAM" <<CONF
auth required pam_pwdfile.so pwdfile=$FTP_PASSWD
account required pam_permit.so
CONF
  cat >"$FTP_CONF" <<CONF
listen=YES
listen_ipv6=NO
anonymous_enable=NO
local_enable=YES
write_enable=YES
local_umask=022
guest_enable=YES
guest_username=nobody
virtual_use_local_privs=YES
pam_service_name=keel-panel-ftp
user_config_dir=$FTP_USERS
chroot_local_user=YES
allow_writeable_chroot=YES
secure_chroot_dir=/var/run/vsftpd/empty
hide_ids=YES
CONF
  vsftpd "$FTP_CONF" -v </dev/null >/dev/null || fail "vsftpd refuses $FTP_CONF"
  systemctl restart vsftpd
}

as_keel() {
  runuser -u keel -- "$KEEL" --config "$CONFIG" "$@"
}

field() {
  python3 -c '
import json, sys
doc = json.load(sys.stdin)
value = doc
for key in sys.argv[1].split("."):
    value = value[int(key)] if isinstance(value, list) else value[key]
print(value)
' "$1"
}

call() {
  local out
  out=$(as_keel ctl "$@" --out json) || {
    say "  keel ctl $1 answered: $out" >&2
    return 1
  }
  printf '%s\n' "$out"
}

range_bound() {
  sed -nE "s/^  uid_range: \{ from: ([0-9]+), to: ([0-9]+) \}$/\\$1/p" "$POLICY"
}

passwd_field() {
  getent passwd "$USER_NAME" | cut -d: -f"$1"
}

shadow_field() {
  getent shadow "$USER_NAME" | cut -d: -f"$1"
}

eventually() {
  local waited=0
  while [ "$waited" -lt "$((WAIT * 4))" ]; do
    if "$@"; then
      return 0
    fi
    sleep 0.25
    waited=$((waited + 1))
  done
  return 1
}

state_is() {
  [ "$(call hosting.user.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "$2" ]
}

inside_range() {
  local uid from to
  uid=$(passwd_field 3)
  from=$(range_bound 1)
  to=$(range_bound 2)
  say "  $USER_NAME has uid $uid; the range is $from to $to"
  [ -n "$uid" ] && [ -n "$from" ] && [ -n "$to" ] && [ "$uid" -ge "$from" ] && [ "$uid" -le "$to" ]
}

home_owned() {
  [ "$(passwd_field 6)" = "$HOME_DIR" ] && [ -d "$HOME_DIR" ] && [ "$(stat -c %u "$HOME_DIR")" = "$(passwd_field 3)" ]
}

shell_is() {
  [ "$(passwd_field 7)" = "$1" ]
}

locked() {
  [ "$(shadow_field 8)" = "1" ] && [ "$(shadow_field 2 | cut -c1)" = "!" ]
}

open_again() {
  [ -z "$(shadow_field 8)" ]
}

gone() {
  ! getent passwd "$USER_NAME" >/dev/null && [ ! -e "$HOME_DIR" ]
}

agent_logged() {
  journalctl -u "$AGENT_UNIT" --since "$1" --no-pager | grep -qF -- "$2"
}

php_version() {
  find /etc/php -mindepth 3 -maxdepth 3 -path '*/fpm/pool.d' 2>/dev/null | cut -d/ -f4 | sort -V | tail -n 1
}

site_state_is() {
  [ "$(call hosting.site.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "$2" ]
}

fetched() {
  [ "$(curl -s -o /dev/null -w '%{http_code}' -H "Host: $2" http://127.0.0.1/)" = "$1" ]
}

serves_index() {
  [ "$(curl -fsS -H "Host: $1" http://127.0.0.1/ 2>/dev/null)" = "keel-panel" ]
}

task_done() {
  [ "$(call task.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "done" ]
}

task_failed() {
  [ "$(call task.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "failed" ]
}

run_proof() {
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  [ -d "$WWW" ] || fail "$WWW is missing; run install first"
  local since account id task
  since=$(date -u '+%Y-%m-%d %H:%M:%S')

  say "--- a hosting user"
  account=$(call keel.account.create kind=user | field data.id) || fail "keel ctl keel.account.create failed"
  say "  the account $account"
  id=$(call hosting.user.create "name=$NAME" "account=$account" | field data.id) || fail "keel ctl hosting.user.create failed"
  check "the hosting user $NAME turns active" eventually state_is "$id" active
  check "$USER_NAME is an account of the host" getent passwd "$USER_NAME"
  check "the uid of $USER_NAME is inside the uid range of the policy" inside_range
  check "the home $HOME_DIR belongs to $USER_NAME" home_owned
  check "$USER_NAME has no login shell" shell_is "$NOLOGIN"

  say "--- the shell"
  check "keel ctl hosting.user.edit grants the shell" call hosting.user.edit "id=$id" shell=true
  check "$USER_NAME gets $LOGIN_SHELL" eventually shell_is "$LOGIN_SHELL"
  check "keel ctl hosting.user.edit takes the shell back" call hosting.user.edit "id=$id" shell=false
  check "$USER_NAME is back to $NOLOGIN" eventually shell_is "$NOLOGIN"

  say "--- a site"
  local php pool site
  php=$(php_version)
  [ -n "$php" ] || fail "no php-fpm under /etc/php; run install first"
  pool=/etc/php/$php/fpm/pool.d/keel-panel-$DOMAIN.conf
  check "the policy names the pool root of php$php-fpm" grep -qF "pools-$php: /etc/php/$php/fpm/pool.d" "$POLICY"
  check "the policy names the check of php$php-fpm" grep -qF "php$php-fpm.test" "$POLICY"
  site=$(call hosting.site.create "user=$id" "domain=$DOMAIN" "aliases=[\"$ALIAS\"]" "php=\"$php\"" | field data.id) || fail "keel ctl hosting.site.create failed"
  check "the site $DOMAIN turns active" eventually site_state_is "$site" active
  check "the server block $VHOST serves $DOMAIN and $ALIAS" grep -qF "server_name $DOMAIN $ALIAS;" "$VHOST"
  check "the server block passes PHP to the pool of php$php" grep -qF "fastcgi_pass unix:/run/php/keel-panel-$site-$php.sock;" "$VHOST"
  check "the server block refuses symlinks another account owns" grep -qF 'disable_symlinks if_not_owner from=$document_root;' "$VHOST"
  check "the pool $pool runs as $USER_NAME" grep -qxF "user = $USER_NAME" "$pool"
  check "nginx accepts the configuration" nginx -t
  check "php-fpm$php accepts the configuration" "php-fpm$php" -t
  say "  the home $HOME_DIR has mode $(stat -c %a "$HOME_DIR"); the panel does not create the document root, so the proof writes it as $USER_NAME and lets nginx search the home"
  chmod o+x "$HOME_DIR"
  runuser -u "$USER_NAME" -- /bin/sh -c "mkdir -p '$DOC_ROOT' && chmod 0755 '$DOC_ROOT' && printf '%s' '<?php echo \"keel-\" . \"panel\";' > '$DOC_ROOT/index.php' && chmod 0644 '$DOC_ROOT/index.php'"
  check "nginx serves the index.php of $DOMAIN through the pool" eventually serves_index "$DOMAIN"
  check "nginx serves it under the alias $ALIAS" serves_index "$ALIAS"

  say "--- suspend and resume"
  check "keel ctl keel.account.suspend suspends $account" call keel.account.suspend "id=$account"
  check "the operating-system account is locked and expired while its account is suspended" eventually locked
  check "the hosting user is suspended" state_is "$id" suspended
  check "the site answers 403 while its account is suspended" eventually fetched 403 "$DOMAIN"
  check "keel ctl keel.account.resume resumes $account" call keel.account.resume "id=$account"
  check "$USER_NAME no longer expires once its account is resumed" eventually open_again
  check "the hosting user is active again" state_is "$id" active
  check "the site serves its index again" eventually serves_index "$DOMAIN"

  say "--- the account deleted"
  task=$(call keel.account.delete "id=$account" | field data.task.id) || task=""
  check "keel ctl keel.account.delete queues a task" test -n "$task"
  check "the deletion task is done" eventually task_done "$task"
  check "$USER_NAME and its home are gone" eventually gone
  check "the server block $VHOST is gone" test ! -e "$VHOST"
  check "the pool $pool is gone" test ! -e "$pool"
  check "nginx still accepts the configuration" nginx -t
  check "the agent journal carries host.user.create" agent_logged "$since" host.user.create
  check "the agent journal carries host.user.modify" agent_logged "$since" host.user.modify
  check "the agent journal carries host.user.delete" agent_logged "$since" host.user.delete
  check "the agent journal carries host.template.apply" agent_logged "$since" host.template.apply
  check "the agent journal carries host.fs.remove" agent_logged "$since" host.fs.remove

  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "panel's hosting users and sites are proven on this machine: an account inside the range with its home, the shell, a site on nginx and php-fpm, the lock and the refusal of a suspended account and the removal with the account"
}

certificate_state_is() {
  [ "$(call hosting.certificate.list 2>/dev/null | field data.data.0.state 2>/dev/null)" = "$1" ]
}

pair_gone() {
  [ -z "$(find "$CERTS" -name 'keel-panel-*' -print -quit)" ]
}

served_over_tls() {
  [ "$(curl -s -o /dev/null -w '%{http_code}' --insecure --resolve "$1:443:127.0.0.1" "https://$1/")" != "000" ]
}

served_certificate() {
  openssl s_client -connect 127.0.0.1:443 -servername "$1" </dev/null 2>/dev/null | openssl x509 -noout -fingerprint -sha256 2>/dev/null
}

ssl_proof() {
  local domain=${1:-}
  [ -n "$domain" ] || { usage >&2; fail "the domain is required"; }
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  [ -d "$CERTS" ] && [ -d "$CHALLENGES" ] || fail "$CERTS or $CHALLENGES is missing; run install first"
  local since account id site vhost stem task
  since=$(date -u '+%Y-%m-%d %H:%M:%S')
  vhost=/etc/nginx/conf.d/keel-panel-$domain.conf

  say "--- a site with a certificate from $ACME_DIRECTORY"
  check "keel ctl settings.set points at the directory" call settings.set key=hosting.certificate.directory "value=\"$ACME_DIRECTORY\""
  account=$(call keel.account.create kind=user | field data.id) || fail "keel ctl keel.account.create failed"
  id=$(call hosting.user.create "name=$NAME" "account=$account" | field data.id) || fail "keel ctl hosting.user.create failed"
  check "the hosting user $NAME turns active" eventually state_is "$id" active
  site=$(call hosting.site.create "user=$id" "domain=$domain" letsEncrypt=true | field data.id) || fail "keel ctl hosting.site.create failed"
  check "the site $domain turns active" eventually site_state_is "$site" active
  WAIT=$((WAIT * 3)) check "the certificate of $domain turns active" eventually certificate_state_is active
  stem=$(grep -oE 'keel-panel-[0-9a-z]+\.crt' "$vhost" | head -n 1)
  stem=${stem%.crt}
  check "the server block listens on 443" grep -qF "listen 443 ssl;" "$vhost"
  check "the server block names the pair under $CERTS" grep -qF "ssl_certificate $CERTS/$stem.crt;" "$vhost"
  check "the server block serves the challenges from $CHALLENGES" grep -qF "alias $CHALLENGES/keel-panel-\$1;" "$vhost"
  check "the key $CERTS/$stem.key is readable by root alone" test "$(stat -c %a "$CERTS/$stem.key")" = 600
  check "no challenge is left in $CHALLENGES" test -z "$(ls -A "$CHALLENGES")"
  check "nginx accepts the configuration" nginx -t
  check "nginx serves $domain over TLS" served_over_tls "$domain"
  check "nginx serves the issued certificate" test "$(served_certificate "$domain")" = "$(openssl x509 -in "$CERTS/$stem.crt" -noout -fingerprint -sha256)"
  say "  issued by: $(openssl x509 -in "$CERTS/$stem.crt" -noout -issuer)"

  say "--- the account deleted"
  task=$(call keel.account.delete "id=$account" | field data.task.id) || task=""
  check "the deletion task is done" eventually task_done "$task"
  check "the pair is gone from $CERTS" eventually pair_gone
  check "nginx still accepts the configuration" nginx -t
  check "the agent journal carries host.fs.write" agent_logged "$since" host.fs.write

  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "panel's certificates are proven on this machine: a site on a real domain got its certificate from $ACME_DIRECTORY over HTTP-01 and nginx served it"
}

sql() {
  mariadb --batch --skip-column-names -e "$1"
}

database_state_is() {
  [ "$(call hosting.database.list 2>/dev/null | python3 -c '
import json, sys
rows = json.load(sys.stdin)["data"]["data"]
print(next((row["state"] for row in rows if row["id"] == sys.argv[1]), ""))
' "$1" 2>/dev/null)" = "$2" ]
}

database_user_state_is() {
  [ "$(call hosting.database.user.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "$2" ]
}

database_exists() {
  [ "$(sql "SHOW DATABASES LIKE '$DATABASE'")" = "$DATABASE" ]
}

database_user_exists() {
  [ "$(sql "SELECT COUNT(*) FROM mysql.user WHERE User = '$DATABASE_USER'")" = "1" ]
}

database_gone() {
  [ -z "$(sql "SHOW DATABASES LIKE '$DATABASE'")" ]
}

database_user_gone() {
  [ "$(sql "SELECT COUNT(*) FROM mysql.user WHERE User = '$DATABASE_USER'")" = "0" ]
}

reaches_database() {
  MYSQL_PWD=$DATABASE_PASSWORD mariadb --batch -u "$DATABASE_USER" -e "CREATE TABLE IF NOT EXISTS proof (a INT); SELECT COUNT(*) FROM proof" "$DATABASE" >/dev/null 2>&1
}

closed_to_user() {
  ! reaches_database
}

sibling_closed_to_user() {
  ! MYSQL_PWD=$DATABASE_PASSWORD mariadb --batch -u "$DATABASE_USER" -e "SELECT 1" "$SIBLING" >/dev/null 2>&1
}

dumped() {
  local archive
  archive=$(ls -t "$BACKUPS"/*.tar.zst 2>/dev/null | head -n 1)
  [ -n "$archive" ] && tar --zstd -tf "$archive" | grep -qx "sources/hosting_database.dumps/$DATABASE.sql"
}

no_password_in_journal() {
  ! journalctl -u "$AGENT_UNIT" --since "$1" --no-pager | grep -qF -- "$STAMP"
}

db_proof() {
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  systemctl is-active --quiet mariadb || fail "mariadb is not running; run install first"
  [ -d "$DUMPS" ] || fail "$DUMPS is missing; run install first"
  local since account id database user task
  since=$(date -u '+%Y-%m-%d %H:%M:%S')

  say "--- a database and its user"
  account=$(call keel.account.create kind=user | field data.id) || fail "keel ctl keel.account.create failed"
  id=$(call hosting.user.create "name=$NAME" "account=$account" | field data.id) || fail "keel ctl hosting.user.create failed"
  check "the hosting user $NAME turns active" eventually state_is "$id" active
  database=$(call hosting.database.create "name=$DATABASE" | field data.id) || fail "keel ctl hosting.database.create failed"
  check "the database $DATABASE turns active" eventually database_state_is "$database" active
  check "mariadb has the database $DATABASE" database_exists
  user=$(call hosting.database.user.create "name=$DATABASE_USER" "password=$(python3 -c 'import json, sys; print(json.dumps(sys.argv[1]))' "$DATABASE_PASSWORD")" "databases=[\"$database\"]" | field data.id) || fail "keel ctl hosting.database.user.create failed"
  check "the database user $DATABASE_USER turns active" eventually database_user_state_is "$user" active
  check "mariadb has the user $DATABASE_USER" database_user_exists
  check "$DATABASE_USER creates a table in $DATABASE with its password" reaches_database
  check "root creates $SIBLING beside $DATABASE" sql "CREATE DATABASE \`$SIBLING\`"
  check "the grant on $DATABASE does not reach $SIBLING" sibling_closed_to_user
  check "root drops $SIBLING" sql "DROP DATABASE \`$SIBLING\`"

  say "--- the grant"
  check "keel ctl hosting.database.user.edit revokes the grant" call hosting.database.user.edit "id=$user" "databases=[]"
  check "$DATABASE is closed to $DATABASE_USER" eventually closed_to_user
  check "keel ctl hosting.database.user.edit grants it again" call hosting.database.user.edit "id=$user" "databases=[\"$database\"]"
  check "$DATABASE_USER reaches $DATABASE again" eventually reaches_database

  say "--- suspend and resume"
  check "keel ctl keel.account.suspend suspends $account" call keel.account.suspend "id=$account"
  check "the login of $DATABASE_USER is refused while its account is suspended" eventually closed_to_user
  check "keel ctl keel.account.resume resumes $account" call keel.account.resume "id=$account"
  check "$DATABASE_USER logs in again" eventually reaches_database

  say "--- the backup"
  task=$(call backup.create | field data.task.id) || task=""
  check "the backup task is done" eventually task_done "$task"
  check "the archive carries the dump of $DATABASE" dumped
  check "no dump is left in $DUMPS" test -z "$(ls -A "$DUMPS")"

  say "--- the account deleted"
  task=$(call keel.account.delete "id=$account" | field data.task.id) || task=""
  check "the deletion task is done" eventually task_done "$task"
  check "the database $DATABASE is dropped" eventually database_gone
  check "the user $DATABASE_USER is dropped" eventually database_user_gone
  check "the agent journal carries mariadb.exec" agent_logged "$since" mariadb.exec
  check "the agent journal carries no password" no_password_in_journal "$since"

  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "panel's databases are proven on this machine: a database and its user in mariadb, the grant, the lock of a suspended account, the dump in the backup and the drop with the account"
}

mailbox_state_is() {
  [ "$(call hosting.mail.mailbox.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "$2" ]
}

mail_domain_state_is() {
  [ "$(call hosting.mail.domain.list 2>/dev/null | python3 -c '
import json, sys
rows = json.load(sys.stdin)["data"]["data"]
print(next((row["state"] for row in rows if row["id"] == sys.argv[1]), ""))
' "$1")" = "$2" ]
}

mapped() {
  [ "$(postmap -q "$1" "hash:$MAIL_MAPS/keel-panel-$2")" = "$3" ]
}

unmapped() {
  ! postmap -q "$1" "hash:$MAIL_MAPS/keel-panel-$2" >/dev/null
}

passwd_placed() {
  [ "$(stat -c '%U:%G %a' "$MAIL_PASSWD")" = "dovecot:dovecot 640" ] && grep -q "^$MAILBOX:{ARGON2ID}\$argon2id\$" "$MAIL_PASSWD"
}

logs_in() {
  doveadm auth test "$MAILBOX" "$1" >/dev/null 2>&1
}

refused_login() {
  ! logs_in "$1"
}

delivered() {
  grep -rqsF -- "$2" "$MAILDIRS/keel-panel-$1/Maildir/new"
}

send_mail() {
  printf 'From: root@localhost\nTo: %s\nSubject: keel %s\n\n%s\n' "$1" "$2" "$2" | sendmail -f root@localhost "$1"
}

archived_maildir() {
  local archive
  archive=$(ls -t "$BACKUPS"/*.tar.zst 2>/dev/null | head -n 1)
  [ -n "$archive" ] && tar --zstd -xOf "$archive" sources/hosting_mail.maildirs/maildirs.tar | tar -t | grep -q "^./keel-panel-$1/Maildir/new/"
}

mailbox_gone() {
  unmapped "$MAILBOX" mailboxes && ! grep -q "^$MAILBOX:" "$MAIL_PASSWD" && [ ! -e "$MAILDIRS/keel-panel-$1" ]
}

mail_proof() {
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  systemctl is-active --quiet postfix || fail "postfix is not running; run install first"
  systemctl is-active --quiet dovecot || fail "dovecot is not running; run install first"
  [ -f "$MAIL_DROP_IN" ] || fail "$MAIL_DROP_IN is missing; run install first"
  local since account domain mailbox task
  since=$(date -u '+%Y-%m-%d %H:%M:%S')

  say "--- a mail domain and a mailbox"
  account=$(call keel.account.create kind=user | field data.id) || fail "keel ctl keel.account.create failed"
  domain=$(call hosting.mail.domain.create "name=$MAIL_DOMAIN" "account=$account" | field data.id) || fail "keel ctl hosting.mail.domain.create failed"
  check "the mail domain $MAIL_DOMAIN turns active" eventually mail_domain_state_is "$domain" active
  mailbox=$(call hosting.mail.mailbox.create "domain=$domain" local=box "password=$MAIL_PASSWORD" quota=64 'aliases=["info"]' | field data.id) || fail "keel ctl hosting.mail.mailbox.create failed"
  check "the mailbox $MAILBOX turns active" eventually mailbox_state_is "$mailbox" active
  check "postfix finds $MAIL_DOMAIN in the domain map" mapped "$MAIL_DOMAIN" domains OK
  check "postfix finds $MAILBOX in the mailbox map" mapped "$MAILBOX" mailboxes "keel-panel-$mailbox/Maildir/"
  check "postfix maps $MAIL_ALIAS to $MAILBOX" mapped "$MAIL_ALIAS" aliases "$MAILBOX"
  check "the passwd-file belongs to dovecot with mode 0640 and an ARGON2ID hash" passwd_placed
  check "dovecot accepts the password of $MAILBOX" logs_in "$MAIL_PASSWORD"
  check "dovecot refuses another password" refused_login "not-$MAIL_PASSWORD"

  say "--- delivery"
  check "sendmail takes a mail to $MAILBOX" send_mail "$MAILBOX" "direct-$STAMP"
  check "sendmail takes a mail to $MAIL_ALIAS" send_mail "$MAIL_ALIAS" "alias-$STAMP"
  check "the mail to $MAILBOX lands in its maildir" eventually delivered "$mailbox" "direct-$STAMP"
  check "the mail to $MAIL_ALIAS lands in the same maildir" eventually delivered "$mailbox" "alias-$STAMP"
  check "the maildir belongs to vmail" test "$(stat -c %U "$MAILDIRS/keel-panel-$mailbox")" = vmail

  say "--- suspend and resume"
  check "keel ctl keel.account.suspend suspends $account" call keel.account.suspend "id=$account"
  check "dovecot refuses $MAILBOX while its account is suspended" eventually refused_login "$MAIL_PASSWORD"
  check "keel ctl keel.account.resume resumes $account" call keel.account.resume "id=$account"
  check "dovecot accepts $MAILBOX again" eventually logs_in "$MAIL_PASSWORD"

  say "--- the backup"
  task=$(call backup.create | field data.task.id) || task=""
  check "the backup task is done" eventually task_done "$task"
  check "the archive carries the maildir of $MAILBOX" archived_maildir "$mailbox"
  check "no archive is left in $DUMPS" test -z "$(ls -A "$DUMPS")"

  say "--- the account deleted"
  task=$(call keel.account.delete "id=$account" | field data.task.id) || task=""
  check "the deletion task is done" eventually task_done "$task"
  check "$MAILBOX is gone from the maps, the passwd-file and $MAILDIRS" eventually mailbox_gone "$mailbox"
  check "$MAIL_DOMAIN is gone from the domain map" eventually unmapped "$MAIL_DOMAIN" domains
  check "the agent journal carries the templates and postmap" agent_logged "$since" "op=host.exec"
  check "the agent journal carries no mail password" no_mail_password_in_journal "$since"

  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "panel's mail is proven on this machine: the virtual maps and the passwd-file, delivery to a mailbox and an alias, the login refused while suspended, the maildirs in the backup and the removal with the account"
}

ftp_state_is() {
  [ "$(call hosting.ftp.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "$2" ]
}

ftp_fetch() {
  curl -fsS --max-time 20 --user "$FTP_LOGIN:$1" "ftp://127.0.0.1$2" 2>/dev/null
}

ftp_logs_in() {
  ftp_fetch "$FTP_PASSWORD" / >/dev/null
}

ftp_refused() {
  ! ftp_fetch "$1" / >/dev/null
}

ftp_files_placed() {
  [ "$(stat -c '%U:%G %a' "$FTP_PASSWD")" = "root:root 600" ] \
    && [ "$(stat -c '%U:%G %a' "$FTP_USERS/$FTP_LOGIN")" = "root:root 600" ] \
    && grep -qF "$FTP_LOGIN:\$6\$rounds=5000\$" "$FTP_PASSWD" \
    && [ "$(cat "$FTP_USERS/$FTP_LOGIN")" = "$(printf 'guest_username=%s\nlocal_root=%s' "$USER_NAME" "$FTP_CHROOT")" ]
}

ftp_chrooted() {
  local listing
  listing=$(ftp_fetch "$FTP_PASSWORD" / | awk '{print $NF}' | sort | tr '\n' ' ')
  [ "$listing" = "inside.txt " ] \
    && [ "$(ftp_fetch "$FTP_PASSWORD" /inside.txt)" = "keel-panel-ftp" ] \
    && ! ftp_fetch "$FTP_PASSWORD" /etc/passwd >/dev/null \
    && ! ftp_fetch "$FTP_PASSWORD" /../index.php >/dev/null
}

ftp_uploads() {
  local file
  file=$(mktemp)
  printf 'uploaded-%s\n' "$STAMP" >"$file"
  curl -fsS --max-time 20 --user "$FTP_LOGIN:$FTP_PASSWORD" -T "$file" "ftp://127.0.0.1/uploaded.txt" >/dev/null 2>&1
  rm -f "$file"
  [ "$(cat "$FTP_CHROOT/uploaded.txt" 2>/dev/null)" = "uploaded-$STAMP" ] && [ "$(stat -c %U "$FTP_CHROOT/uploaded.txt")" = "$USER_NAME" ]
}

broken_conf_refused() {
  local broken status
  broken=$(mktemp)
  cp "$FTP_CONF" "$broken"
  printf 'keel_no_such_option=YES\n' >>"$broken"
  status=0
  vsftpd "$broken" -v </dev/null >/dev/null 2>&1 || status=$?
  rm -f "$broken"
  [ "$status" -ne 0 ]
}

ftp_gone() {
  ! grep -q "^$FTP_LOGIN:" "$FTP_PASSWD" && [ ! -e "$FTP_USERS/$FTP_LOGIN" ]
}

ftp_proof() {
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  systemctl is-active --quiet vsftpd || fail "vsftpd is not running; run install first"
  [ -f "$FTP_PAM" ] || fail "$FTP_PAM is missing; run install first"
  local since account id site ftp task
  since=$(date -u '+%Y-%m-%d %H:%M:%S')

  say "--- the check"
  check "vsftpd accepts $FTP_CONF" vsftpd "$FTP_CONF" -v </dev/null
  check "vsftpd refuses a configuration with an unknown option" broken_conf_refused

  say "--- a site with an FTP account"
  account=$(call keel.account.create kind=user | field data.id) || fail "keel ctl keel.account.create failed"
  id=$(call hosting.user.create "name=$NAME" "account=$account" | field data.id) || fail "keel ctl hosting.user.create failed"
  check "the hosting user $NAME turns active" eventually state_is "$id" active
  site=$(call hosting.site.create "user=$id" "domain=$DOMAIN" | field data.id) || fail "keel ctl hosting.site.create failed"
  check "the site $DOMAIN turns active" eventually site_state_is "$site" active
  runuser -u "$USER_NAME" -- /bin/sh -c "mkdir -p '$FTP_CHROOT' && printf '%s' keel-panel-ftp > '$FTP_CHROOT/inside.txt' && printf '%s' outside > '$DOC_ROOT/index.php'"
  check "an FTP account whose directory leaves the site is refused" \
    test -z "$(call hosting.ftp.create "site=$site" "name=$FTP_NAME" "password=$FTP_PASSWORD" 'directory=../..' 2>/dev/null)"
  ftp=$(call hosting.ftp.create "site=$site" "name=$FTP_NAME" "password=$FTP_PASSWORD" directory=uploads | field data.id) || fail "keel ctl hosting.ftp.create failed"
  check "the FTP account $FTP_LOGIN turns active" eventually ftp_state_is "$ftp" active
  check "the user file and the file of $FTP_LOGIN belong to root with mode 0600" ftp_files_placed
  check "$FTP_LOGIN logs in over FTP" eventually ftp_logs_in
  check "/ of the session is $FTP_CHROOT with nothing of the host above it" ftp_chrooted
  check "an upload lands in $FTP_CHROOT owned by $USER_NAME" ftp_uploads
  check "another password is refused" ftp_refused "not-$FTP_PASSWORD"

  say "--- suspend and resume"
  check "keel ctl keel.account.suspend suspends $account" call keel.account.suspend "id=$account"
  check "$FTP_LOGIN is refused while its account is suspended" eventually ftp_refused "$FTP_PASSWORD"
  check "keel ctl keel.account.resume resumes $account" call keel.account.resume "id=$account"
  check "$FTP_LOGIN logs in again" eventually ftp_logs_in

  say "--- the account deleted"
  task=$(call keel.account.delete "id=$account" | field data.task.id) || task=""
  check "the deletion task is done" eventually task_done "$task"
  check "$FTP_LOGIN is gone from $FTP_PASSWD and $FTP_USERS" eventually ftp_gone
  check "$FTP_LOGIN is refused" ftp_refused "$FTP_PASSWORD"
  check "the agent journal carries the files of vsftpd" agent_logged "$since" "keel-panel-passwd"
  check "the agent journal carries no FTP password" no_ftp_password_in_journal "$since"

  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "panel's FTP is proven on this machine: a virtual user of vsftpd chrooted to a directory of its site as the site's user, a directory outside the site refused, the login refused while suspended and the removal with the account"
}

backup_state() {
  call task.get "id=$1" 2>/dev/null | field "data.result.$2" 2>/dev/null
}

archive_of() {
  printf '%s/keel-panel-%s.tar.zst' "$SITE_BACKUPS" "$1"
}

archive_placed() {
  local archive
  archive=$(archive_of "$1")
  [ -f "$archive" ] && [ "$(stat -c '%U %a' "$archive")" = "keel 600" ]
}

seal_placed() {
  local seal
  seal=$SITE_BACKUPS/.keel-panel-$1.tar.zst.sha256
  [ -f "$seal" ] && [ "$(stat -c '%U %a' "$seal")" = "root 600" ]
}

tamper_archive() {
  local archive
  archive=$(archive_of "$1")
  cp -p "$archive" "$archive.orig" && runuser -u keel -- /bin/sh -c "printf x >> '$archive'"
}

untamper_archive() {
  local archive
  archive=$(archive_of "$1")
  runuser -u keel -- /bin/sh -c "cat '$archive.orig' > '$archive'" && rm -f "$archive.orig"
}

defaced_index() {
  [ "$(cat "$DOC_ROOT/index.php" 2>/dev/null)" = "defaced" ]
}

sandboxed_client() {
  local marker=/tmp/keel-panel-escape-$STAMP
  rm -f "$marker"
  printf '\\! touch %s\nSELECT 1;\n' "$marker" | mariadb --batch --sandbox --skip-column-names >/dev/null 2>&1
  [ ! -e "$marker" ]
}

archive_lists() {
  tar --zstd -tvf "$(archive_of "$1")" | grep -qF -- "$2"
}

archive_holds_no_shadow() {
  ! tar --zstd -xOf "$(archive_of "$1")" --wildcards 'files/*' 2>/dev/null | grep -qF -- "$(head -n 1 /etc/shadow)"
}

dumps_empty() {
  [ -z "$(ls -A "$DUMPS")" ]
}

restored_index() {
  [ "$(cat "$DOC_ROOT/index.php" 2>/dev/null)" = "backup-$STAMP" ] && [ "$(stat -c %U "$DOC_ROOT/index.php")" = "$USER_NAME" ]
}

restored_link() {
  [ "$(readlink "$DOC_ROOT/home.php")" = "index.php" ] && [ "$(stat -c %U "$DOC_ROOT/home.php")" = "$USER_NAME" ]
}

restored_row() {
  [ "$(mariadb --batch --skip-column-names -e "SELECT v FROM \`$DATABASE\`.proof" 2>/dev/null)" = "kept-$STAMP" ]
}

restored_mail() {
  local found
  found=$(grep -rlsF -- "backup-mail-$STAMP" "$MAILDIRS/keel-panel-$1/Maildir/new" | head -n 1)
  [ -n "$found" ] && [ "$(stat -c %U "$found")" = "vmail" ]
}

backup_proof() {
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  systemctl is-active --quiet mariadb || fail "mariadb is not running; run install first"
  [ -d "$SITE_BACKUPS" ] || fail "$SITE_BACKUPS is missing; run install first"
  local since account id site database domain mailbox task first second
  since=$(date -u '+%Y-%m-%d %H:%M:%S')

  say "--- a site with its files, a database and a mailbox"
  account=$(call keel.account.create kind=user | field data.id) || fail "keel ctl keel.account.create failed"
  id=$(call hosting.user.create "name=$NAME" "account=$account" | field data.id) || fail "keel ctl hosting.user.create failed"
  check "the hosting user $NAME turns active" eventually state_is "$id" active
  site=$(call hosting.site.create "user=$id" "domain=$DOMAIN" | field data.id) || fail "keel ctl hosting.site.create failed"
  check "the site $DOMAIN turns active" eventually site_state_is "$site" active
  database=$(call hosting.database.create "name=$DATABASE" | field data.id) || fail "keel ctl hosting.database.create failed"
  check "the database $DATABASE turns active" eventually database_state_is "$database" active
  check "mariadb has the database $DATABASE" database_exists
  mariadb -e "CREATE TABLE \`$DATABASE\`.proof (v TEXT); INSERT INTO \`$DATABASE\`.proof VALUES ('kept-$STAMP')"
  domain=$(call hosting.mail.domain.create "name=$DOMAIN" "account=$account" | field data.id) || fail "keel ctl hosting.mail.domain.create failed"
  check "the mail domain $DOMAIN turns active" eventually mail_domain_state_is "$domain" active
  mailbox=$(call hosting.mail.mailbox.create "domain=$domain" local=box "password=$MAIL_PASSWORD" | field data.id) || fail "keel ctl hosting.mail.mailbox.create failed"
  check "the mailbox $SITE_MAILBOX turns active" eventually mailbox_state_is "$mailbox" active
  send_mail "$SITE_MAILBOX" "backup-mail-$STAMP"
  check "the mail reaches $SITE_MAILBOX" eventually delivered "$mailbox" "backup-mail-$STAMP"
  runuser -u "$USER_NAME" -- /bin/sh -c "mkdir -p '$DOC_ROOT' && printf '%s' 'backup-$STAMP' > '$DOC_ROOT/index.php' && ln -sf index.php '$DOC_ROOT/home.php' && ln -sf /etc/shadow '$DOC_ROOT/shadow'"

  say "--- a backup"
  task=$(call hosting.backup.create "site=$site" | field data.task.id) || task=""
  check "the backup task is done" eventually task_done "$task"
  first=$(backup_state "$task" id)
  check "the backup is ready" test "$(backup_state "$task" state)" = active
  check "the archive lies in $SITE_BACKUPS owned by keel with mode 0600" archive_placed "$first"
  check "the digest of the archive lies beside it owned by root with mode 0600" seal_placed "$first"
  check "the archive carries the index" archive_lists "$first" "files/index.php"
  check "the archive carries the dump of $DATABASE" archive_lists "$first" "databases/$DATABASE.sql"
  check "the archive carries the mail of $SITE_MAILBOX" archive_lists "$first" "mail/$SITE_MAILBOX/Maildir/new/"
  check "the link to /etc/shadow stays a link in the archive" archive_lists "$first" "files/shadow -> /etc/shadow"
  check "the archive holds nothing of /etc/shadow" archive_holds_no_shadow "$first"
  check "$DUMPS is empty again" dumps_empty

  say "--- the restore"
  runuser -u "$USER_NAME" -- /bin/sh -c "printf '%s' defaced > '$DOC_ROOT/index.php' && rm -f '$DOC_ROOT/home.php'"
  mariadb -e "DROP TABLE \`$DATABASE\`.proof"
  grep -rlsF -- "backup-mail-$STAMP" "$MAILDIRS/keel-panel-$mailbox/Maildir/new" | xargs -r rm -f
  tamper_archive "$first" || fail "the archive could not be changed as keel"
  task=$(call hosting.backup.restore "id=$first" | field data.task.id) || task=""
  check "the restore of an archive keel changed fails" eventually task_failed "$task"
  check "the index stays overwritten after the refused restore" defaced_index
  untamper_archive "$first" || fail "the archive could not be put back"
  task=$(call hosting.backup.restore "id=$first" | field data.task.id) || task=""
  check "the restore task is done" eventually task_done "$task"
  check "the index of the site is back after the restore, owned by $USER_NAME" restored_index
  check "the link is back, owned by $USER_NAME" restored_link
  check "the row of $DATABASE is back" restored_row
  check "the mail is back, owned by vmail" restored_mail "$mailbox"
  check "$DUMPS is empty after the restore" dumps_empty
  check "the mariadb client refuses a shell escape under --sandbox" sandboxed_client

  say "--- retention"
  check "a schedule keeping one archive is set" call hosting.backup.schedule.create "site=$site" "schedule=0 3 * * *" keep=1
  task=$(call hosting.backup.create "site=$site" | field data.task.id) || task=""
  check "the second backup task is done" eventually task_done "$task"
  second=$(backup_state "$task" id)
  check "the second archive is kept" archive_placed "$second"
  check "the first archive is removed" test ! -e "$(archive_of "$first")"

  say "--- the account deleted"
  task=$(call keel.account.delete "id=$account" | field data.task.id) || task=""
  check "the deletion task is done" eventually task_done "$task"
  check "the archives of the site are removed" eventually test ! -e "$(archive_of "$second")"
  check "the agent journal carries the pack and the unpack" agent_logged "$since" "hosting.backup.unpack"

  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "panel's site backups are proven on this machine: the files, a database and the mail of a site packed as the archive of the site, a link never followed, the restore as the site's user and vmail, retention by count and the removal with the account"
}

no_ftp_password_in_journal() {
  ! journalctl -u "$AGENT_UNIT" -u "$UNIT" --since "$1" --no-pager | grep -qF -- "$FTP_PASSWORD"
}

no_mail_password_in_journal() {
  ! journalctl -u "$AGENT_UNIT" -u "$UNIT" --since "$1" --no-pager | grep -qF -- "$MAIL_PASSWORD"
}

listen_of() {
  local listen
  listen=$(sed -nE 's/^listen:[[:space:]]*"?([^"]+)"?[[:space:]]*$/\1/p' "$CONFIG" | head -n 1)
  printf '%s\n' "${listen:-127.0.0.1:1500}"
}

api_base() {
  local listen host port
  listen=$(listen_of)
  host=${listen%:*}
  port=${listen##*:}
  case "$host" in 0.0.0.0 | "[::]" | "") host=127.0.0.1 ;; esac
  printf 'https://%s:%s\n' "$host" "$port"
}

file_task() {
  local task
  task=$(call "$@" | field data.task.id) || return 1
  eventually task_done "$task"
}

file_task_fails() {
  local task
  task=$(call "$@" | field data.task.id) || return 1
  eventually task_failed "$task"
}

task_failed() {
  [ "$(call task.get "id=$1" 2>/dev/null | field data.state 2>/dev/null)" = "failed" ]
}

owned_by_user() {
  [ -e "$1" ] && [ "$(stat -c %U "$1")" = "$USER_NAME" ]
}

mode_is() {
  [ "$(stat -c %a "$1")" = "$2" ]
}

listed() {
  call hosting.file.list "user=$1" "path=$2" | python3 -c '
import json, sys
doc = json.load(sys.stdin)
sys.exit(0 if sys.argv[1] in [entry["name"] for entry in doc["data"]["entries"]] else 1)
' "$3"
}

refused_outside() {
  local out
  out=$(as_keel ctl hosting.file.list "user=$1" "path=$2" --out json 2>&1) && return 1
  grep -qF hosting.file.error.outside <<<"$out"
}

downloaded() {
  local id=$1 account=$2 token header body
  call keel.principal.create "login=$FILES_LOGIN" level=user "account=$account" >/dev/null || return 1
  token=$(call auth.token.create --as "$FILES_LOGIN" name=panel-vm | field data.token) || return 1
  [[ $token == kat_* ]] || return 1
  header=$(mktemp)
  body=$(mktemp)
  chmod 0600 "$header"
  printf 'authorization: Bearer %s\n' "$token" >"$header"
  if ! curl -sS --fail --cacert "$CERT" -H @"$header" -H 'content-type: application/json' \
    -d "{\"user\":\"$id\",\"path\":\"docs/main.html\"}" "$(api_base)/api/v1/x/hosting/file.download" >"$body"; then
    rm -f "$header" "$body"
    return 1
  fi
  rm -f "$header"
  [ "$(cat "$body")" = "$FILES_CONTENT" ] || { say "  the download answered: $(head -c 200 "$body")" >&2; rm -f "$body"; return 1; }
  rm -f "$body"
}

files_proof() {
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  [ -d "$TRANSFER" ] || fail "$TRANSFER is missing; run install first"
  local since account id task content
  since=$(date -u '+%Y-%m-%d %H:%M:%S')

  say "--- a hosting user with a home"
  account=$(call keel.account.create kind=user | field data.id) || fail "keel ctl keel.account.create failed"
  id=$(call hosting.user.create "name=$NAME" "account=$account" | field data.id) || fail "keel ctl hosting.user.create failed"
  check "the hosting user $NAME turns active" eventually state_is "$id" active

  say "--- files written as $USER_NAME"
  check "hosting.file.mkdir makes docs" file_task hosting.file.mkdir "user=$id" path=docs
  check "docs belongs to $USER_NAME" owned_by_user "$HOME_DIR/docs"
  content=$(printf '%s' "$FILES_CONTENT" | base64 -w0)
  check "hosting.file.upload writes docs/index.html" file_task hosting.file.upload "user=$id" path=docs/index.html "content=$content"
  check "the upload belongs to $USER_NAME with mode 644" eval 'owned_by_user "$HOME_DIR/docs/index.html" && mode_is "$HOME_DIR/docs/index.html" 644'
  check "the upload carries the content" test "$(cat "$HOME_DIR/docs/index.html")" = "$FILES_CONTENT"
  check "no staged file is left in $TRANSFER" test -z "$(ls -A "$TRANSFER")"
  check "hosting.file.rename moves it to docs/main.html" file_task hosting.file.rename "user=$id" from=docs/index.html to=docs/main.html
  check "hosting.file.chmod sets 600" file_task hosting.file.chmod "user=$id" path=docs/main.html mode=600
  check "docs/main.html has mode 600 and still belongs to $USER_NAME" eval 'mode_is "$HOME_DIR/docs/main.html" 600 && owned_by_user "$HOME_DIR/docs/main.html"'
  check "hosting.file.list shows main.html in docs" listed "$id" docs main.html
  check "hosting.file.download answers the content over HTTPS to a kat_ token" downloaded "$id" "$account"
  check "no staged download is left in $TRANSFER" test -z "$(ls -A "$TRANSFER")"

  say "--- the home is the boundary"
  runuser -u "$USER_NAME" -- ln -s /etc "$HOME_DIR/escape"
  check "a link towards /etc is refused as leaving the home" refused_outside "$id" escape
  check "a parent path is refused as leaving the home" refused_outside "$id" ..
  install -m 0600 -o root -g root /dev/null "$HOME_DIR/docs/root-only"
  check "a mode change of a file root owns fails, the worker being $USER_NAME" file_task_fails hosting.file.chmod "user=$id" path=docs/root-only mode=644
  check "the file of root keeps mode 600" mode_is "$HOME_DIR/docs/root-only" 600
  check "hosting.file.delete removes docs" file_task hosting.file.delete "user=$id" path=docs
  check "docs is gone" test ! -e "$HOME_DIR/docs"
  check "the agent journal carries host.home.write" agent_logged "$since" "host.home.write"

  say "--- the account deleted"
  task=$(call keel.account.delete "id=$account" | field data.task.id) || task=""
  check "the deletion task is done" eventually task_done "$task"
  check "$USER_NAME and its home are gone" eventually gone

  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "panel's file manager is proven on this machine: every operation ran as the hosting user inside its home, uploads and downloads passed through $TRANSFER without leaving anything behind, and paths leaving the home were refused"
}

config_refused() {
  local out
  out=$(as_keel ctl hosting.site.config.set "id=$1" "server=keel_vm_refused on;" --out json 2>&1) && return 1
  grep -qF host.error.validate_failed <<<"$out" && grep -qF '"server"' <<<"$out" \
    && grep -qF 'unknown directive "keel_vm_refused"' <<<"$out"
}

sends_header() {
  curl -sS -o /dev/null -D - -H "Host: $DOMAIN" http://127.0.0.1/ 2>/dev/null \
    | grep -qiF "x-keel-panel: config-$STAMP"
}

answers_installed() {
  [ "$(call hosting.site.config.get "id=$1" | field data.rendered.server)" = "$(cat "$VHOST")" ]
}

config_proof() {
  need_root
  [ -x "$KEEL" ] || fail "$KEEL is not the installed binary; run install first"
  systemctl is-active --quiet "$UNIT" || fail "$UNIT is not running"
  local account id php pool site vhost_sum code task

  say "--- a site"
  php=$(php_version)
  [ -n "$php" ] || fail "no php-fpm under /etc/php; run install first"
  pool=/etc/php/$php/fpm/pool.d/keel-panel-$DOMAIN.conf
  account=$(call keel.account.create kind=user | field data.id) || fail "keel ctl keel.account.create failed"
  id=$(call hosting.user.create "name=$NAME" "account=$account" | field data.id) || fail "keel ctl hosting.user.create failed"
  check "the hosting user $NAME turns active" eventually state_is "$id" active
  site=$(call hosting.site.create "user=$id" "domain=$DOMAIN" "php=\"$php\"" | field data.id) || fail "keel ctl hosting.site.create failed"
  check "the site $DOMAIN turns active" eventually site_state_is "$site" active
  vhost_sum=$(sha256sum "$VHOST" | cut -d' ' -f1)
  code=$(curl -s -o /dev/null -w '%{http_code}' -H "Host: $DOMAIN" http://127.0.0.1/)

  say "--- an override nginx -t refuses"
  check "the override is refused with the output of nginx -t on the field server" config_refused "$site"
  check "the server block of $DOMAIN is unchanged" test "$(sha256sum "$VHOST" | cut -d' ' -f1)" = "$vhost_sum"
  check "nginx -t accepts the configuration in place" nginx -t -q
  check "nginx answers $DOMAIN with $code as before" fetched "$code" "$DOMAIN"
  check "the site $DOMAIN stays active" site_state_is "$site" active

  say "--- an override nginx -t accepts"
  check "keel ctl hosting.site.config.set saves the header and the pool line" call hosting.site.config.set "id=$site" "server=$CONFIG_HEADER" "pool=$CONFIG_POOL"
  check "the server block carries the header" grep -qF "    $CONFIG_HEADER" "$VHOST"
  check "the pool carries the line" grep -qxF "$CONFIG_POOL" "$pool"
  check "nginx -t accepts the server block" nginx -t -q
  check "php-fpm$php -t accepts the pool" "php-fpm$php" -t
  check "nginx sends the header for $DOMAIN" eventually sends_header
  check "hosting.site.config.get answers the installed server block" answers_installed "$site"

  say "--- the account deleted"
  task=$(call keel.account.delete "id=$account" | field data.task.id) || task=""
  check "the deletion task is done" eventually task_done "$task"
  check "the server block of $DOMAIN is gone" eventually test ! -e "$VHOST"

  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "panel's configuration editor is proven on this machine: an override nginx -t refuses never reached the running nginx, and an accepted one was rendered into the server block and the pool and served"
}

case ${1:-} in
  -h | --help) usage ;;
  install) shift; install_keel "$@" ;;
  run) shift; run_proof "$@" ;;
  ssl) shift; ssl_proof "$@" ;;
  db) shift; db_proof "$@" ;;
  mail) shift; mail_proof "$@" ;;
  ftp) shift; ftp_proof "$@" ;;
  files) shift; files_proof "$@" ;;
  backup) shift; backup_proof "$@" ;;
  config) shift; config_proof "$@" ;;
  *) usage >&2; exit 2 ;;
esac
