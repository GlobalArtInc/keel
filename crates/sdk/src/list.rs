use std::marker::PhantomData;
use std::str::FromStr;

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use keel_types::{Filter, FilterOp, FilterValue, ListQuery, Page, Sort, SortDir, Total};
use schemars::JsonSchema;
use sea_orm::sea_query::extension::postgres::PgExpr;
use sea_orm::sea_query::{Condition, Expr, SimpleExpr};
use sea_orm::{
    ColumnTrait, ColumnType, EntityTrait, Order, PaginatorTrait, QueryFilter, QueryOrder, Select,
    Value,
};
use serde::Serialize;

use crate::access::Owned;
use crate::call::Call;
use crate::error::{KeelError, Result};
use crate::function::Limits;
use crate::ui::{UiDef, UiError};

pub const DEFAULT_MAX_PAGE_SIZE: u32 = 200;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListSpec {
    pub search: Vec<String>,
    pub sortable: Vec<String>,
    pub filterable: Vec<String>,
    pub totals: Vec<(String, Total)>,
    pub max_page_size: u32,
    pub default_sort: Option<Sort>,
}

impl ListSpec {
    pub fn new(ui: Option<&UiDef>, limits: &Limits) -> Self {
        let screen = ui.map(UiDef::screen);
        Self {
            search: screen.map(|ui| ui.search.clone()).unwrap_or_default(),
            sortable: screen
                .map(|ui| {
                    ui.columns
                        .iter()
                        .filter(|column| column.sortable)
                        .map(|column| column.key.clone())
                        .collect()
                })
                .unwrap_or_default(),
            filterable: screen
                .map(|ui| ui.columns.iter().map(|column| column.key.clone()).collect())
                .unwrap_or_default(),
            totals: screen
                .map(|ui| {
                    ui.columns
                        .iter()
                        .filter_map(|column| column.total.map(|total| (column.key.clone(), total)))
                        .collect()
                })
                .unwrap_or_default(),
            max_page_size: limits.max_page_size.unwrap_or(DEFAULT_MAX_PAGE_SIZE),
            default_sort: limits.default_sort.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), UiError> {
        match &self.default_sort {
            Some(sort) if !self.sortable.contains(&sort.key) => Err(UiError::Misuse(format!(
                "default sort {:?} is not a sortable column of the list",
                sort.key
            ))),
            _ => Ok(()),
        }
    }

    pub fn check(&self, mut query: ListQuery) -> Result<ListQuery> {
        if query.page == 0 {
            return Err(KeelError::validation("keel.error.list.page").field("page"));
        }
        if query.page_size == 0 {
            return Err(KeelError::validation("keel.error.list.page_size").field("pageSize"));
        }
        query.page_size = query.page_size.min(self.max_page_size);
        if query.sort.is_none() {
            query.sort = self.default_sort.clone();
        }
        if let Some(sort) = &query.sort
            && !self.sortable.contains(&sort.key)
        {
            return Err(KeelError::validation("keel.error.list.not_sortable")
                .field("sort.key")
                .param("field", sort.key.clone()));
        }
        for (index, filter) in query.filters.iter().enumerate() {
            if !self.filterable.contains(&filter.key) {
                return Err(KeelError::validation("keel.error.list.not_filterable")
                    .field(format!("filters[{index}].key"))
                    .param("field", filter.key.clone()));
            }
        }
        Ok(query)
    }
}

#[async_trait]
pub trait ListSource: Send + Sync {
    type Row: Serialize + JsonSchema + Send;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<Self::Row>>;
}

pub struct EntitySource<E> {
    entity: PhantomData<fn() -> E>,
}

pub fn entity_source<E: Owned>() -> EntitySource<E> {
    EntitySource {
        entity: PhantomData,
    }
}

impl<E> Clone for EntitySource<E> {
    fn clone(&self) -> Self {
        Self {
            entity: PhantomData,
        }
    }
}

impl<E> std::fmt::Debug for EntitySource<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EntitySource")
    }
}

