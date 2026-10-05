use std::time::Duration;

use futures::StreamExt;
use keel_client::OnBehalf;
use keel_client::http::HeaderName;
use keel_types::{
    ActorVia, CHALLENGE_PARAM, ErrorCode, Event, EventActorKind, Id, InstanceInfo, Level,
    MetaPrincipal, PROTOCOL, Page, PeerScope, ProvenInstance,
};
use serde_json::{Value, json};

use super::Suite;
use super::events::Stream;
use crate::client::{Answer, Credential, FEED_PATH, INSTANCE_PATH, ON_BEHALF_OF};
use crate::listener::{
    EVENT_ID_HEADER, HOOK_PATH, Listener, Neighbour, PEER_HEADER, Received, SECRET_MEMBER,
};
use crate::neighbour::{self, Identity, PUBLIC_KEY_SCHEME, WEBHOOK_SECRET_PREFIX, is_peer_token};
use crate::report::{Aborted, Failure, ensure, fail};
use crate::signature::{self, HEADER as SIGNATURE_HEADER, WINDOW_SECONDS, now_seconds};

pub const PAIR_CODE: &str = "peer.pair_code";
pub const PAIR: &str = "peer.pair";
pub const CREATE: &str = "peer.create";
pub const RENEW: &str = "peer.renew";
pub const DELETE: &str = "peer.delete";
pub const LIST: &str = "peer.list";
pub const FEATURE: &str = "peers";
pub const CODE_PREFIX: &str = "PAIR-";
pub const CODE_SYMBOLS: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const NEIGHBOUR_CODE: &str = "PAIR-7K2Q-91XD";
pub const LOGIN_PREFIX: &str = "peer-";
pub const ORIGIN_NAME: &str = "conformance";
pub const LOCAL_ORIGIN: &str = "conformance-local";
pub const SCOPED: &str = "keel.account.list";
pub const OUTSIDE: &str = "keel.principal.list";
pub const UPSERT: &str = "keel.principal.upsert";
pub const ADMIN_ONLY: &str = "keel.product.list";
pub const AT_SERVICE: &str = "auth.logout";
pub const SUBSCRIBE: [&str; 2] = ["keel.account.*", "keel.principal.*"];
pub const PRINCIPAL_CREATED: &str = "keel.principal.created";
pub const ACCOUNT_CREATED: &str = "keel.account.created";
pub const RELAYED_MASK: &str = "conformance.*";
pub const RELAYED_TYPE: &str = "conformance.probe.sent";
pub const RELAYED_MODULE: &str = "conformance";
pub const TTL: u32 = 600;
const FEED_WAIT: Duration = Duration::from_secs(30);
const QUIET: Duration = Duration::from_secs(2);
const UNAUTHENTICATED: &str = "keel.error.unauthenticated";
const VALIDATION: &str = "keel.error.validation";

struct Cast {
    first: Identity,
    other: Identity,
    impostor: Identity,
    second: Identity,
    receiver: Identity,
    pretender: Identity,
    inbound: String,
    back: String,
    fresh: String,
    answered: String,
    secret: String,
}

impl Cast {
    fn new(keel: &str) -> Result<Self, Failure> {
        let first = Identity::new(Id::generate(), keel)?;
        let receiver = Identity::new(Id::generate(), keel)?;
        Ok(Self {
            impostor: Identity::new(first.id, keel)?,
            first,
            other: Identity::new(Id::generate(), keel)?,
            second: Identity::new(Id::generate(), keel)?,
            pretender: Identity::new(receiver.id, keel)?,
            receiver,
            inbound: neighbour::peer_token()?,
            back: neighbour::peer_token()?,
            fresh: neighbour::peer_token()?,
            answered: neighbour::peer_token()?,
            secret: neighbour::webhook_secret()?,
        })
    }
}

struct Headers {
    customer: String,
    hand_made: String,
    planted: String,
    foreign: String,
}

struct Names {
    origin: String,
    customer: String,
    account: String,
    planted: String,
    local_account: String,
}

impl Headers {
    fn new(names: &Names, user: Id) -> Result<Self, Failure> {
        Ok(Self {
            customer: on_behalf(&names.origin, &names.customer)?,
            hand_made: on_behalf(&names.origin, &user.to_string())?,
            planted: on_behalf(&names.origin, &names.planted)?,
            foreign: on_behalf(
                &format!("{ORIGIN_NAME}:{}", Id::generate()),
                &names.customer,
            )?,
        })
    }
}

struct Link {
    id: Id,
    token: String,
    secret: String,
    principal: MetaPrincipal,
}

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the peer checks", outcome)?;
    let listener = suite.report.check(
        "the suite opens a listener that plays the neighbouring installations",
        Listener::start().await,
    )?;
    let outcome = suite.client.instance().await;
    let local = suite.report.check(
        "GET /api/v1/instance names the instance, the protocol, the enabled products, the instance key and the peers feature",
        outcome.and_then(|(answer, instance)| {
            handshake(&instance).map_err(|failure| answer.failure(failure.0))?;
            Ok(instance)
        }),
    )?;
    proves_its_key(suite, &local).await?;
    let cast = suite.report.check(
        "the suite describes its neighbours with instance keys of their own and makes the tokens and the secret they pair with",
        Cast::new(&local.keel),
    )?;
    let outcome = suite.find_user().await;
    let user = suite.report.check(
        "keel.principal.list finds the user created by hand for the peer checks",
        outcome,
    )?;
    let names = Names {
        origin: format!("{ORIGIN_NAME}:{}", cast.first.id),
        customer: suite.name("peer-customer"),
        account: suite.name("peer-account"),
        planted: suite.name("peer-planted"),
        local_account: suite.name("local-account"),
    };
    let headers = suite.report.check(
        "the suite writes X-Keel-On-Behalf-Of for each principal it names",
        Headers::new(&names, user.id),
    )?;
    let link = pair_first(suite, &listener, &local, &cast).await?;
    let (customer, account) = act_for(suite, &link, &names, &headers).await?;
    deliveries(suite, &listener, &local, &cast, &link, customer, account).await?;
    let (second, renewed) = pair_second(suite, &local, &cast, &headers).await?;
    revoke(suite, &link, &headers).await?;
    let answer = remove(suite, second).await;
    suite
        .report
        .check("peer.delete removes the second link", answer)?;
    let answer = bearer(suite, &renewed, SCOPED, None).await;
    suite.report.check(
        "the renewed token of the deleted second link answers 401",
        dead(answer),
    )?;
    initiating(suite, &listener, &local, &cast).await?;
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the peer checks", outcome)?;
    Ok(())
}

