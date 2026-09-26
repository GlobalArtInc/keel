use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_system::auth::{
    ADDRESS_SET, MODULE as AUTH, REASON_RESET, RESET_CONFIRM, RESET_PER_CLIENT, RESET_PER_LOGIN,
    RESET_PREFIX, RESET_REQUEST, SETTING_SHELL_URL,
};
use keel_system::notify::{CHANNEL_SET, MODULE as NOTIFY};
use keel_testkit::{Answer, Harness, TestHttp, Who, tokio};

use super::notify_mail::{Catcher, catcher, request, wait_until};

const PASSWORD: &str = "correct-horse";
const RENEWED: &str = "battery-staple-renewed";
const SHELL: &str = "https://panel.example.org:9443";
const JSON: (&str, &str) = ("content-type", "application/json");

struct Stand {
    harness: Harness,
    http: TestHttp,
    address: String,
}

async fn stand(catcher: &Catcher) -> Stand {
    let harness = Harness::new()
        .module(&AUTH)
        .module(&keel_system::settings::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&NOTIFY)
        .boot()
        .await;
    let run = Id::generate().to_string().to_lowercase();
    let address = format!("reset-{run}@example.org");
    for (func, params) in [
        (
            CHANNEL_SET,
            json!({
                "kind": "email",
                "name": format!("catcher {run}"),
                "enabled": true,
                "email": {
                    "host": catcher.smtp_host,
                    "port": catcher.smtp_port,
                    "security": "none",
                    "from": "keel@example.org",
                    "to": "ops@example.org"
                }
            }),
        ),
        (
            "settings.set",
            json!({ "key": SETTING_SHELL_URL, "value": SHELL }),
        ),
        (
            "auth.password.set",
            json!({ "principal": harness.principal(Who::A1).id, "password": PASSWORD }),
        ),
        (
            ADDRESS_SET,
            json!({ "principal": harness.principal(Who::A1).id, "value": address }),
        ),
    ] {
        let reply = harness.call(Who::Admin, func, params).await;
        assert!(reply.data().is_some(), "{func}: {:?}", reply.error());
    }
    let http = harness.http().await;
    Stand {
        harness,
        http,
        address,
    }
}

async fn guest(http: &TestHttp, func: &str, params: Value) -> Answer {
    http.request(
        "POST",
        &format!("/api/v1/call/{func}"),
        &[JSON],
        Some(&params.to_string()),
    )
    .await
}

async fn login(http: &TestHttp, password: &str) -> Answer {
    guest(
        http,
        "auth.login",
        json!({ "login": Who::A1.login(), "password": password }),
    )
    .await
}

async fn mails_to(catcher: &Catcher, address: &str) -> Vec<Value> {
    let Some((status, body)) = request(
        &catcher.api,
        "GET",
        &format!("/api/v1/search?query=to%3A{address}&limit=50"),
    )
    .await
    else {
        return Vec::new();
    };
    assert_eq!(status, 200, "{body}");
    let found: Value =
        serde_json::from_str(&body).unwrap_or_else(|error| panic!("{error}: {body}"));
    found["messages"].as_array().cloned().unwrap_or_default()
}

async fn text_of(catcher: &Catcher, mail: &Value) -> String {
    let id = mail["ID"].as_str().expect("a message id");
    let (status, body) = request(&catcher.api, "GET", &format!("/api/v1/message/{id}"))
        .await
        .expect("the catcher answers");
    assert_eq!(status, 200, "{body}");
    let message: Value =
        serde_json::from_str(&body).unwrap_or_else(|error| panic!("{error}: {body}"));
    message["Text"].as_str().expect("a text body").to_owned()
}

async fn wait_for_mails(catcher: &Catcher, address: &str, count: usize) -> Vec<Value> {
    wait_until(&format!("{count} mail(s) reach {address}"), || async {
        mails_to(catcher, address).await.len() >= count
    })
    .await;
    mails_to(catcher, address).await
}

fn link_in(text: &str) -> String {
    let start = text
        .find(&format!("{SHELL}/reset?token={RESET_PREFIX}"))
        .unwrap_or_else(|| panic!("no reset link in {text}"));
    text[start..]
        .split_whitespace()
        .next()
        .expect("the link")
        .to_owned()
}

async fn count(harness: &Harness, sql: &str) -> String {
    harness.database().rows(sql).await.concat()
}

