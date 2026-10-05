use chrono::{Duration, Utc};
use keel_types::{Descriptor, ErrorCode, Id, Idempotency, InstanceInfo};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Suite;
use super::idempotency::keyed;
use super::schema::{self, Pick};
use crate::client::{Answer, Credential};
use crate::report::{Aborted, Failure, ensure, fail};

pub const KINDS: &str = "svc.kinds";
pub const FEATURE: &str = "svc";
pub const ORIGIN: &str = "conformance";
pub const REQUIRED_KEY: &str = "keel.error.idempotency_required";
pub const VALIDATION: &str = "keel.error.validation";
pub const ERROR_EXISTS: &str = "svc.error.exists";
pub const ERROR_CLOSED: &str = "svc.error.closed";
pub const ERROR_NOT_FOUND: &str = "svc.error.not_found";
pub const ERROR_PERIOD: &str = "svc.error.period";
pub const ERROR_NO_PRINCIPAL: &str = "svc.error.no_principal";
pub const LOGIN_KEY: &str = "/login?key=kok_";
const IDENTITY: [&str; 2] = ["origin", "externalId"];

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub kind: String,
    pub schema_version: u32,
    pub title: String,
    pub account: Value,
    pub params: Value,
    pub state: Value,
    pub functions: Functions,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Functions {
    pub open: String,
    pub suspend: String,
    pub resume: String,
    pub close: String,
    pub set_params: String,
    pub stat: String,
    pub login: String,
}

impl Functions {
    pub fn named(&self) -> [(&'static str, &str); 7] {
        [
            ("open", &self.open),
            ("suspend", &self.suspend),
            ("resume", &self.resume),
            ("close", &self.close),
            ("set_params", &self.set_params),
            ("stat", &self.stat),
            ("login", &self.login),
        ]
    }
}

#[derive(Deserialize)]
struct Catalogue {
    data: Vec<Entry>,
}

#[derive(Clone, Debug)]
struct Opened {
    entry: Entry,
    external: String,
    service: Id,
    account: Id,
}

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the svc checks", outcome)?;
    let outcome = suite.client.instance().await;
    let (_, instance) = suite.report.check(
        "GET /api/v1/instance answers the handshake with its service kinds",
        outcome,
    )?;
    let answer = suite
        .client
        .call(KINDS, json!({}), Credential::Session, &[])
        .await;
    let entries = suite.report.check(
        "svc.kinds answers one entry per service kind of the handshake, sorted, each naming its seven functions",
        answer.and_then(|answer| {
            let (catalogue, _) = answer.data::<Catalogue>()?;
            catalogued(&catalogue.data, &instance).map_err(|failure| answer.failure(failure.0))?;
            Ok(catalogue.data)
        }),
    )?;
    let answer = suite
        .client
        .get("/api/v1/functions", Credential::Session, &[])
        .await;
    suite.report.check(
        "the descriptor listing serves every function of the catalogue, open and close requiring Idempotency-Key",
        answer.and_then(|answer| {
            answer.status_is(200)?;
            let descriptors: Vec<Descriptor> = answer.body_as()?;
            served(&entries, &descriptors)
        }),
    )?;
    let customer = json!({ "origin": ORIGIN, "externalId": suite.name("customer") });
    let mut opened = Vec::new();
    for entry in &entries {
        opened.push(open(suite, entry, &customer).await?);
    }
    if opened.len() > 1 {
        let accounts: Vec<Id> = opened.iter().map(|service| service.account).collect();
        suite.report.check(
            "the services of every kind opened for one customer land in one account",
            ensure(accounts.windows(2).all(|pair| pair[0] == pair[1]), || {
                format!("the accounts are {accounts:?}")
            }),
        )?;
    }
    for service in &opened {
        operate(suite, service).await?;
    }
    if let Some(first) = opened.first() {
        let answer = suite
            .client
            .call(
                &first.entry.functions.login,
                json!({ "externalId": first.external }),
                Credential::Session,
                &[],
            )
            .await;
        suite.report.check(
            &format!(
                "{} answers 409 svc.error.no_principal while the account holds no principal of its origin",
                first.entry.functions.login
            ),
            answer.and_then(|answer| {
                answer
                    .error(409, ErrorCode::Conflict, ERROR_NO_PRINCIPAL)
                    .map(|_| ())
            }),
        )?;
        let params = json!({
            "origin": ORIGIN,
            "externalId": suite.name("customer-principal"),
            "login": suite.name("customer"),
            "level": "user",
            "account": customer,
        });
        let answer = suite
            .client
            .call("keel.principal.upsert", params, Credential::Session, &[])
            .await;
        let account = first.account;
        suite.report.check(
            "keel.principal.upsert places the customer's principal in the account of the services",
            answer.and_then(|answer| {
                let (data, _) = answer.data::<Value>()?;
                ensure(data["accountId"] == json!(account), || {
                    answer.failure("accountId is not the services' account").0
                })
            }),
        )?;
    }
    for service in &opened {
        let answer = suite
            .client
            .call(
                &service.entry.functions.login,
                json!({ "externalId": service.external }),
                Credential::Session,
                &[],
            )
            .await
            .map(Answer::confidential);
        suite.report.check(
            &format!(
                "{} answers the url of a one-time login key",
                service.entry.functions.login
            ),
            answer.and_then(|answer| {
                let (data, _) = answer.data::<Value>()?;
                ensure(
                    data["url"]
                        .as_str()
                        .is_some_and(|url| url.contains(LOGIN_KEY)),
                    || answer.failure("data.url is not a login link").0,
                )
            }),
        )?;
    }
    for service in &opened {
        close(suite, service).await?;
    }
    if let Some(first) = opened.first() {
        let answer = suite
            .client
            .call(
                &first.entry.functions.stat,
                json!({ "externalId": suite.name("nobody"), "from": hour_ago(), "to": now() }),
                Credential::Session,
                &[],
            )
            .await;
        suite.report.check(
            &format!(
                "{} of an unknown external id answers 404 svc.error.not_found",
                first.entry.functions.stat
            ),
            answer.and_then(|answer| {
                answer
                    .error(404, ErrorCode::NotFound, ERROR_NOT_FOUND)
                    .map(|_| ())
            }),
        )?;
    }
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the svc checks", outcome)?;
    Ok(())
}