async fn proves_its_key(suite: &mut Suite, local: &InstanceInfo) -> Result<(), Aborted> {
    let challenge = suite.report.check(
        "the suite makes a fresh challenge of 32 random bytes",
        neighbour::challenge(),
    )?;
    let answer = suite
        .client
        .get(
            &format!("{INSTANCE_PATH}?{CHALLENGE_PARAM}={challenge}"),
            Credential::Guest,
            &[],
        )
        .await;
    suite.report.check(
        "GET /api/v1/instance with a fresh challenge answers the handshake and a proof its instance key signed over the challenge, the identity, the version, the protocol and the products",
        answer.and_then(|answer| {
            answer.status_is(200)?;
            let proven = answer.body_as::<ProvenInstance>()?;
            ensure(&proven.instance == local, || {
                answer
                    .failure("the proven handshake differs from the plain one")
                    .0
            })?;
            ensure(
                neighbour::proves(&proven.instance, &challenge, &proven.proof),
                || answer.failure("the proof does not verify under the published key").0,
            )
        }),
    )?;
    let answer = suite
        .client
        .get(
            &format!("{INSTANCE_PATH}?{CHALLENGE_PARAM}=short"),
            Credential::Guest,
            &[],
        )
        .await;
    suite.report.check(
        "GET /api/v1/instance with a malformed challenge answers 422 keel.error.challenge on challenge",
        answer.and_then(|answer| {
            let error = answer.error(422, ErrorCode::Validation, VALIDATION)?;
            ensure(
                error.fields.iter().any(|field| {
                    field.path == CHALLENGE_PARAM
                        && field.key.as_deref() == Some("keel.error.challenge")
                }),
                || answer.failure(format!("fields[] is {:?}", error.fields)).0,
            )
        }),
    )?;
    Ok(())
}

async fn pair_first(
    suite: &mut Suite,
    listener: &Listener,
    local: &InstanceInfo,
    cast: &Cast,
) -> Result<Link, Aborted> {
    let granted = PeerScope {
        functions: vec![SCOPED.to_owned(), UPSERT.to_owned()],
        level: Level::Admin,
        on_behalf: true,
    };
    let answer = pair_code(suite, &granted).await;
    let code = suite.report.check(
        "peer.pair_code as the admin answers a one-time PAIR-XXXX-XXXX code",
        coded(answer),
    )?;
    let url = listener.peer_url("a");
    let request = json!({
        "code": code,
        "instance": cast.first.instance(),
        "url": url,
        "inboundToken": cast.inbound,
        "inboundScopes": { "functions": [], "level": "service", "onBehalf": false },
        "subscribe": SUBSCRIBE,
    });
    let answer = guest(suite, PAIR, request.clone()).await;
    let (token, secret) = suite.report.check(
        "peer.pair with the code answers the receiver's instance, a kpt_ token, the granted scopes and a webhook secret",
        paired(answer, local, &granted, true),
    )?;
    let mut again = request;
    again["instance"] = json!(cast.other.instance());
    let answer = guest(suite, PAIR, again).await;
    suite.report.check(
        "peer.pair with the used code answers 422 peer.error.code on code",
        answer.and_then(|answer| {
            let error = answer.error(422, ErrorCode::Validation, VALIDATION)?;
            ensure(
                error.fields.iter().any(|field| {
                    field.path == "code" && field.key.as_deref() == Some("peer.error.code")
                }),
                || answer.failure(format!("fields[] is {:?}", error.fields)).0,
            )
        }),
    )?;
    let answer = suite
        .client
        .call(
            LIST,
            json!({ "search": cast.first.id }),
            Credential::Session,
            &[],
        )
        .await;
    let id = suite.report.check(
        "peer.list finds the link pinning the neighbour's key with its address and the granted scopes",
        answer.and_then(|answer| listed(&answer, &cast.first.instance(), Some(&url), &granted)),
    )?;
    let answer = bearer(suite, &token, SCOPED, None).await;
    let principal = suite.report.check(
        "keel.account.list with the kpt_ token answers as the link's principal at the granted level",
        answer.and_then(|answer| {
            let (_, meta) = answer.data::<Page<Value>>()?;
            let principal = meta
                .principal
                .ok_or_else(|| answer.failure("no meta.principal"))?;
            linked(&principal, id, Level::Admin).map_err(|failure| answer.failure(failure.0))?;
            Ok(principal)
        }),
    )?;
    let answer = bearer(suite, &token, OUTSIDE, None).await;
    suite.report.check(
        "keel.principal.list, outside the token's functions, answers 403 keel.error.token_scope",
        answer.and_then(|answer| {
            let error = answer.error(403, ErrorCode::Forbidden, "keel.error.token_scope")?;
            ensure(error.params["function"] == json!(OUTSIDE), || {
                answer
                    .failure("params.function does not name the function")
                    .0
            })
        }),
    )?;
    Ok(Link {
        id,
        token,
        secret: secret.unwrap_or_default(),
        principal,
    })
}

