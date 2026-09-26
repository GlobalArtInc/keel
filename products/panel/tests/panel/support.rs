use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use keel::prelude::*;
use keel_testkit::{Builder, Harness, Reply, Who, tokio};

pub(crate) const PATIENCE: Duration = Duration::from_secs(20);
pub(crate) const FIRST_UID: u32 = 100_000;
pub(crate) const LAST_UID: u32 = 100_009;
pub(crate) const PHP: &str = "8.4";
pub(crate) const OLDER_PHP: &str = "8.3";

const GETENT: &str = "db=$1; shift\n[ \"${1:-}\" = -- ] && shift\ncase $db in passwd) file=ROOT/passwd;; group) file=ROOT/group;; *) exit 1;; esac\nif [ $# -eq 0 ]; then cat \"$file\"; exit 0; fi\nline=$(awk -F: -v k=\"$1\" '$1 == k || $3 == k' \"$file\")\n[ -n \"$line\" ] || exit 2\nprintf '%s\\n' \"$line\"\n";
const USERADD: &str = "echo \"useradd $*\" >> LOG\nuid=; gid=; shell=; home=\nwhile [ $# -gt 1 ]; do\n  case $1 in\n    --uid) uid=$2; shift 2;;\n    --gid) gid=$2; shift 2;;\n    --shell) shell=$2; shift 2;;\n    --home-dir) home=$2; shift 2;;\n    --create-home) mkdir -p \"$home\"; shift;;\n    --no-create-home) shift;;\n    *) echo \"useradd: unexpected $1\" >&2; exit 3;;\n  esac\ndone\nprintf '%s:x:%s:%s::%s:%s\\n' \"$1\" \"$uid\" \"$gid\" \"$home\" \"$shell\" >> ROOT/passwd\n";
const USERDEL: &str = "echo \"userdel $*\" >> LOG\nfor last; do :; done\nhome=$(awk -F: -v k=\"$last\" '$1 == k {print $6}' ROOT/passwd)\nif [ \"$1\" = --remove ] && [ -n \"$home\" ]; then rm -rf \"$home\"; fi\nawk -F: -v k=\"$last\" '$1 != k' ROOT/passwd > ROOT/passwd.new && mv ROOT/passwd.new ROOT/passwd\n";
const GROUPADD: &str =
    "echo \"groupadd $*\" >> LOG\nprintf '%s:x:%s:\\n' \"$3\" \"$2\" >> ROOT/group\n";
const GROUPDEL: &str = "echo \"groupdel $*\" >> LOG\nawk -F: -v k=\"$1\" '$1 != k' ROOT/group > ROOT/group.new && mv ROOT/group.new ROOT/group\n";
const CHECK: &str = "echo \"NAME $*\" >> LOG\nif [ -f FLAG ]; then echo \"NAME: [emerg] the configuration of the test is broken\" >&2; exit 1; fi\nexit 0\n";
const SYSTEMCTL: &str = "echo \"systemctl $*\" >> LOG\nexit 0\n";
const MARIADB: &str = "input=$(cat)\n{ echo \"mariadb $*\"; printf '%s\\n' \"$input\"; } >> LOG\n";
const MARIADB_DUMP: &str = "echo \"mariadb-dump $*\" >> LOG\nfor last; do :; done\nprintf -- '-- dump of %s\\n' \"$last\"\n";
const TAR: &str = "echo \"tar $*\" >> LOG\nprintf 'the maildirs of the test\\n'\n";
const KEEL: &str = r#"echo "keel $*" >> LOG
[ "$1" = agent-exec ] && [ "$2" = --uid ] || exit 126
shift 3
[ "$1" = --policy ] && shift 2
shift
[ "$1" = agent-home ] && [ "$2" = --home ] || exit 2
shift 3
op=$1
shift
case $op in
  list)
    printf '{"entries":['
    sep=
    for entry in "$1"/*; do
      [ -e "$entry" ] || continue
      if [ -d "$entry" ]; then kind=dir; size=0; else kind=file; size=$(wc -c < "$entry" | tr -d ' '); fi
      printf '%s{"name":"%s","kind":"%s","size":%s,"mode":"0644","modified":0}' "$sep" "${entry##*/}" "$kind" "$size"
      sep=,
    done
    printf '],"truncated":false}'
    ;;
  mkdir) mkdir "$1" ;;
  rename) if [ -e "$2" ]; then echo "$2 already exists" >&2; exit 4; fi; mv "$1" "$2" ;;
  remove) rm -rf "$1" ;;
  chmod) chmod "$2" "$1" ;;
  read) cat "$1" ;;
  write) cat > "$1" ;;
  *) exit 2 ;;
