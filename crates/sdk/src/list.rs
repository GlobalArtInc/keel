use std::marker::PhantomData;
use std::str::FromStr;

use async_trait::async_trait;
use keel_types::{Filter, FilterOp, FilterValue, ListQuery, Page, Sort, SortDir, Total};
use schemars::JsonSchema;
use sea_orm::sea_query::extension::postgres::PgExpr;
use sea_orm::sea_query::{Condition, Expr, SimpleExpr};
use sea_orm::{
    ColumnTrait, ColumnType, EntityTrait, Order, PaginatorTrait, QueryFilter, QueryOrder, Value,
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
        let query = call.params();
        let mut select = call.account_scope().apply(E::find());
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
            select = select.filter(filter_expr::<E>(filter, index)?);
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
}

fn column<E: EntityTrait>(key: &str, path: &str) -> Result<E::Column> {
    E::Column::from_str(key).map_err(|_| {
        KeelError::validation("keel.error.list.unknown_field")
            .field(path)
            .param("field", key)
    })
}

fn filter_expr<E: EntityTrait>(filter: &Filter, index: usize) -> Result<SimpleExpr> {
    let path = format!("filters[{index}].value");
    let column = column::<E>(&filter.key, &format!("filters[{index}].key"))?;
    let column_type = column.def().get_column_type().clone();
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
        _ => Value::from(text),
    })
}

fn escape_like(text: &str) -> String {
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

    #[test]
    fn filters_are_typed_by_the_column() {
        let sql = |filter: Filter| {
            item::Entity::find()
                .filter(filter_expr::<item::Entity>(&filter, 0).unwrap())
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
        let bad = filter_expr::<item::Entity>(
            &Filter {
                key: "weight".into(),
                op: FilterOp::Eq,
                value: FilterValue::One("heavy".into()),
            },
            2,
        )
        .unwrap_err();
        assert_eq!(bad.field_path(), Some("filters[2].value"));
        let unknown = filter_expr::<item::Entity>(
            &Filter {
                key: "colour".into(),
                op: FilterOp::Eq,
                value: FilterValue::One("red".into()),
            },
            0,
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
        )
        .unwrap_err();
        assert_eq!(ranged.field_path(), Some("filters[0].op"));
    }

    #[test]
    fn search_text_is_escaped_for_like() {
        assert_eq!(escape_like("50%_a\\b"), "50\\%\\_a\\\\b");
    }
}
