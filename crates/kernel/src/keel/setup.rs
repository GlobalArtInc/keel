use std::net::SocketAddr;
use std::sync::{Mutex, PoisonError};

use keel_sdk::prelude::*;
use keel_sdk::system::{ContextExt, PasswordStore, PrincipalSource};
use rand::TryRng;
use rand::rngs::SysRng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::principals::{self, DEFAULT_ROLE, NewPrincipal};
use crate::clock::Clock;

pub(super) const SETUP_ADMIN: &str = "keel.setup.admin";
pub const PATH: &str = "/setup";
pub const KEY_PREFIX: &str = "kok_";
const KEY_LEN: usize = 32;
const LOCALHOST: &str = "localhost";
const ERROR_KEY: &str = "keel.setup.error.key";
const ERROR_ENTROPY: &str = "keel.setup.error.entropy";
const ERROR_GONE: &str = "keel.error.function_not_found";

#[derive(Deserialize, Serialize, JsonSchema)]
struct SetupRequest {
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    key: Secret,
    #[schemars(length(min = 1, max = 64))]
    login: String,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" })))]
    password: Secret,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Created {
    id: Id,
    login: String,
}

#[derive(Default)]
pub struct Handle {
    key: Mutex<Option<Secret>>,
}

impl Handle {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn issue(&self) -> Result<()> {
        let mut bytes = [0u8; KEY_LEN];
        SysRng.try_fill_bytes(&mut bytes).map_err(|error| {
            KeelError::internal(ERROR_ENTROPY).source(std::io::Error::other(error.to_string()))
        })?;
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        *self.key.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(Secret::new(format!("{KEY_PREFIX}{hex}")));
        Ok(())
    }

    pub fn link(&self, listen: SocketAddr, host_name: Option<&str>) -> Option<String> {
        let key = self.key.lock().unwrap_or_else(PoisonError::into_inner);
        key.as_ref().map(|key| {
            format!(
                "https://{}{PATH}?key={}",
                authority(listen, host_name),
                key.expose()
            )
        })
    }

    fn take(&self, offered: &str) -> Option<Secret> {
        let mut held = self.key.lock().unwrap_or_else(PoisonError::into_inner);
        let admitted = held.as_ref().is_some_and(|key| {
            let expected = Sha256::digest(key.expose().as_bytes());
            let given = Sha256::digest(offered.as_bytes());
            expected.ct_eq(&given).into()
        });
        if admitted { held.take() } else { None }
    }