esac
"#;
const USERMOD: &str = "echo \"usermod $*\" >> LOG\nshell=; lock=\nwhile [ $# -gt 1 ]; do\n  case $1 in\n    --shell) shell=$2; shift 2;;\n    --expiredate) shift 2;;\n    --lock) lock=yes; shift;;\n    --unlock) lock=no; shift;;\n    *) echo \"usermod: unexpected $1\" >&2; exit 3;;\n  esac\ndone\nif [ \"$lock\" = yes ]; then : > ROOT/locked-$1; fi\nif [ \"$lock\" = no ]; then rm -f ROOT/locked-$1; fi\nif [ -n \"$shell\" ]; then awk -F: -v OFS=: -v k=\"$1\" -v s=\"$shell\" '$1 == k { $7 = s } { print }' ROOT/passwd > ROOT/passwd.new && mv ROOT/passwd.new ROOT/passwd; fi\nexit 0\n";

pub(crate) struct Site {
    dir: tempfile::TempDir,
}

impl Site {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        for sub in [
            "www",
            "bin",
            "vhosts",
            "zones",
            "named",
            "certs",
            "acme",
            "dumps",
            "mail",
            "dovecot",
            "maildirs",
            "ftp",
            "ftp-users",
            "transfer",
        ] {
            fs::create_dir(dir.path().join(sub)).expect("a directory of the site");
        }
        for version in [PHP, OLDER_PHP] {
            fs::create_dir(dir.path().join(format!("pools-{version}")))
                .expect("a pool directory of the site");
        }
        let site = Self { dir };
        fs::write(site.path("passwd"), "root:x:0:0:root:/root:/bin/sh\n").expect("passwd");
        fs::write(site.path("group"), "root:x:0:\n").expect("group");
        fs::write(site.path("shells"), "/bin/sh\n/bin/bash\n").expect("shells");
        for (name, body) in [
            ("getent", GETENT),
            ("useradd", USERADD),
            ("userdel", USERDEL),
            ("groupadd", GROUPADD),
            ("groupdel", GROUPDEL),
            ("usermod", USERMOD),
            ("systemctl", SYSTEMCTL),
            ("mariadb", MARIADB),
            ("mariadb-dump", MARIADB_DUMP),
            ("tar", TAR),
            ("keel", KEEL),
        ] {
            site.stub(name, body);
        }
        for tool in [
            "named-checkconf".to_owned(),
            "nginx".to_owned(),
            format!("php-fpm{PHP}"),
            format!("php-fpm{OLDER_PHP}"),
            "postfix".to_owned(),
            "doveconf".to_owned(),
            "postmap".to_owned(),
            "vsftpd".to_owned(),
        ] {
            let body = CHECK
                .replace("FLAG", &site.flag(&tool).display().to_string())
                .replace("NAME", &tool);
            site.stub(&tool, &body);
        }
        site
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn stub(&self, name: &str, body: &str) {
        let body = body
            .replace("ROOT", &self.dir.path().display().to_string())
            .replace("LOG", &self.path("calls.log").display().to_string());
        write_tool(&self.path("bin").join(name), &format!("#!/bin/sh\n{body}"));
    }

