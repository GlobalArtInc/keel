use keel_sdk::types::Id;
use keel_sdk::{KeelError, Result};

pub const ERROR_HEADER: &str = "peer.error.header";
pub const ERROR_ORIGIN: &str = "peer.error.origin";
pub const ERROR_CUSTOMER: &str = "peer.error.customer";
pub const ORIGIN: &str = "origin";
pub const EXTERNAL_ID: &str = "external_id";
pub const MAX_ORIGIN_NAME: usize = 64;
pub const MAX_EXTERNAL_ID: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Customer<'a> {
    pub origin: &'a str,
    pub external_id: &'a str,
}

fn malformed() -> KeelError {
    KeelError::validation(ERROR_HEADER)
}

pub fn parse(header: &str) -> Result<Customer<'_>> {
    let mut origin = None;
    let mut external_id = None;
    for part in header.split(';') {
        let (name, value) = part.trim().split_once('=').ok_or_else(malformed)?;
        let slot = match name {
            ORIGIN => &mut origin,
            EXTERNAL_ID => &mut external_id,
            _ => return Err(malformed()),
        };
        if slot.replace(value).is_some() {
            return Err(malformed());
        }
    }
    let (Some(origin), Some(external_id)) = (origin, external_id) else {
        return Err(malformed());
    };
    if origin_parts(origin).is_none() || !is_external_id(external_id) {
        return Err(malformed());
    }
    Ok(Customer {
        origin,
        external_id,
    })
}

fn origin_parts(origin: &str) -> Option<(&str, Id)> {
    let (name, instance) = origin.split_once(':')?;
    let mut bytes = name.bytes();
    let leading = bytes.next()?;
    let named = name.len() <= MAX_ORIGIN_NAME
        && leading.is_ascii_lowercase()
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    named.then_some(())?;
    Some((name, instance.parse().ok()?))
}

pub fn names_peer(origin: &str, instance: &str) -> bool {
    origin_parts(origin).is_some_and(|(_, id)| id.to_string() == instance)
}

pub fn names_an_instance(origin: &str) -> bool {
    origin_parts(origin).is_some()
}

pub fn is_external_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_EXTERNAL_ID
        && text
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b';' | b',' | b'"' | b'\\'))
}

pub fn not_reached(customer: &Customer<'_>) -> KeelError {
    KeelError::not_found(ERROR_CUSTOMER)
        .param("origin", customer.origin)
        .param("externalId", customer.external_id)
}

pub fn foreign_origin(customer: &Customer<'_>) -> KeelError {
    KeelError::forbidden(ERROR_ORIGIN).param("origin", customer.origin)
}

#[cfg(test)]
mod tests {
    use keel_sdk::types::ErrorCode;

    use super::*;

    const INSTANCE: &str = "01J9A00000000000000000000A";

    #[test]
    fn the_header_names_an_origin_and_an_external_id_once_each_in_either_order() {
        let header = format!("origin=bill:{INSTANCE}; external_id=client-42");
        assert_eq!(
            parse(&header).unwrap(),
            Customer {
                origin: &format!("bill:{INSTANCE}"),
                external_id: "client-42",
            }
        );
        let swapped = format!("  external_id=acc=7/x@y.z ;origin=crm_2:{INSTANCE}  ");
        let customer = parse(&swapped).unwrap();
        assert_eq!(customer.external_id, "acc=7/x@y.z");
        assert_eq!(customer.origin, format!("crm_2:{INSTANCE}"));
    }

