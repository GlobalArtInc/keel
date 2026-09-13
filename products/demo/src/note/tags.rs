use keel::prelude::*;
use keel::sdk::escape_like;
use keel::sdk::sea_orm;
use keel::sdk::sea_orm::sea_query::extension::postgres::PgExpr;
use keel::sdk::sea_orm::sea_query::{
    Alias, Asterisk, Expr, ExprTrait, Func, Order, Query, SimpleExpr,
};
use keel::sdk::sea_orm::{ConnectionTrait, EntityTrait, FromQueryResult, QueryTrait};
use keel::types::{Filter, FilterOp, FilterValue};

use super::item;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema, FromQueryResult)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
pub struct TagCount {
    pub tag: String,
    pub count: i64,
}

#[derive(Clone, Copy, Debug)]
struct TagSource;

const TAG: &str = "tag";
const COUNT: &str = "count";

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<TagCount>>("note.tag.list")
        .kind(Kind::List)
        .access(Level::User, "note.read")
        .key(TAG)
        .unowned()
        .ui(List::new("note.tag.list.title")
            .search([TAG])
            .column(Column::new(TAG, ColumnType::Text).sortable().primary())
            .column(
                Column::new(COUNT, ColumnType::Number)
                    .sortable()
                    .total(Total::Sum),
            ))
        .default_sort(COUNT, SortDir::Desc)
        .source(TagSource)
}

#[async_trait]
impl ListSource for TagSource {
    type Row = TagCount;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<TagCount>> {
        let query = call.params();
        let mut grouped = call
            .account_scope()
            .apply(item::Entity::find())
            .into_query();
        grouped
            .clear_selects()
            .expr_as(tag(), Alias::new(TAG))
            .expr_as(count(), Alias::new(COUNT))
            .from_function(
                Func::cust(Alias::new("unnest")).arg(Expr::col((item::Entity, item::Column::Tags))),
                Alias::new(TAG),
            )
            .group_by_col(Alias::new(TAG));
        if !query.search.is_empty() && spec.search.iter().any(|field| field == TAG) {
            let pattern = format!("%{}%", escape_like(&query.search));
            grouped.and_where(tag().ilike(pattern.as_str()));
        }
        for (index, filter) in query.filters.iter().enumerate() {
            match filter.key.as_str() {
                COUNT => grouped.and_having(condition(count(), filter, index)?),
                _ => grouped.and_where(condition(tag(), filter, index)?),
            };
        }
        let db = call.db();
        let backend = db.get_database_backend();
        let counted = Query::select()
            .expr_as(count(), Alias::new(COUNT))
            .from_subquery(grouped.clone(), Alias::new("grouped"))
            .to_owned();
        let total = db
            .query_one(backend.build(&counted))
            .await?
            .map(|row| row.try_get::<i64>("", COUNT))
            .transpose()?
            .unwrap_or_default();
        if let Some(sort) = &query.sort {
            let order = match sort.dir {
                SortDir::Asc => Order::Asc,
                SortDir::Desc => Order::Desc,
            };
            grouped.order_by(Alias::new(sort.key.as_str()), order);
        }
        grouped
            .order_by(Alias::new(TAG), Order::Asc)
            .limit(u64::from(query.page_size))
            .offset(u64::from(query.page - 1) * u64::from(query.page_size));
        let data = TagCount::find_by_statement(backend.build(&grouped))
            .all(db)
            .await?;
        Ok(Page {
            data,
            total: u64::try_from(total).unwrap_or_default(),
            totals: None,
        })
    }
}

fn tag() -> SimpleExpr {
    Expr::col(Alias::new(TAG)).into()
}

fn count() -> SimpleExpr {
    Func::count(Expr::col(Asterisk)).into()
}

fn condition(target: SimpleExpr, filter: &Filter, index: usize) -> Result<SimpleExpr> {
    let path = format!("filters[{index}].value");
    let typed = |text: &str| typed_value(&filter.key, text, &path);
    Ok(match (&filter.value, filter.op) {
        (FilterValue::One(text), FilterOp::Eq) => target.eq(typed(text)?),
        (FilterValue::One(text), FilterOp::Ne) => target.ne(typed(text)?),
        (FilterValue::One(text), FilterOp::Gt) => target.gt(typed(text)?),
        (FilterValue::One(text), FilterOp::Gte) => target.gte(typed(text)?),
        (FilterValue::One(text), FilterOp::Lt) => target.lt(typed(text)?),
        (FilterValue::One(text), FilterOp::Lte) => target.lte(typed(text)?),
        (FilterValue::Many(texts), FilterOp::Eq) => target.is_in(
            texts
                .iter()
                .map(|text| typed(text))
                .collect::<Result<Vec<_>>>()?,
        ),
        (FilterValue::Many(texts), FilterOp::Ne) => target.is_not_in(
            texts
                .iter()
                .map(|text| typed(text))
                .collect::<Result<Vec<_>>>()?,
        ),
        (FilterValue::Many(_), _) => {
            return Err(KeelError::validation("keel.error.list.list_value_op")
                .field(format!("filters[{index}].op"))
                .param("field", filter.key.clone()));
        }
    })
}

fn typed_value(key: &str, text: &str, path: &str) -> Result<sea_orm::Value> {
    if key != COUNT {
        return Ok(sea_orm::Value::from(text));
    }
    text.parse::<i64>()
        .map(|count| sea_orm::Value::BigInt(Some(count)))
        .map_err(|_| {
            KeelError::validation("keel.error.list.filter_value")
                .field(path)
                .param("value", text)
        })
}

#[cfg(test)]
mod tests {
    use keel::sdk::sea_orm::sea_query::PostgresQueryBuilder;

    use super::*;

    fn sql(expr: SimpleExpr) -> String {
        Query::select()
            .expr(expr)
            .to_owned()
            .to_string(PostgresQueryBuilder)
    }

    fn filter(key: &str, op: FilterOp, value: FilterValue) -> Filter {
        Filter {
            key: key.into(),
            op,
            value,
        }
    }

    #[test]
    fn a_tag_filter_compares_text_and_a_count_filter_the_grouped_count_as_an_integer() {
        let by_tag = condition(
            tag(),
            &filter(TAG, FilterOp::Eq, FilterValue::One("work".into())),
            0,
        )
        .expect("a text filter");
        assert_eq!(sql(by_tag), r#"SELECT "tag" = 'work'"#);
        let by_count = condition(
            count(),
            &filter(COUNT, FilterOp::Gte, FilterValue::One("2".into())),
            0,
        )
        .expect("an integer filter");
        assert_eq!(sql(by_count), r#"SELECT COUNT(*) >= 2"#);
        let many = condition(
            tag(),
            &filter(
                TAG,
                FilterOp::Ne,
                FilterValue::Many(vec!["a".into(), "b".into()]),
            ),
            0,
        )
        .expect("a list filter");
        assert_eq!(sql(many), r#"SELECT "tag" NOT IN ('a', 'b')"#);
        let vague = condition(
            count(),
            &filter(COUNT, FilterOp::Eq, FilterValue::One("many".into())),
            3,
        )
        .expect_err("not an integer");
        assert_eq!(vague.key(), "keel.error.list.filter_value");
        assert_eq!(vague.field_path(), Some("filters[3].value"));
        let ranged = condition(
            count(),
            &filter(COUNT, FilterOp::Gt, FilterValue::Many(vec!["1".into()])),
            1,
        )
        .expect_err("a list value takes eq or ne");
        assert_eq!(ranged.field_path(), Some("filters[1].op"));
    }
}