    fn restore(&self, key: Secret) {
        let mut held = self.key.lock().unwrap_or_else(PoisonError::into_inner);
        if held.is_none() {
            *held = Some(key);
        }
    }
}

fn authority(listen: SocketAddr, host_name: Option<&str>) -> String {
    let host = if listen.ip().is_unspecified() {
        host_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(LOCALHOST)
            .to_owned()
    } else if listen.is_ipv6() {
        format!("[{}]", listen.ip())
    } else {
        listen.ip().to_string()
    };
    format!("{host}:{}", listen.port())
}

pub(super) async fn issue_when_unclaimed(ctx: &Context) -> Result<bool> {
    let handle = ctx.require::<Handle>()?;
    if ctx.require::<dyn PasswordStore>().is_err() {
        tracing::debug!("no module stores passwords; no setup link is issued");
        return Ok(false);
    }
    let principals = ctx.require::<dyn PrincipalSource>()?;
    if !principals
        .at_or_above(Level::Admin, &ctx.db())
        .await?
        .is_empty()
    {
        return Ok(false);
    }
    handle.issue()?;
    Ok(true)
}

pub(super) fn admin() -> FunctionDef {
    function::<SetupRequest, Created>(SETUP_ADMIN)
        .kind(Kind::Form)
        .level(Level::Guest)
        .unowned()
        .handler(|call: Call<SetupRequest>| async move { run(&call).await })
}

async fn run(call: &Call<SetupRequest>) -> Result<Created> {
    let handle = call.require::<Handle>()?;
    let store = call.require::<dyn PasswordStore>()?;
    let principals = call.require::<dyn PrincipalSource>()?;
    if !principals
        .at_or_above(Level::Admin, call.db())
        .await?
        .is_empty()
    {
        return Err(KeelError::not_found(ERROR_GONE).param("function", SETUP_ADMIN));
    }
    let Some(key) = handle.take(call.params().key.expose()) else {
        return Err(KeelError::unauthenticated(ERROR_KEY));
    };
    match create(call, store.as_ref()).await {
        Ok(created) => {
            tracing::info!(login = created.login, id = %created.id, "the first administrator is created through the setup link");
            Ok(created)
        }
        Err(error) => {
            handle.restore(key);
            Err(error)
        }
    }
}

async fn create(call: &Call<SetupRequest>, store: &dyn PasswordStore) -> Result<Created> {
    let params = call.params();
    let tx = call.tx()?;
    let now = call.require::<dyn Clock>()?.now();
    let id = principals::insert(
        tx,
        NewPrincipal {
            account_id: Id::ROOT_ACCOUNT.to_string(),
            login: params.login.clone(),
            level: Level::Admin,
            role: DEFAULT_ROLE.to_owned(),
            lang: call.principal().lang.clone(),
        },
        now,
    )
    .await?;
    store.set(id, &params.login, &params.password, tx).await?;
    Ok(Created {
        id,
        login: params.login.clone(),
    })
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json;

    use super::*;

    #[test]
    fn the_setup_function_is_a_guest_form_with_the_key_and_the_password_as_secrets() {
        let def = admin();
        assert_eq!(def.name(), SETUP_ADMIN);
        assert_eq!(def.kind(), Kind::Form);
        assert_eq!(def.mode(), Mode::Write);
        assert_eq!(def.access().level, Level::Guest);
        assert!(def.access().permission.is_none());
        assert!(!def.is_internal());
        let descriptor = serde_json::to_value(def.descriptor().expect("compiles")).unwrap();
        assert_eq!(
            descriptor["params"]["required"],
            json!(["key", "login", "password"])
        );
        for field in ["key", "password"] {
            assert_eq!(
                descriptor["params"]["properties"][field]["x-keel"]["secret"],
                json!(true),
                "{field}"
            );
            assert_eq!(
                descriptor["params"]["properties"][field]["$ref"],
                json!("#/$defs/Secret"),
                "{field}"
            );
        }
        assert_eq!(
            descriptor["params"]["$defs"]["Secret"]["writeOnly"],
            json!(true)
        );
        assert_eq!(
            descriptor["params"]["properties"]["login"]["maxLength"],
            json!(64)
        );
        assert!(descriptor.get("ui").is_none_or(Value::is_null));
    }

    #[test]
    fn a_handle_issues_one_key_that_is_taken_once_by_its_exact_text_and_restored_on_request() {
        let handle = Handle::new();
        let listen: SocketAddr = "127.0.0.1:1500".parse().unwrap();
        assert_eq!(handle.link(listen, None), None);
        handle.issue().unwrap();
        let link = handle.link(listen, Some("panel.example")).unwrap();
        let key = link
            .strip_prefix("https://127.0.0.1:1500/setup?key=")
            .unwrap()
            .to_owned();
        assert!(key.starts_with(KEY_PREFIX), "{key}");
        assert_eq!(key.len(), KEY_PREFIX.len() + KEY_LEN * 2);
        assert!(
            key[KEY_PREFIX.len()..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        );
        assert!(handle.take(&key[..key.len() - 1]).is_none());
        assert!(handle.take("kok_").is_none());
        let taken = handle.take(&key).expect("the exact key is taken");
        assert_eq!(taken.expose(), key);
        assert!(handle.take(&key).is_none());
        assert_eq!(handle.link(listen, None), None);
        handle.restore(taken);
        assert!(handle.link(listen, None).is_some());
        assert!(handle.take(&key).is_some());
        handle.issue().unwrap();
        assert!(
            handle.take(&key).is_none(),
            "a new key replaces the old one"
        );
    }

    #[test]
    fn the_link_names_the_host_for_an_unspecified_address_and_brackets_an_ipv6_one() {
        let handle = Handle::new();
        handle.issue().unwrap();
        let any: SocketAddr = "0.0.0.0:8443".parse().unwrap();
        let any6: SocketAddr = "[::]:8443".parse().unwrap();
        let six: SocketAddr = "[::1]:1500".parse().unwrap();
        assert!(
            handle
                .link(any, Some(" panel.example "))
                .unwrap()
                .starts_with("https://panel.example:8443/setup?key=kok_")
        );
        assert!(
            handle
                .link(any6, Some(""))
                .unwrap()
                .starts_with("https://localhost:8443/setup?key=kok_")
        );
        assert!(
            handle
                .link(any, None)
                .unwrap()
                .starts_with("https://localhost:8443/setup?key=kok_")
        );
        assert!(
            handle
                .link(six, Some("panel.example"))
                .unwrap()
                .starts_with("https://[::1]:1500/setup?key=kok_")
        );
    }
}
