use keel_sdk::prelude::*;
use keel_sdk::schemars::JsonSchema;
use keel_sdk::serde::{Deserialize, Serialize};
use keel_sdk::types::{ActionRole, Platform};
use semver::Version;

use super::channel::{Channel, Manifest, Source};
use super::config::UpdateConfig;
use super::{APPLY, CHANNEL_KEY, CHECK, MENU_ICON, ROLLBACK, history};

pub(super) const ERROR_VERSION: &str = "update.error.version";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub(super) struct Nothing {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "snake_case")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "snake_case")]
pub(super) enum Blocker {
    MinUpgradeFrom,
    Contract,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct Status {
    pub(super) running: String,
    pub(super) running_contract: Option<u32>,
    pub(super) channel: Channel,
    pub(super) channel_url: String,
    pub(super) available: String,
    pub(super) notes: String,
    pub(super) min_upgrade_from: String,
    pub(super) contract: u32,
    pub(super) up_to_date: bool,
    pub(super) allowed: bool,
    pub(super) blocked_by: Option<Blocker>,
    pub(super) previous: Option<String>,
}

pub(super) struct Site {
    pub(super) running: Version,
    pub(super) channel: Channel,
    pub(super) source: Source,
}

pub(super) async fn site<P: Send + Sync>(call: &Call<P>) -> Result<Site> {
    let platform = call.require::<Platform>()?;
    let running = Version::parse(&platform.version).map_err(|error| {
        KeelError::internal(ERROR_VERSION)
            .param("version", platform.version.as_str())
            .source(error)
    })?;
    let channel: Channel = call.setting(CHANNEL_KEY).await?;
    let config = call.config::<UpdateConfig>()?;
    let source = Source::new(&config.base()?)?;
    Ok(Site {
        running,
        channel,
        source,
    })
}

pub(super) async fn status(site: &Site, db: &Db) -> Result<Status> {
    let head = site.source.head(site.channel).await?;
    let running_contract = site
        .source
        .manifest(&site.running)
        .await?
        .map(|manifest| manifest.contract);
    let previous = history::previous_of(db, &site.running).await?;
    Ok(Status::of(
        &site.running,
        site.channel,
        site.source.base(),
        &head,
        running_contract,
        previous,
    ))
}

impl Status {
    fn of(
        running: &Version,
        channel: Channel,
        channel_url: &str,
        head: &Manifest,
        running_contract: Option<u32>,
        previous: Option<Version>,
    ) -> Self {
        let up_to_date = *running >= head.version;
        let blocked_by = if up_to_date {
            None
        } else if *running < head.min_upgrade_from {
            Some(Blocker::MinUpgradeFrom)
        } else if running_contract.is_some_and(|contract| contract != head.contract) {
            Some(Blocker::Contract)
        } else {
            None
        };
        Self {
            running: running.to_string(),
            running_contract,
            channel,
            channel_url: channel_url.to_owned(),
            available: head.version.to_string(),
            notes: head.notes.clone(),
            min_upgrade_from: head.min_upgrade_from.to_string(),
            contract: head.contract,
            up_to_date,
            allowed: !up_to_date && blocked_by.is_none(),
            blocked_by,
            previous: previous.map(|version| version.to_string()),
        }
    }
}

pub(super) fn check() -> FunctionDef {
    function::<Nothing, Status>(CHECK)
        .kind(Kind::Get)
        .level(Level::Admin)
        .unowned()
        .ui(ObjectScreen::new("update.check.title")
            .action(
                Action::new("apply", ActionRole::Button)
                    .screen(APPLY)
                    .icon(MENU_ICON)
                    .hidden_if("!allowed"),
            )
            .action(
                Action::new("rollback", ActionRole::Button)
                    .screen(ROLLBACK)
                    .icon("undo-2")
                    .hidden_if("!previous"),
            )
            .action(Action::new("refresh", ActionRole::Refresh)))
        .handler(|call: Call<Nothing>| async move {
            let site = site(&call).await?;
            status(&site, call.db()).await
        })
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::{self, json};

    use super::*;

    fn of(
        running: &str,
        channel: Channel,
        head: &Manifest,
        running_contract: Option<u32>,
        previous: Option<Version>,
    ) -> Status {
        Status::of(
            &Version::parse(running).unwrap(),
            channel,
            "http://127.0.0.1:1/",
            head,
            running_contract,
            previous,
        )
    }

    fn head(version: &str, min_upgrade_from: &str, contract: u32) -> Manifest {
        Manifest {
            version: Version::parse(version).unwrap(),
            sha256: "00".into(),
            min_upgrade_from: Version::parse(min_upgrade_from).unwrap(),
            contract,
            notes: "Notes".into(),
        }
    }

    #[test]
    fn a_newer_head_the_running_version_reaches_is_allowed_and_the_other_cases_say_why() {
        let allowed = of(
            "0.1.0",
            Channel::Stable,
            &head("0.2.0", "0.1.0", 1),
            Some(1),
            None,
        );
        assert!(allowed.allowed);
        assert!(!allowed.up_to_date);
        assert_eq!(allowed.blocked_by, None);
        assert_eq!(allowed.available, "0.2.0");
        assert_eq!(allowed.running, "0.1.0");
        assert_eq!(allowed.channel_url, "http://127.0.0.1:1/");
        assert_eq!(allowed.previous, None);

        let unknown_contract = of(
            "0.1.0",
            Channel::Stable,
            &head("0.2.0", "0.1.0", 1),
            None,
            None,
        );
        assert!(unknown_contract.allowed);
        assert_eq!(unknown_contract.running_contract, None);

        let current = of(
            "0.2.0",
            Channel::Beta,
            &head("0.2.0", "0.1.0", 1),
            Some(1),
            Some(Version::new(0, 1, 0)),
        );
        assert!(current.up_to_date);
        assert!(!current.allowed);
        assert_eq!(current.blocked_by, None);
        assert_eq!(current.previous.as_deref(), Some("0.1.0"));
        assert_eq!(current.channel, Channel::Beta);

        let ahead = of(
            "0.3.0",
            Channel::Stable,
            &head("0.2.0", "0.1.0", 1),
            Some(1),
            None,
        );
        assert!(ahead.up_to_date);
        assert!(!ahead.allowed);

        let too_old = of(
            "0.1.0",
            Channel::Stable,
            &head("0.3.0", "0.2.0", 1),
            Some(1),
            None,
        );
        assert!(!too_old.allowed);
        assert_eq!(too_old.blocked_by, Some(Blocker::MinUpgradeFrom));
        assert_eq!(too_old.min_upgrade_from, "0.2.0");

        let across = of(
            "0.2.0",
            Channel::Stable,
            &head("1.0.0", "0.2.0", 2),
            Some(1),
            None,
        );
        assert!(!across.allowed);
        assert_eq!(across.blocked_by, Some(Blocker::Contract));
        assert_eq!(across.contract, 2);
        assert_eq!(across.running_contract, Some(1));
        assert_eq!(json!(across.blocked_by), json!("contract"));
        assert_eq!(json!(Blocker::MinUpgradeFrom), json!("min_upgrade_from"));
    }

    #[test]
    fn the_check_is_an_admin_get_whose_actions_read_the_status() {
        let def = check();
        assert_eq!(def.kind(), Kind::Get);
        assert_eq!(def.mode(), Mode::Read);
        assert_eq!(def.access().level, Level::Admin);
        let descriptor = serde_json::to_value(def.descriptor().expect("compiles")).unwrap();
        assert_eq!(descriptor["ui"]["title"], json!("update.check.title"));
        let actions = descriptor["ui"]["actions"].as_array().unwrap();
        assert_eq!(
            actions
                .iter()
                .map(|action| (action["id"].as_str().unwrap(), action["screen"].clone()))
                .collect::<Vec<_>>(),
            [
                ("apply", json!(APPLY)),
                ("rollback", json!(ROLLBACK)),
                ("refresh", Value::Null),
            ]
        );
        assert!(actions[0]["hidden_if"].is_object());
        assert!(actions[1]["hidden_if"].is_object());
        assert_eq!(actions[2]["role"], json!("refresh"));
        assert_eq!(
            descriptor["result"]["properties"]["blockedBy"]["anyOf"][0]["$ref"],
            json!("#/$defs/Blocker")
        );
    }
}