#[async_trait]
impl<E> ListSource for EntitySource<E>
where
    E: Owned,
    E::Model: Serialize + JsonSchema + Send + Sync,
{
    type Row = E::Model;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<E::Model>> {
        fetch_page(call.account_scope().apply(E::find()), call, spec).await
    }
}

pub async fn fetch_page<E>(
    mut select: Select<E>,
    call: &Call<ListQuery>,
    spec: &ListSpec,
) -> Result<Page<E::Model>>
where
    E: EntityTrait,
    E::Model: Send + Sync,
{
    let query = call.params();
    if !query.search.is_empty() && !spec.search.is_empty() {
        let pattern = format!("%{}%", escape_like(&query.search));
        let mut any = Condition::any();
        for key in &spec.search {
            let column = column::<E>(key, "search")?;
            any = any.add(Expr::col(column.as_column_ref()).ilike(pattern.as_str()));
        }
        select = select.filter(any);
    }
    for (index, filter) in query.filters.iter().enumerate() {
        select = select.filter(filter_expr::<E>(filter, index, call.tz())?);
    }
    if let Some(sort) = &query.sort {
        let order = match sort.dir {
            SortDir::Asc => Order::Asc,
            SortDir::Desc => Order::Desc,
        };
        select = select.order_by(column::<E>(&sort.key, "sort.key")?, order);
    }
    let paginator = select.paginate(call.db(), u64::from(query.page_size));
    let total = paginator.num_items().await?;
    let data = paginator.fetch_page(u64::from(query.page) - 1).await?;
    Ok(Page {
        data,
        total,
        totals: None,
    })
}

fn column<E: EntityTrait>(key: &str, path: &str) -> Result<E::Column> {
    E::Column::from_str(key).map_err(|_| {
        KeelError::validation("keel.error.list.unknown_field")
            .field(path)
            .param("field", key)
    })
}

fn filter_expr<E: EntityTrait>(filter: &Filter, index: usize, tz: Tz) -> Result<SimpleExpr> {
    let path = format!("filters[{index}].value");
    let column = column::<E>(&filter.key, &format!("filters[{index}].key"))?;
    let column_type = column.def().get_column_type().clone();
    if matches!(column_type, ColumnType::Array(_)) {
        return Err(KeelError::validation("keel.error.list.not_filterable")
            .field(format!("filters[{index}].key"))
            .param("field", filter.key.clone()));
    }
    if is_instant(&column_type) {
        return instant_expr(column, filter, index, tz);
    }
    let convert = |text: &str| typed_value(&column_type, text, &path);
    Ok(match (&filter.value, filter.op) {
        (FilterValue::One(text), FilterOp::Eq) => column.eq(convert(text)?),
        (FilterValue::One(text), FilterOp::Ne) => column.ne(convert(text)?),
        (FilterValue::One(text), FilterOp::Gt) => column.gt(convert(text)?),
        (FilterValue::One(text), FilterOp::Gte) => column.gte(convert(text)?),
        (FilterValue::One(text), FilterOp::Lt) => column.lt(convert(text)?),
        (FilterValue::One(text), FilterOp::Lte) => column.lte(convert(text)?),
        (FilterValue::Many(texts), FilterOp::Eq) => column.is_in(
            texts
                .iter()
                .map(|text| convert(text))
                .collect::<Result<Vec<_>>>()?,
        ),
        (FilterValue::Many(texts), FilterOp::Ne) => column.is_not_in(
            texts
                .iter()
                .map(|text| convert(text))
                .collect::<Result<Vec<_>>>()?,
        ),
        (FilterValue::Many(_), _) => {
            return Err(KeelError::validation("keel.error.list.list_value_op")
                .field(format!("filters[{index}].op"))
                .param("field", filter.key.clone()));
        }
    })
}

