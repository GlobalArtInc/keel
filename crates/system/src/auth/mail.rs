use std::collections::BTreeMap;

use keel_sdk::chrono::{DateTime, Duration, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::system::{Caller, CallerOptions};
use keel_sdk::types::{Scope, Via};
use keel_sdk::{MailTemplate, Secret};

use super::entity::mail_token;
use super::password::{hex, random_bytes};
use super::session;

pub const RESET_PREFIX: &str = "kpr_";
const VERIFY_PREFIX: &str = "kav_";
pub const RESET_TEMPLATE: &str = "auth.password.reset";
pub const VERIFY_TEMPLATE: &str = "auth.address.verify";
const SEND_TO: &str = "notify.send_to";
pub const SETTING_SHELL_URL: &str = "auth.shell_url";
const TOKEN_LEN: usize = 32;
const SENDER_LOGIN: &str = "owner";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Reset,
    Verify,
}

impl Purpose {
    const fn as_str(self) -> &'static str {
        match self {
            Purpose::Reset => "reset",
            Purpose::Verify => "verify",
        }
    }

    const fn prefix(self) -> &'static str {
        match self {
            Purpose::Reset => RESET_PREFIX,
            Purpose::Verify => VERIFY_PREFIX,
        }
    }

    const fn path(self) -> &'static str {
        match self {
            Purpose::Reset => "/reset",
            Purpose::Verify => "/verify",
        }
    }
}

fn hash(token: &str) -> String {
    session::hash(token)
}

pub async fn issue(
    db: &Db,
    purpose: Purpose,
    principal: Id,
    value: Option<&str>,
    ttl: Duration,
    now: DateTime<Utc>,
) -> Result<Secret> {
    let token = Secret::new(format!(
        "{}{}",
        purpose.prefix(),
        hex(&random_bytes(TOKEN_LEN)?)
    ));
    mail_token::ActiveModel {
        token_hash: Set(hash(token.expose())),
        purpose: Set(purpose.as_str().to_owned()),
        principal_id: Set(principal.to_string()),
        value: Set(value.map(str::to_owned)),
        expires_at: Set(now + ttl),
        used_at: Set(None),
        created_at: Set(now),
    }
    .insert(db)
    .await?;
    Ok(token)
}

pub async fn redeem(
    db: &Db,
    purpose: Purpose,
    token: &str,
    now: DateTime<Utc>,
) -> Result<Option<mail_token::Model>> {
    if !token.starts_with(purpose.prefix()) {
        return Ok(None);
    }
    let mut rows = mail_token::Entity::update_many()
        .col_expr(mail_token::Column::UsedAt, Expr::value(now))
        .filter(mail_token::Column::TokenHash.eq(hash(token)))
        .filter(mail_token::Column::Purpose.eq(purpose.as_str()))
        .filter(mail_token::Column::UsedAt.is_null())
        .filter(mail_token::Column::ExpiresAt.gt(now))
        .exec_with_returning(db)
        .await?;
    Ok(rows.pop())
}

pub async fn consume_all(
    db: &Db,
    purpose: Purpose,
    principal: Id,
    now: DateTime<Utc>,
) -> Result<()> {
    mail_token::Entity::update_many()
        .col_expr(mail_token::Column::UsedAt, Expr::value(now))
        .filter(mail_token::Column::PrincipalId.eq(principal.to_string()))
        .filter(mail_token::Column::Purpose.eq(purpose.as_str()))
        .filter(mail_token::Column::UsedAt.is_null())
        .exec(db)
        .await?;
    Ok(())
}

pub fn principal_of(row: &mail_token::Model) -> Result<Id> {
    row.principal_id.parse().map_err(|_| {
        KeelError::internal("auth.error.token_principal")
            .param("principal", row.principal_id.as_str())
    })
}

pub fn shell_base(value: &str) -> Option<&str> {
    let base = value.trim().trim_end_matches('/');
    let rest = base
        .strip_prefix("https://")
        .or_else(|| base.strip_prefix("http://"))?;
    let clean = !rest.is_empty()
        && !rest
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '?' | '#' | '\\'));
    clean.then_some(base)
}

pub fn link(base: &str, purpose: Purpose, token: &Secret) -> String {
    format!("{base}{}?token={}", purpose.path(), token.expose())
}

pub async fn send(
    caller: &dyn Caller,
    principal: Id,
    template: &str,
    params: Value,
    unverified: bool,
) -> Result<()> {
    caller
        .call(
            sender(),
            SEND_TO,
            json!({
                "principal": principal,
                "template": template,
                "params": params,
                "unverified": unverified,
            }),
            CallerOptions::default(),
        )
        .await?;
    Ok(())
}

fn sender() -> Principal {
    Principal {
        id: Id::ROOT_ACCOUNT,
        login: SENDER_LOGIN.to_owned(),
        level: Level::Owner,
        account_id: Id::ROOT_ACCOUNT,
        role: SENDER_LOGIN.to_owned(),
        actor: None,
        scope: Scope::Full,
        via: Via::Internal,
        external_id: None,
        origin: None,
        lang: "en".to_owned(),
        grants: BTreeMap::new(),
    }
}

pub fn templates() -> [MailTemplate; 2] {
    [
        MailTemplate::new(
            RESET_TEMPLATE,
            include_str!("../../templates/auth/password_reset.en.subject.txt"),
            include_str!("../../templates/auth/password_reset.en.body.txt"),
        )
        .lang(
            "ru",
            include_str!("../../templates/auth/password_reset.ru.subject.txt"),
            include_str!("../../templates/auth/password_reset.ru.body.txt"),
        ),
        MailTemplate::new(
            VERIFY_TEMPLATE,
            include_str!("../../templates/auth/address_verify.en.subject.txt"),
            include_str!("../../templates/auth/address_verify.en.body.txt"),
        )
        .lang(
            "ru",
            include_str!("../../templates/auth/address_verify.ru.subject.txt"),
            include_str!("../../templates/auth/address_verify.ru.body.txt"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_base_is_an_http_address_without_query_and_a_link_names_the_route_and_the_token() {
        assert_eq!(
            shell_base(" https://panel.example.org:9443/ "),
            Some("https://panel.example.org:9443")
        );
        assert_eq!(
            shell_base("http://127.0.0.1:1500"),
            Some("http://127.0.0.1:1500")
        );
        for refused in [
            "",
            "panel.example.org",
            "https://",
            "https://a b",
            "https://a?x=1",
            "https://a#top",
            "ftp://a",
        ] {
            assert_eq!(shell_base(refused), None, "{refused}");
        }
        let token = Secret::new("kpr_00ff");
        assert_eq!(
            link("https://panel.example.org", Purpose::Reset, &token),
            "https://panel.example.org/reset?token=kpr_00ff"
        );
        assert_eq!(
            link(
                "https://panel.example.org",
                Purpose::Verify,
                &Secret::new("kav_1")
            ),
            "https://panel.example.org/verify?token=kav_1"
        );
        assert_eq!(sender().via, Via::Internal);
        let templates = templates();
        assert_eq!(templates[0].id(), RESET_TEMPLATE);
        assert_eq!(templates[1].id(), VERIFY_TEMPLATE);
        for template in &templates {
            assert_eq!(template.module(), "auth");
            assert_eq!(template.languages().collect::<Vec<_>>(), ["en", "ru"]);
            for (_, text) in template.texts() {
                assert!(text.body.contains("{params.link}"), "{}", template.id());
            }
        }
    }
}
