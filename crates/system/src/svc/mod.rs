mod entity;
mod functions;
mod kinds;

use keel_sdk::prelude::*;
use keel_sdk::schemars::{Schema, json_schema};
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel_sdk::system::AccountSource;
use keel_sdk::{InstanceInfoSource, ServiceKindDef};

use entity::service;

pub use functions::{
    ACCOUNT_RESUME, ACCOUNT_SUSPEND, ACCOUNT_UPSERT, CLOSE, ERROR_CLOSED, ERROR_EXISTS,
    ERROR_NOT_FOUND, ERROR_PERIOD, ERROR_STORED, EVENT_CLOSED, EVENT_OPENED, EVENT_RESUMED,
    EVENT_SUSPENDED, EVENT_UPDATED, LOCK_PREFIX, LOGIN, OPEN, RESUME, SET_PARAMS, STAT, SUBJECT,
    SUSPEND, VERBS, lock_key, name,
};
pub use kinds::{Catalogue, Entry, Functions, is_kind};

pub const FEATURE: &str = "svc";
pub const KINDS: &str = "svc.kinds";
pub const RETENTION_DAYS_KEY: &str = "svc.retention_days";
pub const RETENTION_DAYS: u32 = 30;
pub const RETENTION_DAYS_MAX: u32 = 3_650;
pub const ERROR_KIND: &str = "svc.error.kind";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/svc").table("keel_migration_svc");

pub static MODULE: ModuleDef = ModuleDef {
    id: "svc",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.require::<dyn Clock>()
        .require::<dyn SettingsSource>()
        .require::<dyn AccountSource>()
        .contribute(InstanceInfoSource::new().feature(FEATURE))
        .contribute(
            SettingDef::new(RETENTION_DAYS_KEY, retention_schema())
                .default(RETENTION_DAYS)
                .level(Level::Admin),
        )
        .contribute(AccountLifecycle::new().on_delete(forget))
        .within(ACCOUNT_SUSPEND, functions::unmarked)
        .within(ACCOUNT_RESUME, functions::unmarked)
        .derive(|kinds: Vec<ServiceKindDef>, r| kinds::derive(kinds, r))
        .i18n("en", include_str!("../../i18n/svc/en.json"))
        .i18n("ru", include_str!("../../i18n/svc/ru.json"));
    Ok(())
}

fn retention_schema() -> Schema {
    json_schema!({ "type": "integer", "minimum": 1, "maximum": RETENTION_DAYS_MAX })
}

