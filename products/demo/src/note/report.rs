use std::collections::BTreeMap;

use keel::prelude::*;
use keel::sdk::chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use keel::sdk::ui::{Band, Chart, Report};
use keel::types::ChartType;

use super::item;

pub(super) const REPORT: &str = "note.report.tags";
const SERIES: &str = "note.report.series";
const READ: &str = "note.read";
const TAGS: &str = "tags";

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Period {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    to: Option<NaiveDate>,
}

#[derive(Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel::sdk::schemars", rename_all = "lowercase")]
enum By {
    Tag,
    Day,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct SeriesQuery {
    by: By,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    to: Option<NaiveDate>,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Series {
    points: Vec<Point>,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Point {
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    value: u64,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct TagsReport {
    bands: TagsBands,
    charts: TagsCharts,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct TagsBands {
    tags: Vec<TagNotes>,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct TagNotes {
    tag: String,
    notes: u64,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct TagsCharts {
    created: Period,
    tags: Period,
    share: Period,
}

pub(super) fn report() -> FunctionDef {
    function::<Period, TagsReport>(REPORT)
        .kind(Kind::Report)
        .access(Level::User, READ)
        .unowned()
        .ui(Report::new("note.report.tags.title")
            .band(
                Band::new(TAGS)
                    .column(Column::new("tag", ColumnType::Text).primary())
                    .column(Column::new("notes", ColumnType::Number).total(Total::Sum))
                    .formats([ExportFormat::Csv]),
            )
            .chart(Chart::new("created", ChartType::Line, SERIES).params([("by", "day")]))
            .chart(Chart::new(TAGS, ChartType::Bar, SERIES).params([("by", "tag")]))
            .chart(Chart::new("share", ChartType::Pie, SERIES).params([("by", "tag")])))
        .handler(|call: Call<Period>| async move {
            let period = call.params().clone();
            let tags = by_tag(&created(&call, &period).await?)
                .into_iter()
                .map(|(tag, notes)| TagNotes { tag, notes })
                .collect();
            Ok(TagsReport {
                bands: TagsBands { tags },
                charts: TagsCharts {
                    created: period.clone(),
                    tags: period.clone(),
                    share: period,
                },
            })
        })
}

pub(super) fn series() -> FunctionDef {
    function::<SeriesQuery, Series>(SERIES)
        .kind(Kind::Get)
        .access(Level::User, READ)
        .unowned()
        .handler(|call: Call<SeriesQuery>| async move {
            let query = call.params();
            let period = Period {
                from: query.from,
                to: query.to,
            };
            let notes = created(&call, &period).await?;
            let points = match query.by {
                By::Tag => by_tag(&notes)
                    .into_iter()
                    .map(|(tag, value)| Point {
                        at: None,
                        label: Some(tag),
                        value,
                    })
                    .collect(),
                By::Day => by_day(&notes)
                    .into_iter()
                    .map(|(day, value)| Point {
                        at: Some(midnight(day)),
                        label: None,
                        value,
                    })
                    .collect(),
            };
            Ok(Series { points })
        })
}

async fn created<P>(call: &Call<P>, period: &Period) -> Result<Vec<(Vec<String>, DateTime<Utc>)>> {
    let mut notes = call.account_scope().apply(item::Entity::find());
    if let Some(from) = period.from {
        notes = notes.filter(item::Column::CreatedAt.gte(midnight(from)));
    }
    if let Some(to) = period.to.and_then(|to| to.succ_opt()) {
        notes = notes.filter(item::Column::CreatedAt.lt(midnight(to)));
    }
    Ok(notes
        .select_only()
        .column(item::Column::Tags)
        .column(item::Column::CreatedAt)
        .into_tuple::<(Vec<String>, DateTime<Utc>)>()
        .all(call.db())
        .await?)
}

fn by_tag(notes: &[(Vec<String>, DateTime<Utc>)]) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for tag in notes.iter().flat_map(|(tags, _)| tags) {
        *counts.entry(tag.clone()).or_insert(0) += 1;
    }
    counts
}

fn by_day(notes: &[(Vec<String>, DateTime<Utc>)]) -> BTreeMap<NaiveDate, u64> {
    let mut counts = BTreeMap::new();
    for (_, at) in notes {
        *counts.entry(at.date_naive()).or_insert(0) += 1;
    }
    counts
}

fn midnight(day: NaiveDate) -> DateTime<Utc> {
    day.and_time(NaiveTime::MIN).and_utc()
}