async fn open(suite: &mut Suite, entry: &Entry, customer: &Value) -> Result<Opened, Aborted> {
    let functions = entry.functions.clone();
    let built = account_for(&entry.account, customer)
        .and_then(|account| Ok((account, schema::instance(&entry.params, Pick::Least)?)));
    let (account, params) = suite.report.check(
        &format!(
            "the account and the parameters of {} are built from the schemas of the catalogue",
            entry.kind
        ),
        built,
    )?;
    let external = suite.name(&entry.kind);
    let body = json!({ "externalId": external, "account": account, "params": params });
    let answer = suite
        .client
        .call(&functions.open, body.clone(), Credential::Session, &[])
        .await;
    suite.report.check(
        &format!(
            "{} without an Idempotency-Key answers 422 keel.error.idempotency_required",
            functions.open
        ),
        required(answer, &functions.open),
    )?;
    let key = suite.name(&format!("{}-open", entry.kind));
    let answer = keyed(suite, &functions.open, body.clone(), &key).await;
    let first = suite.report.check(
        &format!(
            "{} with a key opens the service active with its state",
            functions.open
        ),
        status(answer, "active", false),
    )?;
    let answer = keyed(suite, &functions.open, body.clone(), &key).await;
    suite.report.check(
        &format!(
            "{} twice with one key replays the same service",
            functions.open
        ),
        status(answer, "active", true).and_then(|again| {
            ensure(again == first, || {
                format!("the replay answers {again:?} instead of {first:?}")
            })
        }),
    )?;
    let other = suite.name(&format!("{}-open-again", entry.kind));
    let answer = keyed(suite, &functions.open, body, &other).await;
    suite.report.check(
        &format!(
            "{} of the same external id under another key answers 409 svc.error.exists",
            functions.open
        ),
        answer.and_then(|answer| {
            answer
                .error(409, ErrorCode::Conflict, ERROR_EXISTS)
                .map(|_| ())
        }),
    )?;
    Ok(Opened {
        entry: entry.clone(),
        external,
        service: first.0,
        account: first.1,
    })
}

