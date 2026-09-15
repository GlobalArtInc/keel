pub mod email;

use std::fmt;
use std::str::FromStr;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::system::SecretCipher;

use super::entity::channel;

pub const ERROR_KIND_UNAVAILABLE: &str = "notify.error.channel_kind_unavailable";
pub const ERROR_KIND: &str = "notify.error.channel_kind";
pub const ERROR_CONFIG: &str = "notify.error.channel_config";
pub const ERROR_EMAIL_MISSING: &str = "notify.error.channel_email_missing";
pub const ERROR_ID: &str = "notify.error.id";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "lowercase")]
pub enum ChannelKind {
    Email,
    Telegram,
    Webhook,
}

impl ChannelKind {
    pub const ALL: [ChannelKind; 3] = [
        ChannelKind::Email,
        ChannelKind::Telegram,
        ChannelKind::Webhook,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ChannelKind::Email => "email",
            ChannelKind::Telegram => "telegram",
            ChannelKind::Webhook => "webhook",
        }
    }

    pub const fn is_available(self) -> bool {
        matches!(self, ChannelKind::Email)
    }
}

impl FromStr for ChannelKind {
    type Err = ();

    fn from_str(text: &str) -> std::result::Result<Self, ()> {
        ChannelKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or(())
    }
}

impl fmt::Display for ChannelKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "lowercase")]
pub enum Security {
    #[default]
    None,
    Starttls,
    Tls,
}

impl Security {
    pub const ALL: [Security; 3] = [Security::None, Security::Starttls, Security::Tls];