async fn act_for(
    suite: &mut Suite,
    link: &Link,
    names: &Names,
    headers: &Headers,
) -> Result<(Id, Id), Aborted> {
    let upsert = json!({
        "origin": names.origin,
        "externalId": names.customer,
        "login": names.customer,
        "level": "user",
        "account": { "origin": names.origin, "externalId": names.account },
    });
    let answer = suite
        .client
        .call(UPSERT, upsert, Credential::Bearer(link.token.clone()), &[])
        .await;
    let (customer, account) = suite.report.check(
        "keel.principal.upsert with the kpt_ token places a customer under the neighbour's origin",
        answer.and_then(|answer| {
            let (data, _) = answer.data::<Value>()?;
            ensure(data["created"] == json!(true), || {
                answer.failure("created is not true").0
            })?;
            let id = super::id_of(&data, "id").map_err(|failure| answer.failure(failure.0))?;
            let account =
                super::id_of(&data, "accountId").map_err(|failure| answer.failure(failure.0))?;
            Ok((id, account))
        }),
    )?;
    let answer = bearer(suite, &link.token, SCOPED, Some(&headers.customer)).await;
    suite.report.check(
        "keel.account.list on behalf of the customer runs as the customer, narrowed to its account, with the link as actor via peer",
        answer.and_then(|answer| {
            let (page, meta) = answer.data::<Page<Value>>()?;
            let listed = super::ids(&page).map_err(|failure| answer.failure(failure.0))?;
            ensure(listed == [account], || {
                answer.failure(format!("the accounts are {listed:?}")).0
            })?;
            let principal = meta
                .principal
                .ok_or_else(|| answer.failure("no meta.principal"))?;
            acting(&principal, &names.customer, &link.principal)
                .map_err(|failure| answer.failure(failure.0))
        }),
    )?;
    let answer = bearer(suite, &link.token, SCOPED, Some(&headers.hand_made)).await;
    suite.report.check(
        "on behalf of the user created by hand, which carries no origin, answers 404 peer.error.customer",
        out_of_reach(answer),
    )?;
    let planted = json!({
        "origin": names.origin,
        "externalId": names.planted,
        "login": names.planted,
        "level": "user",
        "account": { "origin": LOCAL_ORIGIN, "externalId": names.local_account },
    });
    let answer = suite
        .client
        .call(UPSERT, planted, Credential::Session, &[])
        .await;
    suite.report.check(
        "keel.principal.upsert as the admin places a principal under the neighbour's origin in an account of another origin",
        answer.and_then(|answer| answer.data::<Value>().map(|_| ())),
    )?;
    let answer = bearer(suite, &link.token, SCOPED, Some(&headers.planted)).await;
    suite.report.check(
        "on behalf of that principal, in an account the neighbour does not own, answers 404 peer.error.customer",
        out_of_reach(answer),
    )?;
    let answer = bearer(suite, &link.token, SCOPED, Some(&headers.foreign)).await;
    suite.report.check(
        "on behalf under the origin of another instance answers 403 peer.error.origin",
        answer.and_then(|answer| {
            answer
                .error(403, ErrorCode::Forbidden, "peer.error.origin")
                .map(|_| ())
        }),
    )?;
    Ok((customer, account))
}

async fn deliveries(
    suite: &mut Suite,
    listener: &Listener,
    local: &InstanceInfo,
    cast: &Cast,
    link: &Link,
    customer: Id,
    account: Id,
) -> Result<(), Aborted> {
    let hit = suite.report.check(
        "the subscription made at pairing delivers keel.principal.created of the customer to the neighbour's hook",
        listener
            .wait_for("peer/a", "the customer's keel.principal.created", |hit| {
                hit.event()
                    .is_ok_and(|event| event.kind == PRINCIPAL_CREATED)
                    && hit.about(customer)
            })
            .await,
    )?;
    suite.report.check(
        "the delivery carries X-Keel-Peer, the event id, the link as actor and a signature under the link's secret",
        delivered(&hit, local, &link.principal, &link.secret),
    )?;
    let outcome = feed(suite, &link.token, &[]).await;
    suite.report.check(
        "the pull feed with the kpt_ token serves the customer's events in order under the masks of the pairing",
        outcome.and_then(|events| fed(&events, local, customer, account)),
    )?;
    let outcome = feed(suite, &link.token, &[SUBSCRIBE[1]]).await;
    suite.report.check(
        "the pull feed narrowed by types serves keel.principal.* alone",
        outcome.and_then(|events| {
            ensure(
                !events.is_empty()
                    && events
                        .iter()
                        .all(|event| event.kind.starts_with("keel.principal.")),
                || format!("the feed served {:?}", kinds(&events)),
            )
        }),
    )?;
    let answer = suite.client.get(FEED_PATH, Credential::Session, &[]).await;
    suite.report.check(
        "the pull feed refuses the admin's session with 403 peer.error.feed",
        answer.and_then(|answer| {
            answer
                .error(403, ErrorCode::Forbidden, "peer.error.feed")
                .map(|_| ())
        }),
    )?;
    let answer = suite.client.get(FEED_PATH, Credential::Guest, &[]).await;
    suite
        .report
        .check("the pull feed refuses a guest with 401", dead(answer))?;
    let instance = cast.first.id;
    let answer = deliver(
        suite,
        instance,
        &event(instance),
        link.secret.as_bytes(),
        now_seconds(),
    )
    .await;
    suite.report.check(
        "the installation's hook refuses what the link's own secret signs, since the receiver keeps it in the subscription alone",
        unsigned(answer),
    )?;
    Ok(())
}