#[tokio::test]
async fn a_reset_mails_a_single_use_link_that_sets_the_password_and_revokes_sessions() {
    let catcher = catcher().await;
    let Stand {
        harness,
        http,
        address,
    } = stand(&catcher).await;
    let signed_in = login(&http, PASSWORD).await;
    assert_eq!(signed_in.status, 200, "{}", signed_in.body);
    let cookie = signed_in.session_cookie().expect("a session cookie");
    let asked = guest(&http, RESET_REQUEST, json!({ "login": Who::A1.login() })).await;
    assert_eq!(asked.status, 200, "{}", asked.body);
    assert_eq!(asked.json()["data"], json!({}));
    let mails = wait_for_mails(&catcher, &address, 1).await;
    assert_eq!(mails.len(), 1);
    assert_eq!(
        mails[0]["Subject"],
        json!(format!("Reset the password of {}", Who::A1.login()))
    );
    let text = text_of(&catcher, &mails[0]).await;
    let link = link_in(&text);
    let token = link
        .split_once("token=")
        .map(|(_, token)| token.to_owned())
        .expect("a token");
    assert_eq!(token.len(), RESET_PREFIX.len() + 64, "{link}");
    assert!(text.contains("60 minutes"), "{text}");

    let short = guest(
        &http,
        RESET_CONFIRM,
        json!({ "token": token, "password": "short" }),
    )
    .await;
    assert_eq!(short.status, 422, "{}", short.body);
    let set = guest(
        &http,
        RESET_CONFIRM,
        json!({ "token": token, "password": RENEWED }),
    )
    .await;
    assert_eq!(set.status, 200, "{}", set.body);
    assert_eq!(set.json()["data"], json!({ "login": Who::A1.login() }));
    assert!(set.session_cookie().is_none());

    let gone = http
        .request(
            "GET",
            "/api/v1/boot",
            &[("cookie", cookie.as_str()), JSON],
            None,
        )
        .await;
    assert_eq!(gone.status, 401, "{}", gone.body);
    assert_eq!(
        harness
            .database()
            .rows("SELECT reason FROM auth_session WHERE revoked_at IS NOT NULL")
            .await,
        [REASON_RESET]
    );
    let again = guest(
        &http,
        RESET_CONFIRM,
        json!({ "token": token, "password": "another-long-password" }),
    )
    .await;
    assert_eq!(again.status, 422, "{}", again.body);
    assert_eq!(
        again.json()["error"]["fields"][0]["key"],
        json!("auth.error.reset_token")
    );
    let old = login(&http, PASSWORD).await;
    assert_eq!(old.status, 401, "{}", old.body);
    let new = login(&http, RENEWED).await;
    assert_eq!(new.status, 200, "{}", new.body);
    assert!(new.session_cookie().is_some());
    harness.finish().await;
}

#[tokio::test]
async fn an_unknown_login_answers_the_same_and_sends_nothing() {
    let catcher = catcher().await;
    let Stand {
        harness,
        http,
        address,
    } = stand(&catcher).await;
    let unknown = guest(&http, RESET_REQUEST, json!({ "login": "nobody-here" })).await;
    let without_address = guest(&http, RESET_REQUEST, json!({ "login": Who::A2.login() })).await;
    let known = guest(&http, RESET_REQUEST, json!({ "login": Who::A1.login() })).await;
    for answer in [&unknown, &without_address, &known] {
        assert_eq!(answer.status, 200, "{}", answer.body);
        assert_eq!(answer.json()["data"], known.json()["data"]);
        assert!(answer.session_cookie().is_none());
    }
    wait_for_mails(&catcher, &address, 1).await;
    assert_eq!(
        count(&harness, "SELECT count(*)::text FROM notify_direct").await,
        "1"
    );
    assert_eq!(
        count(&harness, "SELECT count(*)::text FROM auth_mail_token").await,
        "1"
    );
    assert_eq!(
        count(
            &harness,
            &format!(
                "SELECT count(*)::text FROM notify_direct WHERE address = '{address}' AND principal_id = '{}'",
                harness.principal(Who::A1).id
            ),
        )
        .await,
        "1"
    );
    harness.finish().await;
}

#[tokio::test]
async fn an_owner_is_never_mailed_a_reset_link() {
    let catcher = catcher().await;
    let Stand { harness, http, .. } = stand(&catcher).await;
    let owner = harness.principal(Who::A2).id;
    let run = Id::generate().to_string().to_lowercase();
    let raised = harness
        .database()
        .rows(&format!(
            "UPDATE keel_principal SET level = 'owner' WHERE id = '{owner}' RETURNING level"
        ))
        .await;
    assert_eq!(raised, vec!["owner".to_owned()]);
    let inserted = harness
        .database()
        .rows(&format!(
            "INSERT INTO auth_principal_address (principal_id, kind, value, verified_at, updated_at) \
             VALUES ('{owner}', 'email', 'owner-{run}@example.org', now(), now()) RETURNING kind"
        ))
        .await;
    assert_eq!(inserted, vec!["email".to_owned()]);
    let asked = guest(&http, RESET_REQUEST, json!({ "login": Who::A2.login() })).await;
    assert_eq!(asked.status, 200, "{}", asked.body);
    assert_eq!(asked.json()["data"], json!({}));
    assert_eq!(
        count(&harness, "SELECT count(*)::text FROM auth_mail_token").await,
        "0"
    );
    assert_eq!(
        count(&harness, "SELECT count(*)::text FROM notify_direct").await,
        "0"
    );
    harness.finish().await;
}

