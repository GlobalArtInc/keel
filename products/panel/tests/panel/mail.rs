use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use keel::prelude::*;
use keel::types::ErrorCode;
use keel_testkit::{Builder, Harness, Reply, Who, tokio};

use crate::bridge::with_dns;
use crate::support::{Site, composed, data, eventually, rows};

const DOMAIN_CREATE: &str = "hosting.mail.domain.create";
const MAILBOX_CREATE: &str = "hosting.mail.mailbox.create";
const MAILBOX_EDIT: &str = "hosting.mail.mailbox.edit";
const MAILBOX_DELETE: &str = "hosting.mail.mailbox.delete";
const PASSWORD: &str = "correct horse battery";
const SPF: &str = "\"v=spf1 mx -all\"";
const BROKEN: &str = "postfix: [emerg] the configuration of the test is broken";

pub(crate) fn mail(site: &Site, builder: Builder) -> Builder {
    let owner = fs::metadata(site.path("passwd")).expect("the passwd of the site");
    let (uid, gid) = (owner.uid(), owner.gid());
    let passwd = fs::read_to_string(site.path("passwd")).expect("passwd");
    fs::write(
        site.path("passwd"),
        format!("{passwd}dovecot:x:{uid}:{gid}::/nonexistent:/usr/sbin/nologin\n"),
    )
    .expect("the dovecot user");
    let group = fs::read_to_string(site.path("group")).expect("group");
    fs::write(site.path("group"), format!("{group}dovecot:x:{gid}:\n")).expect("the dovecot group");
    builder.section(
        "hosting_mail",
        json!({
            "maildirs_dir": site.path("maildirs").display().to_string(),
            "dumps_dir": site.path("dumps").display().to_string(),
        }),
    )
}

fn map(site: &Site, name: &str) -> String {
    fs::read_to_string(site.path("mail").join(format!("keel-panel-{name}"))).unwrap_or_default()
}

fn passwd_file(site: &Site) -> String {
    fs::read_to_string(site.path("dovecot").join("keel-panel-passwd")).unwrap_or_default()
}

pub(crate) async fn domain_of_a1(harness: &Harness, name: &str) -> String {
    let a1 = harness.fixtures().accounts.a1;
    created(
        harness,
        Who::Admin,
        DOMAIN_CREATE,
        json!({ "name": name, "account": a1 }),
    )
    .await
}

async fn created(harness: &Harness, who: Who, function: &str, params: Value) -> String {
    let reply = harness.call(who, function, params).await;
    data(&reply)["id"].as_str().expect("an id").to_owned()
}

async fn state_is(harness: &Harness, table: &str, id: &str, state: &str) {
    let sql = format!("SELECT state FROM {table} WHERE id = '{id}'");
    eventually(&format!("{table} {id} in state {state}"), async || {
        rows(harness, &sql).await == [state]
    })
    .await;
}

async fn mailbox(harness: &Harness, domain: &str, local: &str) -> String {
    created(
        harness,
        Who::A1,
        MAILBOX_CREATE,
        json!({ "domain": domain, "local": local, "password": PASSWORD }),
    )
    .await
}

async fn account(harness: &Harness, verb: &str) {
    let a1 = harness.fixtures().accounts.a1;
    let reply = harness
        .call(
            Who::Reseller,
            &format!("keel.account.{verb}"),
            json!({ "id": a1 }),
        )
        .await;
    data(&reply);
}