async fn pair_second(
    suite: &mut Suite,
    local: &InstanceInfo,
    cast: &Cast,
    headers: &Headers,
) -> Result<(Id, String), Aborted> {
    let narrow = PeerScope {
        functions: vec![SCOPED.to_owned(), ADMIN_ONLY.to_owned()],
        level: Level::User,
        on_behalf: false,
    };
    let answer = pair_code(suite, &narrow).await;
    let code = suite.report.check(
        "peer.pair_code issues a second code at the level user without on-behalf",
        coded(answer),
    )?;
    let request = |identity: &Identity| {
        json!({
            "code": code,
            "instance": identity.instance(),
            "inboundToken": cast.back,
            "inboundScopes": { "functions": [], "level": "service" },
        })
    };
    let answer = guest(suite, PAIR, request(&cast.impostor)).await;
    suite.report.check(
        "peer.pair presenting the paired instance with another key answers 403 peer.error.instance_key",
        answer.and_then(|answer| {
            answer
                .error(403, ErrorCode::Forbidden, "peer.error.instance_key")
                .map(|_| ())
        }),
    )?;
    let answer = guest(suite, PAIR, request(&cast.first)).await;
    suite.report.check(
        "peer.pair presenting the paired instance with its key answers 409 peer.error.paired",
        answer.and_then(|answer| {
            answer
                .error(409, ErrorCode::Conflict, "peer.error.paired")
                .map(|_| ())
        }),
    )?;
    let answer = guest(suite, PAIR, request(&cast.second)).await;
    let (token, _) = suite.report.check(
        "the code the refused pairings left unused pairs the second neighbour, without a secret as it gave no address",
        paired(answer, local, &narrow, false),
    )?;
    let answer = suite
        .client
        .call(
            LIST,
            json!({ "search": cast.second.id }),
            Credential::Session,
            &[],
        )
        .await;
    let link = suite.report.check(
        "peer.list finds the second link without an address",
        answer.and_then(|answer| listed(&answer, &cast.second.instance(), None, &narrow)),
    )?;
    let answer = bearer(suite, &token, SCOPED, None).await;
    suite.report.check(
        "keel.account.list with the second token answers at the level user",
        answer.and_then(|answer| {
            let (_, meta) = answer.data::<Page<Value>>()?;
            let principal = meta
                .principal
                .ok_or_else(|| answer.failure("no meta.principal"))?;
            linked(&principal, link, Level::User).map_err(|failure| answer.failure(failure.0))
        }),
    )?;
    let answer = bearer(suite, &token, ADMIN_ONLY, None).await;
    suite.report.check(
        "keel.product.list, inside the masks but above the token's level, answers 403 keel.error.level",
        answer.and_then(|answer| {
            let error = answer.error(403, ErrorCode::Forbidden, "keel.error.level")?;
            ensure(error.params["level"] == json!("admin"), || {
                answer.failure("params.level is not admin").0
            })
        }),
    )?;
    let answer = bearer(suite, &token, SCOPED, Some(&headers.customer)).await;
    suite.report.check(
        "a token granted no on-behalf answers 403 keel.error.on_behalf to X-Keel-On-Behalf-Of",
        answer.and_then(|answer| {
            answer
                .error(403, ErrorCode::Forbidden, "keel.error.on_behalf")
                .map(|_| ())
        }),
    )?;
    let answer = guest(
        suite,
        RENEW,
        json!({ "token": token, "inboundToken": cast.fresh }),
    )
    .await;
    let renewed = suite.report.check(
        "peer.renew with the link's token answers a fresh kpt_ token",
        answer.and_then(|answer| {
            let (data, _) = answer.data::<Value>()?;
            let fresh = data["token"]
                .as_str()
                .filter(|fresh| is_peer_token(fresh) && *fresh != token)
                .ok_or_else(|| answer.failure("data.token is not a fresh kpt_ token"))?;
            Ok(fresh.to_owned())
        }),
    )?;
    let answer = bearer(suite, &token, SCOPED, None).await;
    suite
        .report
        .check("the token the renewal retired answers 401", dead(answer))?;
    let answer = bearer(suite, &renewed, SCOPED, None).await;
    suite.report.check(
        "the renewed token answers 200",
        answer.and_then(|answer| answer.data::<Page<Value>>().map(|_| ())),
    )?;
    Ok((link, renewed))
}

async fn revoke(suite: &mut Suite, link: &Link, headers: &Headers) -> Result<(), Aborted> {
    let answer = remove(suite, link.id).await;
    suite
        .report
        .check("peer.delete removes the first link", answer)?;
    let answer = bearer(suite, &link.token, SCOPED, None).await;
    suite
        .report
        .check("the token of the deleted link answers 401", dead(answer))?;
    let answer = bearer(suite, &link.token, SCOPED, Some(&headers.customer)).await;
    suite.report.check(
        "an on-behalf call with the deleted link's token answers 401",
        dead(answer),
    )?;
    let answer = suite
        .client
        .get(FEED_PATH, Credential::Bearer(link.token.clone()), &[])
        .await;
    suite.report.check(
        "the pull feed with the deleted link's token answers 401",
        dead(answer),
    )?;
    Ok(())
}