async fn operate(suite: &mut Suite, service: &Opened) -> Result<(), Aborted> {
    let functions = &service.entry.functions;
    let external = &service.external;
    let answer = suite
        .client
        .call(
            &functions.stat,
            json!({ "externalId": external, "from": hour_ago(), "to": now() }),
            Credential::Session,
            &[],
        )
        .await;
    let id = service.service;
    suite.report.check(
        &format!("{} answers the usage of the last hour", functions.stat),
        answer.and_then(|answer| {
            let (data, _) = answer.data::<Value>()?;
            ensure(data["serviceId"] == json!(id), || {
                answer.failure("serviceId is not the service").0
            })?;
            ensure(data.get("usage").is_some(), || answer.failure("no usage").0)
        }),
    )?;
    let answer = suite
        .client
        .call(
            &functions.stat,
            json!({ "externalId": external, "from": now(), "to": hour_ago() }),
            Credential::Session,
            &[],
        )
        .await;
    suite.report.check(
        &format!(
            "{} of a period that ends before it starts answers 422 svc.error.period on to",
            functions.stat
        ),
        answer.and_then(|answer| {
            let error = answer.error(422, ErrorCode::Validation, VALIDATION)?;
            ensure(
                error
                    .fields
                    .iter()
                    .any(|field| field.path == "to" && field.key.as_deref() == Some(ERROR_PERIOD)),
                || answer.failure("no svc.error.period on to").0,
            )
        }),
    )?;
    let params = suite.report.check(
        &format!(
            "the next parameters of {} are built from its schema",
            service.entry.kind
        ),
        schema::instance(&service.entry.params, Pick::Next),
    )?;
    let answer = suite
        .client
        .call(
            &functions.set_params,
            json!({ "externalId": external, "params": params }),
            Credential::Session,
            &[],
        )
        .await;
    suite.report.check(
        &format!(
            "{} applies new parameters and answers the state",
            functions.set_params
        ),
        same(status(answer, "active", false), service),
    )?;
    for (function, expected) in [
        (&functions.suspend, "suspended"),
        (&functions.resume, "active"),
    ] {
        let answer = suite
            .client
            .call(
                function,
                json!({ "externalId": external }),
                Credential::Session,
                &[],
            )
            .await;
        suite.report.check(
            &format!("{function} answers the service {expected}"),
            same(status(answer, expected, false), service),
        )?;
    }
    Ok(())
}

async fn close(suite: &mut Suite, service: &Opened) -> Result<(), Aborted> {
    let functions = &service.entry.functions;
    let body = json!({ "externalId": service.external });
    let answer = suite
        .client
        .call(&functions.close, body.clone(), Credential::Session, &[])
        .await;
    suite.report.check(
        &format!(
            "{} without an Idempotency-Key answers 422 keel.error.idempotency_required",
            functions.close
        ),
        required(answer, &functions.close),
    )?;
    let key = suite.name(&format!("{}-close", service.entry.kind));
    let answer = keyed(suite, &functions.close, body.clone(), &key).await;
    suite.report.check(
        &format!(
            "{} with a key closes the service with a retention date and deletes nothing",
            functions.close
        ),
        answer.and_then(|answer| {
            let (data, _) = answer.data::<Value>()?;
            ensure(data["status"] == json!("closed"), || {
                answer.failure("the status is not closed").0
            })?;
            ensure(data["serviceId"] == json!(service.service), || {
                answer.failure("serviceId is not the service").0
            })?;
            ensure(data["retainUntil"].is_string(), || {
                answer.failure("no retainUntil").0
            })
        }),
    )?;
    let answer = suite
        .client
        .call(&functions.suspend, body.clone(), Credential::Session, &[])
        .await;
    suite.report.check(
        &format!(
            "{} of a closed service answers 409 svc.error.closed",
            functions.suspend
        ),
        answer.and_then(|answer| {
            answer
                .error(409, ErrorCode::Conflict, ERROR_CLOSED)
                .map(|_| ())
        }),
    )?;
    let answer = suite
        .client
        .call(
            &functions.stat,
            json!({ "externalId": service.external, "from": hour_ago(), "to": now() }),
            Credential::Session,
            &[],
        )
        .await;
    suite.report.check(
        &format!("{} still answers on the closed service", functions.stat),
        answer.and_then(|answer| answer.data::<Value>().map(|_| ())),
    )?;
    Ok(())
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn hour_ago() -> String {
    (Utc::now() - Duration::hours(1)).to_rfc3339()
}

pub fn account_for(schema: &Value, customer: &Value) -> Result<Value, Failure> {
    let mut rest = schema.clone();
    if let Some(required) = rest.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|name| !IDENTITY.iter().any(|member| name == member));
    }
    let Value::Object(mut account) = schema::instance(&rest, Pick::Least)? else {
        return Err(fail("the account schema does not build an object"));
    };
    for member in IDENTITY {
        account.insert(member.to_owned(), customer[member].clone());
    }
    Ok(Value::Object(account))
}

