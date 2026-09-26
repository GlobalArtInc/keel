use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::system::Installation;
use keel_sdk::types::LicenseState;
use keel_testkit::license::Issuer;
use proptest::prelude::*;
use proptest::test_runner::Config;

use super::lease::{ERROR_INSTANCE, ERROR_LEASE, ERROR_SIGNATURE, Lease, Leased};

const CASES: u32 = 512;
const SECONDS: std::ops::Range<i64> = -8_000_000_000_000..8_000_000_000_000;

fn config() -> Config {
    let from_environment = Config::default();
    Config {
        cases: from_environment.cases.max(CASES),
        failure_persistence: None,
        ..from_environment
    }
}

fn issuer() -> &'static Issuer {
    static ISSUER: std::sync::OnceLock<Issuer> = std::sync::OnceLock::new();
    ISSUER.get_or_init(|| Issuer::generate().expect("a throwaway license key"))
}

fn installation() -> Installation {
    Installation {
        instance: Some(Id::ROOT_ACCOUNT),
        products: Vec::new(),
    }
}

fn instant() -> impl Strategy<Value = DateTime<Utc>> {
    SECONDS.prop_map(|seconds| DateTime::from_timestamp(seconds, 0).expect("an instant in range"))
}

fn scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<u32>().prop_map(Value::from),
        ".{0,12}".prop_map(Value::from),
        instant().prop_map(|at| json!(at)),
        Just(json!(Id::ROOT_ACCOUNT)),
        Just(json!(Id::generate())),
    ]
}

fn leased() -> impl Strategy<Value = Value> {
    prop_oneof![
        3 => ("[a-z]{1,6}", "[a-z]{1,6}").prop_map(|(id, edition)| json!({ "id": id, "edition": edition })),
        1 => scalar(),
    ]
}

fn document() -> impl Strategy<Value = Value> {
    (
        prop_oneof![3 => Just(json!(Id::ROOT_ACCOUNT)), 1 => scalar()],
        prop_oneof![3 => instant().prop_map(|at| json!(at)), 1 => scalar()],
        prop_oneof![3 => instant().prop_map(|at| json!(at)), 1 => scalar()],
        prop_oneof![3 => any::<u32>().prop_map(Value::from), 1 => scalar()],
        prop_oneof![3 => prop::collection::vec(leased(), 0..4).prop_map(Value::from), 1 => scalar()],
        any::<u8>(),
    )
        .prop_map(|(instance, issued, expires, grace, products, drop)| {
            let mut fields = keel_sdk::serde_json::Map::new();
            for (index, (key, value)) in [
                ("instance_id", instance),
                ("issued_at", issued),
                ("expires_at", expires),
                ("grace_days", grace),
                ("products", products),
            ]
            .into_iter()
            .enumerate()
            {
                if usize::from(drop % 8) != index {
                    fields.insert(key.to_owned(), value);
                }
            }
            Value::Object(fields)
        })
}

fn rank(state: LicenseState) -> u8 {
    match state {
        LicenseState::Valid => 0,
        LicenseState::Grace => 1,
        LicenseState::ReadOnly => 2,
        LicenseState::None => 3,
    }
}

fn lease_at(issued_at: DateTime<Utc>, expires_at: DateTime<Utc>, grace_days: u32) -> Lease {
    Lease {
        instance_id: Id::ROOT_ACCOUNT,
        issued_at,
        expires_at,
        grace_days,
        products: vec![Leased {
            id: "panel".to_owned(),
            edition: "pro".to_owned(),
        }],
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn any_text_that_the_issuer_did_not_sign_is_refused_by_its_signature(text in prop_oneof![".{0,256}", "\\{\"payload\":\"[A-Za-z0-9+/=]{0,64}\",\"signature\":\"[A-Za-z0-9+/=]{0,64}\"\\}"]) {
        let refused = Lease::open(&text, issuer().key().as_ref(), &installation()).unwrap_err();
        prop_assert_eq!(refused.key(), ERROR_SIGNATURE);
        prop_assert_eq!(refused.field_path(), Some("lease"));
    }

    #[test]
    fn a_signed_document_opens_as_a_lease_of_this_instance_or_is_refused_as_one(document in document()) {
        let signed = issuer().sign(&document).expect("the document is signed");
        match Lease::open(&signed, issuer().key().as_ref(), &installation()) {
            Ok(lease) => {
                prop_assert_eq!(lease.instance_id, Id::ROOT_ACCOUNT);
                prop_assert_eq!(json!(lease.issued_at), document["issued_at"].clone());
                prop_assert_eq!(json!(lease.expires_at), document["expires_at"].clone());
                prop_assert_eq!(json!(lease.grace_days), document["grace_days"].clone());
            }
            Err(refused) => {
                prop_assert!([ERROR_LEASE, ERROR_INSTANCE].contains(&refused.key()), "{}", refused.key());
                prop_assert_eq!(refused.field_path(), Some("lease"));
            }
        }
    }

    #[test]
    fn a_changed_byte_of_a_signed_lease_never_opens_as_another_lease(
        issued in instant(),
        expires in instant(),
        grace in any::<u32>(),
        at in any::<prop::sample::Index>(),
        byte in any::<u8>(),
    ) {
        let document = json!({
            "instance_id": Id::ROOT_ACCOUNT,
            "issued_at": issued,
            "expires_at": expires,
            "grace_days": grace,
            "products": [{ "id": "panel", "edition": "pro" }],
        });
        let mut signed = issuer().sign(&document).expect("the lease is signed").into_bytes();
        let index = at.index(signed.len());
        signed[index] = byte;
        let text = String::from_utf8_lossy(&signed);
        if let Ok(lease) = Lease::open(&text, issuer().key().as_ref(), &installation()) {
            prop_assert_eq!((lease.issued_at, lease.expires_at, lease.grace_days), (issued, expires, grace));
            prop_assert_eq!(lease.edition("panel"), Some("pro"));
        }
    }

    #[test]
    fn the_state_only_moves_forward_and_grace_never_ends_before_expiry(
        issued in instant(),
        expires in instant(),
        grace in any::<u32>(),
        first in instant(),
        second in instant(),
    ) {
        let lease = lease_at(issued, expires, grace);
        prop_assert!(lease.grace_ends_at() >= lease.expires_at);
        let (earlier, later) = if first <= second { (first, second) } else { (second, first) };
        prop_assert!(rank(lease.state(earlier)) <= rank(lease.state(later)));
        prop_assert_eq!(lease.state(earlier) == LicenseState::Valid, earlier < expires);
        prop_assert_ne!(lease.state(later), LicenseState::None);
    }

    #[test]
    fn of_two_leases_only_the_older_is_ever_refused_as_a_replacement(
        first in instant(),
        second in instant(),
        expires in instant(),
    ) {
        let one = lease_at(first, expires, 0);
        let other = lease_at(second, expires, 0);
        prop_assert!(one.replaces(Some(&other)).is_ok() || other.replaces(Some(&one)).is_ok());
        prop_assert_eq!(one.replaces(Some(&other)).is_err(), first < second);
    }
}
