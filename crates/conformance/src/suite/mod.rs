mod auth;
mod boot;
mod errors;
mod events;
mod export;
mod features;
mod fields;
mod health;
mod idempotency;
mod lists;
mod peers;
mod probes;
mod schema;
mod svc;
mod tokens;
mod webhooks;

use keel_types::{ActorVia, Id, MetaPrincipal, Page};
use serde_json::{Value, json};

use crate::client::{Client, Credential};
use crate::report::{Aborted, Failure, Outcome, Report, ensure, fail};

pub struct Settings {
    pub url: String,
    pub cacert: Option<std::path::PathBuf>,
    pub admin_login: String,
    pub admin_password: String,
    pub user_login: String,
}

pub struct Suite {
    pub client: Client,
    pub settings: Settings,
    pub report: Report,
    pub run: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub id: Id,
    pub account: Id,
}

pub async fn run(settings: Settings) -> Result<Outcome, Failure> {
    let mut suite = Suite {
        client: Client::new(&settings.url, settings.cacert.as_deref())?,
        settings,
        report: Report::default(),
        run: Id::generate().to_string().to_lowercase(),
    };
    let groups: [(&str, Step); 14] = [
        ("health", |suite| Box::pin(health::run(suite))),
        ("boot", |suite| Box::pin(boot::run(suite))),
        ("auth", |suite| Box::pin(auth::run(suite))),
        ("token", |suite| Box::pin(tokens::run(suite))),
        ("list", |suite| Box::pin(lists::run(suite))),
        ("export", |suite| Box::pin(export::run(suite))),
        ("error", |suite| Box::pin(errors::run(suite))),
        ("features", |suite| Box::pin(features::run(suite))),
        ("fields", |suite| Box::pin(fields::run(suite))),
        ("webhook", |suite| Box::pin(webhooks::run(suite))),
        ("events", |suite| Box::pin(events::run(suite))),
        ("idempotency", |suite| Box::pin(idempotency::run(suite))),
        ("svc", |suite| Box::pin(svc::run(suite))),
        ("peers", |suite| Box::pin(peers::run(suite))),
    ];
    for (name, group) in groups {
        if group(&mut suite).await.is_err() {
            suite.report.aborted(name);
        }
    }
    let missing = suite.client.without_request_id().to_vec();
    let _ = suite.report.check(
        &format!(
            "every response carried X-Keel-Request-Id ({} responses)",
            suite.client.responses()
        ),
        ensure(missing.is_empty(), || {
            format!("without the header: {}", missing.join(", "))
        }),
    );
    Ok(suite.report.outcome())
}

type Group<'a> = std::pin::Pin<Box<dyn Future<Output = Result<(), Aborted>> + 'a>>;
type Step = fn(&mut Suite) -> Group<'_>;

impl Suite {
    pub fn name(&self, what: &str) -> String {
        format!("conformance-{}-{what}", self.run)
    }

    pub async fn login(&mut self) -> Result<MetaPrincipal, Failure> {
        let answer = self
            .client
            .call(
                "auth.login",
                json!({ "login": self.settings.admin_login, "password": self.settings.admin_password }),
                Credential::Guest,
                &[],
            )
            .await?;
        let (data, _) = answer.data::<Value>()?;
        ensure(data["state"] == "authenticated", || {
            answer.failure("state is not authenticated").0
        })?;
        let principal: MetaPrincipal = serde_json::from_value(data["principal"].clone())
            .map_err(|error| answer.failure(format!("data.principal does not fit: {error}")))?;
        ensure(principal.login == self.settings.admin_login, || {
            answer.failure("the principal is not the admin").0
        })?;
        answer.session_cookie()?;
        ensure(self.client.session().is_some(), || {
            answer.failure("the cookie was not kept").0
        })?;
        Ok(principal)
    }

    pub async fn logout(&mut self) -> Result<(), Failure> {
        let answer = self
            .client
            .call("auth.logout", json!({}), Credential::Session, &[])
            .await?;
        answer.data::<Value>()?;
        answer.clears_session()?;
        ensure(self.client.session().is_none(), || {
            answer.failure("the cleared cookie was kept").0
        })
    }

    pub async fn find_user(&mut self) -> Result<User, Failure> {
        let login = self.settings.user_login.clone();
        let answer = self
            .client
            .call(
                "keel.principal.list",
                json!({ "search": login }),
                Credential::Session,
                &[],
            )
            .await?;
        let (page, _) = answer.data::<Page<Value>>()?;
        let row = page
            .data
            .iter()
            .find(|row| row["login"] == login)
            .ok_or_else(|| answer.failure(format!("no principal with login {login}")))?;
        let id = id_of(row, "id").map_err(|failure| answer.failure(failure.0))?;
        let account = id_of(row, "accountId").map_err(|failure| answer.failure(failure.0))?;
        Ok(User { id, account })
    }

    pub async fn su(&mut self, user: &User) -> Result<MetaPrincipal, Failure> {
        let answer = self
            .client
            .call(
                "auth.su",
                json!({ "principal": user.id }),
                Credential::Session,
                &[],
            )
            .await?;
        let (data, _) = answer.data::<Value>()?;
        let principal: MetaPrincipal = serde_json::from_value(data["principal"].clone())
            .map_err(|error| answer.failure(format!("data.principal does not fit: {error}")))?;
        ensure(principal.login == self.settings.user_login, || {
            answer.failure("the principal is not the user").0
        })?;
        let actor = principal
            .actor
            .as_ref()
            .ok_or_else(|| answer.failure("no actor"))?;
        ensure(
            actor.login == self.settings.admin_login && actor.via == ActorVia::Su,
            || answer.failure("the actor is not the admin via su").0,
        )?;
        ensure(answer.set_cookie().is_none(), || {
            answer.failure("auth.su set a cookie").0
        })?;
        Ok(principal)
    }

    pub async fn su_exit(&mut self) -> Result<MetaPrincipal, Failure> {
        let answer = self
            .client
            .call("auth.su.exit", json!({}), Credential::Session, &[])
            .await?;
        let (data, _) = answer.data::<Value>()?;
        let principal: MetaPrincipal = serde_json::from_value(data["principal"].clone())
            .map_err(|error| answer.failure(format!("data.principal does not fit: {error}")))?;
        ensure(
            principal.login == self.settings.admin_login && principal.actor.is_none(),
            || {
                answer
                    .failure("the principal is not the admin without an actor")
                    .0
            },
        )?;
        Ok(principal)
    }
}

pub fn id_of(row: &Value, member: &str) -> Result<Id, Failure> {
    row[member]
        .as_str()
        .ok_or_else(|| fail(format!("no {member} in {row}")))?
        .parse()
        .map_err(|error| fail(format!("{member} is not an id: {error}")))
}

pub fn ids(page: &Page<Value>) -> Result<Vec<Id>, Failure> {
    page.data.iter().map(|row| id_of(row, "id")).collect()
}
