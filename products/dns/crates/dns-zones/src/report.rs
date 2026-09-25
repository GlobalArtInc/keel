use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Days, NaiveDate, NaiveTime, Utc};
use keel::sdk::sea_orm;
use keel::sdk::sea_orm::sea_query::{Expr, Func, SimpleExpr};
use keel::sdk::sea_orm::{
    ColumnTrait, EntityTrait, FromQueryResult, QueryFilter, QueryOrder, QuerySelect,
};
use keel::sdk::ui::{Band, Chart, Report};
use keel::types::ChartType;

use crate::zone;

pub(crate) const REPORT: &str = "dns.report.zones";
const READ: &str = "dns.zone.read";
const FEATURE: &str = "dns.zones";
const SERIES: &str = "stats.series";
const PERIOD: &str = "dns.report.error.period";
const DEFAULT_DAYS: u64 = 29;
const ZONES: &str = "zones";
const CREATED: &str = "created";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ZonesQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    to: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-keel" = json!({ "level": "admin", "options": "keel.account.options" })))]
    account: Option<Id>,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ZonesReport {
    bands: ZonesBands,
    charts: ZonesCharts,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ZonesBands {
    accounts: Vec<AccountZones>,
}

#[derive(Serialize, JsonSchema, FromQueryResult)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel::sdk::schemars", rename_all = "camelCase")]
struct AccountZones {
    account_id: String,
    zones: i64,
    created: i64,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ZonesCharts {
    zones: SeriesWindow,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct SeriesWindow {
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<Id>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
}

pub(crate) fn zones() -> FunctionDef {
    function::<ZonesQuery, ZonesReport>(REPORT)
        .kind(Kind::Report)
        .access(Level::User, READ)
        .feature(FEATURE)
        .unowned()
        .ui(Report::new("dns.report.zones.title")
            .band(
                Band::new("accounts")
                    .column(Column::new("accountId", ColumnType::Text).primary())
                    .column(Column::new(ZONES, ColumnType::Number).total(Total::Sum))
                    .column(Column::new(CREATED, ColumnType::Number).total(Total::Sum))
                    .formats([ExportFormat::Csv, ExportFormat::Json]),
            )
            .chart(
                Chart::new(ZONES, ChartType::Line, SERIES)
                    .params([("key", crate::LIMIT), ("resolution", "day")]),
            ))
        .handler(|call: Call<ZonesQuery>| async move {
            let today = call.require::<dyn Clock>()?.now().date_naive();
            let (from, to) = window(call.params(), today)?;
            let mut zones = call.account_scope().apply(zone::Entity::find());
            if let Some(account) = call.params().account {
                zones = zones.filter(zone::Column::AccountId.eq(account.to_string()));
            }
            let accounts = zones
                .filter(zone::Column::CreatedAt.lt(to))
                .select_only()
                .column(zone::Column::AccountId)
                .column_as(
                    SimpleExpr::from(Func::count(Expr::col(zone::Column::Id))),
                    ZONES,
                )
                .column_as(
                    SimpleExpr::from(Func::sum(
                        Expr::case(Expr::col(zone::Column::CreatedAt).gte(from), 1).finally(0),
                    )),
                    CREATED,
                )
                .group_by(zone::Column::AccountId)
                .order_by_asc(zone::Column::AccountId)
                .into_model::<AccountZones>()
                .all(call.db())
                .await?;
            Ok(ZonesReport {
                bands: ZonesBands { accounts },
                charts: ZonesCharts {
                    zones: SeriesWindow {
                        subject: call.params().account,
                        from,
                        to,
                    },
                },
            })
        })
}

fn window(query: &ZonesQuery, today: NaiveDate) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let last = query.to.unwrap_or(today);
    let first = query.from.unwrap_or_else(|| {
        last.checked_sub_days(Days::new(DEFAULT_DAYS))
            .unwrap_or(last)
    });
    let end = last.checked_add_days(Days::new(1));
    match end {
        Some(end) if first <= last => Ok((midnight(first), midnight(end))),
        _ => Err(KeelError::validation(PERIOD)
            .field("to")
            .param("from", first.to_string())
            .param("to", last.to_string())),
    }
}

fn midnight(day: NaiveDate) -> DateTime<Utc> {
    day.and_time(NaiveTime::MIN).and_utc()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        text.parse().expect("a date")
    }

    fn query(from: Option<&str>, to: Option<&str>) -> ZonesQuery {
        ZonesQuery {
            from: from.map(day),
            to: to.map(day),
            account: None,
        }
    }

    #[test]
    fn the_period_covers_whole_utc_days_and_defaults_to_thirty_days_up_to_today() {
        let today = day("2026-09-25");
        let (from, to) = window(&query(None, None), today).expect("a window");
        assert_eq!(from.to_rfc3339(), "2026-08-27T00:00:00+00:00");
        assert_eq!(to.to_rfc3339(), "2026-09-26T00:00:00+00:00");
        let (from, to) =
            window(&query(Some("2026-09-01"), Some("2026-09-01")), today).expect("one day");
        assert_eq!(from.to_rfc3339(), "2026-09-01T00:00:00+00:00");
        assert_eq!(to.to_rfc3339(), "2026-09-02T00:00:00+00:00");
        let reversed = window(&query(Some("2026-09-02"), Some("2026-09-01")), today)
            .expect_err("the end before the start");
        assert_eq!(reversed.key(), PERIOD);
        assert_eq!(reversed.field_path(), Some("to"));
    }
}
