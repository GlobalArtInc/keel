use keel::prelude::*;
use keel::sdk::limit::Unit;
use keel::sdk::sea_orm::{EntityTrait, PaginatorTrait};
use keel::types::StatValue;

use crate::zone;

const WIDGET: &str = "dns.widget.zones";
const READ: &str = "dns.zone.read";
const FEATURE: &str = "dns.zones";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Nothing {}

pub(crate) fn zones(per_account: &'static str, total: &'static str) -> FunctionDef {
    function::<Nothing, StatValue>(WIDGET)
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(FEATURE)
        .unowned()
        .ui(ObjectScreen::new("dns.widget.zones.title"))
        .handler(move |call: Call<Nothing>| async move {
            let scope = *call.account_scope();
            let zones = scope.apply(zone::Entity::find()).count(call.db()).await?;
            let (series, subject) = match scope {
                AccountScope::All => (Some(total), None),
                AccountScope::Own(account) => (Some(per_account), Some(account)),
                AccountScope::Subtree(_) | AccountScope::Nothing => (None, None),
            };
            Ok(StatValue {
                value: zones as f64,
                unit: Unit::Count.as_str().to_owned(),
                series: series.map(str::to_owned),
                subject,
            })
        })
}

pub(crate) fn widget() -> DashboardWidget {
    DashboardWidget::new(WidgetKind::Stat, WIDGET)
}
