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
DATABASE=${USER_NAME}_wp
DATABASE_USER=${USER_NAME}_app
SIBLING=${USER_NAME}xwp
DATABASE_PASSWORD="it's $STAMP \\ \"quoted\""
STAGING=https://acme-staging-v02.api.letsencrypt.org/directory
ACME_DIRECTORY=${KEEL_PANEL_VM_ACME:-$STAGING}

usage() {
  cat <<EOF
usage: hack/panel-vm.sh install <keel binary or tar.zst>
       hack/panel-vm.sh run
       hack/panel-vm.sh ssl <domain>
       hack/panel-vm.sh db

Proves the hosting users and the sites of the panel bundle on a virtual
machine, run as root from a checkout of the repository.

"install" installs nginx, php-fpm, mariadb, curl, openssl and zstd,
prepares $WWW, the certificate root $CERTS, the challenge root $CHALLENGES
and the dump root $DUMPS, then runs
deploy/install.sh --product panel --binary <archive> --yes and prints the
setup link.

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
  DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends nginx php-fpm mariadb-server mariadb-client curl openssl zstd
  install -d -m 0755 -o root -g root "$WWW" "$CHALLENGES"
  install -d -m 0711 -o root -g root "$DUMPS"
  install -d -m 0700 -o root -g root "$CERTS"
  local log
  log=$(mktemp)
  if ! bash deploy/install.sh --product panel --binary "$archive" --yes 2>&1 | tee "$log"; then
    fail "deploy/install.sh failed; its output is in $log"
  fi
  check "$UNIT is active" systemctl is-active --quiet "$UNIT"
  check "$AGENT_UNIT is active" systemctl is-active --quiet "$AGENT_UNIT"
  check "the installer printed the setup link" grep -qF "setup link:   https://" "$log"
  [ "$failures" -eq 0 ] || fail "$failures check(s) failed"
  say "keel is installed with panel; run: hack/panel-vm.sh run"
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

case ${1:-} in
  -h | --help) usage ;;
  install) shift; install_keel "$@" ;;
  run) shift; run_proof "$@" ;;
  ssl) shift; ssl_proof "$@" ;;
  db) shift; db_proof "$@" ;;
  *) usage >&2; exit 2 ;;
esac