#[tokio::test]
async fn the_reset_request_is_rate_limited() {
    let catcher = catcher().await;
    let Stand {
        harness,
        http,
        address,
    } = stand(&catcher).await;
    let limited = |answer: &Answer| {
        assert_eq!(answer.status, 429, "{}", answer.body);
        assert_eq!(
            answer.json()["error"]["key"],
            json!("auth.error.reset_limited")
        );
    };
    for login in [Who::A1.login(), "nobody-here"] {
        for _ in 0..RESET_PER_LOGIN {
            let answer = guest(&http, RESET_REQUEST, json!({ "login": login })).await;
            assert_eq!(answer.status, 200, "{login}: {}", answer.body);
        }
        limited(&guest(&http, RESET_REQUEST, json!({ "login": login })).await);
    }
    let spent = 2 * RESET_PER_LOGIN;
    for n in spent..RESET_PER_CLIENT {
        let answer = guest(
            &http,
            RESET_REQUEST,
            json!({ "login": format!("ghost{n}") }),
        )
        .await;
        assert_eq!(answer.status, 200, "{}", answer.body);
    }
    limited(&guest(&http, RESET_REQUEST, json!({ "login": "one-more" })).await);
    let mails = wait_for_mails(&catcher, &address, 3).await;
    assert_eq!(
        mails.len(),
        usize::try_from(RESET_PER_LOGIN).expect("a count")
    );
    assert_eq!(
        count(&harness, "SELECT count(*)::text FROM auth_mail_token").await,
        RESET_PER_LOGIN.to_string()
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_principal_confirms_its_own_address_through_the_mailed_link() {
    let catcher = catcher().await;
    let Stand { harness, http, .. } = stand(&catcher).await;
    let run = Id::generate().to_string().to_lowercase();
    let own = format!("own-{run}@example.org");
    let seeded = harness
        .call(
            Who::Admin,
            "auth.password.set",
            json!({ "principal": harness.principal(Who::A2).id, "password": PASSWORD }),
        )
        .await;
    assert!(seeded.data().is_some(), "{:?}", seeded.error());
    let missing = harness
        .call(Who::A2, ADDRESS_SET, json!({ "value": own }))
        .await;
    let missing = missing
        .error()
        .expect("a self-set without current is refused");
    assert_eq!(missing.key, "keel.error.validation");
    assert_eq!(
        missing
            .fields
            .iter()
            .map(|field| (field.path.as_str(), field.key.as_deref()))
            .collect::<Vec<_>>(),
        [("current", Some("auth.error.current_required"))]
    );
    let wrong = harness
        .call(
            Who::A2,
            ADDRESS_SET,
            json!({ "value": own, "current": RENEWED }),
        )
        .await;
    assert_eq!(
        wrong.error().map(|error| error.key.as_str()),
        Some("auth.error.credentials")
    );
    let unset = harness.call(Who::A2, "auth.address.get", json!({})).await;
    assert_eq!(
        unset.data().map(|view| view["value"].clone()),
        Some(Value::Null)
    );
    let set = harness
        .call(
            Who::A2,
            ADDRESS_SET,
            json!({ "value": own, "current": PASSWORD }),
        )
        .await;
    let view = set
        .data()
        .unwrap_or_else(|| panic!("the address is set: {:?}", set.error()));
    assert_eq!(view["value"], json!(own));
    assert_eq!(view["verified"], json!(false));
    let foreign = harness
        .call(
            Who::A2,
            ADDRESS_SET,
            json!({ "principal": harness.principal(Who::A1).id, "value": own }),
        )
        .await;
    assert_eq!(
        foreign.error().map(|error| error.key.as_str()),
        Some("keel.error.forbidden_field")
    );
    let mails = wait_for_mails(&catcher, &own, 1).await;
    assert_eq!(mails.len(), 1, "only the checked request mails a link");
    let text = text_of(&catcher, &mails[0]).await;
    let start = text
        .find(&format!("{SHELL}/verify?token=kav_"))
        .unwrap_or_else(|| panic!("no confirmation link in {text}"));
    let token = text[start..]
        .split_whitespace()
        .next()
        .and_then(|link| link.split_once("token="))
        .map(|(_, token)| token.to_owned())
        .expect("a token");
    let confirmed = guest(&http, "auth.address.verify", json!({ "token": token })).await;
    assert_eq!(confirmed.status, 200, "{}", confirmed.body);
    assert_eq!(confirmed.json()["data"], json!({ "value": own }));
    let again = guest(&http, "auth.address.verify", json!({ "token": token })).await;
    assert_eq!(again.status, 422, "{}", again.body);
    let read = harness.call(Who::A2, "auth.address.get", json!({})).await;
    assert_eq!(
        read.data().map(|view| view["verified"].clone()),
        Some(json!(true))
    );
    harness.finish().await;
}