#[tokio::test]
async fn a_mailbox_lands_in_the_virtual_maps_and_the_passwd_file() {
    let site = Site::new();
    let harness = mail(&site, composed(&site)).boot().await;
    assert!(harness.modules().contains(&"dns_peer".to_owned()));
    let domain = domain_of_a1(&harness, "Example.ORG").await;
    state_is(&harness, "hosting_mail_domain", &domain, "active").await;
    assert_eq!(map(&site, "domains"), "example.org OK\n");

    let reply = harness
        .call(
            Who::A1,
            MAILBOX_CREATE,
            json!({
                "domain": domain,
                "local": "Box",
                "password": PASSWORD,
                "quota": 512,
                "aliases": ["info"],
                "forwards": ["me@example.net"],
            }),
        )
        .await;
    let answer = data(&reply);
    assert_eq!(answer["address"], "box@example.org");
    assert!(answer.get("passwordHash").is_none(), "{answer}");
    let id = answer["id"].as_str().expect("an id").to_owned();
    state_is(&harness, "hosting_mailbox", &id, "active").await;

    assert_eq!(
        map(&site, "mailboxes"),
        format!("box@example.org keel-panel-{id}/Maildir/\n")
    );
    assert_eq!(
        map(&site, "aliases"),
        "box@example.org box@example.org, me@example.net\ninfo@example.org box@example.org\n"
    );
    let home = site.path("maildirs").join(format!("keel-panel-{id}"));
    let line = passwd_file(&site);
    let (user, rest) = line.split_once(':').expect("a passwd line");
    assert_eq!(user, "box@example.org");
    assert!(
        rest.starts_with("{ARGON2ID}$argon2id$v=19$m=19456,t=2,p=1$"),
        "{line}"
    );
    assert!(!rest.contains(PASSWORD), "{line}");
    assert!(
        line.ends_with(&format!(
            "::::{}::userdb_quota_storage_size=512M\n",
            home.display()
        )),
        "{line}"
    );
    let meta = fs::metadata(site.path("dovecot").join("keel-panel-passwd")).expect("the passwd");
    assert_eq!(meta.permissions().mode() & 0o777, 0o640);
    let mail = site.path("mail");
    let calls = site.calls();
    for expected in [
        "postfix check".to_owned(),
        format!(
            "postmap hash:{0}/keel-panel-domains hash:{0}/keel-panel-mailboxes hash:{0}/keel-panel-aliases",
            mail.display()
        ),
        "systemctl reload postfix".to_owned(),
        "doveconf -n".to_owned(),
        "systemctl reload dovecot".to_owned(),
    ] {
        assert!(calls.contains(&expected), "{expected} in {calls:?}");
    }

    let taken = harness
        .call(
            Who::A1,
            MAILBOX_CREATE,
            json!({ "domain": domain, "local": "info", "password": PASSWORD }),
        )
        .await;
    let error = taken.error().expect("an alias is taken as a mailbox name");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(error.fields[0].path, "local");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("hosting.mail.mailbox.error.exists")
    );

    account(&harness, "suspend").await;
    eventually("the suspended mailbox refuses logins", async || {
        passwd_file(&site).ends_with("userdb_quota_storage_size=512M nologin=y\n")
    })
    .await;
    assert_eq!(map(&site, "mailboxes").lines().count(), 1);
    account(&harness, "resume").await;
    eventually("the resumed mailbox takes logins again", async || {
        passwd_file(&site).ends_with("userdb_quota_storage_size=512M\n")
    })
    .await;

    fs::create_dir_all(home.join("Maildir/cur")).expect("a maildir");
    let backup = tempfile::tempdir().expect("a backup directory");
    let source = harness
        .registry()
        .contributions()
        .of::<BackupSource>()
        .find(|source| source.name() == "hosting_mail.maildirs")
        .expect("the maildirs are a backup source")
        .write_hook();
    source(BackupDir::new(
        "hosting_mail.maildirs",
        backup.path().to_owned(),
        Db::from_connection(harness.db().clone()),
        harness.data_dir(),
        harness.registry().services().clone(),
    ))
    .await
    .expect("the maildirs are archived");
    assert_eq!(
        fs::read_to_string(backup.path().join("maildirs.tar")).expect("the archive"),
        "the maildirs of the test\n"
    );
    assert!(site.calls().contains(&format!(
        "tar --create --file=- --directory={} .",
        site.path("maildirs").display()
    )));
    assert_eq!(
        fs::read_dir(site.path("dumps")).expect("the dumps").count(),
        0
    );

    let reply = harness
        .call(Who::A1, MAILBOX_DELETE, json!({ "ids": [id] }))
        .await;
    assert_eq!(data(&reply)["deleted"], 1);
    eventually("the maildir is removed", async || !home.exists()).await;
    assert_eq!(map(&site, "mailboxes"), "");
    assert_eq!(map(&site, "aliases"), "");
    assert_eq!(passwd_file(&site), "");
    assert_eq!(map(&site, "domains"), "example.org OK\n");
}