fn is_instant(column_type: &ColumnType) -> bool {
    matches!(
        column_type,
        ColumnType::TimestampWithTimeZone | ColumnType::Timestamp | ColumnType::DateTime
    )
}

enum Bound {
    Instant(DateTime<Utc>),
    Day {
        start: DateTime<Utc>,
        next: DateTime<Utc>,
    },
}

fn day_start(tz: Tz, date: NaiveDate) -> Option<DateTime<Utc>> {
    [NaiveTime::MIN, NaiveTime::from_hms_opt(1, 0, 0)?]
        .into_iter()
        .find_map(|time| {
            tz.from_local_datetime(&date.and_time(time))
                .earliest()
                .map(|at| at.with_timezone(&Utc))
        })
}

fn bound(text: &str, tz: Tz, path: &str) -> Result<Bound> {
    let mismatch = || {
        KeelError::validation("keel.error.list.filter_value")
            .field(path)
            .param("value", text)
    };
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let start = day_start(tz, date).ok_or_else(mismatch)?;
        let next = date
            .succ_opt()
            .and_then(|tomorrow| day_start(tz, tomorrow))
            .unwrap_or(start + Duration::days(1));
        return Ok(Bound::Day { start, next });
    }
    DateTime::parse_from_rfc3339(text)
        .map(|at| Bound::Instant(at.with_timezone(&Utc)))
        .map_err(|_| mismatch())
}

fn instant_one<C: ColumnTrait>(column: C, bound: Bound, op: FilterOp) -> SimpleExpr {
    match (bound, op) {
        (Bound::Instant(at), FilterOp::Eq) => column.eq(at),
        (Bound::Instant(at), FilterOp::Ne) => column.ne(at),
        (Bound::Instant(at), FilterOp::Gt) => column.gt(at),
        (Bound::Instant(at), FilterOp::Gte) => column.gte(at),
        (Bound::Instant(at), FilterOp::Lt) => column.lt(at),
        (Bound::Instant(at), FilterOp::Lte) => column.lte(at),
        (Bound::Day { start, next }, FilterOp::Eq) => column.gte(start).and(column.lt(next)),
        (Bound::Day { start, next }, FilterOp::Ne) => column.lt(start).or(column.gte(next)),
        (Bound::Day { next, .. }, FilterOp::Gt) => column.gte(next),
        (Bound::Day { start, .. }, FilterOp::Gte) => column.gte(start),
        (Bound::Day { start, .. }, FilterOp::Lt) => column.lt(start),
        (Bound::Day { next, .. }, FilterOp::Lte) => column.lt(next),
    }
}

fn instant_expr<C: ColumnTrait>(
    column: C,
    filter: &Filter,
    index: usize,
    tz: Tz,
) -> Result<SimpleExpr> {
    let path = format!("filters[{index}].value");
    match (&filter.value, filter.op) {
        (FilterValue::One(text), op) => Ok(instant_one(column, bound(text, tz, &path)?, op)),
        (FilterValue::Many(texts), op @ (FilterOp::Eq | FilterOp::Ne)) => {
            let mut condition = if op == FilterOp::Eq {
                Condition::any()
            } else {
                Condition::all()
            };
            for text in texts {
                condition = condition.add(instant_one(column, bound(text, tz, &path)?, op));
            }
            Ok(condition.into())
        }
        (FilterValue::Many(_), _) => Err(KeelError::validation("keel.error.list.list_value_op")
            .field(format!("filters[{index}].op"))
            .param("field", filter.key.clone())),
    }
}

