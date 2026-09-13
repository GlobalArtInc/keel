use keel::prelude::*;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set,
};

use super::{MAX_LINE, MAX_PER_ACCOUNT, NoteKind, item};

#[derive(Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct NoteImport {
    #[schemars(extend("x-ui" = json!({ "control": "code" })))]
    pub text: String,
    #[serde(default)]
    pub kind: NoteKind,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "check": "name" }), "x-ui" = json!({ "control": "tags" })))]
    pub tags: Vec<String>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub account: Option<Id>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ImportSource {
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Checked {
    pub lines: u64,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Imported {
    pub imported: u64,
    pub ids: Vec<Id>,
}

pub fn import() -> FunctionDef {
    function::<NoteImport, Imported>("note.import")
        .kind(Kind::Wizard)
        .access(Level::User, "note.manage")
        .unowned()
        .ui(Form::new("note.import.title")
            .page("source", ["text"])
            .validate("note.import.check")
            .page("options", ["kind", "tags", "account"])
            .result(ScreenResult::Redirect))
        .handler(|call: Call<NoteImport>| async move {
            let params = call.params();
            let titles = lines(&params.text)?;
            let account = call.resolve_account(params.account).await?.to_string();
            let tx = call.tx()?;
            let kept = item::Entity::find()
                .filter(item::Column::AccountId.eq(account.as_str()))
                .count(tx)
                .await?;
            if kept + titles.len() as u64 > MAX_PER_ACCOUNT {
                return Err(
                    KeelError::limit_exceeded("note.error.limit").param("max", MAX_PER_ACCOUNT)
                );
            }
            let now = call.require::<dyn Clock>()?.now();
            let mut ids = Vec::with_capacity(titles.len());
            for title in titles {
                let id = Id::generate();
                item::ActiveModel {
                    id: Set(id.to_string()),
                    account_id: Set(account.clone()),
                    title: Set(title),
                    body: Set(String::new()),
                    kind: Set(params.kind.as_str().to_owned()),
                    tags: Set(params.tags.clone()),
                    due: Set(None),
                    created_at: Set(now),
                    updated_at: Set(now),
                }
                .insert(tx)
                .await?;
                ids.push(id);
            }
            call.emit(
                "note.imported",
                &json!({ "ids": ids, "account_id": account }),
            )?;
            Ok(Imported {
                imported: ids.len() as u64,
                ids,
            })
        })
}

pub fn check() -> FunctionDef {
    function::<ImportSource, Checked>("note.import.check")
        .kind(Kind::Action)
        .mode(Mode::Read)
        .access(Level::User, "note.manage")
        .unowned()
        .handler(|call: Call<ImportSource>| async move {
            let titles = lines(call.params().text.as_deref().unwrap_or_default())?;
            Ok(Checked {
                lines: titles.len() as u64,
            })
        })
}

fn lines(text: &str) -> Result<Vec<String>> {
    let mut titles = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let title = line.trim();
        if title.is_empty() {
            continue;
        }
        if title.chars().count() > MAX_LINE {
            return Err(KeelError::validation("note.error.line_too_long")
                .field("text")
                .param("line", index + 1)
                .param("max", MAX_LINE));
        }
        titles.push(title.to_owned());
    }
    if titles.is_empty() {
        return Err(KeelError::validation("note.error.empty").field("text"));
    }
    Ok(titles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_keep_the_non_empty_trimmed_lines_and_refuse_a_long_line_or_none() {
        assert_eq!(
            lines(" one \n\n\ttwo\r\n").expect("two lines"),
            ["one", "two"]
        );
        let long = format!("short\n{}\n", "x".repeat(MAX_LINE + 1));
        let error = lines(&long).expect_err("the second line is too long");
        assert_eq!(error.key(), "note.error.line_too_long");
        assert_eq!(error.field_path(), Some("text"));
        assert_eq!(error.params()["line"], json!(2));
        assert_eq!(error.status(), 422);
        let exact = "x".repeat(MAX_LINE);
        assert_eq!(lines(&exact).expect("the limit is inclusive"), [exact]);
        let empty = lines("\n \n").expect_err("no line");
        assert_eq!(empty.key(), "note.error.empty");
        assert_eq!(empty.field_path(), Some("text"));
    }
}