async fn forget(call: Call<Id>) -> Result<()> {
    let account = call.params().to_string();
    let removed = service::Entity::delete_many()
        .filter(service::Column::AccountId.eq(account.as_str()))
        .exec(call.tx()?)
        .await?;
    if let Some(task) = call.task() {
        task.log(&format!(
            "forgot {} service(s) of account {account}",
            removed.rows_affected
        ))
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use keel_sdk::lifecycle::AccountChange;
    use keel_sdk::runtime::Contributions;
    use keel_sdk::svc::{LoginTarget, OpenRequest, Opened, Period, Service, ServiceAccount};
    use keel_sdk::types::{ErrorCode, PeerScope, Scope, Via};
    use keel_sdk::{ServiceKind, async_trait::async_trait};

    use super::*;

    #[derive(Deserialize, Serialize, JsonSchema)]
    #[serde(crate = "keel_sdk::serde")]
    #[schemars(crate = "keel_sdk::schemars")]
    struct Quota {
        limit: u32,
    }

    #[derive(Deserialize, Serialize, JsonSchema)]
    #[serde(crate = "keel_sdk::serde")]
    #[schemars(crate = "keel_sdk::schemars")]
    struct Anonymous {
        nickname: String,
    }

    macro_rules! kind {
        ($name:ident, $kind:literal, $version:literal, $account:ty) => {
            struct $name;

            #[async_trait]
            impl ServiceKind for $name {
                const KIND: &'static str = $kind;
                const TITLE: &'static str = "probe.service.title";
                const SCHEMA_VERSION: u32 = $version;
                type Params = Quota;
                type State = Quota;
                type Account = $account;

                async fn open(
                    &self,
                    _: &Call<()>,
                    request: OpenRequest<$account, Quota>,
                ) -> Result<Opened<Quota>> {
                    Ok(Opened {
                        state: request.params,
                    })
                }

                async fn suspend(&self, _: &Call<()>, _: &Service, _: Value) -> Result<()> {
                    Ok(())
                }

                async fn resume(&self, _: &Call<()>, _: &Service) -> Result<()> {
                    Ok(())
                }

                async fn close(&self, _: &Call<()>, _: &Service) -> Result<()> {
                    Ok(())
                }

                async fn set_params(
                    &self,
                    _: &Call<()>,
                    _: &Service,
                    params: Quota,
                ) -> Result<Quota> {
                    Ok(params)
                }

                async fn stat(&self, _: &Call<()>, _: &Service, _: Period) -> Result<Value> {
                    Ok(json!({}))
                }

                async fn login(&self, _: &Call<()>, _: &Service) -> Result<LoginTarget> {
                    Ok(LoginTarget { url: "/".into() })
                }
            }
        };
    }

    kind!(Zones, "probe.zones", 2, ServiceAccount);
    kind!(Boxes, "boxes", 1, ServiceAccount);
    kind!(Shouting, "Probe.Zones", 1, ServiceAccount);
    kind!(Unversioned, "probe.empty", 0, ServiceAccount);
    kind!(Nameless, "probe.nameless", 1, Anonymous);
    kind!(Twin, "probe.zones", 5, ServiceAccount);

    fn derived(offered: Vec<ServiceKindDef>) -> std::result::Result<Registrar, KeelError> {
        let mut registrar = Registrar::new(MODULE.id);
        (MODULE.init)(&mut registrar).expect("init registers");
        let hook = registrar.take_derive().expect("init leaves a derive hook");
        let mut bag = Contributions::default();
        for def in offered {
            bag.add(def);
        }
        hook(&[&bag])(&mut registrar)?;
        Ok(registrar)
    }

    #[test]
    fn the_module_registers_seven_functions_per_kind_and_the_catalogue_from_the_contributions() {
        let registrations = derived(vec![ServiceKindDef::new(Zones), ServiceKindDef::new(Boxes)])
            .expect("the kinds are valid")
            .into_registrations()
            .expect("the registrations are valid");
        let functions: Vec<(&str, Kind, Mode, Level, Idempotency)> = registrations
            .functions
            .iter()
            .map(|def| {
                (
                    def.name(),
                    def.kind(),
                    def.mode(),
                    def.access().level,
                    def.idempotency(),
                )
            })
            .collect();
        let mut expected = Vec::new();
        for kind in ["boxes", "probe.zones"] {
            for (verb, function, mode, idempotency) in [
                (OPEN, Kind::Action, Mode::Write, Idempotency::Required),
                (SUSPEND, Kind::Action, Mode::Write, Idempotency::Optional),
                (RESUME, Kind::Action, Mode::Write, Idempotency::Optional),
                (CLOSE, Kind::Action, Mode::Write, Idempotency::Required),
                (SET_PARAMS, Kind::Action, Mode::Write, Idempotency::Optional),
                (STAT, Kind::Get, Mode::Read, Idempotency::Optional),
                (LOGIN, Kind::Action, Mode::Write, Idempotency::Optional),
            ] {
                expected.push((name(kind, verb), function, mode, Level::Admin, idempotency));
            }
        }
        expected.push((
            KINDS.to_owned(),
            Kind::Get,
            Mode::Read,
            Level::Admin,
            Idempotency::Optional,
        ));
        let expected: Vec<(&str, Kind, Mode, Level, Idempotency)> = expected
            .iter()
            .map(|(name, kind, mode, level, idempotency)| {
                (name.as_str(), *kind, *mode, *level, *idempotency)
            })
            .collect();
        assert_eq!(functions, expected);
        for def in &registrations.functions {
            let descriptor = def.descriptor().expect("the descriptor compiles");
            assert_eq!(descriptor.module, "svc");
            assert!(def.owned().is_none(), "{}", def.name());
            assert_eq!(def.access().permission, None);
            assert!(!def.is_internal());
        }
        let open = registrations
            .functions
            .iter()
            .find(|def| def.name() == "svc.probe.zones.open")
            .unwrap();
        assert_eq!(
            open.params().as_value()["required"],
            json!(["externalId", "account", "params"])
        );
        let sources: Vec<&InstanceInfoSource> = registrations
            .contributions
            .of::<InstanceInfoSource>()
            .collect();
        assert_eq!(
            sources,
            [
                &InstanceInfoSource::new().feature(FEATURE),
                &InstanceInfoSource::new().service_kind("boxes"),
                &InstanceInfoSource::new().service_kind("probe.zones"),
            ]
        );
        let settings: Vec<(&str, Option<&Value>, Level)> = registrations
            .contributions
            .of::<SettingDef>()
            .map(|def| (def.key(), def.default_value(), def.level_value()))
            .collect();
        assert_eq!(
            settings,
            [(
                RETENTION_DAYS_KEY,
                Some(&json!(RETENTION_DAYS)),
                Level::Admin
            )]
        );
        let lifecycles: Vec<&AccountLifecycle> = registrations
            .contributions
            .of::<AccountLifecycle>()
            .collect();
        assert_eq!(lifecycles.len(), 1);
        assert_eq!(lifecycles[0].module(), "svc");
        assert!(lifecycles[0].handler(AccountChange::Delete).is_some());
        assert!(lifecycles[0].handler(AccountChange::Suspend).is_none());
        let hooked: Vec<&str> = registrations
            .within
            .iter()
            .map(|hook| hook.function.as_str())
            .collect();
        assert_eq!(hooked, [ACCOUNT_SUSPEND, ACCOUNT_RESUME]);
        assert!(registrations.before.is_empty());
        assert!(registrations.menu.is_empty());
        assert!(registrations.permissions.is_empty());
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert_eq!(MODULE.id, "svc");
        assert!(MODULE.requires.is_empty());
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_svc")
        );
    }

    #[test]
    fn without_a_kind_the_module_offers_the_empty_catalogue_and_the_feature_alone() {
        let registrations = derived(Vec::new()).unwrap().into_registrations().unwrap();
        let names: Vec<&str> = registrations
            .functions
            .iter()
            .map(FunctionDef::name)
            .collect();
        assert_eq!(names, [KINDS]);
        let kinds: Vec<&String> = registrations
            .contributions
            .of::<InstanceInfoSource>()
            .flat_map(|source| source.service_kinds())
            .collect();
        assert!(kinds.is_empty());
    }

    #[test]
    fn a_kind_with_a_bad_name_no_version_an_account_without_identity_or_a_twin_is_refused() {
        for (offered, kind, reason) in [
            (
                vec![ServiceKindDef::new(Shouting)],
                "Probe.Zones",
                "its name is not lowercase segments of letters, digits and underscores joined by dots",
            ),
            (
                vec![ServiceKindDef::new(Unversioned)],
                "probe.empty",
                "its schema version is not a number from 1 to 2147483647",
            ),
            (
                vec![ServiceKindDef::new(Nameless)],
                "probe.nameless",
                "its account does not require origin and externalId",
            ),
            (
                vec![ServiceKindDef::new(Zones), ServiceKindDef::new(Twin)],
                "probe.zones",
                "it is contributed twice",
            ),
        ] {
            let error = derived(offered).expect_err("the kind is refused");
            assert_eq!(error.code(), ErrorCode::Internal);
            assert_eq!(error.key(), ERROR_KIND);
            assert_eq!(error.params()["kind"], kind);
            assert_eq!(error.params()["reason"], reason);
        }
    }

    #[test]
    fn a_kind_name_is_dotted_lowercase_segments() {
        for good in ["dns.zones", "hosting.sites", "boxes", "a1.b_2"] {
            assert!(is_kind(good), "{good}");
        }
        for bad in [
            "",
            "dns.",
            ".zones",
            "dns..zones",
            "1dns.zones",
            "dns.Zones",
            "dns-zones",
            "dns.zones*",
        ] {
            assert!(!is_kind(bad), "{bad}");
        }
    }

    #[test]
    fn a_stored_row_that_cannot_be_read_names_its_member() {
        let row = |status: &str, peer: Option<&str>, version: i32| service::Model {
            id: "01J9S00000000000000000000S".into(),
            kind: "probe.zones".into(),
            external_id: "svc-1".into(),
            account_id: "01J8A0000000000000000000A1".into(),
            status: status.into(),
            params_json: json!({ "limit": 1 }),
            state_json: json!({ "limit": 1 }),
            schema_version: version,
            peer_id: peer.map(str::to_owned),
            retain_until: None,
            suspended_account: false,
            created_at: "2026-10-04T12:00:00Z".parse().unwrap(),
            updated_at: "2026-10-04T12:00:00Z".parse().unwrap(),
        };
        let read = functions::service_of(&row("suspended", Some("01J9A0000000000000000000A1"), 2))
            .expect("the row reads");
        assert_eq!(read.status, keel_sdk::svc::ServiceStatus::Suspended);
        assert_eq!(
            read.peer_id,
            Some("01J9A0000000000000000000A1".parse().unwrap())
        );
        assert_eq!(read.schema_version, 2);
        for (row, member) in [
            (row("deleted", None, 2), "status"),
            (row("active", Some("link"), 2), "peerId"),
            (row("active", None, -1), "schemaVersion"),
        ] {
            let error = functions::service_of(&row).unwrap_err();
            assert_eq!(error.code(), ErrorCode::Internal);
            assert_eq!(error.key(), ERROR_STORED);
            assert_eq!(error.params()["member"], member);
            assert_eq!(error.params()["id"], "01J9S00000000000000000000S");
        }
    }

    #[test]
    fn the_link_a_call_came_through_is_the_peer_of_its_service() {
        let link: Id = "01J9A0000000000000000000A1".parse().unwrap();
        let mut principal = Principal {
            id: Id::generate(),
            login: "peer-link".into(),
            level: Level::Admin,
            account_id: Id::ROOT_ACCOUNT,
            role: "peer".into(),
            actor: None,
            scope: Scope::Peer(PeerScope {
                functions: vec!["svc.*".into()],
                level: Level::Admin,
                on_behalf: false,
            }),
            via: Via::Peer(link),
            external_id: None,
            origin: None,
            lang: "en".into(),
            grants: Default::default(),
        };
        assert_eq!(functions::peer_of(&principal), Some(link));
        principal.via = Via::Http;
        assert_eq!(functions::peer_of(&principal), None);
        principal.via = Via::Internal;
        assert_eq!(functions::peer_of(&principal), None);
    }
}