fn typed_value(column_type: &ColumnType, text: &str, path: &str) -> Result<Value> {
    let mismatch = || {
        KeelError::validation("keel.error.list.filter_value")
            .field(path)
            .param("value", text)
    };
    Ok(match column_type {
        ColumnType::TinyInteger
        | ColumnType::SmallInteger
        | ColumnType::Integer
        | ColumnType::BigInteger
        | ColumnType::TinyUnsigned
        | ColumnType::SmallUnsigned
        | ColumnType::Unsigned
        | ColumnType::BigUnsigned => Value::BigInt(Some(text.parse().map_err(|_| mismatch())?)),
        ColumnType::Float | ColumnType::Double => {
            Value::Double(Some(text.parse().map_err(|_| mismatch())?))
        }
        ColumnType::Boolean => Value::Bool(Some(match text {
            "true" | "1" => true,
            "false" | "0" => false,
            _ => return Err(mismatch()),
        })),
        ColumnType::Date => Value::ChronoDate(Some(Box::new(
            NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| mismatch())?,
        ))),
        _ => Value::from(text),
    })
}

pub fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_types::FilterValue;
    use sea_orm::{DbBackend, QueryTrait};

    mod item {
        use sea_orm::entity::prelude::*;
        use serde::Serialize;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, schemars::JsonSchema)]
        #[sea_orm(table_name = "memo_item")]
        pub struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub id: String,
            pub account_id: String,
            pub title: String,
            pub weight: i32,
            pub pinned: bool,
            pub due: Option<Date>,
            pub tags: Vec<String>,
            pub updated_at: DateTimeUtc,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    impl Owned for item::Entity {
        fn account_column() -> Self::Column {
            item::Column::AccountId
        }
    }

    fn spec() -> ListSpec {
        ListSpec {
            search: vec!["title".into()],
            sortable: vec!["title".into(), "weight".into()],
            filterable: vec!["title".into(), "weight".into(), "pinned".into()],
            totals: Vec::new(),
            max_page_size: 50,
            default_sort: Some(Sort {
                key: "title".into(),
                dir: SortDir::Asc,
            }),
        }
    }

    #[test]
    fn check_caps_the_page_size_and_applies_the_default_sort() {
        let query = spec()
            .check(ListQuery {
                page_size: 500,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(query.page_size, 50);
        assert_eq!(query.sort.unwrap().key, "title");
    }

    #[test]
    fn a_default_sort_on_a_column_not_marked_sortable_is_a_definition_error() {
        spec().validate().expect("title is sortable");
        let unsorted = ListSpec {
            default_sort: None,
            ..spec()
        };
        unsorted
            .validate()
            .expect("no default sort, nothing to check");
        let misdeclared = ListSpec {
            default_sort: Some(Sort {
                key: "pinned".into(),
                dir: SortDir::Desc,
            }),
            ..spec()
        };
        let error = misdeclared.validate().expect_err("pinned is not sortable");
        assert!(matches!(error, UiError::Misuse(_)));
        assert!(error.to_string().contains("pinned"), "{error}");
        let blank = ListSpec::new(
            None,
            &Limits {
                default_sort: Some(Sort {
                    key: "title".into(),
                    dir: SortDir::Asc,
                }),
                ..Limits::default()
            },
        );
        blank
            .validate()
            .expect_err("a list without a screen has no sortable column");
    }

    #[test]
    fn check_refuses_an_unsortable_or_unfilterable_field_and_a_zero_page() {
        let sorted = ListQuery {
            sort: Some(Sort {
                key: "account_id".into(),
                dir: SortDir::Asc,
            }),
            ..Default::default()
        };
        let error = spec().check(sorted).unwrap_err();
        assert_eq!(error.key(), "keel.error.list.not_sortable");
        assert_eq!(error.field_path(), Some("sort.key"));
        let filtered = ListQuery {
            filters: vec![Filter {
                key: "account_id".into(),
                op: FilterOp::Eq,
                value: FilterValue::One("x".into()),
            }],
            ..Default::default()
        };
        let error = spec().check(filtered).unwrap_err();
        assert_eq!(error.field_path(), Some("filters[0].key"));
        let zero = ListQuery {
            page: 0,
            ..Default::default()
        };
        assert_eq!(spec().check(zero).unwrap_err().field_path(), Some("page"));
    }

    fn sql_in(tz: Tz, filter: Filter) -> String {
        item::Entity::find()
            .filter(filter_expr::<item::Entity>(&filter, 0, tz).unwrap())
            .build(DbBackend::Postgres)
            .to_string()
    }

    fn one(op: FilterOp, text: &str) -> Filter {
        Filter {
            key: "updated_at".into(),
            op,
            value: FilterValue::One(text.into()),
        }
    }

    #[test]
    fn a_bare_date_on_a_datetime_column_resolves_to_the_day_in_the_callers_zone() {
        let tokyo = chrono_tz::Asia::Tokyo;
        let tail = |sql: String| sql.split(" WHERE ").nth(1).unwrap().to_owned();
        assert_eq!(
            tail(sql_in(tokyo, one(FilterOp::Gte, "2026-09-14"))),
            r#""memo_item"."updated_at" >= '2026-09-13 15:00:00.000000 +00:00'"#
        );
        assert_eq!(
            tail(sql_in(tokyo, one(FilterOp::Lt, "2026-09-14"))),
            r#""memo_item"."updated_at" < '2026-09-13 15:00:00.000000 +00:00'"#
        );
        assert_eq!(
            tail(sql_in(tokyo, one(FilterOp::Gt, "2026-09-13"))),
            r#""memo_item"."updated_at" >= '2026-09-13 15:00:00.000000 +00:00'"#
        );
        assert_eq!(
            tail(sql_in(tokyo, one(FilterOp::Lte, "2026-09-13"))),
            r#""memo_item"."updated_at" < '2026-09-13 15:00:00.000000 +00:00'"#
        );
        assert_eq!(
            tail(sql_in(tokyo, one(FilterOp::Eq, "2026-09-14"))),
            r#""memo_item"."updated_at" >= '2026-09-13 15:00:00.000000 +00:00' AND "memo_item"."updated_at" < '2026-09-14 15:00:00.000000 +00:00'"#
        );
        assert_eq!(
            tail(sql_in(tokyo, one(FilterOp::Ne, "2026-09-14"))),
            r#""memo_item"."updated_at" < '2026-09-13 15:00:00.000000 +00:00' OR "memo_item"."updated_at" >= '2026-09-14 15:00:00.000000 +00:00'"#
        );
        assert_eq!(
            tail(sql_in(Tz::UTC, one(FilterOp::Gte, "2026-09-14"))),
            r#""memo_item"."updated_at" >= '2026-09-14 00:00:00.000000 +00:00'"#
        );
        assert_eq!(
            tail(sql_in(
                tokyo,
                one(FilterOp::Gte, "2026-09-13T20:00:00+02:00")
            )),
            r#""memo_item"."updated_at" >= '2026-09-13 18:00:00.000000 +00:00'"#
        );
        assert_eq!(
            tail(sql_in(
                tokyo,
                Filter {
                    key: "updated_at".into(),
                    op: FilterOp::Eq,
                    value: FilterValue::Many(vec![
                        "2026-09-14".into(),
                        "2026-09-16T00:00:00Z".into()
                    ]),
                }
            )),
            r#"("memo_item"."updated_at" >= '2026-09-13 15:00:00.000000 +00:00' AND "memo_item"."updated_at" < '2026-09-14 15:00:00.000000 +00:00') OR "memo_item"."updated_at" = '2026-09-16 00:00:00.000000 +00:00'"#
        );
        let vague =
            filter_expr::<item::Entity>(&one(FilterOp::Gte, "yesterday"), 3, tokyo).unwrap_err();
        assert_eq!(vague.key(), "keel.error.list.filter_value");
        assert_eq!(vague.field_path(), Some("filters[3].value"));
        let local =
            filter_expr::<item::Entity>(&one(FilterOp::Gte, "2026-09-13T20:00:00"), 0, tokyo)
                .unwrap_err();
        assert_eq!(local.key(), "keel.error.list.filter_value");
        let ranged = filter_expr::<item::Entity>(
            &Filter {
                key: "updated_at".into(),
                op: FilterOp::Gt,
                value: FilterValue::Many(vec!["2026-09-14".into()]),
            },
            0,
            tokyo,
        )
        .unwrap_err();
        assert_eq!(ranged.field_path(), Some("filters[0].op"));
        let skipped = day_start(
            chrono_tz::America::Santiago,
            NaiveDate::from_ymd_opt(2026, 9, 6).unwrap(),
        )
        .unwrap();
        assert_eq!(skipped.to_rfc3339(), "2026-09-06T04:00:00+00:00");
    }

    #[test]
    fn filters_are_typed_by_the_column() {
        let sql = |filter: Filter| {
            item::Entity::find()
                .filter(filter_expr::<item::Entity>(&filter, 0, Tz::UTC).unwrap())
                .build(DbBackend::Postgres)
                .to_string()
        };
        assert!(
            sql(Filter {
                key: "weight".into(),
                op: FilterOp::Gte,
                value: FilterValue::One("300".into()),
            })
            .ends_with(r#"WHERE "memo_item"."weight" >= 300"#)
        );
        assert!(
            sql(Filter {
                key: "pinned".into(),
                op: FilterOp::Eq,
                value: FilterValue::One("true".into()),
            })
            .ends_with(r#"WHERE "memo_item"."pinned" = TRUE"#)
        );
        assert!(
            sql(Filter {
                key: "title".into(),
                op: FilterOp::Eq,
                value: FilterValue::Many(vec!["a".into(), "b".into()]),
            })
            .ends_with(r#"WHERE "memo_item"."title" IN ('a', 'b')"#)
        );
        assert!(
            sql(Filter {
                key: "due".into(),
                op: FilterOp::Lte,
                value: FilterValue::One("2026-09-20".into()),
            })
            .ends_with(r#"WHERE "memo_item"."due" <= '2026-09-20'"#)
        );
        let vague_day = filter_expr::<item::Entity>(
            &Filter {
                key: "due".into(),
                op: FilterOp::Eq,
                value: FilterValue::One("2026-09-20T00:00:00Z".into()),
            },
            1,
            Tz::UTC,
        )
        .unwrap_err();
        assert_eq!(vague_day.key(), "keel.error.list.filter_value");
        assert_eq!(vague_day.field_path(), Some("filters[1].value"));
        let bad = filter_expr::<item::Entity>(
            &Filter {
                key: "weight".into(),
                op: FilterOp::Eq,
                value: FilterValue::One("heavy".into()),
            },
            2,
            Tz::UTC,
        )
        .unwrap_err();
        assert_eq!(bad.field_path(), Some("filters[2].value"));
        let array = filter_expr::<item::Entity>(
            &Filter {
                key: "tags".into(),
                op: FilterOp::Eq,
                value: FilterValue::One("x".into()),
            },
            4,
            Tz::UTC,
        )
        .unwrap_err();
        assert_eq!(array.key(), "keel.error.list.not_filterable");
        assert_eq!(array.field_path(), Some("filters[4].key"));
        let unknown = filter_expr::<item::Entity>(
            &Filter {
                key: "colour".into(),
                op: FilterOp::Eq,
                value: FilterValue::One("red".into()),
            },
            0,
            Tz::UTC,
        )
        .unwrap_err();
        assert_eq!(unknown.key(), "keel.error.list.unknown_field");
        let ranged = filter_expr::<item::Entity>(
            &Filter {
                key: "weight".into(),
                op: FilterOp::Gt,
                value: FilterValue::Many(vec!["1".into()]),
            },
            0,
            Tz::UTC,
        )
        .unwrap_err();
        assert_eq!(ranged.field_path(), Some("filters[0].op"));
    }

    #[test]
    fn search_text_is_escaped_for_like() {
        assert_eq!(escape_like("50%_a\\b"), "50\\%\\_a\\\\b");
    }
}