async fn initiating(
    suite: &mut Suite,
    listener: &Listener,
    local: &InstanceInfo,
    cast: &Cast,
) -> Result<(), Aborted> {
    let receiver = cast.receiver.id;
    let secret = cast.secret.as_bytes();
    let scopes = json!({ "functions": [], "level": "service", "onBehalf": false });
    let pair_answer = json!({
        "peerId": receiver,
        "token": cast.answered,
        "scopes": scopes,
        SECRET_MEMBER: cast.secret,
    });
    listener.neighbour(
        "c",
        Neighbour {
            identity: cast.receiver.clone(),
            answer: pair_answer.clone(),
        },
    );
    let url = listener.peer_url("c");
    let answer = create(suite, &url).await;
    let link = suite.report.check(
        "peer.create reads the neighbour's handshake, pairs with it and answers the link",
        answer.and_then(|answer| {
            let (data, _) = answer.data::<Value>()?;
            ensure(data["instanceId"] == json!(receiver), || {
                answer.failure("instanceId is not the neighbour's").0
            })?;
            ensure(
                data["publicKey"] == json!(cast.receiver.instance().public_key),
                || {
                    answer
                        .failure("the link does not pin the neighbour's key")
                        .0
                },
            )?;
            ensure(data["url"] == json!(url), || {
                answer
                    .failure("the link does not keep the neighbour's address")
                    .0
            })?;
            ensure(data["granted"] == scopes, || {
                answer
                    .failure("granted is not the scopes the neighbour answered")
                    .0
            })?;
            super::id_of(&data, "id").map_err(|failure| answer.failure(failure.0))
        }),
    )?;
    let pairings = listener.pairings("c");
    let handed = suite.report.check(
        "the pairing request carries the code, the installation's handshake, its address, a kpt_ token and the masks",
        pairing(&pairings, NEIGHBOUR_CODE, local),
    )?;
    listener.neighbour(
        "d",
        Neighbour {
            identity: cast.pretender.clone(),
            answer: pair_answer,
        },
    );
    let answer = create(suite, &listener.peer_url("d")).await;
    suite.report.check(
        "peer.create to an address presenting the paired neighbour under another key answers 403 peer.error.instance_key and sends it no pairing request",
        answer.and_then(|answer| {
            answer.error(403, ErrorCode::Forbidden, "peer.error.instance_key")?;
            let sent = listener.pairings("d").len();
            ensure(sent == 0, || {
                answer
                    .failure(format!("the installation sent {sent} pairing requests to the impostor"))
                    .0
            })
        }),
    )?;
    let answer = bearer(suite, &handed, SCOPED, None).await;
    suite.report.check(
        "keel.account.list with the token handed to the neighbour answers 403 keel.error.level, the default scopes granting the level service",
        answer.and_then(|answer| {
            let error = answer.error(403, ErrorCode::Forbidden, "keel.error.level")?;
            ensure(error.params["level"] == json!("user"), || {
                answer.failure("params.level is not user").0
            })
        }),
    )?;
    let answer = bearer(suite, &handed, AT_SERVICE, None).await;
    suite.report.check(
        "auth.logout, within the level service but outside the empty functions of that token, answers 403 keel.error.token_scope",
        answer.and_then(|answer| {
            let error = answer.error(403, ErrorCode::Forbidden, "keel.error.token_scope")?;
            ensure(error.params["function"] == json!(AT_SERVICE), || {
                answer
                    .failure("params.function does not name the function")
                    .0
            })
        }),
    )?;
    let outcome = Stream::open(suite, None).await;
    let mut stream = suite.report.check(
        "the admin's session opens the event stream to watch the neighbour's event arrive",
        outcome,
    )?;
    let probe = event(receiver);
    let answer = deliver(suite, receiver, &probe, secret, now_seconds()).await;
    suite.report.check(
        "the installation's hook takes an event signed with the link's secret and answers 200 at once",
        hooked(answer, probe.id, false),
    )?;
    let outcome = stream.arrival(probe.id).await;
    suite.report.check(
        "the event reaches the local bus and the admin's stream under its own id and the neighbour's instance",
        outcome.and_then(|arrived| {
            ensure(
                arrived.instance_id == receiver
                    && arrived.kind == RELAYED_TYPE
                    && arrived.module == RELAYED_MODULE,
                || format!("the stream carries {arrived:?}"),
            )
        }),
    )?;
    let answer = deliver(suite, receiver, &probe, secret, now_seconds()).await;
    suite.report.check(
        "a repeated delivery of the event answers 200 as a duplicate",
        hooked(answer, probe.id, true),
    )?;
    let outcome = stream.absent(probe.id, QUIET).await;
    suite
        .report
        .check("the repeat creates no second event on the bus", outcome)?;
    drop(stream);
    let refusals: [(&str, Id, &[u8], i64); 3] = [
        (
            "a delivery signed with another secret answers 401 peer.error.signature",
            receiver,
            b"whs_not_the_secret",
            now_seconds(),
        ),
        (
            "a delivery signed outside the 300 second window answers 401 peer.error.signature",
            receiver,
            secret,
            now_seconds() - WINDOW_SECONDS - 1,
        ),
        (
            "a delivery naming an instance no link pins answers 401 peer.error.signature",
            Id::generate(),
            secret,
            now_seconds(),
        ),
    ];
    for (name, peer, key, t) in refusals {
        let answer = deliver(suite, peer, &event(receiver), key, t).await;
        suite.report.check(name, unsigned(answer))?;
    }
    let answer = deliver(
        suite,
        receiver,
        &event(Id::generate()),
        secret,
        now_seconds(),
    )
    .await;
    suite.report.check(
        "a signed event of another instance answers 422 peer.error.event",
        answer.and_then(|answer| {
            let error = answer.error(422, ErrorCode::Validation, "peer.error.event")?;
            ensure(error.params["reason"] == json!("instance"), || {
                answer.failure("params.reason is not instance").0
            })
        }),
    )?;
    let answer = remove(suite, link).await;
    suite.report.check(
        "peer.delete removes the link the installation initiated",
        answer,
    )?;
    let answer = deliver(suite, receiver, &event(receiver), secret, now_seconds()).await;
    suite.report.check(
        "a delivery for the deleted link answers 401 peer.error.signature",
        unsigned(answer),
    )?;
    let answer = bearer(suite, &handed, SCOPED, None).await;
    suite.report.check(
        "the token handed to the neighbour of the deleted link answers 401",
        dead(answer),
    )?;
    Ok(())
}

async fn create(suite: &mut Suite, url: &str) -> Result<Answer, Failure> {
    suite
        .client
        .call(
            CREATE,
            json!({ "url": url, "code": NEIGHBOUR_CODE, "subscribe": [RELAYED_MASK] }),
            Credential::Session,
            &[],
        )
        .await
}

async fn guest(suite: &mut Suite, func: &str, params: Value) -> Result<Answer, Failure> {
    suite
        .client
        .call(func, params, Credential::Guest, &[])
        .await
        .map(Answer::confidential)
}

async fn bearer(
    suite: &mut Suite,
    token: &str,
    func: &str,
    on_behalf: Option<&str>,
) -> Result<Answer, Failure> {
    let extra: Vec<(HeaderName, &str)> = on_behalf
        .into_iter()
        .map(|header| (ON_BEHALF_OF, header))
        .collect();
    suite
        .client
        .call(
            func,
            json!({}),
            Credential::Bearer(token.to_owned()),
            &extra,
        )
        .await
}

async fn pair_code(suite: &mut Suite, scope: &PeerScope) -> Result<Answer, Failure> {
    suite
        .client
        .call(
            PAIR_CODE,
            json!({
                "scopes": scope.functions,
                "level": scope.level,
                "onBehalf": scope.on_behalf,
                "ttl": TTL,
            }),
            Credential::Session,
            &[],
        )
        .await
        .map(Answer::confidential)
}

async fn remove(suite: &mut Suite, link: Id) -> Result<(), Failure> {
    let answer = suite
        .client
        .call(DELETE, json!({ "id": link }), Credential::Session, &[])
        .await?;
    let (data, _) = answer.data::<Value>()?;
    ensure(data["deleted"] == json!(true), || {
        answer.failure("data.deleted is not true").0
    })
}

async fn feed(suite: &mut Suite, token: &str, types: &[&str]) -> Result<Vec<Event>, Failure> {
    let mut stream = suite.client.feed(token, 0)?;
    if !types.is_empty() {
        stream = stream.types(types.iter().copied());
    }
    let collect = async move {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.map_err(|error| fail(format!("the feed failed: {error}")))?);
        }
        Ok(events)
    };
    tokio::time::timeout(FEED_WAIT, collect)
        .await
        .map_err(|_| fail(format!("the feed did not end within {FEED_WAIT:?}")))?
}