pub fn catalogued(entries: &[Entry], instance: &InstanceInfo) -> Result<(), Failure> {
    let kinds: Vec<&str> = entries.iter().map(|entry| entry.kind.as_str()).collect();
    let mut sorted = kinds.clone();
    sorted.sort_unstable();
    ensure(kinds == sorted, || {
        format!("the kinds {kinds:?} are not sorted")
    })?;
    ensure(kinds == instance.service_kinds, || {
        format!(
            "the catalogue names {kinds:?} while the handshake names {:?}",
            instance.service_kinds
        )
    })?;
    ensure(
        kinds.is_empty() || instance.features.iter().any(|feature| feature == FEATURE),
        || format!("the handshake offers kinds without the feature {FEATURE}"),
    )?;
    for entry in entries {
        ensure(entry.schema_version >= 1 && !entry.title.is_empty(), || {
            format!("the kind {} has no title or schema version", entry.kind)
        })?;
        for (verb, function) in entry.functions.named() {
            let expected = format!("svc.{}.{verb}", entry.kind);
            ensure(function == expected, || {
                format!(
                    "the {verb} function of {} is {function}, not {expected}",
                    entry.kind
                )
            })?;
        }
        for (name, schema) in [
            ("params", &entry.params),
            ("state", &entry.state),
            ("account", &entry.account),
        ] {
            ensure(schema.is_object(), || {
                format!("the {name} schema of {} is not an object", entry.kind)
            })?;
        }
    }
    Ok(())
}

pub fn served(entries: &[Entry], descriptors: &[Descriptor]) -> Result<(), Failure> {
    for entry in entries {
        for (verb, function) in entry.functions.named() {
            let descriptor = descriptors
                .iter()
                .find(|descriptor| descriptor.name == function)
                .ok_or_else(|| fail(format!("the listing lacks {function}")))?;
            let expected = if matches!(verb, "open" | "close") {
                Idempotency::Required
            } else {
                Idempotency::Optional
            };
            ensure(descriptor.idempotency == expected, || {
                format!(
                    "{function} declares idempotency {:?}",
                    descriptor.idempotency
                )
            })?;
        }
    }
    Ok(())
}

fn required(answer: Result<Answer, Failure>, function: &str) -> Result<(), Failure> {
    let answer = answer?;
    let error = answer.error(422, ErrorCode::Validation, REQUIRED_KEY)?;
    ensure(error.params["function"] == json!(function), || {
        answer
            .failure("params.function does not name the function")
            .0
    })
}

fn status(
    answer: Result<Answer, Failure>,
    expected: &str,
    replay: bool,
) -> Result<(Id, Id, Value), Failure> {
    let answer = answer?;
    let (data, _) = answer.data::<Value>()?;
    ensure(answer.replayed() == replay, || {
        answer
            .failure(format!("X-Keel-Idempotent-Replay is not {replay}"))
            .0
    })?;
    ensure(data["status"] == json!(expected), || {
        answer.failure(format!("the status is not {expected}")).0
    })?;
    ensure(data["state"].is_object(), || {
        answer.failure("the state is not an object").0
    })?;
    let service = super::id_of(&data, "serviceId").map_err(|failure| answer.failure(failure.0))?;
    let account = super::id_of(&data, "accountId").map_err(|failure| answer.failure(failure.0))?;
    Ok((service, account, data["state"].clone()))
}

