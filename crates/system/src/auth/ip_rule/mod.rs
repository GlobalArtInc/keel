pub mod functions;

use std::net::IpAddr;
use std::str::FromStr;
use std::sync::{Arc, PoisonError, RwLock};

use ipnet::IpNet;
use keel_sdk::async_trait::async_trait;
use keel_sdk::schemars::JsonSchema;
use keel_sdk::sea_orm::EntityTrait;
use keel_sdk::serde::{Deserialize, Serialize};
use keel_sdk::serde_json::Value;
use keel_sdk::system::CallExt;
use keel_sdk::types::Level;
use keel_sdk::{Call, Db, Guard, KeelError, Result};

use super::entity::ip_rule;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel_sdk::schemars")]
pub enum RuleAction {
    Allow,
    Deny,
}

impl RuleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleAction::Allow => "allow",
            RuleAction::Deny => "deny",
        }
    }
}

impl FromStr for RuleAction {
    type Err = ();

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "allow" => Ok(RuleAction::Allow),
            "deny" => Ok(RuleAction::Deny),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub action: RuleAction,
    pub net: IpNet,
    pub level: Level,
}

impl Rule {
    fn of(row: &ip_rule::Model) -> Result<Self> {
        Ok(Self {
            action: row.action.parse().map_err(|()| {
                KeelError::internal("auth.error.ip_rule_action")
                    .param("id", row.id.as_str())
                    .param("action", row.action.as_str())
            })?,
            net: row.cidr.parse().map_err(|_| {
                KeelError::internal("auth.error.ip_rule_net")
                    .param("id", row.id.as_str())
                    .param("net", row.cidr.as_str())
            })?,
            level: row.level.parse().map_err(|_| {
                KeelError::internal("auth.error.ip_rule_level")
                    .param("id", row.id.as_str())
                    .param("level", row.level.as_str())
            })?,
        })
    }
}

pub fn verdict(rules: &[Rule], ip: IpAddr, level: Level) -> Result<()> {
    let ip = ip.to_canonical();
    let applicable = rules.iter().filter(|rule| rule.level <= level);
    let mut allowed = None;
    for rule in applicable {
        let matches = rule.net.contains(&ip);
        match rule.action {
            RuleAction::Deny if matches => return Err(denied()),
            RuleAction::Deny => {}
            RuleAction::Allow => allowed = Some(allowed.unwrap_or(false) || matches),
        }
    }
    match allowed {
        Some(false) => Err(denied()),
        _ => Ok(()),
    }
}

fn denied() -> KeelError {
    KeelError::forbidden("auth.error.ip_denied")
}

#[derive(Default)]
pub struct IpRules {
    cache: RwLock<Cache>,
}

#[derive(Default)]
struct Cache {
    rules: Option<Arc<[Rule]>>,
    generation: u64,
}

impl IpRules {
    pub fn invalidate(&self) {
        let mut cache = self.cache.write().unwrap_or_else(PoisonError::into_inner);
        cache.rules = None;
        cache.generation += 1;
    }

    async fn rules(&self, db: &Db) -> Result<Arc<[Rule]>> {
        let generation = {
            let cache = self.cache.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(rules) = &cache.rules {
                return Ok(rules.clone());
            }
            cache.generation
        };
        let rows = ip_rule::Entity::find().all(db).await?;
        let loaded: Arc<[Rule]> = rows
            .iter()
            .map(Rule::of)
            .collect::<Result<Vec<Rule>>>()?
            .into();
        let mut cache = self.cache.write().unwrap_or_else(PoisonError::into_inner);
        if cache.generation == generation && cache.rules.is_none() {
            cache.rules = Some(loaded.clone());
        }
        Ok(loaded)
    }
}

