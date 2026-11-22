use keel_sdk::prelude::*;

pub fn node(boot: &Value, path: &str) -> Option<Value> {
    boot["menu"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|group| group["items"].as_array().into_iter().flatten())
        .flat_map(|item| {
            std::iter::once(item).chain(item["children"].as_array().into_iter().flatten())
        })
        .find(|candidate| candidate["path"] == json!(path))
        .cloned()
}