fn same(answered: Result<(Id, Id, Value), Failure>, service: &Opened) -> Result<(), Failure> {
    let (id, account, _) = answered?;
    ensure(id == service.service && account == service.account, || {
        format!(
            "the answer names the service {id} in {account}, not {} in {}",
            service.service, service.account
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &str) -> Entry {
        let name = |verb: &str| format!("svc.{kind}.{verb}");
        Entry {
            kind: kind.to_owned(),
            schema_version: 1,
            title: format!("{kind}.title"),
            account: json!({
                "type": "object",
                "properties": {
                    "origin": { "type": "string", "pattern": "^[!#-+\\--:<-\\[\\]-~]+$" },
                    "externalId": { "type": "string", "pattern": "^[!#-+\\--:<-\\[\\]-~]+$" },
                    "kind": { "type": ["string", "null"] }
                },
                "required": ["origin", "externalId"]
            }),
            params: json!({ "type": "object" }),
            state: json!({ "type": "object" }),
            functions: Functions {
                open: name("open"),
                suspend: name("suspend"),
                resume: name("resume"),
                close: name("close"),
                set_params: name("set_params"),
                stat: name("stat"),
                login: name("login"),
            },
        }
    }

    fn handshake(kinds: &[&str], features: &[&str]) -> InstanceInfo {
        InstanceInfo {
            instance_id: Id::generate(),
            keel: "0.3.0".into(),
            protocol: keel_types::PROTOCOL,
            products: Vec::new(),
            public_key: "ed25519:x".into(),
            features: features
                .iter()
                .map(|feature| (*feature).to_owned())
                .collect(),
            service_kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
        }
    }

    #[test]
    fn the_catalogue_agrees_with_the_handshake_in_order_and_names_each_function_after_its_kind() {
        let both = [entry("dns.zones"), entry("hosting.sites")];
        catalogued(
            &both,
            &handshake(&["dns.zones", "hosting.sites"], &["peers", "svc"]),
        )
        .unwrap();
        catalogued(&[], &handshake(&[], &["peers"])).unwrap();
        assert!(catalogued(&both, &handshake(&["dns.zones"], &["svc"])).is_err());
        assert!(
            catalogued(
                &both,
                &handshake(&["dns.zones", "hosting.sites"], &["peers"])
            )
            .is_err()
        );
        let reversed = [entry("hosting.sites"), entry("dns.zones")];
        assert!(
            catalogued(
                &reversed,
                &handshake(&["hosting.sites", "dns.zones"], &["svc"])
            )
            .is_err()
        );
        let mut renamed = entry("dns.zones");
        renamed.functions.stat = "svc.dns.zones.usage".into();
        assert!(catalogued(&[renamed], &handshake(&["dns.zones"], &["svc"])).is_err());
    }

    #[test]
    fn the_account_carries_the_customer_and_builds_the_other_required_members() {
        let customer = json!({ "origin": ORIGIN, "externalId": "acc-7" });
        let account = account_for(&entry("dns.zones").account, &customer).unwrap();
        assert_eq!(account, customer);
        let mut wider = entry("dns.zones").account;
        wider["properties"]["region"] = json!({ "type": "string", "enum": ["eu"] });
        wider["required"] = json!(["origin", "externalId", "region"]);
        assert_eq!(
            account_for(&wider, &customer).unwrap(),
            json!({ "origin": ORIGIN, "externalId": "acc-7", "region": "eu" })
        );
        assert!(account_for(&json!({ "type": "string" }), &customer).is_err());
    }

    #[test]
    fn the_listing_must_serve_each_function_with_the_key_required_on_open_and_close_alone() {
        let descriptor = |name: String, idempotency: Idempotency| -> Descriptor {
            serde_json::from_value(json!({
                "keel": 1,
                "name": name,
                "module": "svc",
                "kind": "action",
                "mode": "write",
                "access": { "level": "admin" },
                "idempotency": idempotency,
                "params": {},
                "result": {}
            }))
            .unwrap()
        };
        let zones = entry("dns.zones");
        let listing: Vec<Descriptor> = zones
            .functions
            .named()
            .iter()
            .map(|(verb, name)| {
                let idempotency = if matches!(*verb, "open" | "close") {
                    Idempotency::Required
                } else {
                    Idempotency::Optional
                };
                descriptor((*name).to_owned(), idempotency)
            })
            .collect();
        served(std::slice::from_ref(&zones), &listing).unwrap();
        assert!(served(std::slice::from_ref(&zones), &listing[1..]).is_err());
        let mut loose = listing.clone();
        loose[0].idempotency = Idempotency::Optional;
        assert!(served(std::slice::from_ref(&zones), &loose).is_err());
        let mut strict = listing;
        strict[5].idempotency = Idempotency::Required;
        assert!(served(&[zones], &strict).is_err());
    }
}
