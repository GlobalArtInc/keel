use std::sync::{Arc, OnceLock};

use keel_sdk::chrono_tz::Tz;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::OnConflict;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use keel_sdk::serde_json;
use keel_sdk::system::{LanguageSource, PreferenceSource, Preferences};

use super::entity::pref;

pub const LANG: &str = "lang";
pub const TZ: &str = "tz";
pub const THEME: &str = "theme";
const COLUMNS_PREFIX: &str = "columns.";
const THEME_MAX_LEN: usize = 32;
const FUNCTION_MAX_LEN: usize = 128;
const COLUMN_KEY_MAX_LEN: usize = 64;
const COLUMNS_MAX_ENTRIES: usize = 64;
const COLUMNS_MAX_BYTES: usize = 4096;
const COLUMNS_MAX_ROWS: u64 = 128;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct PrefPick {
    #[schemars(length(min = 1, max = 160))]
    pub key: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct PrefSet {
    #[schemars(length(min = 1, max = 160))]
    pub key: String,
    pub value: Value,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Pref {
    pub key: String,
    pub value: Option<Value>,
}

#[derive(Default)]
pub struct Prefs {
    languages: OnceLock<Arc<dyn LanguageSource>>,
}

impl Prefs {
    pub fn attach(&self, languages: Arc<dyn LanguageSource>) {
        let _ = self.languages.set(languages);
    }

    fn languages(&self) -> Result<Vec<String>> {
        self.languages
            .get()
            .map(|source| source.languages())
            .ok_or_else(|| KeelError::internal("settings.error.not_started"))
    }

    fn check(&self, key: &str, value: &Value) -> Result<()> {
        if key.starts_with(COLUMNS_PREFIX) {
            return check_columns(value);
        }
        let text = value
            .as_str()
            .ok_or_else(|| KeelError::validation("settings.error.pref_text").field("value"))?;
        let fits = match key {
            LANG => self.languages()?.iter().any(|known| known == text),
            TZ => text.parse::<Tz>().is_ok(),
            _ => text.chars().count() <= THEME_MAX_LEN,
        };
        if fits {
            return Ok(());
        }
        Err(match key {
            LANG => KeelError::validation("settings.error.pref_lang")
                .field("value")
                .param("lang", text),
            TZ => KeelError::validation("settings.error.pref_tz")
                .field("value")
                .param("tz", text),
            _ => KeelError::validation("settings.error.pref_text").field("value"),
        })
    }
}

fn check_columns(value: &Value) -> Result<()> {
    let refused = || KeelError::validation("settings.error.pref_columns").field("value");
    let members = value.as_object().ok_or_else(refused)?;
    let well_formed = members.values().all(|member| {
        member.as_array().is_some_and(|entries| {
            entries.len() <= COLUMNS_MAX_ENTRIES
                && entries.iter().all(|entry| {
                    entry
                        .as_str()
                        .is_some_and(|column| column.chars().count() <= COLUMN_KEY_MAX_LEN)
                })
        })
    });
    if !well_formed || serde_json::to_vec(value)?.len() > COLUMNS_MAX_BYTES {
        return Err(refused());
    }
    Ok(())
}

fn refuse_owner(principal: &Principal) -> Result<()> {
    if principal.level >= Level::Owner {
        return Err(KeelError::forbidden("settings.error.pref_owner"));
    }
    Ok(())
}

async fn check_columns_rows(db: &Db, principal: Id, key: &str) -> Result<()> {
    if !key.starts_with(COLUMNS_PREFIX) {
        return Ok(());
    }
    let kept = pref::Entity::find()
        .filter(pref::Column::PrincipalId.eq(principal.to_string()))
        .filter(pref::Column::Key.starts_with(COLUMNS_PREFIX))
        .filter(pref::Column::Key.ne(key))
        .count(db)
        .await?;
    if kept >= COLUMNS_MAX_ROWS {
        return Err(
            KeelError::limit_exceeded("settings.error.pref_columns_many")
                .param("max", COLUMNS_MAX_ROWS),
        );
    }
    Ok(())
}

fn check_key(key: &str) -> Result<()> {
    let known = matches!(key, LANG | TZ | THEME)
        || key.strip_prefix(COLUMNS_PREFIX).is_some_and(|function| {
            !function.is_empty()
                && function.len() <= FUNCTION_MAX_LEN
                && function
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
        });
    if known {
        Ok(())
    } else {
        Err(KeelError::validation("settings.error.pref_key")
            .field("key")
            .param("key", key))
    }
}

async fn stored(db: &Db, principal: Id, key: &str) -> Result<Option<Value>> {
    Ok(
        pref::Entity::find_by_id((principal.to_string(), key.to_owned()))
            .one(db)
            .await?
            .map(|row| row.value),
    )
}

pub fn get() -> FunctionDef {
    function::<PrefPick, Pref>("settings.pref.get")
        .kind(Kind::Get)
        .level(Level::User)
        .unowned()
        .handler(|call: Call<PrefPick>| async move {
            refuse_owner(call.principal())?;
            let key = call.params().key.clone();
            check_key(&key)?;
            let value = stored(call.db(), call.principal().id, &key).await?;
            Ok(Pref { key, value })
        })
}

pub fn set(prefs: Arc<Prefs>) -> FunctionDef {
    function::<PrefSet, Pref>("settings.pref.set")
        .kind(Kind::Form)
        .level(Level::User)
        .unowned()
        .ui(Form::new("settings.pref.set.title").page("main", ["key", "value"]))
        .handler(move |call: Call<PrefSet>| {
            let prefs = prefs.clone();
            async move {
                let caller = call.principal();
                refuse_owner(caller)?;
                let params = call.params();
                check_key(&params.key)?;
                prefs.check(&params.key, &params.value)?;
                check_columns_rows(call.tx()?, caller.id, &params.key).await?;
                let now = call.require::<dyn Clock>()?.now();
                pref::Entity::insert(pref::ActiveModel {
                    principal_id: Set(caller.id.to_string()),
                    key: Set(params.key.clone()),
                    value: Set(params.value.clone()),
                    updated_at: Set(now),
                })
                .on_conflict(
                    OnConflict::columns([pref::Column::PrincipalId, pref::Column::Key])
                        .update_columns([pref::Column::Value, pref::Column::UpdatedAt])
                        .to_owned(),
                )
                .exec(call.tx()?)
                .await?;
                Ok(Pref {
                    key: params.key.clone(),
                    value: Some(params.value.clone()),
                })
            }
        })
}

#[async_trait]
impl PreferenceSource for Prefs {
    async fn preferences(&self, principal: Id, db: &Db) -> Result<Preferences> {
        let rows = pref::Entity::find()
            .filter(pref::Column::PrincipalId.eq(principal.to_string()))
            .filter(pref::Column::Key.is_in([LANG, TZ]))
            .all(db)
            .await?;
        let mut preferences = Preferences::default();
        for row in rows {
            let text = row.value.as_str().map(str::to_owned);
            match row.key.as_str() {
                LANG => preferences.lang = text,
                TZ => preferences.tz = text,
                _ => {}
            }
        }
        Ok(preferences)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Served;

    impl LanguageSource for Served {
        fn languages(&self) -> Vec<String> {
            vec!["en".into(), "ru".into()]
        }
    }

    #[test]
    fn the_keys_are_lang_tz_theme_and_columns_of_a_function() {
        for key in [
            LANG,
            TZ,
            THEME,
            "columns.note.list",
            "columns.keel.account.list",
        ] {
            check_key(key).unwrap_or_else(|error| panic!("{key}: {error}"));
        }
        for key in [
            "widgets",
            "columns.",
            "columns.Note.List",
            "columns.note list",
            "LANG",
        ] {
            let error = check_key(key).expect_err(key);
            assert_eq!(error.key(), "settings.error.pref_key");
            assert_eq!(error.field_path(), Some("key"));
            assert_eq!(error.params()["key"], key);
        }
    }

    #[test]
    fn a_language_must_be_served_a_zone_must_be_known_and_a_theme_is_a_short_text() {
        let prefs = Prefs::default();
        assert_eq!(
            prefs
                .check(LANG, &json!("ru"))
                .expect_err("not started")
                .key(),
            "settings.error.not_started"
        );
        prefs.attach(Arc::new(Served));
        prefs.check(LANG, &json!("ru")).expect("ru is served");
        let unknown = prefs
            .check(LANG, &json!("de"))
            .expect_err("de is not served");
        assert_eq!(unknown.key(), "settings.error.pref_lang");
        assert_eq!(unknown.field_path(), Some("value"));
        prefs.check(TZ, &json!("Asia/Tokyo")).expect("a known zone");
        let nowhere = prefs
            .check(TZ, &json!("Mars/Olympus"))
            .expect_err("an unknown zone");
        assert_eq!(nowhere.key(), "settings.error.pref_tz");
        assert_eq!(nowhere.params()["tz"], "Mars/Olympus");
        let number = prefs.check(TZ, &json!(9)).expect_err("a number");
        assert_eq!(number.key(), "settings.error.pref_text");
        prefs.check(THEME, &json!("dark")).expect("a theme");
        assert!(prefs.check(THEME, &json!("x".repeat(33))).is_err());
    }

    #[test]
    fn columns_are_an_object_of_short_column_key_lists_within_four_kib() {
        let prefs = Prefs::default();
        prefs
            .check("columns.note.list", &json!({}))
            .expect("an empty object");
        prefs
            .check(
                "columns.note.list",
                &json!({ "order": ["title", "updatedAt"], "hidden": ["id"] }),
            )
            .expect("order and visibility");
        let long_list: Vec<String> = (0..COLUMNS_MAX_ENTRIES).map(|n| format!("c{n}")).collect();
        prefs
            .check("columns.note.list", &json!({ "order": long_list }))
            .expect("the entry cap is inclusive");
        for value in [
            json!("title"),
            json!(["title"]),
            json!(null),
            json!({ "order": "title" }),
            json!({ "order": [1] }),
            json!({ "order": [{ "key": "title" }] }),
            json!({ "order": ["x".repeat(COLUMN_KEY_MAX_LEN + 1)] }),
            json!({ "order": vec!["c"; COLUMNS_MAX_ENTRIES + 1] }),
            json!({ "order": (0..COLUMNS_MAX_ENTRIES).map(|n| format!("{n:>64}")).collect::<Vec<_>>() }),
        ] {
            let Err(error) = prefs.check("columns.note.list", &value) else {
                panic!("{value} should be refused");
            };
            assert_eq!(error.key(), "settings.error.pref_columns");
            assert_eq!(error.field_path(), Some("value"));
        }
    }
}