#[async_trait]
impl Guard for IpRules {
    async fn check(&self, _: &str, call: &Call<Value>) -> Result<()> {
        let Some(ip) = call.client().and_then(|client| client.ip) else {
            return Ok(());
        };
        let rules = self.rules(call.db()).await?;
        verdict(&rules, ip, call.principal().level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(action: RuleAction, net: &str, level: Level) -> Rule {
        Rule {
            action,
            net: net.parse().expect("a network"),
            level,
        }
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("an address")
    }

    #[test]
    fn a_deny_rule_refuses_its_network_at_and_above_its_level_only() {
        let rules = [rule(RuleAction::Deny, "127.0.0.1/32", Level::User)];
        assert!(verdict(&rules, ip("127.0.0.1"), Level::Guest).is_ok());
        assert!(verdict(&rules, ip("127.0.0.1"), Level::User).is_err());
        assert!(verdict(&rules, ip("::ffff:127.0.0.1"), Level::Admin).is_err());
        assert!(verdict(&rules, ip("10.0.0.1"), Level::Admin).is_ok());
        let error = verdict(&rules, ip("127.0.0.1"), Level::Owner).unwrap_err();
        assert_eq!(error.key(), "auth.error.ip_denied");
    }

    #[test]
    fn with_an_allow_rule_in_force_the_address_must_match_one_of_them() {
        let rules = [
            rule(RuleAction::Allow, "10.0.0.0/8", Level::Admin),
            rule(RuleAction::Allow, "192.0.2.0/24", Level::Admin),
            rule(RuleAction::Deny, "10.9.9.9/32", Level::Guest),
        ];
        assert!(verdict(&rules, ip("127.0.0.1"), Level::Admin).is_err());
        assert!(verdict(&rules, ip("127.0.0.1"), Level::User).is_ok());
        assert!(verdict(&rules, ip("10.1.1.1"), Level::Admin).is_ok());
        assert!(verdict(&rules, ip("192.0.2.7"), Level::Owner).is_ok());
        assert!(verdict(&rules, ip("10.9.9.9"), Level::Admin).is_err());
        assert!(verdict(&rules, ip("10.9.9.9"), Level::Guest).is_err());
        assert!(verdict(&[], ip("127.0.0.1"), Level::Guest).is_ok());
    }

    #[test]
    fn a_stored_row_becomes_a_rule_and_a_corrupt_one_is_reported() {
        let now = keel_sdk::chrono::Utc::now();
        let row = ip_rule::Model {
            id: "r1".into(),
            action: "deny".into(),
            cidr: "10.0.0.0/8".into(),
            level: "reseller".into(),
            comment: None,
            created_at: now,
        };
        assert_eq!(
            Rule::of(&row).expect("a valid row"),
            rule(RuleAction::Deny, "10.0.0.0/8", Level::Reseller)
        );
        let bad_action = ip_rule::Model {
            action: "drop".into(),
            ..row.clone()
        };
        assert_eq!(
            Rule::of(&bad_action).unwrap_err().key(),
            "auth.error.ip_rule_action"
        );
        let bad_net = ip_rule::Model {
            cidr: "everywhere".into(),
            ..row.clone()
        };
        assert_eq!(
            Rule::of(&bad_net).unwrap_err().key(),
            "auth.error.ip_rule_net"
        );
        let bad_level = ip_rule::Model {
            level: "root".into(),
            ..row
        };
        assert_eq!(
            Rule::of(&bad_level).unwrap_err().key(),
            "auth.error.ip_rule_level"
        );
        assert_eq!("allow".parse::<RuleAction>(), Ok(RuleAction::Allow));
        assert_eq!(RuleAction::Deny.as_str(), "deny");
        assert!("both".parse::<RuleAction>().is_err());
    }

    #[test]
    fn the_cache_is_empty_until_loaded_and_an_invalidation_bumps_the_generation() {
        let rules = IpRules::default();
        let loaded = || {
            rules
                .cache
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .rules
                .is_some()
        };
        let generation = || {
            rules
                .cache
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .generation
        };
        assert!(!loaded());
        {
            let mut cache = rules.cache.write().unwrap_or_else(PoisonError::into_inner);
            cache.rules = Some(Arc::from(Vec::new()));
        }
        assert!(loaded());
        assert_eq!(generation(), 0);
        rules.invalidate();
        assert!(!loaded());
        assert_eq!(generation(), 1);
    }
}
