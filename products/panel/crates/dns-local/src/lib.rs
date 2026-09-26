use keel::prelude::*;
use keel::types::FieldError;
use keel_hosting_web::api::{DnsProvider, Invoke, SiteZone};

const CREATE: &str = "dns.zone.create";
const ZONE_NAME: &str = "name";
const SITE_DOMAIN: &str = "domain";
const SITE_ZONE: &str = "dnsZone";

pub static MODULE: ModuleDef = ModuleDef {
    id: "dns_local",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["hosting_site", "dns"],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn DnsProvider>(Arc::new(Local));
    Ok(())
}

struct Local;

#[async_trait]
impl DnsProvider for Local {
    async fn add_zone(&self, invoke: &Invoke<'_>, zone: &SiteZone) -> Result<()> {
        let account = (!zone.own_account).then_some(zone.account);
        invoke(CREATE, json!({ "name": zone.name, "account": account }))
            .await
            .map(drop)
            .map_err(onto_the_site)
    }
}

fn onto_the_site(error: KeelError) -> KeelError {
    let answer = error.to_call_error();
    if answer.fields.is_empty() {
        return error;
    }
    let fields = answer.fields.into_iter().map(|field| FieldError {
        path: site_field(&field.path).to_owned(),
        ..field
    });
    answer
        .params
        .into_iter()
        .fold(
            KeelError::with_key(answer.code, answer.key),
            |error, (name, value)| error.param(name, value),
        )
        .fields(fields)
}

fn site_field(path: &str) -> &'static str {
    if path == ZONE_NAME {
        SITE_DOMAIN
    } else {
        SITE_ZONE
    }
}

#[cfg(test)]
mod tests {
    use keel::types::ErrorCode;

    use super::*;

    #[test]
    fn a_field_of_the_zone_lands_on_the_matching_field_of_the_site_form() {
        let taken = KeelError::conflict("dns.zone.error.exists")
            .field("name")
            .param("name", "example.com");
        let moved = onto_the_site(taken).to_call_error();
        assert_eq!(moved.code, ErrorCode::Validation);
        assert_eq!(moved.fields.len(), 1);
        assert_eq!(moved.fields[0].path, "domain");
        assert_eq!(
            moved.fields[0].key.as_deref(),
            Some("dns.zone.error.exists")
        );
        assert_eq!(moved.fields[0].params["name"], json!("example.com"));
        let other = KeelError::validation("dns.zone.error.template").field("template");
        assert_eq!(
            onto_the_site(other).to_call_error().fields[0].path,
            "dnsZone"
        );
    }

    #[test]
    fn an_error_without_fields_passes_unchanged() {
        let limited =
            KeelError::limit_exceeded("keel.error.limit_exceeded").param("limit", "dns.zones");
        let passed = onto_the_site(limited).to_call_error();
        assert_eq!(passed.code, ErrorCode::LimitExceeded);
        assert_eq!(passed.key, "keel.error.limit_exceeded");
        assert_eq!(passed.params["limit"], json!("dns.zones"));
        assert!(passed.fields.is_empty());
    }
}
