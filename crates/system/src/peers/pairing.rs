use keel_sdk::chrono::{DateTime, Duration, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::OnConflict;
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::serde_json;
use keel_sdk::system::{
    Caller, CallerOptions, Features, NewPrincipal, PrincipalSource, SecretCipher,
};
use keel_sdk::types::{InstanceInfo, PROTOCOL, PeerScope, Scope, is_mask};
use rand::TryRng;
use rand::rngs::SysRng;

use super::codes;
use super::continuation;
use super::entity::{pair_code, product};
use super::functions::{EVENT_CREATED, Peer, STATUS_ACTIVE, by_instance, peer_of};
use super::remote::{Remote, base_of, checked};
use crate::auth::api::{SETTING_SHELL_URL, is_peer_token, peer_token, token_hash};

pub const PAIR_CODE: &str = "peer.pair_code";
pub const PAIR: &str = "peer.pair";
pub const CREATE: &str = "peer.create";
pub const LINK: &str = "peer.link";
pub const TTL_DEFAULT: u32 = 600;
pub const TTL_MIN: u32 = 60;
pub const TTL_MAX: u32 = 86_400;
pub const MAX_MASKS: usize = 64;
pub const LOGIN_PREFIX: &str = "peer-";
pub const ROLE: &str = "peer";
pub const LANG: &str = "en";
pub const WEBHOOK_SECRET_PREFIX: &str = "whs_";
pub const ERROR_CODE: &str = "peer.error.code";
pub const ERROR_MASK: &str = "peer.error.mask";
pub const ERROR_LEVEL: &str = "peer.error.level";
pub const ERROR_SCOPE: &str = "peer.error.scope";
pub const ERROR_SELF: &str = "peer.error.self";
pub const ERROR_PROTOCOL: &str = "peer.error.protocol";
pub const ERROR_INSTANCE_KEY: &str = "peer.error.instance_key";
pub const ERROR_PAIRED: &str = "peer.error.paired";
pub const ERROR_TOKEN: &str = "peer.error.token";
const SECRET_BYTES: usize = 32;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct PairCodeIssue {
    #[serde(default)]
    #[schemars(length(max = MAX_MASKS), extend("x-ui" = json!({ "control": "tags" })))]
    scopes: Vec<String>,
    level: Option<Level>,
    #[serde(default)]
    on_behalf: bool,
    #[schemars(range(min = TTL_MIN, max = TTL_MAX))]
    ttl: Option<u32>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct PairCode {
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    code: String,
    expires_at: DateTime<Utc>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct PairRequest {
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    code: Secret,
    instance: InstanceInfo,
    #[serde(default)]
    #[schemars(length(min = 1, max = 2048))]
    url: Option<String>,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    inbound_token: Secret,
    inbound_scopes: PeerScope,
    #[serde(default)]
    #[schemars(length(max = MAX_MASKS))]
    subscribe: Vec<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct PairAnswer {
    peer_id: Id,
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    token: String,
    scopes: PeerScope,
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    webhook_secret: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct PairCreate {
    #[schemars(url, length(min = 1, max = 2048))]
    url: String,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    code: Secret,
    #[serde(default)]
    #[schemars(length(max = MAX_MASKS), extend("x-ui" = json!({ "control": "tags" })))]
    subscribe: Vec<String>,
    #[serde(default)]
    #[schemars(length(max = MAX_MASKS), extend("x-ui" = json!({ "control": "tags" })))]
    scopes: Vec<String>,
    level: Option<Level>,
    #[serde(default)]
    on_behalf: bool,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct Paired {
    #[serde(flatten)]
    peer: Peer,
    granted: PeerScope,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct Link {
    instance: InstanceInfo,
    #[schemars(length(min = 1, max = 2048))]
    url: String,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    inbound_token: Secret,
    inbound_scopes: PeerScope,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    outbound_token: Secret,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    webhook_secret: Option<Secret>,
}

pub fn compatible(protocol: u32) -> bool {
    protocol.abs_diff(PROTOCOL) <= 1
}

pub(super) fn grant(
    functions: Vec<String>,
    level: Level,
    on_behalf: bool,
    ceiling: Level,
    paths: (&str, &str),
) -> Result<PeerScope> {
    let (masks_path, level_path) = paths;
    for (index, mask) in functions.iter().enumerate() {
        if !is_mask(mask) {
            return Err(KeelError::validation(ERROR_MASK)
                .field(format!("{masks_path}[{index}]"))
                .param("mask", mask.as_str()));
        }
    }
    let top = ceiling.min(Level::Admin);
    if level < Level::Service || level > top {
        return Err(KeelError::validation(ERROR_LEVEL)
            .field(level_path)
            .param("level", level.as_str())
            .param("ceiling", top.as_str()));
    }
    Ok(PeerScope {
        functions,
        level,
        on_behalf,
    })
}

fn granted_by(caller: &Principal, scope: &PeerScope) -> Result<()> {
    if caller.scope.contains(&Scope::Peer(scope.clone())) {
        Ok(())
    } else {
        Err(KeelError::forbidden(ERROR_SCOPE))
    }
}

fn events(subscribe: &[String]) -> Result<()> {
    for (index, mask) in subscribe.iter().enumerate() {
        if !is_mask(mask) {
            return Err(KeelError::validation(ERROR_MASK)
                .field(format!("subscribe[{index}]"))
                .param("mask", mask.as_str()));
        }
    }
    Ok(())
}

fn protocol_refused(protocol: u32) -> KeelError {
    KeelError::conflict(ERROR_PROTOCOL)
        .param("protocol", protocol)
        .param("supported", PROTOCOL)
}

pub(super) fn tofu(existing: &product::Model, presented: &InstanceInfo) -> KeelError {
    if existing.public_key != presented.public_key {
        KeelError::forbidden(ERROR_INSTANCE_KEY).param("instanceId", existing.instance_id.as_str())
    } else {
        KeelError::conflict(ERROR_PAIRED).param("instanceId", existing.instance_id.as_str())
    }
}

fn webhook_secret() -> Result<String> {
    let mut bytes = [0u8; SECRET_BYTES];
    SysRng.try_fill_bytes(&mut bytes).map_err(|error| {
        KeelError::internal(codes::ERROR_ENTROPY).source(std::io::Error::other(error.to_string()))
    })?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{WEBHOOK_SECRET_PREFIX}{hex}"))
}

struct NewLink<'a> {
    instance: &'a InstanceInfo,
    url: Option<String>,
    inbound_token: &'a str,
    inbound_scopes: &'a PeerScope,
    outbound_token: &'a str,
    webhook_secret: Option<&'a str>,
}

async fn refused_by_existing(tx: &Db, instance: &str, presented: &InstanceInfo) -> Result<()> {
    match by_instance(tx, instance).await? {
        Some(existing) => Err(tofu(&existing, presented)),
        None => Ok(()),
    }
}

async fn insert_link<P>(call: &Call<P>, new: NewLink<'_>) -> Result<product::Model> {
    let tx = call.tx()?;
    let instance = new.instance.instance_id.to_string();
    refused_by_existing(tx, &instance, new.instance).await?;
    let now = call.require::<dyn Clock>()?.now();
    let cipher = call.require::<dyn SecretCipher>()?;
    let id = Id::generate();
    let principal = call
        .require::<dyn PrincipalSource>()?
        .create(
            NewPrincipal {
                account: Id::ROOT_ACCOUNT,
                login: format!("{LOGIN_PREFIX}{}", id.to_string().to_lowercase()),
                level: Level::Service,
                role: ROLE.to_owned(),
                lang: LANG.to_owned(),
            },
            now,
            tx,
        )
        .await?;
    let webhook_secret_enc = new
        .webhook_secret
        .map(|secret| cipher.seal(secret.as_bytes()))
        .transpose()?;
    let inserted = product::Entity::insert(product::ActiveModel {
        id: Set(id.to_string()),
        instance_id: Set(instance.clone()),
        products_json: Set(serde_json::to_value(&new.instance.products)?),
        keel_version: Set(new.instance.keel.clone()),
        protocol: Set(i32::try_from(new.instance.protocol).unwrap_or(i32::MAX)),
        url: Set(new.url),
        public_key: Set(new.instance.public_key.clone()),
        inbound_token_hash: Set(token_hash(new.inbound_token)),
        inbound_scopes_json: Set(serde_json::to_value(new.inbound_scopes)?),
        outbound_token_enc: Set(cipher.seal(new.outbound_token.as_bytes())?),
        webhook_secret_enc: Set(webhook_secret_enc),
        status: Set(STATUS_ACTIVE.to_owned()),
        last_seen_at: Set(Some(now)),
        created_at: Set(now),
        principal_id: Set(Some(principal.id.to_string())),
    })
    .on_conflict(
        OnConflict::column(product::Column::InstanceId)
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(tx)
    .await?;
    if inserted == 0 {
        refused_by_existing(tx, &instance, new.instance).await?;
        return Err(KeelError::conflict(ERROR_PAIRED).param("instanceId", instance));
    }
    let row = product::Entity::find_by_id(id.to_string())
        .one(tx)
        .await?
        .ok_or_else(super::functions::not_found)?;
    call.emit(
        EVENT_CREATED,
        &json!({ "id": row.id, "instanceId": row.instance_id }),
    )?;
    Ok(row)
}

pub(super) fn pair_code() -> FunctionDef {
    function::<PairCodeIssue, PairCode>(PAIR_CODE)
        .kind(Kind::Form)
        .level(Level::Admin)
        .unowned()
        .ui(Form::new("peer.pair_code.title").page("main", ["scopes", "level", "onBehalf", "ttl"]))
        .handler(|call: Call<PairCodeIssue>| async move {
            let params = call.params();
            let caller = call.principal();
            let scope = grant(
                params.scopes.clone(),
                params.level.unwrap_or(Level::Service),
                params.on_behalf,
                caller.level,
                ("scopes", "level"),
            )?;
            granted_by(caller, &scope)?;
            let now = call.require::<dyn Clock>()?.now();
            let ttl = params.ttl.unwrap_or(TTL_DEFAULT);
            let code = codes::generate()?;
            let expires_at = now + Duration::seconds(i64::from(ttl));
            pair_code::ActiveModel {
                id: Set(Id::generate().to_string()),
                code_hash: Set(codes::hash(code.expose())),
                scopes_json: Set(serde_json::to_value(&scope)?),
                expires_at: Set(expires_at),
                created_by: Set(caller.id.to_string()),
                created_at: Set(now),
            }
            .insert(call.tx()?)
            .await?;
            Ok(PairCode {
                code: code.into_inner(),
                expires_at,
            })
        })
}

pub(super) fn pair() -> FunctionDef {
    function::<PairRequest, PairAnswer>(PAIR)
        .kind(Kind::Action)
        .level(Level::Guest)
        .unowned()
        .handler(|call: Call<PairRequest>| async move {
            let params = call.params();
            let local = call.require::<InstanceInfo>()?;
            if !is_peer_token(params.inbound_token.expose()) {
                return Err(KeelError::validation(ERROR_TOKEN).field("inboundToken"));
            }
            grant(
                params.inbound_scopes.functions.clone(),
                params.inbound_scopes.level,
                params.inbound_scopes.on_behalf,
                Level::Admin,
                ("inboundScopes.functions", "inboundScopes.level"),
            )?;
            events(&params.subscribe)?;
            if params.instance.instance_id == local.instance_id {
                return Err(KeelError::conflict(ERROR_SELF));
            }
            if !compatible(params.instance.protocol) {
                return Err(protocol_refused(params.instance.protocol));
            }
            let now = call.require::<dyn Clock>()?.now();
            let tx = call.tx()?;
            let code = pair_code::Entity::delete_many()
                .filter(pair_code::Column::CodeHash.eq(codes::hash(params.code.expose())))
                .exec_with_returning(tx)
                .await?
                .pop()
                .filter(|code| code.expires_at > now)
                .ok_or_else(|| KeelError::validation(ERROR_CODE).field("code"))?;
            let scopes: PeerScope = serde_json::from_value(code.scopes_json).map_err(|error| {
                KeelError::internal(super::functions::ERROR_STORED)
                    .param("id", code.id.as_str())
                    .source(error)
            })?;
            let url = params
                .url
                .as_deref()
                .map(|url| checked(url, "url"))
                .transpose()?;
            let token = peer_token()?;
            let secret = match (&url, params.subscribe.is_empty()) {
                (Some(_), false) => Some(webhook_secret()?),
                _ => None,
            };
            insert_link(
                &call,
                NewLink {
                    instance: &params.instance,
                    url,
                    inbound_token: token.expose(),
                    inbound_scopes: &scopes,
                    outbound_token: params.inbound_token.expose(),
                    webhook_secret: secret.as_deref(),
                },
            )
            .await?;
            Ok(PairAnswer {
                peer_id: local.instance_id,
                token: token.into_inner(),
                scopes,
                webhook_secret: secret,
            })
        })
}

pub(super) fn create() -> FunctionDef {
    function::<PairCreate, Paired>(CREATE)
        .kind(Kind::Form)
        .mode(Mode::Read)
        .level(Level::Admin)
        .unowned()
        .ui(Form::new("peer.create.title").page(
            "main",
            ["url", "code", "subscribe", "scopes", "level", "onBehalf"],
        ))
        .handler(|call: Call<PairCreate>| async move {
            let params = call.params();
            let caller = call.principal();
            let principal = continuation(
                caller,
                call.require::<dyn Features>().ok().as_deref(),
                CREATE,
            )?;
            let scopes = grant(
                params.scopes.clone(),
                params.level.unwrap_or(Level::Service),
                params.on_behalf,
                caller.level,
                ("scopes", "level"),
            )?;
            granted_by(caller, &scopes)?;
            events(&params.subscribe)?;
            let local = call.require::<InstanceInfo>()?;
            let remote = Remote::new(&checked(&params.url, "url")?)?;
            let presented = remote.handshake().await?;
            if presented.instance_id == local.instance_id {
                return Err(KeelError::conflict(ERROR_SELF));
            }
            if !compatible(presented.protocol) {
                return Err(protocol_refused(presented.protocol));
            }
            if let Some(existing) =
                by_instance(call.db(), &presented.instance_id.to_string()).await?
            {
                return Err(tofu(&existing, &presented));
            }
            let own: String = call.setting(SETTING_SHELL_URL).await?;
            let own = checked(&own, SETTING_SHELL_URL).ok();
            let inbound = peer_token()?;
            let answer: PairAnswer = remote
                .call(
                    PAIR,
                    &json!({
                        "code": params.code.expose(),
                        "instance": local.as_ref(),
                        "url": own,
                        "inboundToken": inbound.expose(),
                        "inboundScopes": scopes,
                        "subscribe": params.subscribe,
                    }),
                )
                .await?;
            if answer.peer_id != presented.instance_id || !is_peer_token(&answer.token) {
                return Err(
                    KeelError::peer(super::remote::ERROR_ANSWER).param("url", remote.base())
                );
            }
            let linked = call
                .require::<dyn Caller>()?
                .call(
                    principal,
                    LINK,
                    json!({
                        "instance": presented,
                        "url": remote.base(),
                        "inboundToken": inbound.expose(),
                        "inboundScopes": scopes,
                        "outboundToken": answer.token,
                        "webhookSecret": answer.webhook_secret,
                    }),
                    CallerOptions {
                        request_id: Some(call.request_id().to_owned()),
                        task: None,
                    },
                )
                .await?;
            Ok(Paired {
                peer: serde_json::from_value(linked)?,
                granted: answer.scopes,
            })
        })
}

pub(super) fn link() -> FunctionDef {
    function::<Link, Peer>(LINK)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .internal()
        .handler(|call: Call<Link>| async move {
            let params = call.params();
            let row = insert_link(
                &call,
                NewLink {
                    instance: &params.instance,
                    url: Some(base_of(&params.url)),
                    inbound_token: params.inbound_token.expose(),
                    inbound_scopes: &params.inbound_scopes,
                    outbound_token: params.outbound_token.expose(),
                    webhook_secret: params.webhook_secret.as_ref().map(Secret::expose),
                },
            )
            .await?;
            peer_of(row)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_neighbour_one_protocol_apart_is_compatible_and_two_apart_is_not() {
        assert!(compatible(PROTOCOL));
        assert!(compatible(PROTOCOL + 1));
        assert!(compatible(PROTOCOL - 1));
        assert!(!compatible(PROTOCOL + 2));
        assert!(!compatible(u32::MAX));
    }

    #[test]
    fn a_grant_checks_every_mask_and_keeps_the_level_between_service_and_the_caller_up_to_admin() {
        let granted = grant(
            vec!["svc.*".into(), "auth.key.issue".into()],
            Level::Admin,
            true,
            Level::Owner,
            ("scopes", "level"),
        )
        .unwrap();
        assert_eq!(granted.level, Level::Admin);
        assert!(granted.on_behalf);
        let mask = grant(
            vec!["svc.*".into(), "svc..x".into()],
            Level::User,
            false,
            Level::Admin,
            ("inboundScopes.functions", "inboundScopes.level"),
        )
        .unwrap_err()
        .to_call_error();
        assert_eq!(mask.fields[0].path, "inboundScopes.functions[1]");
        assert_eq!(mask.fields[0].key.as_deref(), Some(ERROR_MASK));
        for (level, ceiling) in [
            (Level::Guest, Level::Admin),
            (Level::Owner, Level::Owner),
            (Level::Admin, Level::Reseller),
        ] {
            let refused = grant(Vec::new(), level, false, ceiling, ("scopes", "level"))
                .unwrap_err()
                .to_call_error();
            assert_eq!(refused.fields[0].path, "level");
            assert_eq!(refused.fields[0].key.as_deref(), Some(ERROR_LEVEL));
        }
    }

    #[test]
    fn a_caller_grants_no_more_than_its_own_scope() {
        let mut caller = keel_sdk::types::Principal {
            id: Id::generate(),
            login: "root".into(),
            level: Level::Admin,
            account_id: Id::ROOT_ACCOUNT,
            role: "owner".into(),
            actor: None,
            scope: Scope::Full,
            via: keel_sdk::types::Via::Http,
            external_id: None,
            origin: None,
            lang: "en".into(),
            grants: Default::default(),
        };
        let scope = PeerScope {
            functions: vec!["svc.*".into()],
            level: Level::Admin,
            on_behalf: true,
        };
        granted_by(&caller, &scope).unwrap();
        caller.scope = Scope::Functions(vec!["peer.*".into()]);
        assert_eq!(granted_by(&caller, &scope).unwrap_err().key(), ERROR_SCOPE);
        caller.scope = Scope::Functions(vec!["peer.*".into(), "svc.*".into()]);
        granted_by(&caller, &scope).unwrap();
    }

    #[test]
    fn a_webhook_secret_is_the_prefix_and_thirty_two_random_bytes_as_hex() {
        let secret = webhook_secret().unwrap();
        assert!(secret.starts_with(WEBHOOK_SECRET_PREFIX));
        assert_eq!(secret.len(), WEBHOOK_SECRET_PREFIX.len() + SECRET_BYTES * 2);
        assert_ne!(webhook_secret().unwrap(), secret);
    }
}
