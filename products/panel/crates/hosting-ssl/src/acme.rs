use std::time::Duration;

use instant_acme::{
    Account, AccountBuilder, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier,
    NewAccount, NewOrder, OrderStatus, RetryPolicy,
};
use keel::prelude::*;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel::sdk::serde_json;
use keel::sdk::system::SecretCipher;
use keel_host::api::{Attributes, Dest, Host, Origin};

use crate::install::{Installer, PREFIX};
use crate::pem::Material;

pub(crate) const DIRECTORY: &str = "hosting.certificate.directory";
pub(crate) const EMAIL: &str = "hosting.certificate.email";
pub(crate) const PRODUCTION: &str = "https://acme-v02.api.letsencrypt.org/directory";

const CHALLENGES: &str = "acme";
const CHALLENGE_MODE: &str = "0644";
const TOKEN_MAX: usize = 128;
const WAIT: Duration = Duration::from_secs(120);
const ACME: &str = "hosting.certificate.error.acme";
const ORDER: &str = "hosting.certificate.error.order";
const CHALLENGE: &str = "hosting.certificate.error.challenge";
const TOKEN: &str = "hosting.certificate.error.token";
const DIRECTORY_CA: &str = "hosting.certificate.error.directory_ca";
const CREDENTIALS: &str = "hosting.certificate.error.credentials";
const READ_ACCOUNT: &str =
    "SELECT credentials FROM hosting_certificate_account WHERE directory = $1";
const WRITE_ACCOUNT: &str = "INSERT INTO hosting_certificate_account (directory, credentials, created_at) VALUES ($1, $2, $3) ON CONFLICT (directory) DO NOTHING";

pub(crate) struct Directory {
    pub(crate) url: String,
    pub(crate) email: String,
}

pub(crate) async fn order<P>(
    call: &Call<P>,
    installer: &Installer,
    directory: &Directory,
    names: &[String],
) -> Result<Material> {
    let origin = Origin::new(crate::PRODUCT, call.principal(), call.request_id());
    let account = account(call, installer, directory).await?;
    let identifiers: Vec<Identifier> = names.iter().cloned().map(Identifier::Dns).collect();
    let mut order = account
        .new_order(&NewOrder::new(&identifiers))
        .await
        .map_err(refused)?;
    let host = installer.host().on(origin);
    let mut placed = Vec::new();
    let answered = answer(&mut order, &host, &mut placed).await;
    let finished = match answered {
        Ok(()) => finish(&mut order, names).await,
        Err(error) => Err(error),
    };
    for token in &placed {
        host.fs()
            .remove(&Dest::new(CHALLENGES, &challenge_file(token)), false)
            .await?;
    }
    let (chain, key) = finished?;
    Material::issued(chain, key)
}

async fn answer(
    order: &mut instant_acme::Order,
    host: &Host<'_>,
    placed: &mut Vec<String>,
) -> Result<()> {
    let mut authorizations = order.authorizations();
    while let Some(authorization) = authorizations.next().await {
        let mut authorization = authorization.map_err(refused)?;
        if authorization.status != AuthorizationStatus::Pending {
            continue;
        }
        let name = match &authorization.identifier().identifier {
            Identifier::Dns(name) => name.clone(),
            other => format!("{other:?}"),
        };
        let mut challenge = authorization
            .challenge(ChallengeType::Http01)
            .ok_or_else(|| KeelError::unavailable(CHALLENGE).param("name", name.as_str()))?;
        let token = challenge.token.clone();
        if !plain_token(&token) {
            return Err(KeelError::unavailable(TOKEN).param("token", token));
        }
        let content = challenge.key_authorization().as_str().to_owned();
        host.fs()
            .write(
                &Dest::new(CHALLENGES, &challenge_file(&token)),
                &content,
                &Attributes::default().mode(CHALLENGE_MODE),
            )
            .await?;
        placed.push(token);
        challenge.set_ready().await.map_err(refused)?;
    }
    Ok(())
}

async fn finish(order: &mut instant_acme::Order, names: &[String]) -> Result<(String, String)> {
    let retries = RetryPolicy::new().timeout(WAIT);
    let status = order.poll_ready(&retries).await.map_err(refused)?;
    if status != OrderStatus::Ready {
        return Err(KeelError::unavailable(ORDER)
            .param("names", names.join(", "))
            .param("status", format!("{status:?}").to_lowercase())
            .param("detail", problems(order).await));
    }
    let key = order.finalize().await.map_err(refused)?;
    let chain = order.poll_certificate(&retries).await.map_err(refused)?;
    Ok((chain, key))
}

async fn problems(order: &mut instant_acme::Order) -> String {
    let mut found = Vec::new();
    let mut authorizations = order.authorizations();
    while let Some(Ok(authorization)) = authorizations.next().await {
        found.extend(
            authorization
                .challenges
                .iter()
                .filter_map(|challenge| challenge.error.as_ref())
                .map(ToString::to_string),
        );
    }
    found.join("; ")
}

async fn account<P>(
    call: &Call<P>,
    installer: &Installer,
    directory: &Directory,
) -> Result<Account> {
    let cipher = call.require::<dyn SecretCipher>()?;
    let db = call.db();
    let stored = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            READ_ACCOUNT,
            [directory.url.as_str().into()],
        ))
        .await?;
    if let Some(stored) = stored {
        let sealed: String = stored.try_get("", "credentials")?;
        let credentials: AccountCredentials = serde_json::from_slice(&cipher.open(&sealed)?)
            .map_err(|_| KeelError::internal(CREDENTIALS))?;
        return builder(installer)?
            .from_credentials(credentials)
            .await
            .map_err(refused);
    }
    let contact = (!directory.email.is_empty()).then(|| format!("mailto:{}", directory.email));
    let contacts: Vec<&str> = contact.iter().map(String::as_str).collect();
    let (account, credentials) = builder(installer)?
        .create(
            &NewAccount {
                contact: &contacts,
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            directory.url.clone(),
            None,
        )
        .await
        .map_err(refused)?;
    let sealed = cipher.seal(&serde_json::to_vec(&credentials)?)?;
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        WRITE_ACCOUNT,
        [
            directory.url.as_str().into(),
            sealed.into(),
            installer.store().now()?.into(),
        ],
    ))
    .await?;
    Ok(account)
}

fn builder(installer: &Installer) -> Result<AccountBuilder> {
    let trust = &installer.store().config()?.directory_ca;
    let built = if trust.is_empty() {
        Account::builder()
    } else {
        Account::builder_with_root(trust)
    };
    built.map_err(|error| {
        KeelError::internal(DIRECTORY_CA)
            .param("path", trust.as_str())
            .param("detail", error.to_string())
    })
}

fn challenge_file(token: &str) -> String {
    format!("{PREFIX}{token}")
}

fn plain_token(token: &str) -> bool {
    (1..=TOKEN_MAX).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn refused(error: instant_acme::Error) -> KeelError {
    KeelError::unavailable(ACME).param("detail", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_challenge_file_is_the_token_under_the_drop_in_prefix_and_only_a_plain_token_passes() {
        assert_eq!(challenge_file("abc_DEF-1"), "keel-panel-abc_DEF-1");
        assert!(plain_token("LoqXcYV8q5ONbJQxbmR7SCTNo3tiAXDfowyjxAjEuX0"));
        for bad in ["", "../x", "a/b", "a.b", "a b", &"a".repeat(TOKEN_MAX + 1)] {
            assert!(!plain_token(bad), "{bad}");
        }
    }
}