#[tokio::test]
async fn a_domain_with_a_zone_gets_mx_and_spf_through_the_bridge() {
    let site = Site::new();
    let harness = mail(&site, with_dns(&site)).boot().await;
    assert!(harness.modules().contains(&"dns_local".to_owned()));
    let zone = created(
        &harness,
        Who::A1,
        "dns.zone.create",
        json!({ "name": "zoned.example" }),
    )
    .await;
    let domain = created(
        &harness,
        Who::A1,
        DOMAIN_CREATE,
        json!({ "name": "zoned.example" }),
    )
    .await;
    let records = format!(
        "SELECT type || ' ' || name || ' ' || value || ' ' || coalesce(priority::text, '-') FROM dns_record WHERE zone_id = '{zone}' ORDER BY type"
    );
    assert_eq!(
        rows(&harness, &records).await,
        ["MX @ zoned.example 10".to_owned(), format!("TXT @ {SPF} -")]
    );
    state_is(&harness, "hosting_mail_domain", &domain, "active").await;

    let bare = domain_of_a1(&harness, "bare.example").await;
    state_is(&harness, "hosting_mail_domain", &bare, "active").await;
    assert_eq!(
        rows(&harness, "SELECT count(*)::text FROM dns_record").await,
        ["2"]
    );
    assert_eq!(map(&site, "domains"), "bare.example OK\nzoned.example OK\n");

    let taken = harness
        .call(Who::A2, DOMAIN_CREATE, json!({ "name": "zoned.example" }))
        .await;
    let error = taken.error().expect("a mail domain exists once");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("hosting.mail.domain.error.exists")
    );
}

#[tokio::test]
async fn a_failing_postfix_check_rolls_back() {
    let site = Site::new();
    let harness = mail(&site, composed(&site)).boot().await;
    let domain = domain_of_a1(&harness, "example.org").await;
    let first = mailbox(&harness, &domain, "first").await;
    state_is(&harness, "hosting_mailbox", &first, "active").await;
    let mailboxes = map(&site, "mailboxes");
    let passwd = passwd_file(&site);
    let postmaps = |site: &Site| {
        site.calls()
            .iter()
            .filter(|call| call.starts_with("postmap "))
            .count()
    };
    let before = postmaps(&site);

    site.break_check("postfix", true);
    let second = mailbox(&harness, &domain, "second").await;
    state_is(&harness, "hosting_mailbox", &second, "error").await;
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT error FROM hosting_mailbox WHERE id = '{second}'")
        )
        .await,
        [BROKEN]
    );
    assert_eq!(map(&site, "mailboxes"), mailboxes);
    assert_eq!(passwd_file(&site), passwd);
    assert_eq!(postmaps(&site), before);
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT state FROM hosting_mailbox WHERE id = '{first}'")
        )
        .await,
        ["active"]
    );

    site.break_check("postfix", false);
    let reply = harness
        .call(
            Who::A1,
            MAILBOX_EDIT,
            json!({ "id": second, "quota": 0, "aliases": [], "forwards": [] }),
        )
        .await;
    data(&reply);
    state_is(&harness, "hosting_mailbox", &second, "active").await;
    assert!(map(&site, "mailboxes").contains("second@example.org "));
    assert!(passwd_file(&site).contains("second@example.org:{ARGON2ID}"));
    assert_eq!(postmaps(&site), before + 1);

    site.break_check("postmap", true);
    let third = mailbox(&harness, &domain, "third").await;
    state_is(&harness, "hosting_mailbox", &third, "error").await;
    let placed = map(&site, "mailboxes");
    assert!(placed.contains("third@example.org "));
    let failed = postmaps(&site);
    assert_eq!(failed, before + 2);

    site.break_check("postmap", false);
    let reply = harness
        .call(
            Who::A1,
            MAILBOX_EDIT,
            json!({ "id": second, "password": "another horse battery", "quota": 0, "aliases": [], "forwards": [] }),
        )
        .await;
    data(&reply);
    state_is(&harness, "hosting_mailbox", &third, "active").await;
    assert_eq!(map(&site, "mailboxes"), placed);
    assert_eq!(postmaps(&site), failed + 1);
}

fn unproven(reply: &Reply) -> bool {
    reply
        .error()
        .and_then(|error| error.fields.first())
        .and_then(|field| field.key.as_deref())
        == Some("hosting.mail.domain.error.unproven")
}