    #[test]
    fn a_header_out_of_its_grammar_is_a_validation_error() {
        let long = "x".repeat(MAX_EXTERNAL_ID + 1);
        let named = "b".repeat(MAX_ORIGIN_NAME + 1);
        for header in [
            String::new(),
            format!("origin=bill:{INSTANCE}"),
            "external_id=client-42".to_owned(),
            format!("origin=bill:{INSTANCE}; external_id=client-42;"),
            format!("origin=bill:{INSTANCE}; external_id=client-42; external_id=client-43"),
            format!("origin=bill:{INSTANCE}; origin=bill:{INSTANCE}; external_id=client-42"),
            format!("origin=bill:{INSTANCE}; external_id=client-42; account=acc-7"),
            format!("Origin=bill:{INSTANCE}; external_id=client-42"),
            format!("origin = bill:{INSTANCE}; external_id=client-42"),
            format!("origin=bill:{INSTANCE}; external_id="),
            format!("origin=bill:{INSTANCE}; external_id=client 42"),
            format!("origin=bill:{INSTANCE}; external_id=client,42"),
            format!("origin=bill:{INSTANCE}; external_id=client\"42"),
            format!("origin=bill:{INSTANCE}; external_id=клиент"),
            format!("origin=bill:{INSTANCE}; external_id={long}"),
            "origin=bill; external_id=client-42".to_owned(),
            "origin=bill:not-an-id; external_id=client-42".to_owned(),
            format!(
                "origin=bill:{}; external_id=client-42",
                INSTANCE.to_lowercase()
            ),
            format!("origin=:{INSTANCE}; external_id=client-42"),
            format!("origin=Bill:{INSTANCE}; external_id=client-42"),
            format!("origin=2bill:{INSTANCE}; external_id=client-42"),
            format!("origin=bi-ll:{INSTANCE}; external_id=client-42"),
            format!("origin={named}:{INSTANCE}; external_id=client-42"),
            format!("origin=bill:{INSTANCE}:x; external_id=client-42"),
            format!("origin=bill:{INSTANCE}, external_id=client-42"),
        ] {
            let error = parse(&header).expect_err(&header);
            assert_eq!(error.code(), ErrorCode::Validation, "{header}");
            assert_eq!(error.key(), ERROR_HEADER, "{header}");
        }
        let longest = "x".repeat(MAX_EXTERNAL_ID);
        parse(&format!("origin=bill:{INSTANCE}; external_id={longest}")).unwrap();
    }

    #[test]
    fn an_origin_names_the_peer_whose_instance_it_carries_whatever_its_name() {
        assert!(names_peer(&format!("bill:{INSTANCE}"), INSTANCE));
        assert!(names_peer(&format!("panel:{INSTANCE}"), INSTANCE));
        assert!(!names_peer(
            &format!("bill:{INSTANCE}"),
            "01J9B00000000000000000000B"
        ));
        assert!(!names_peer(INSTANCE, INSTANCE));
        assert!(!names_peer(&format!("Bill:{INSTANCE}"), INSTANCE));
        assert!(!names_peer("", INSTANCE));
    }

    #[test]
    fn an_origin_names_an_instance_exactly_when_it_could_name_a_peer() {
        for origin in [
            format!("bill:{INSTANCE}"),
            format!("crm_2:{INSTANCE}"),
            format!("{}:{INSTANCE}", "b".repeat(MAX_ORIGIN_NAME)),
        ] {
            assert!(names_an_instance(&origin), "{origin}");
            assert!(names_peer(&origin, INSTANCE), "{origin}");
        }
        for origin in [
            String::new(),
            "erp".to_owned(),
            "erp:7".to_owned(),
            INSTANCE.to_owned(),
            format!(":{INSTANCE}"),
            format!("Bill:{INSTANCE}"),
            format!("bill:{}", INSTANCE.to_lowercase()),
            format!("bill:{INSTANCE}:x"),
            format!("{}:{INSTANCE}", "b".repeat(MAX_ORIGIN_NAME + 1)),
        ] {
            assert!(!names_an_instance(&origin), "{origin}");
            assert!(!names_peer(&origin, INSTANCE), "{origin}");
        }
    }

    #[test]
    fn the_refusals_name_what_the_peer_sent() {
        let origin = format!("bill:{INSTANCE}");
        let customer = Customer {
            origin: &origin,
            external_id: "client-42",
        };
        let missing = not_reached(&customer);
        assert_eq!(missing.code(), ErrorCode::NotFound);
        assert_eq!(missing.key(), ERROR_CUSTOMER);
        assert_eq!(missing.params()["origin"], origin.as_str());
        assert_eq!(missing.params()["externalId"], "client-42");
        let foreign = foreign_origin(&customer);
        assert_eq!(foreign.code(), ErrorCode::Forbidden);
        assert_eq!(foreign.key(), ERROR_ORIGIN);
        assert_eq!(foreign.params()["origin"], origin.as_str());
    }
}