async fn deliver(
    suite: &mut Suite,
    peer: Id,
    event: &Event,
    secret: &[u8],
    t: i64,
) -> Result<Answer, Failure> {
    let body = serde_json::to_value(event)?;
    let bytes = serde_json::to_vec(&body)?;
    let signed = signature::sign(secret, t, &bytes)?;
    let peer = peer.to_string();
    let id = event.id.to_string();
    let headers = [
        (HeaderName::from_static(PEER_HEADER), peer.as_str()),
        (HeaderName::from_static(EVENT_ID_HEADER), id.as_str()),
        (HeaderName::from_static(SIGNATURE_HEADER), signed.as_str()),
    ];
    suite
        .client
        .post(HOOK_PATH, body, Credential::Guest, &headers)
        .await
}

pub fn event(instance: Id) -> Event {
    Event {
        id: Id::generate(),
        seq: 1,
        kind: RELAYED_TYPE.to_owned(),
        at: chrono::Utc::now(),
        instance_id: instance,
        module: RELAYED_MODULE.to_owned(),
        actor: None,
        subject: None,
        data: json!({}),
    }
}

pub fn on_behalf(origin: &str, external_id: &str) -> Result<String, Failure> {
    OnBehalf::new(origin, external_id)
        .map(|on_behalf| on_behalf.header())
        .map_err(|error| fail(format!("the on-behalf header is refused: {error}")))
}

pub fn is_code(code: &str) -> bool {
    let Some(rest) = code.strip_prefix(CODE_PREFIX) else {
        return false;
    };
    let groups: Vec<&str> = rest.split('-').collect();
    groups.len() == 2
        && groups.iter().all(|group| {
            group.len() == 4 && group.chars().all(|symbol| CODE_SYMBOLS.contains(symbol))
        })
}

pub fn handshake(instance: &InstanceInfo) -> Result<(), Failure> {
    ensure(instance.protocol == PROTOCOL, || {
        format!("the protocol is {}, not {PROTOCOL}", instance.protocol)
    })?;
    ensure(!instance.keel.is_empty(), || "keel is empty".to_owned())?;
    ensure(!instance.products.is_empty(), || {
        "no product is enabled".to_owned()
    })?;
    ensure(instance.public_key.starts_with(PUBLIC_KEY_SCHEME), || {
        format!(
            "the public key {} is not {PUBLIC_KEY_SCHEME}…",
            instance.public_key
        )
    })?;
    ensure(
        instance.features.iter().any(|feature| feature == FEATURE),
        || format!("the features {:?} lack {FEATURE}", instance.features),
    )?;
    for list in [
        &instance.products,
        &instance.features,
        &instance.service_kinds,
    ] {
        let mut sorted = list.clone();
        sorted.sort_unstable();
        sorted.dedup();
        ensure(&sorted == list, || {
            format!("{list:?} is not sorted without repeats")
        })?;
    }
    Ok(())
}

fn coded(answer: Result<Answer, Failure>) -> Result<String, Failure> {
    let answer = answer?;
    let (data, _) = answer.data::<Value>()?;
    let code = data["code"]
        .as_str()
        .filter(|code| is_code(code))
        .ok_or_else(|| answer.failure("data.code is not a PAIR-XXXX-XXXX code"))?;
    ensure(data["expiresAt"].is_string(), || {
        answer.failure("no expiresAt").0
    })?;
    Ok(code.to_owned())
}

fn paired(
    answer: Result<Answer, Failure>,
    local: &InstanceInfo,
    granted: &PeerScope,
    subscribed: bool,
) -> Result<(String, Option<String>), Failure> {
    let answer = answer?;
    let (data, _) = answer.data::<Value>()?;
    ensure(data["peerId"] == json!(local.instance_id), || {
        answer.failure("peerId is not the receiver's instance").0
    })?;
    let token = data["token"]
        .as_str()
        .filter(|token| is_peer_token(token))
        .ok_or_else(|| answer.failure("data.token is not a kpt_ token"))?;
    let scopes: PeerScope = serde_json::from_value(data["scopes"].clone())
        .map_err(|error| answer.failure(format!("data.scopes do not fit: {error}")))?;
    ensure(&scopes == granted, || {
        answer.failure(format!("the scopes are {scopes:?}")).0
    })?;
    let secret = match &data[SECRET_MEMBER] {
        Value::Null => None,
        Value::String(secret) if secret.starts_with(WEBHOOK_SECRET_PREFIX) => Some(secret.clone()),
        _ => return Err(answer.failure("webhookSecret is not a whs_ secret")),
    };
    ensure(secret.is_some() == subscribed, || {
        answer
            .failure("a webhook secret answers a pairing exactly when it gave an address and masks")
            .0
    })?;
    Ok((token.to_owned(), secret))
}

fn listed(
    answer: &Answer,
    neighbour: &InstanceInfo,
    url: Option<&str>,
    granted: &PeerScope,
) -> Result<Id, Failure> {
    let (page, _) = answer.data::<Page<Value>>()?;
    let row = page
        .data
        .first()
        .filter(|_| page.total == 1)
        .ok_or_else(|| answer.failure(format!("{} links", page.total)))?;
    ensure(row["instanceId"] == json!(neighbour.instance_id), || {
        answer.failure("instanceId is not the neighbour's").0
    })?;
    ensure(row["publicKey"] == json!(neighbour.public_key), || {
        answer
            .failure("the link does not pin the neighbour's key")
            .0
    })?;
    ensure(row["url"] == json!(url), || {
        answer
            .failure("the link does not keep the neighbour's address")
            .0
    })?;
    ensure(row["scopes"] == json!(granted), || {
        answer
            .failure("the link does not keep the granted scopes")
            .0
    })?;
    ensure(row["status"] == json!("active"), || {
        answer.failure("the link is not active").0
    })?;
    super::id_of(row, "id").map_err(|failure| answer.failure(failure.0))
}

pub fn linked(principal: &MetaPrincipal, link: Id, level: Level) -> Result<(), Failure> {
    let login = format!("{LOGIN_PREFIX}{}", link.to_string().to_lowercase());
    ensure(principal.login == login, || {
        format!("the principal is {}, not {login}", principal.login)
    })?;
    ensure(principal.level == level, || {
        format!(
            "the principal's level is {:?}, not {level:?}",
            principal.level
        )
    })?;
    ensure(principal.account == Id::ROOT_ACCOUNT, || {
        format!("the link's principal lives in {}", principal.account)
    })?;
    ensure(principal.actor.is_none(), || {
        "the link calling as itself carries an actor".to_owned()
    })
}

