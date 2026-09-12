use axum::Extension;
use axum::extract::Query;
use axum::http::HeaderMap;
use axum::response::Response;
use keel_types::Principal;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::live::Live;
use super::reply::{self, RequestId};
use crate::loader::REQUIRED_LANGUAGE;
use crate::meta;
use crate::registry::Registry;

pub(super) const LANG_PARAM: &str = "lang";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Catalogue {
    pub lang: String,
    pub messages: Map<String, Value>,
}

pub fn catalogue(registry: &Registry, lang: &str) -> Catalogue {
    let mut messages = registry
        .i18n(REQUIRED_LANGUAGE)
        .cloned()
        .unwrap_or_default();
    let requested = registry.i18n(lang);
    if let Some(overlay) = requested
        && lang != REQUIRED_LANGUAGE
    {
        merge_over(&mut messages, overlay);
    }
    Catalogue {
        lang: if requested.is_some() {
            lang.to_owned()
        } else {
            REQUIRED_LANGUAGE.to_owned()
        },
        messages,
    }
}

fn merge_over(base: &mut Map<String, Value>, overlay: &Map<String, Value>) {
    for (key, value) in overlay {
        match (base.get_mut(key), value) {
            (Some(Value::Object(existing)), Value::Object(nested)) => merge_over(existing, nested),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

pub(super) async fn serve(
    live: Live,
    Extension(RequestId(request_id)): Extension<RequestId>,
    Extension(principal): Extension<Principal>,
    Query(pairs): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    let lang = pairs
        .iter()
        .find(|(key, _)| key == LANG_PARAM)
        .map(|(_, value)| value.trim())
        .filter(|lang| !lang.is_empty())
        .unwrap_or(&principal.lang)
        .to_ascii_lowercase();
    let catalogue = catalogue(&live.registry, &lang);
    let version = meta::catalogue_version(&live.registry, &catalogue.lang);
    if reply::matches_etag(&headers, &version) {
        return reply::not_modified(&version);
    }
    reply::cached(reply::ok(catalogue, &request_id, &principal), &version)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_language_is_served_over_the_english_catalogue_leaf_by_leaf() {
        let mut base = json!({ "keel": { "a": "A", "b": "B", "nested": { "x": "X" } } })
            .as_object()
            .cloned()
            .unwrap();
        let overlay = json!({ "keel": { "b": "Б", "nested": { "y": "Y" } }, "extra": "E" })
            .as_object()
            .cloned()
            .unwrap();
        merge_over(&mut base, &overlay);
        assert_eq!(
            Value::Object(base),
            json!({ "keel": { "a": "A", "b": "Б", "nested": { "x": "X", "y": "Y" } }, "extra": "E" })
        );
    }
}
