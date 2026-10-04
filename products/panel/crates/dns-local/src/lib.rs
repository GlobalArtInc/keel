use keel::prelude::*;
use keel::types::FieldError;
use keel_hosting_web::api::{AfterCommit, DnsProvider, Invoke, SiteZone, ZoneRecord};

const CREATE: &str = "dns.zone.create";
const LIST: &str = "dns.zone.list";
const CLAIM: &str = "dns.zone.claim";
const RECORD_CREATE: &str = "dns.record.create";
const PAGE: u32 = 100;
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
    async fn add_zone(
        &self,
        invoke: &Invoke<'_>,
        _: &AfterCommit<'_>,
        _: &Id,
        zone: &SiteZone,
    ) -> Result<()> {
        let account = (!zone.own_account).then_some(zone.account);
        invoke(CREATE, json!({ "name": zone.name, "account": account }))
            .await
            .map(drop)
            .map_err(onto_the_site)
    }

    async fn foreign(&self, invoke: &Invoke<'_>, zone: &SiteZone) -> Result<bool> {
        let account = (!zone.own_account).then_some(zone.account);
        let answer = invoke(CLAIM, json!({ "name": zone.name, "account": account })).await?;
        Ok(answer["foreign"].as_bool().unwrap_or(true))
    }

    async fn add_records(
        &self,
        invoke: &Invoke<'_>,
        zone: &SiteZone,
        records: &[ZoneRecord],
    ) -> Result<bool> {
        let page = invoke(LIST, json!({ "search": zone.name, "pageSize": PAGE })).await?;
        let Some(id) = zone_id(&page, zone) else {
            return Ok(false);
        };
        for record in records {
            invoke(
                RECORD_CREATE,
                json!({
                    "zone": id,
                    "type": record.kind,
                    "name": record.name,
                    "value": record.value,
                    "priority": record.priority,
                }),
            )
            .await?;
        }
        Ok(true)
    }
}

fn zone_id(page: &Value, zone: &SiteZone) -> Option<String> {
    let account = zone.account.to_string();
    page["data"]
        .as_array()?
        .iter()
        .find(|row| {
            row["name"].as_str() == Some(zone.name.as_str())
                && match row["accountId"].as_str() {
                    Some(owner) => owner == account,
                    None => zone.own_account,
                }
        })
        .and_then(|row| row["id"].as_str())
        .map(str::to_owned)
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
    fn a_zone_is_found_by_its_exact_name_within_the_account() {
        let account = Id::generate();
        let zone = SiteZone {
            name: "example.org".to_owned(),
            account,
            own_account: true,
        };
        let page = json!({ "data": [
            { "id": "z1", "name": "mail.example.org", "accountId": account.to_string() },
            { "id": "z2", "name": "example.org", "accountId": Id::generate().to_string() },
            { "id": "z3", "name": "example.org", "accountId": account.to_string() },
        ], "total": 3 });
        assert_eq!(zone_id(&page, &zone).as_deref(), Some("z3"));
        assert_eq!(zone_id(&json!({ "data": [], "total": 0 }), &zone), None);
        let scoped = json!({ "data": [{ "id": "z4", "name": "example.org" }], "total": 1 });
        assert_eq!(zone_id(&scoped, &zone).as_deref(), Some("z4"));
        let behalf = SiteZone {
            own_account: false,
            ..zone
        };
        assert_eq!(zone_id(&scoped, &behalf), None);
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