    pub const fn as_str(self) -> &'static str {
        match self {
            Security::None => "none",
            Security::Starttls => "starttls",
            Security::Tls => "tls",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct EmailInput {
    #[schemars(length(min = 1, max = 253))]
    pub host: String,
    #[schemars(range(min = 1, max = 65535), extend("x-keel" = json!({ "check": "port" })))]
    pub port: u16,
    #[serde(default)]
    pub security: Security,
    #[serde(default)]
    #[schemars(length(max = 256))]
    pub username: Option<String>,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" })))]
    pub password: Option<Secret>,
    #[schemars(extend("x-keel" = json!({ "check": "email" })))]
    pub from: String,
    #[schemars(extend("x-keel" = json!({ "check": "email" })))]
    pub to: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct EmailView {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: Option<String>,
    pub has_password: bool,
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct StoredEmail {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
    pub to: String,
}

impl StoredEmail {
    pub fn view(&self) -> EmailView {
        EmailView {
            host: self.host.clone(),
            port: self.port,
            security: self.security,
            username: self.username.clone(),
            has_password: self.password.is_some(),
            from: self.from.clone(),
            to: self.to.clone(),
        }
    }

    pub fn seal(
        input: &EmailInput,
        previous: Option<&StoredEmail>,
        cipher: &dyn SecretCipher,
    ) -> Result<Self> {
        let password = match &input.password {
            Some(password) if !password.expose().is_empty() => {
                Some(cipher.seal(password.expose().as_bytes())?)
            }
            _ => previous.and_then(|stored| stored.password.clone()),
        };
        Ok(Self {
            host: input.host.clone(),
            port: input.port,
            security: input.security,
            username: input.username.clone().filter(|name| !name.is_empty()),
            password,
            from: input.from.clone(),
            to: input.to.clone(),
        })
    }

    pub fn open_password(&self, cipher: &dyn SecretCipher) -> Result<Option<Secret>> {
        self.password
            .as_deref()
            .map(|sealed| {
                let bytes = cipher.open(sealed)?;
                Ok(Secret::new(String::from_utf8_lossy(&bytes).into_owned()))
            })
            .transpose()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Channel {
    pub id: Id,
    pub kind: ChannelKind,
    pub name: String,
    pub enabled: bool,
    pub email: Option<EmailView>,
    pub last_sent_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

pub fn kind_of(row: &channel::Model) -> Result<ChannelKind> {
    row.kind.parse().map_err(|()| {
        KeelError::internal(ERROR_KIND)
            .param("id", row.id.as_str())
            .param("kind", row.kind.as_str())
    })
}

pub fn email_of(row: &channel::Model) -> Result<StoredEmail> {
    serde_json::from_value(row.config_json.clone()).map_err(|error| {
        KeelError::internal(ERROR_CONFIG)
            .param("id", row.id.as_str())
            .source(error)
    })
}

pub fn channel_of(row: channel::Model) -> Result<Channel> {
    let kind = kind_of(&row)?;
    let email = match kind {
        ChannelKind::Email => Some(email_of(&row)?.view()),
        ChannelKind::Telegram | ChannelKind::Webhook => None,
    };
    Ok(Channel {
        id: row
            .id
            .parse()
            .map_err(|_| KeelError::internal(ERROR_ID).param("id", row.id.as_str()))?,
        kind,
        name: row.name,
        enabled: row.enabled,
        email,
        last_sent_at: row.last_sent_at,
        last_error: row.last_error,
        created_at: row.created_at,
        created_by: row.created_by,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Reversing;

    impl SecretCipher for Reversing {
        fn seal(&self, plaintext: &[u8]) -> Result<String> {
            Ok(format!(
                "sealed:{}",
                String::from_utf8_lossy(plaintext)
                    .chars()
                    .rev()
                    .collect::<String>()
            ))
        }

        fn open(&self, sealed: &str) -> Result<Vec<u8>> {
            Ok(sealed
                .trim_start_matches("sealed:")
                .chars()
                .rev()
                .collect::<String>()
                .into_bytes())
        }
    }

    fn input(password: Option<&str>) -> EmailInput {
        EmailInput {
            host: "mail.example.org".into(),
            port: 587,
            security: Security::Starttls,
            username: Some("keel".into()),
            password: password.map(Secret::new),
            from: "keel@example.org".into(),
            to: "ops@example.org".into(),
        }
    }

    #[test]
    fn kinds_and_security_name_themselves_and_email_alone_is_available() {
        for kind in ChannelKind::ALL {
            assert_eq!(kind.as_str().parse::<ChannelKind>(), Ok(kind));
            assert_eq!(kind.to_string(), kind.as_str());
            assert_eq!(kind.is_available(), kind == ChannelKind::Email);
        }
        assert_eq!("pigeon".parse::<ChannelKind>(), Err(()));
        assert_eq!(Security::default(), Security::None);
        assert_eq!(
            Security::ALL.map(Security::as_str),
            ["none", "starttls", "tls"]
        );
    }

    #[test]
    fn the_password_is_sealed_on_set_kept_on_an_empty_edit_and_opened_for_the_transport() {
        let sealed = StoredEmail::seal(&input(Some("hunter2")), None, &Reversing).expect("seals");
        assert_eq!(sealed.password.as_deref(), Some("sealed:2retnuh"));
        assert!(sealed.view().has_password);
        assert_eq!(sealed.view().username.as_deref(), Some("keel"));
        assert_eq!(
            sealed
                .open_password(&Reversing)
                .expect("opens")
                .map(Secret::into_inner)
                .as_deref(),
            Some("hunter2")
        );
        let kept = StoredEmail::seal(&input(None), Some(&sealed), &Reversing).expect("seals");
        assert_eq!(kept.password, sealed.password);
        let blank = StoredEmail::seal(&input(Some("")), Some(&sealed), &Reversing).expect("seals");
        assert_eq!(blank.password, sealed.password);
        let fresh = StoredEmail::seal(&input(None), None, &Reversing).expect("seals");
        assert!(fresh.password.is_none());
        assert!(!fresh.view().has_password);
        assert!(
            fresh
                .open_password(&Reversing)
                .expect("nothing to open")
                .is_none()
        );
        let text = serde_json::to_value(&sealed).expect("serialises");
        assert_eq!(text["password"], json!("sealed:2retnuh"));
        assert!(text.get("hasPassword").is_none());
    }
}
