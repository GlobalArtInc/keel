use keel_sdk::runtime::ListSpec;
use keel_types::Total;
use serde_json::{Map, Number, Value};

pub fn totals(spec: &ListSpec, visible: impl Fn(&str) -> bool, page: &mut Value) {
    if spec.totals.is_empty() {
        return;
    }
    let Some(rows) = page.get("data").and_then(Value::as_array) else {
        return;
    };
    let mut totals = Map::new();
    for (key, total) in &spec.totals {
        if !visible(key) {
            continue;
        }
        let value = match total {
            Total::Count => Value::from(
                rows.iter()
                    .filter(|row| row.get(key).is_some_and(|value| !value.is_null()))
                    .count(),
            ),
            _ => {
                let values: Vec<&Number> = rows
                    .iter()
                    .filter_map(|row| row.get(key).and_then(Value::as_number))
                    .collect();
                aggregate(*total, &values)
            }
        };
        totals.insert(key.clone(), value);
    }
    page["totals"] = Value::Object(totals);
}

fn aggregate(total: Total, values: &[&Number]) -> Value {
    let integers: Option<Vec<i64>> = values.iter().map(|number| number.as_i64()).collect();
    let floats: Vec<f64> = values.iter().filter_map(|number| number.as_f64()).collect();
    match total {
        Total::Count => Value::from(values.len()),
        Total::Sum => match integers {
            Some(integers) => Value::from(integers.iter().sum::<i64>()),
            None => Value::from(floats.iter().sum::<f64>()),
        },
        Total::Avg => {
            if floats.is_empty() {
                Value::Null
            } else {
                Value::from(floats.iter().sum::<f64>() / floats.len() as f64)
            }
        }
        Total::Min => match integers {
            Some(integers) => integers
                .iter()
                .min()
                .map_or(Value::Null, |min| Value::from(*min)),
            None => floats
                .iter()
                .copied()
                .reduce(f64::min)
                .map_or(Value::Null, Value::from),
        },
        Total::Max => match integers {
            Some(integers) => integers
                .iter()
                .max()
                .map_or(Value::Null, |max| Value::from(*max)),
            None => floats
                .iter()
                .copied()
                .reduce(f64::max)
                .map_or(Value::Null, Value::from),
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec(totals: &[(&str, Total)]) -> ListSpec {
        ListSpec {
            totals: totals
                .iter()
                .map(|(key, total)| ((*key).to_owned(), *total))
                .collect(),
            ..ListSpec::default()
        }
    }

    #[test]
    fn totals_are_computed_over_the_rows_of_the_page_for_every_declared_column() {
        let mut page = json!({
            "data": [ { "pages": 10, "rating": 1.5, "title": "a" }, { "pages": 5, "rating": 2.5, "title": "b" }, { "pages": 1, "title": "c" } ],
            "total": 3, "totals": null
        });
        totals(
            &spec(&[
                ("pages", Total::Sum),
                ("rating", Total::Avg),
                ("title", Total::Count),
            ]),
            |_| true,
            &mut page,
        );
        assert_eq!(
            page["totals"],
            json!({ "pages": 16, "rating": 2.0, "title": 3 })
        );
        let mut page = json!({ "data": [ { "pages": 1.5 }, { "pages": 2 } ], "total": 2 });
        totals(&spec(&[("pages", Total::Min)]), |_| true, &mut page);
        assert_eq!(page["totals"]["pages"], json!(1.5));
        let mut page = json!({ "data": [ { "pages": 4 }, { "pages": 2 } ], "total": 2 });
        totals(&spec(&[("pages", Total::Max)]), |_| true, &mut page);
        assert_eq!(page["totals"]["pages"], json!(4));
    }

    #[test]
    fn a_column_the_caller_cannot_see_has_no_total_even_on_an_empty_page() {
        let mut page = json!({ "data": [ { "title": "a" } ], "total": 1, "totals": null });
        totals(
            &spec(&[("pages", Total::Sum)]),
            |key| key != "pages",
            &mut page,
        );
        assert_eq!(page["totals"], json!({}));
        let mut page = json!({ "data": [], "total": 0, "totals": null });
        totals(
            &spec(&[("pages", Total::Sum), ("rating", Total::Avg)]),
            |key| key != "rating",
            &mut page,
        );
        assert_eq!(page["totals"], json!({ "pages": 0 }));
        let mut page = json!({ "data": [], "total": 0, "totals": null });
        totals(
            &spec(&[("pages", Total::Sum), ("rating", Total::Avg)]),
            |_| true,
            &mut page,
        );
        assert_eq!(page["totals"], json!({ "pages": 0, "rating": null }));
        let mut page = json!({ "data": [ { "pages": 1 } ], "total": 1, "totals": null });
        totals(&spec(&[]), |_| true, &mut page);
        assert_eq!(page["totals"], Value::Null);
    }
}
