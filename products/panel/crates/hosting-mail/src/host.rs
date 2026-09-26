use std::io;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Utc};
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder};
use keel::sdk::tokio;
use keel::sdk::tokio::sync::Mutex;
use keel_host::api::{Dest, Host, HostExecutor, Origin};

use crate::config::MailConfig;
use crate::{PRODUCT, STATE_ACTIVE, STATE_ERROR, domain, mailbox};

pub(crate) const MAP_TEMPLATE: &str = "panel/postfix-map.j2";
pub(crate) const PASSWD_TEMPLATE: &str = "panel/dovecot-passwd.j2";
pub(crate) const POSTFIX_CHECK: &str = "postfix.check";
pub(crate) const DOVECOT_CHECK: &str = "dovecot.check";

const POSTFIX: &str = "postfix";
const DOVECOT: &str = "dovecot";
const POSTMAP: &str = "postmap";
const ARCHIVE: &str = "mail.archive";
const MAPS_ROOT: &str = "mail";
const PASSWD_ROOT: &str = "dovecot";
const MAILDIRS_ROOT: &str = "maildirs";
const DUMPS_ROOT: &str = "dumps";
const DROP_IN_PREFIX: &str = "keel-panel-";
const DOMAINS_FILE: &str = "keel-panel-domains";
const MAILBOXES_FILE: &str = "keel-panel-mailboxes";
const ALIASES_FILE: &str = "keel-panel-aliases";
const PASSWD_FILE: &str = "keel-panel-passwd";
const ARCHIVE_FILE: &str = "keel-panel-maildirs.tar";
const ARCHIVE_ENTRY: &str = "maildirs.tar";
const MAP_MODE: &str = "0644";
const PASSWD_MODE: &str = "0640";
const PASSWD_OWNER: &str = "dovecot";
const DELIVERED: &str = "OK";
const MAILDIR: &str = "Maildir/";
const DETACHED: &str = "hosting.mail.error.detached";
const BACKUP_FAILED: &str = "hosting.mail.error.backup";

#[derive(Default)]
pub(crate) struct Changed {
    pub(crate) domains: Vec<String>,
    pub(crate) mailboxes: Vec<String>,
    pub(crate) gone: Vec<String>,
}

#[derive(Serialize)]
#[serde(crate = "keel::sdk::serde")]
struct Entry {
    key: String,
    value: String,
}

#[derive(Serialize)]
#[serde(crate = "keel::sdk::serde")]
struct Account {
    user: String,
    password: String,
    home: String,
    extra: String,
}

struct Rendered {
    domains: Vec<String>,
    mailboxes: Vec<String>,
}

struct Attached {
    host: Arc<dyn HostExecutor>,
    clock: Arc<dyn Clock>,
    config: Arc<MailConfig>,
}

#[derive(Default)]
pub(crate) struct Mail {
    turn: Mutex<()>,
    attached: OnceLock<Attached>,
    maps_stale: AtomicBool,
}

