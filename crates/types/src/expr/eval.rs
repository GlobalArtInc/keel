use std::cmp::Ordering;

use regex::Regex;
use serde_json::Value;

use super::{Context, Expr};

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("invalid pattern {pattern:?}: {source}")]
    Pattern {
        pattern: String,
        #[source]
        source: regex::Error,
    },
}

impl Expr {
    pub fn eval(&self, ctx: &Context) -> Result<bool, EvalError> {
        Ok(match self {
            Expr::Eq(var, literal) => equal(&ctx.resolve(var), literal),
            Expr::Ne(var, literal) => !equal(&ctx.resolve(var), literal),
            Expr::Gt(var, literal) => {
                compare(&ctx.resolve(var), literal) == Some(Ordering::Greater)
            }
            Expr::Lt(var, literal) => compare(&ctx.resolve(var), literal) == Some(Ordering::Less),
            Expr::In(var, list) => {
                let value = ctx.resolve(var);
                list.iter().any(|item| equal(&value, item))
            }
            Expr::Matches(var, pattern) => {
                let regex = Regex::new(pattern).map_err(|source| EvalError::Pattern {
                    pattern: pattern.clone(),
                    source,
                })?;
                match ctx.resolve(var) {
                    Value::String(text) => regex.is_match(&text),
                    _ => false,
                }
            }
            Expr::Truthy(var) => truthy(&ctx.resolve(var)),
            Expr::Empty(var) => empty(&ctx.resolve(var)),
            Expr::And(items) => {
                for item in items {
                    if !item.eval(ctx)? {
                        return Ok(false);
                    }
                }
                true
            }
            Expr::Or(items) => {
                for item in items {
                    if item.eval(ctx)? {
                        return Ok(true);
                    }
                }
                false
            }
            Expr::Not(inner) => !inner.eval(ctx)?,
        })
    }
}

fn equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| equal(x, y))
        }
        _ => left == right,
    }
}

fn compare(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64()?.partial_cmp(&b.as_f64()?),
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(members) => !members.is_empty(),
    }
}

fn empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(members) => members.is_empty(),
        Value::Bool(_) | Value::Number(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields(value: Value) -> Context {
        Context {
            fields: value.as_object().cloned().unwrap_or_default(),
            ..Default::default()
        }
    }

    #[test]
    fn numbers_compare_by_value_regardless_of_representation() {
        let ctx = fields(json!({ "ttl": 300.0 }));
        assert!(Expr::parse("ttl == 300").unwrap().eval(&ctx).unwrap());
        assert!(Expr::parse("ttl > 299.5").unwrap().eval(&ctx).unwrap());
        assert!(!Expr::parse("ttl < 300").unwrap().eval(&ctx).unwrap());
        assert!(
            Expr::parse("ttl in [100, 300]")
                .unwrap()
                .eval(&ctx)
                .unwrap()
        );
    }

    #[test]
    fn mismatched_types_never_compare_greater_or_less() {
        let ctx = fields(json!({ "ttl": "600" }));
        assert!(!Expr::parse("ttl > 300").unwrap().eval(&ctx).unwrap());
        assert!(!Expr::parse("ttl < 300").unwrap().eval(&ctx).unwrap());
        assert!(Expr::parse("ttl != 300").unwrap().eval(&ctx).unwrap());
    }

    #[test]
    fn truthy_and_empty_follow_the_value_kind() {
        for (value, is_truthy, is_empty) in [
            (json!(null), false, true),
            (json!(false), false, false),
            (json!(true), true, false),
            (json!(0), false, false),
            (json!(2), true, false),
            (json!(""), false, true),
            (json!("x"), true, false),
            (json!([]), false, true),
            (json!([1]), true, false),
            (json!({}), false, true),
            (json!({ "a": 1 }), true, false),
        ] {
            assert_eq!(truthy(&value), is_truthy, "truthy of {value}");
            assert_eq!(empty(&value), is_empty, "empty of {value}");
        }
    }

    #[test]
    fn matches_applies_a_regular_expression_to_strings_only() {
        let ast = Expr::Matches("name".into(), "^[a-z]+\\.org$".into());
        assert!(ast.eval(&fields(json!({ "name": "a.org" }))).unwrap());
        assert!(!ast.eval(&fields(json!({ "name": "A.org" }))).unwrap());
        assert!(!ast.eval(&fields(json!({ "name": 7 }))).unwrap());
        let broken = Expr::Matches("name".into(), "(".into());
        assert!(matches!(
            broken.eval(&fields(json!({}))),
            Err(EvalError::Pattern { .. })
        ));
    }

    #[test]
    fn and_and_or_short_circuit_before_a_broken_pattern() {
        let broken = Expr::Matches("name".into(), "(".into());
        let ctx = fields(json!({ "flag": false, "name": "x" }));
        let and = Expr::And(vec![Expr::Truthy("flag".into()), broken.clone()]);
        assert!(!and.eval(&ctx).unwrap());
        let or = Expr::Or(vec![
            Expr::Not(Box::new(Expr::Truthy("flag".into()))),
            broken,
        ]);
        assert!(or.eval(&ctx).unwrap());
    }
}