pub fn acting(
    principal: &MetaPrincipal,
    customer: &str,
    link: &MetaPrincipal,
) -> Result<(), Failure> {
    ensure(principal.login == customer, || {
        format!(
            "the principal is {}, not the customer {customer}",
            principal.login
        )
    })?;
    let actor = principal
        .actor
        .as_ref()
        .ok_or_else(|| fail("the call on behalf carries no actor"))?;
    ensure(
        actor.id == link.id && actor.login == link.login && actor.via == ActorVia::Peer,
        || {
            format!(
                "the actor is {} via {:?}, not the link via peer",
                actor.login, actor.via
            )
        },
    )
}

fn delivered(
    hit: &Received,
    local: &InstanceInfo,
    link: &MetaPrincipal,
    secret: &str,
) -> Result<(), Failure> {
    let event = hit.event()?;
    let local_id = local.instance_id.to_string();
    ensure(hit.header(PEER_HEADER) == Some(local_id.as_str()), || {
        format!(
            "X-Keel-Peer is {:?}, not {local_id}",
            hit.header(PEER_HEADER)
        )
    })?;
    ensure(
        hit.header(EVENT_ID_HEADER) == Some(event.id.to_string().as_str()),
        || format!("X-Keel-Event-Id is {:?}", hit.header(EVENT_ID_HEADER)),
    )?;
    ensure(event.instance_id == local.instance_id, || {
        format!("the event names the instance {}", event.instance_id)
    })?;
    ensure(
        event
            .actor
            .as_ref()
            .is_some_and(|actor| actor.kind == EventActorKind::Peer && actor.id == link.id),
        || format!("the actor is {:?}, not the link as a peer", event.actor),
    )?;
    signature::verify(
        secret.as_bytes(),
        hit.header(SIGNATURE_HEADER).unwrap_or_default(),
        &hit.body,
        now_seconds(),
    )
}

pub fn fed(
    events: &[Event],
    local: &InstanceInfo,
    customer: Id,
    account: Id,
) -> Result<(), Failure> {
    let about = |kind: &str, id: Id| {
        events.iter().any(|event| {
            event.kind == kind
                && event
                    .subject
                    .as_ref()
                    .is_some_and(|subject| subject.id == id)
        })
    };
    ensure(
        about(ACCOUNT_CREATED, account) && about(PRINCIPAL_CREATED, customer),
        || {
            format!(
                "the feed lacks the customer's created events: {:?}",
                kinds(events)
            )
        },
    )?;
    ensure(
        events.iter().all(|event| {
            event.instance_id == local.instance_id
                && SUBSCRIBE
                    .iter()
                    .any(|mask| event.kind.starts_with(mask.trim_end_matches('*')))
        }),
        || {
            format!(
                "the feed serves {:?} beyond the masks or the instance",
                kinds(events)
            )
        },
    )?;
    ensure(
        events.windows(2).all(|pair| pair[0].seq < pair[1].seq),
        || "the feed is not in the order of seq".to_owned(),
    )
}

fn kinds(events: &[Event]) -> Vec<&str> {
    events.iter().map(|event| event.kind.as_str()).collect()
}

fn pairing(pairings: &[Value], code: &str, local: &InstanceInfo) -> Result<String, Failure> {
    let [request] = pairings else {
        return Err(fail(format!(
            "the neighbour received {} pairing requests",
            pairings.len()
        )));
    };
    ensure(request["code"] == json!(code), || {
        "the code differs".to_owned()
    })?;
    let instance: InstanceInfo = serde_json::from_value(request["instance"].clone())
        .map_err(|error| fail(format!("the instance does not fit: {error}")))?;
    ensure(
        instance.instance_id == local.instance_id && instance.public_key == local.public_key,
        || "the instance is not the installation's handshake".to_owned(),
    )?;
    ensure(
        request["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://")),
        || "the request carries no address of the installation; set auth.shell_url".to_owned(),
    )?;
    let token = request["inboundToken"]
        .as_str()
        .filter(|token| is_peer_token(token))
        .ok_or_else(|| fail("inboundToken is not a kpt_ token"))?;
    let scopes: PeerScope = serde_json::from_value(request["inboundScopes"].clone())
        .map_err(|error| fail(format!("inboundScopes do not fit: {error}")))?;
    ensure(
        scopes
            == PeerScope {
                functions: Vec::new(),
                level: Level::Service,
                on_behalf: false,
            },
        || format!("inboundScopes are {scopes:?}, not the defaults"),
    )?;
    ensure(request["subscribe"] == json!([RELAYED_MASK]), || {
        format!("subscribe is {}", request["subscribe"])
    })?;
    Ok(token.to_owned())
}

fn hooked(answer: Result<Answer, Failure>, id: Id, duplicate: bool) -> Result<(), Failure> {
    let answer = answer?;
    answer.status_is(200)?;
    let body = answer.json()?;
    ensure(
        body == json!({ "data": { "eventId": id, "duplicate": duplicate } }),
        || {
            answer
                .failure(format!("the answer is not duplicate {duplicate}"))
                .0
        },
    )
}

fn out_of_reach(answer: Result<Answer, Failure>) -> Result<(), Failure> {
    let answer = answer?;
    answer
        .error(404, ErrorCode::NotFound, "peer.error.customer")
        .map(|_| ())
}

fn unsigned(answer: Result<Answer, Failure>) -> Result<(), Failure> {
    let answer = answer?;
    answer
        .error(401, ErrorCode::Unauthenticated, "peer.error.signature")
        .map(|_| ())
}

