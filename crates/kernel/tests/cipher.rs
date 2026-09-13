use std::sync::Arc;

use keel_kernel::loader::BootError;
use keel_kernel::{Composition, Kernel};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::system::{SecondFactorSource, SecretCipher};
use keel_testkit::{Harness, TestDatabase, Who, tokio};
use keel_types::ErrorCode;

#[derive(Deserialize, Serialize, JsonSchema)]
struct Plain {
    value: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Sealed {
    sealed: String,
}

static VAULT: ModuleDef = ModuleDef {
    id: "vault",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: vault_init,
};

fn vault_init(r: &mut Registrar) -> Result<()> {
    r.require::<dyn SecretCipher>()
        .function(
            function::<Plain, Sealed>("vault.seal")
                .kind(Kind::Action)
                .level(Level::User)
                .unowned()
                .handler(|call: Call<Plain>| async move {
                    let cipher = call.require::<dyn SecretCipher>()?;
                    Ok(Sealed {
                        sealed: cipher.seal(call.params().value.as_bytes())?,
                    })
                }),
        )
        .function(
            function::<Sealed, Plain>("vault.open")
                .kind(Kind::Get)
                .level(Level::User)
                .unowned()
                .handler(|call: Call<Sealed>| async move {
                    let cipher = call.require::<dyn SecretCipher>()?;
                    let bytes = cipher.open(&call.params().sealed)?;
                    Ok(Plain {
                        value: String::from_utf8_lossy(&bytes).into_owned(),
                    })
                }),
        );
    Ok(())
}

struct Always(bool);

#[async_trait]
impl SecondFactorSource for Always {
    async fn enrolled(&self, _: Id, _: &Db) -> Result<bool> {
        Ok(self.0)
    }
}

static ENROLLED: ModuleDef = ModuleDef {
    id: "enrolled",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: enrolled_init,
};

fn enrolled_init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn SecondFactorSource>(Arc::new(Always(true)));
    Ok(())
}

#[tokio::test]
async fn a_handler_seals_through_the_cipher_and_only_the_composition_with_the_same_key_opens_it() {
    let first = Harness::new().module(&VAULT).boot().await;
    let reply = first
        .call(
            Who::A1,
            "vault.seal",
            json!({ "value": "JBSWY3DPEHPK3PXP" }),
        )
        .await;
    let sealed = reply
        .data()
        .unwrap_or_else(|| panic!("vault.seal failed: {:?}", reply.error()))["sealed"]
        .as_str()
        .expect("the sealed text")
        .to_owned();
    assert_ne!(sealed, "JBSWY3DPEHPK3PXP");
    assert!(!sealed.contains("JBSWY3DPEHPK3PXP"), "{sealed}");
    assert!(sealed.contains("A256GCM"), "{sealed}");
    let opened = first
        .call(Who::A1, "vault.open", json!({ "sealed": sealed }))
        .await;
    assert_eq!(
        opened.data().map(|data| data["value"].clone()),
        Some(json!("JBSWY3DPEHPK3PXP")),
        "{:?}",
        opened.error()
    );
    let cipher = first
        .registry()
        .services()
        .get::<dyn SecretCipher>()
        .expect("the kernel provides the cipher");
    assert_eq!(cipher.open(&sealed).expect("opens"), b"JBSWY3DPEHPK3PXP");

    let second = Harness::new().module(&VAULT).boot().await;
    let refused = second
        .call(Who::A1, "vault.open", json!({ "sealed": sealed }))
        .await;
    let error = refused.error().expect("another key does not open it");
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(error.key, "keel.error.secret_open");
    second.finish().await;
    first.finish().await;

    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let error = Kernel::boot(Composition::new().module(&VAULT), &storage)
        .await
        .expect_err("a composition without a key has no cipher");
    let BootError::Freeze(problems) = &error else {
        panic!("{error}");
    };
    let text = problems.to_string();
    assert!(
        text.contains("module vault requires the service") && text.contains("SecretCipher"),
        "{text}"
    );
    storage.close().await.expect("the pool closes");
    database.finish().await;
}

#[tokio::test]
async fn boot_fills_principal_totp_from_the_second_factor_source_and_the_socket_keeps_false() {
    let bare = Harness::new().boot().await;
    let http = bare.http().await;
    let booted = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(booted.status, 200, "{}", booted.body);
    assert_eq!(booted.json()["principal"]["totp"], json!(false));
    http.stop().await.expect("the server stops");
    bare.finish().await;

    let enrolled = Harness::new().module(&ENROLLED).boot().await;
    let http = enrolled.http().await;
    let booted = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(booted.status, 200, "{}", booted.body);
    assert_eq!(booted.json()["principal"]["login"], json!("a1"));
    assert_eq!(booted.json()["principal"]["totp"], json!(true));
    let guest = http.get(Who::Guest, "/api/v1/boot").await;
    assert_eq!(guest.status, 200, "{}", guest.body);
    assert_eq!(guest.json()["principal"], json!(null));
    let socket = enrolled.socket().await;
    let frame = socket.boot().await;
    let frame = keel_sdk::serde_json::to_value(&frame).expect("an envelope");
    assert_eq!(frame["ok"], json!(true), "{frame}");
    assert_eq!(frame["data"]["principal"]["level"], json!("owner"));
    assert_eq!(frame["data"]["principal"]["totp"], json!(false));
    socket.stop().await.expect("the socket stops");
    http.stop().await.expect("the server stops");
    enrolled.finish().await;
}