    fn policy(&self) -> String {
        let bin = self.path("bin");
        format!(
            "policy:
  uid_range: {{ from: {FIRST_UID}, to: {LAST_UID} }}
  file_roots: {{ www: {www}, vhosts: {vhosts}, pools-{PHP}: {pools}, pools-{OLDER_PHP}: {older}, zones: {zones}, named: {named}, certs: {certs}, acme: {acme}, dumps: {dumps}, mail: {mail}, dovecot: {dovecot}, maildirs: {maildirs}, ftp: {ftp}, ftp-users: {ftp_users}, transfer: {transfer} }}
  services: [nginx, php*-fpm, named, postfix, dovecot]
  checks: {{ nginx.test: [\"{bin}/nginx\", \"-t\"], php{PHP}-fpm.test: [\"{bin}/php-fpm{PHP}\", \"-t\"], php{OLDER_PHP}-fpm.test: [\"{bin}/php-fpm{OLDER_PHP}\", \"-t\"], named.check: [\"{bin}/named-checkconf\", \"-z\"], postfix.check: [\"{bin}/postfix\", check], dovecot.check: [\"{bin}/doveconf\", -n], vsftpd.check: [\"{bin}/vsftpd\", /etc/vsftpd.conf, -v] }}
  exec_allow: [{{ mariadb: [\"{bin}/mariadb\", --batch, --skip-column-names] }}, {{ mariadb-dump: [\"{bin}/mariadb-dump\", --single-transaction] }}, {{ postmap: [\"{bin}/postmap\", \"hash:{mail}/keel-panel-domains\", \"hash:{mail}/keel-panel-mailboxes\", \"hash:{mail}/keel-panel-aliases\"] }}, {{ mail.archive: [\"{bin}/tar\", --create, --file=-, \"--directory={maildirs}\", .] }}]
  products:
    panel: {{ user_prefix: hp_, drop_in_prefix: keel-panel-, account_roots: [ftp-users] }}
    dns: {{ drop_in_prefix: keel-dns- }}
  tools:
    getent: {bin}/getent
    useradd: {bin}/useradd
    userdel: {bin}/userdel
    groupadd: {bin}/groupadd
    groupdel: {bin}/groupdel
    usermod: {bin}/usermod
    systemctl: {bin}/systemctl
    shells: {shells}
    keel: {bin}/keel
",
            www = self.path("www").display(),
            vhosts = self.path("vhosts").display(),
            pools = self.path(&format!("pools-{PHP}")).display(),
            older = self.path(&format!("pools-{OLDER_PHP}")).display(),
            zones = self.path("zones").display(),
            named = self.path("named").display(),
            certs = self.path("certs").display(),
            acme = self.path("acme").display(),
            dumps = self.path("dumps").display(),
            mail = self.path("mail").display(),
            dovecot = self.path("dovecot").display(),
            maildirs = self.path("maildirs").display(),
            ftp = self.path("ftp").display(),
            ftp_users = self.path("ftp-users").display(),
            transfer = self.path("transfer").display(),
            bin = bin.display(),
            shells = self.path("shells").display(),
        )
    }

    pub(crate) fn home(&self, name: &str) -> PathBuf {
        fs::canonicalize(self.path("www"))
            .expect("the www root")
            .join(format!("keel-panel-{name}"))
    }

    pub(crate) fn passwd_line(&self, user: &str) -> Option<String> {
        fs::read_to_string(self.path("passwd"))
            .unwrap_or_default()
            .lines()
            .find(|line| line.split(':').next() == Some(user))
            .map(str::to_owned)
    }

    fn flag(&self, tool: &str) -> PathBuf {
        self.path(&format!("{tool}.broken"))
    }

    pub(crate) fn break_check(&self, tool: &str, broken: bool) {
        if broken {
            fs::write(self.flag(tool), "").expect("the flag of a broken check");
        } else {
            fs::remove_file(self.flag(tool)).expect("the flag of a broken check is removed");
        }
    }

    pub(crate) fn vhost(&self, domain: &str) -> PathBuf {
        self.path("vhosts")
            .join(format!("keel-panel-{domain}.conf"))
    }

    pub(crate) fn pool(&self, domain: &str) -> PathBuf {
        self.pool_of(PHP, domain)
    }

    pub(crate) fn pool_of(&self, version: &str, domain: &str) -> PathBuf {
        self.path(&format!("pools-{version}"))
            .join(format!("keel-panel-{domain}.conf"))
    }

    pub(crate) fn locked(&self, user: &str) -> bool {
        self.path(&format!("locked-{user}")).exists()
    }

    pub(crate) fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.path("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn write_tool(path: &Path, body: &str) {
    let mut tee = Command::new("tee")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("tee starts");
    tee.stdin
        .take()
        .expect("the stdin of tee")
        .write_all(body.as_bytes())
        .expect("the stub is written");
    assert!(tee.wait().expect("tee ends").success());
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("the stub mode");
}

pub(crate) async fn panel(site: &Site) -> Harness {
    composed(site).boot().await
}

pub(crate) fn composed(site: &Site) -> Builder {
    Harness::new()
        .product(&keel_panel::MANIFEST)
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_system::settings::MODULE)
        .section(
            "hosting",
            json!({ "transfer_dir": site.path("transfer").display().to_string() }),
        )
        .agent(&site.policy())
}

pub(crate) fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

pub(crate) async fn create(harness: &Harness, who: Who, name: &str) -> String {
    let reply = harness
        .call(who, "hosting.user.create", json!({ "name": name }))
        .await;
    data(&reply)["id"].as_str().expect("an id").to_owned()
}

pub(crate) async fn rows(harness: &Harness, sql: &str) -> Vec<String> {
    harness.database().rows(sql).await
}

pub(crate) async fn eventually<F>(what: &str, mut done: F)
where
    F: AsyncFnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !done().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} did not happen within {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) async fn state_becomes(harness: &Harness, id: &str, state: &str) {
    let sql = format!("SELECT state FROM hosting_user WHERE id = '{id}'");
    eventually(
        &format!("the hosting user {id} in state {state}"),
        async || rows(harness, &sql).await == [state],
    )
    .await;
}