fn dead(answer: Result<Answer, Failure>) -> Result<(), Failure> {
    let answer = answer?;
    answer
        .error(401, ErrorCode::Unauthenticated, UNAUTHENTICATED)
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use keel_types::{EventSubject, MetaActor};

    use super::*;

    #[test]
    fn a_pair_code_is_two_groups_of_four_symbols_without_i_l_o_or_u() {
        assert!(is_code("PAIR-7K2Q-91XD"));
        for refused in [
            "PAIR-7K2Q91XD",
            "PAIR-7K2Q-91X",
            "PAIR-7K2I-91XD",
            "pair-7k2q-91xd",
            "CODE-7K2Q-91XD",
            "PAIR-7K2Q-91XD-0000",
        ] {
            assert!(!is_code(refused), "{refused}");
        }
    }

    #[test]
    fn the_handshake_is_refused_without_the_protocol_a_product_the_key_scheme_or_the_peers_feature()
    {
        let mut good = Identity::new(Id::generate(), "0.3.0").unwrap().instance();
        good.products = vec!["dns".into(), "panel".into()];
        good.features = vec!["peers".into(), "svc".into()];
        good.service_kinds = vec!["dns.zones".into(), "hosting.sites".into()];
        handshake(&good).unwrap();
        let mut broken = Vec::new();
        let mut copy = good.clone();
        copy.protocol = PROTOCOL + 1;
        broken.push(copy);
        let mut copy = good.clone();
        copy.products.clear();
        broken.push(copy);
        let mut copy = good.clone();
        copy.public_key = "rsa:x".into();
        broken.push(copy);
        let mut copy = good.clone();
        copy.features = vec!["svc".into()];
        broken.push(copy);
        let mut copy = good.clone();
        copy.service_kinds = vec!["hosting.sites".into(), "dns.zones".into()];
        broken.push(copy);
        for instance in broken {
            assert!(handshake(&instance).is_err(), "{instance:?}");
        }
    }

    fn principal(login: &str, actor: Option<MetaActor>) -> MetaPrincipal {
        MetaPrincipal {
            id: Id::generate(),
            login: login.into(),
            level: Level::User,
            account: Id::generate(),
            actor,
        }
    }

    #[test]
    fn an_on_behalf_call_runs_as_the_customer_with_the_link_as_actor_via_peer_and_nothing_else() {
        let link = Id::generate();
        let mut me = principal(&format!("peer-{}", link.to_string().to_lowercase()), None);
        me.account = Id::ROOT_ACCOUNT;
        me.level = Level::Admin;
        linked(&me, link, Level::Admin).unwrap();
        assert!(linked(&me, link, Level::User).is_err());
        assert!(linked(&me, Id::generate(), Level::Admin).is_err());
        let actor = |via: ActorVia, id: Id| MetaActor {
            id,
            login: me.login.clone(),
            level: Level::Admin,
            via,
        };
        acting(
            &principal("alice", Some(actor(ActorVia::Peer, me.id))),
            "alice",
            &me,
        )
        .unwrap();
        assert!(
            acting(
                &principal("alice", Some(actor(ActorVia::Su, me.id))),
                "alice",
                &me
            )
            .is_err()
        );
        assert!(
            acting(
                &principal("alice", Some(actor(ActorVia::Peer, Id::generate()))),
                "alice",
                &me
            )
            .is_err()
        );
        assert!(acting(&principal("alice", None), "alice", &me).is_err());
        assert!(
            acting(
                &principal("bob", Some(actor(ActorVia::Peer, me.id))),
                "alice",
                &me
            )
            .is_err()
        );
    }

    #[test]
    fn the_feed_must_hold_the_customers_created_events_in_order_within_the_masks_and_the_instance()
    {
        let local = Identity::new(Id::generate(), "0.3.0").unwrap().instance();
        let customer = Id::generate();
        let account = Id::generate();
        let at = |seq: u64, kind: &str, subject: Id| {
            let mut event = event(local.instance_id);
            event.seq = seq;
            event.kind = kind.to_owned();
            event.subject = Some(EventSubject {
                kind: kind
                    .rsplit_once('.')
                    .map(|(entity, _)| entity)
                    .unwrap_or(kind)
                    .to_owned(),
                id: subject,
                account_id: Some(account),
            });
            event
        };
        let good = [
            at(3, ACCOUNT_CREATED, account),
            at(4, PRINCIPAL_CREATED, customer),
        ];
        fed(&good, &local, customer, account).unwrap();
        assert!(fed(&good[..1], &local, customer, account).is_err());
        assert!(
            fed(
                &[good[1].clone(), good[0].clone()],
                &local,
                customer,
                account
            )
            .is_err()
        );
        let mut foreign = good.to_vec();
        foreign.push(at(5, "webhook.subscription.saved", Id::generate()));
        assert!(fed(&foreign, &local, customer, account).is_err());
        let mut elsewhere = good.to_vec();
        let mut relayed = at(6, PRINCIPAL_CREATED, Id::generate());
        relayed.instance_id = Id::generate();
        elsewhere.push(relayed);
        assert!(fed(&elsewhere, &local, customer, account).is_err());
    }

    #[test]
    fn the_pairing_request_must_carry_the_code_the_handshake_an_address_a_token_and_the_masks() {
        let local = Identity::new(Id::generate(), "0.3.0").unwrap().instance();
        let token = neighbour::peer_token().unwrap();
        let good = json!({
            "code": "PAIR-CONF-0001",
            "instance": local,
            "url": "https://127.0.0.1:8443",
            "inboundToken": token,
            "inboundScopes": { "functions": [], "level": "service", "onBehalf": false },
            "subscribe": [RELAYED_MASK],
        });
        assert_eq!(
            pairing(std::slice::from_ref(&good), "PAIR-CONF-0001", &local).unwrap(),
            token
        );
        assert!(pairing(&[], "PAIR-CONF-0001", &local).is_err());
        assert!(pairing(&[good.clone(), good.clone()], "PAIR-CONF-0001", &local).is_err());
        assert!(pairing(std::slice::from_ref(&good), "PAIR-CONF-0002", &local).is_err());
        for (member, value) in [
            ("url", Value::Null),
            ("inboundToken", json!("kpt_short")),
            ("subscribe", json!([])),
            (
                "inboundScopes",
                json!({ "functions": ["svc.*"], "level": "service" }),
            ),
        ] {
            let mut broken = good.clone();
            broken[member] = value;
            let refused = pairing(&[broken], "PAIR-CONF-0001", &local);
            assert!(refused.is_err(), "{member}");
        }
        let mut other = good;
        other["instance"] = json!(Identity::new(Id::generate(), "0.3.0").unwrap().instance());
        assert!(pairing(&[other], "PAIR-CONF-0001", &local).is_err());
    }
}