#[tokio::test]
async fn a_customer_cannot_add_a_mail_domain_outside_its_own_zones() {
    let site = Site::new();
    let harness = mail(&site, with_dns(&site)).boot().await;
    created(
        &harness,
        Who::A1,
        "dns.zone.create",
        json!({ "name": "own.example" }),
    )
    .await;
    created(
        &harness,
        Who::Reseller,
        "dns.zone.create",
        json!({ "name": "reseller.example" }),
    )
    .await;

    let loose = harness
        .call(Who::A1, DOMAIN_CREATE, json!({ "name": "loose.example" }))
        .await;
    assert!(unproven(&loose), "{:?}", loose.error());
    let near = harness
        .call(Who::A1, DOMAIN_CREATE, json!({ "name": "disown.example" }))
        .await;
    assert!(unproven(&near), "{:?}", near.error());
    assert!(
        rows(&harness, "SELECT name FROM hosting_mail_domain")
            .await
            .is_empty()
    );

    created(
        &harness,
        Who::A1,
        DOMAIN_CREATE,
        json!({ "name": "mail.own.example" }),
    )
    .await;
    created(
        &harness,
        Who::A1,
        DOMAIN_CREATE,
        json!({ "name": "own.example" }),
    )
    .await;
    created(
        &harness,
        Who::A1,
        DOMAIN_CREATE,
        json!({ "name": "mail.reseller.example" }),
    )
    .await;
    created(
        &harness,
        Who::Admin,
        "dns.zone.create",
        json!({ "name": "root.example" }),
    )
    .await;
    let rooted = harness
        .call(
            Who::A1,
            DOMAIN_CREATE,
            json!({ "name": "www.root.example" }),
        )
        .await;
    assert!(unproven(&rooted), "{:?}", rooted.error());
    harness.finish().await;

    let peer_site = Site::new();
    let peered = mail(&peer_site, composed(&peer_site)).boot().await;
    let refused = peered
        .call(Who::A1, DOMAIN_CREATE, json!({ "name": "own.example" }))
        .await;
    assert!(unproven(&refused), "{:?}", refused.error());
}

#[tokio::test]
async fn an_administrator_may_add_a_mail_domain_without_a_zone() {
    let site = Site::new();
    let harness = mail(&site, with_dns(&site)).boot().await;
    let a1 = harness.fixtures().accounts.a1;
    let domain = created(
        &harness,
        Who::Admin,
        DOMAIN_CREATE,
        json!({ "name": "loose.example", "account": a1 }),
    )
    .await;
    state_is(&harness, "hosting_mail_domain", &domain, "active").await;
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT account_id FROM hosting_mail_domain WHERE id = '{domain}'")
        )
        .await,
        [a1.to_string()]
    );
}

#[tokio::test]
async fn a_customer_cannot_take_the_apex_of_an_ancestor_zone_as_a_mail_domain() {
    let site = Site::new();
    let harness = mail(&site, with_dns(&site)).boot().await;
    created(
        &harness,
        Who::Reseller,
        "dns.zone.create",
        json!({ "name": "reseller.example" }),
    )
    .await;

    let apex = harness
        .call(
            Who::A1,
            DOMAIN_CREATE,
            json!({ "name": "reseller.example" }),
        )
        .await;
    assert!(unproven(&apex), "{:?}", apex.error());
    created(
        &harness,
        Who::A1,
        DOMAIN_CREATE,
        json!({ "name": "shop.reseller.example" }),
    )
    .await;
}

#[tokio::test]
async fn a_reseller_adds_a_mail_domain_for_its_customer_inside_the_customers_zone() {
    let site = Site::new();
    let harness = mail(&site, with_dns(&site)).boot().await;
    let a1 = harness.fixtures().accounts.a1;
    created(
        &harness,
        Who::A1,
        "dns.zone.create",
        json!({ "name": "own.example" }),
    )
    .await;

    let domain = created(
        &harness,
        Who::Reseller,
        DOMAIN_CREATE,
        json!({ "name": "mail.own.example", "account": a1 }),
    )
    .await;
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT account_id FROM hosting_mail_domain WHERE id = '{domain}'")
        )
        .await,
        [a1.to_string()]
    );
}
