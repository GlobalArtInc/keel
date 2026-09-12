use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Guest,
    Service,
    User,
    Reseller,
    Admin,
    Owner,
}

#[derive(Debug, thiserror::Error)]
#[error("unknown level: {0}")]
pub struct UnknownLevel(String);

impl Level {
    pub const ALL: [Level; 6] = [
        Level::Guest,
        Level::Service,
        Level::User,
        Level::Reseller,
        Level::Admin,
        Level::Owner,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Level::Guest => "guest",
            Level::Service => "service",
            Level::User => "user",
            Level::Reseller => "reseller",
            Level::Admin => "admin",
            Level::Owner => "owner",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Level {
    type Err = UnknownLevel;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Level::ALL
            .into_iter()
            .find(|level| level.as_str() == text)
            .ok_or_else(|| UnknownLevel(text.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_compares_in_ladder_order() {
        assert!(Level::Guest < Level::Service);
        assert!(Level::Service < Level::User);
        assert!(Level::User < Level::Reseller);
        assert!(Level::Reseller < Level::Admin);
        assert!(Level::Admin < Level::Owner);
        let mut shuffled = [
            Level::Admin,
            Level::Guest,
            Level::Owner,
            Level::User,
            Level::Service,
            Level::Reseller,
        ];
        shuffled.sort();
        assert_eq!(shuffled, Level::ALL);
    }

    #[test]
    fn level_uses_lowercase_names_on_the_wire() {
        for level in Level::ALL {
            let wire = serde_json::to_value(level).unwrap();
            assert_eq!(wire, serde_json::json!(level.as_str()));
            assert_eq!(serde_json::from_value::<Level>(wire).unwrap(), level);
            assert_eq!(level.as_str().parse::<Level>().unwrap(), level);
        }
    }

    #[test]
    fn unknown_level_fails_to_deserialise() {
        assert!(serde_json::from_value::<Level>(serde_json::json!("root")).is_err());
        assert!(serde_json::from_value::<Level>(serde_json::json!("Admin")).is_err());
        assert!("root".parse::<Level>().is_err());
    }
}