impl Mail {
    pub(crate) fn attach(&self, ctx: &Context) -> Result<()> {
        let _ = self.attached.set(Attached {
            host: ctx.require::<dyn HostExecutor>()?,
            clock: ctx.require::<dyn Clock>()?,
            config: ctx.config::<MailConfig>()?,
        });
        self.maps_stale.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn attached(&self) -> Result<&Attached> {
        self.attached
            .get()
            .ok_or_else(|| KeelError::unavailable(DETACHED))
    }

    pub(crate) async fn apply(&self, ctx: &AfterContext, changed: Changed) -> Result<()> {
        let attached = self.attached()?;
        let _turn = self.turn.lock().await;
        let db = ctx.db()?;
        let origin = Origin::new(PRODUCT, ctx.principal(), ctx.request_id());
        let host = attached.host.on(origin);
        let outcome = render(&host, db, &attached.config, &self.maps_stale).await;
        settle(db, attached.clock.now(), &changed, &outcome).await?;
        let mut removed = Ok(());
        for id in &changed.gone {
            if let Err(error) = host
                .fs()
                .remove(&Dest::new(MAILDIRS_ROOT, &maildir_of(id)), true)
                .await
            {
                removed = removed.and(Err(error));
            }
        }
        outcome.map(drop).and(removed)
    }

    pub(crate) async fn archive(&self, dir: &BackupDir) -> Result<()> {
        let attached = self.attached()?;
        if mailbox::Entity::find().count(dir.db()).await? == 0 {
            return Ok(());
        }
        let request = Id::generate().to_string();
        let host = attached.host.on(Origin::installation(PRODUCT, &request));
        let dest = Dest::new(DUMPS_ROOT, ARCHIVE_FILE);
        host.exec_into(ARCHIVE, &dest).await?;
        let source = attached.config.dump_path(ARCHIVE_FILE);
        let copied = copy(Path::new(&source), &dir.path().join(ARCHIVE_ENTRY)).await;
        host.fs().remove(&dest, false).await?;
        copied.map_err(|error| {
            KeelError::internal(BACKUP_FAILED)
                .param("file", source.as_str())
                .source(error)
        })
    }
}

async fn settle(
    db: &Db,
    now: DateTime<Utc>,
    changed: &Changed,
    outcome: &Result<Rendered>,
) -> Result<()> {
    let (state, error, domains, mailboxes) = match outcome {
        Ok(rendered) => (
            STATE_ACTIVE,
            None,
            domain::Column::Id.is_in(rendered.domains.clone()),
            mailbox::Column::Id.is_in(rendered.mailboxes.clone()),
        ),
        Err(error) => (
            STATE_ERROR,
            Some(detail(error)),
            domain::Column::Id.is_in(changed.domains.clone()),
            mailbox::Column::Id.is_in(changed.mailboxes.clone()),
        ),
    };
    domain::Entity::update_many()
        .col_expr(domain::Column::State, Expr::value(state))
        .col_expr(domain::Column::Error, Expr::value(error.clone()))
        .col_expr(domain::Column::UpdatedAt, Expr::value(now))
        .filter(domains)
        .exec(db)
        .await?;
    mailbox::Entity::update_many()
        .col_expr(mailbox::Column::State, Expr::value(state))
        .col_expr(mailbox::Column::Error, Expr::value(error))
        .col_expr(mailbox::Column::UpdatedAt, Expr::value(now))
        .filter(mailboxes)
        .exec(db)
        .await?;
    Ok(())
}

async fn render(
    host: &Host<'_>,
    db: &Db,
    config: &MailConfig,
    maps_stale: &AtomicBool,
) -> Result<Rendered> {
    let domains = domain::Entity::find()
        .order_by_asc(domain::Column::Name)
        .all(db)
        .await?;
    let mailboxes = mailbox::Entity::find()
        .order_by_asc(mailbox::Column::Address)
        .all(db)
        .await?;
    for (file, entries) in [
        (DOMAINS_FILE, domain_entries(&domains)),
        (MAILBOXES_FILE, mailbox_entries(&mailboxes)),
        (ALIASES_FILE, alias_entries(&mailboxes)),
    ] {
        if host
            .template(MAP_TEMPLATE)
            .vars(&json!({ "entries": entries }))
            .dest(MAPS_ROOT, file)
            .mode(MAP_MODE)
            .validate(POSTFIX_CHECK)
            .apply()
            .await?
        {
            maps_stale.store(true, Ordering::SeqCst);
        }
    }
    if maps_stale.load(Ordering::SeqCst) {
        host.exec(POSTMAP).await?;
        host.services().reload(POSTFIX, Some(POSTFIX_CHECK)).await?;
        maps_stale.store(false, Ordering::SeqCst);
    }
    host.template(PASSWD_TEMPLATE)
        .vars(&json!({ "mailboxes": accounts(&mailboxes, config) }))
        .dest(PASSWD_ROOT, PASSWD_FILE)
        .mode(PASSWD_MODE)
        .owner(PASSWD_OWNER)
        .group(PASSWD_OWNER)
        .validate(DOVECOT_CHECK)
        .reload(DOVECOT)
        .apply()
        .await?;
    Ok(Rendered {
        domains: domains.into_iter().map(|row| row.id).collect(),
        mailboxes: mailboxes.into_iter().map(|row| row.id).collect(),
    })
}

fn domain_entries(domains: &[domain::Model]) -> Vec<Entry> {
    domains
        .iter()
        .map(|row| Entry {
            key: row.name.clone(),
            value: DELIVERED.to_owned(),
        })
        .collect()
}

fn mailbox_entries(mailboxes: &[mailbox::Model]) -> Vec<Entry> {
    mailboxes
        .iter()
        .map(|row| Entry {
            key: row.address.clone(),
            value: format!("{}/{MAILDIR}", maildir_of(&row.id)),
        })
        .collect()
}

fn alias_entries(mailboxes: &[mailbox::Model]) -> Vec<Entry> {
    let mut entries = Vec::new();
    for row in mailboxes {
        let domain = row.address.split_once('@').map_or("", |(_, domain)| domain);
        for alias in &row.aliases.0 {
            entries.push(Entry {
                key: format!("{alias}@{domain}"),
                value: row.address.clone(),
            });
        }
        if !row.forwards.0.is_empty() {
            let targets: Vec<&str> = std::iter::once(row.address.as_str())
                .chain(row.forwards.0.iter().map(String::as_str))
                .collect();
            entries.push(Entry {
                key: row.address.clone(),
                value: targets.join(", "),
            });
        }
    }
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    entries
}

fn accounts(mailboxes: &[mailbox::Model], config: &MailConfig) -> Vec<Account> {
    mailboxes
        .iter()
        .map(|row| {
            let mut extra = Vec::new();
            if row.quota > 0 {
                extra.push(format!("userdb_quota_storage_size={}M", row.quota));
            }
            if row.suspended {
                extra.push("nologin=y".to_owned());
            }
            Account {
                user: row.address.clone(),
                password: row.password_hash.clone(),
                home: config.home(&maildir_of(&row.id)),
                extra: extra.join(" "),
            }
        })
        .collect()
}

fn maildir_of(id: &str) -> String {
    format!("{DROP_IN_PREFIX}{id}")
}

async fn copy(source: &Path, target: &Path) -> io::Result<()> {
    let meta = tokio::fs::symlink_metadata(source).await?;
    if !meta.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    tokio::fs::copy(source, target).await.map(drop)
}

fn detail(error: &KeelError) -> String {
    error
        .params()
        .get("detail")
        .and_then(Value::as_str)
        .map_or_else(|| error.key().to_owned(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox::Names;

    fn mailbox(id: &str, address: &str, aliases: &[&str], forwards: &[&str]) -> mailbox::Model {
        let now = Utc::now();
        mailbox::Model {
            id: id.to_owned(),
            account_id: "a".to_owned(),
            domain_id: "d".to_owned(),
            local: address.split('@').next().unwrap_or_default().to_owned(),
            address: address.to_owned(),
            password_hash: "{ARGON2ID}$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA".to_owned(),
            quota: 0,
            aliases: Names(aliases.iter().map(|name| (*name).to_owned()).collect()),
            forwards: Names(forwards.iter().map(|name| (*name).to_owned()).collect()),
            suspended: false,
            state: crate::STATE_PENDING.to_owned(),
            error: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn aliases_point_at_their_mailbox_and_forwards_keep_a_copy() {
        let rows = [
            mailbox("m1", "box@example.org", &["info", "sales"], &[]),
            mailbox(
                "m2",
                "desk@example.net",
                &[],
                &["me@example.com", "you@example.com"],
            ),
        ];
        let entries: Vec<(String, String)> = alias_entries(&rows)
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect();
        assert_eq!(
            entries,
            [
                (
                    "desk@example.net".to_owned(),
                    "desk@example.net, me@example.com, you@example.com".to_owned()
                ),
                ("info@example.org".to_owned(), "box@example.org".to_owned()),
                ("sales@example.org".to_owned(), "box@example.org".to_owned()),
            ]
        );
    }

    #[test]
    fn a_passwd_line_carries_the_home_by_row_id_the_quota_and_the_suspension() {
        let config = MailConfig::default();
        let mut row = mailbox("01J9", "box@example.org", &[], &[]);
        row.quota = 512;
        row.suspended = true;
        let account = &accounts(&[row], &config)[0];
        assert_eq!(account.user, "box@example.org");
        assert!(account.password.starts_with("{ARGON2ID}$argon2id$"));
        assert_eq!(account.home, "/var/lib/keel-panel-mail/keel-panel-01J9");
        assert_eq!(account.extra, "userdb_quota_storage_size=512M nologin=y");
    }
}
