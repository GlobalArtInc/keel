use keel::prelude::*;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set,
};
use keel::sdk::serde_json;
use keel::types::AccountState;

use super::{Batch, Deleted, NoteEdit, Pick, item};

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<item::Model>>("note.list")
        .kind(Kind::List)
        .access(Level::User, "note.read")
        .owned::<item::Entity>()
        .ui(List::new("note.list.title")
            .search(["title"])
            .column(
                Column::new("title", ColumnType::Link)
                    .to("note.edit", [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(Column::new("updated_at", ColumnType::Datetime).sortable())
            .action(
                Action::new("create", ActionRole::New)
                    .screen("note.edit")
                    .icon("plus"),
            )
            .action(
                Action::new("edit", ActionRole::Edit)
                    .screen("note.edit")
                    .default(),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function("note.delete")
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("title".to_owned()),
                        input: None,
                    }),
            )
            .row_actions(["edit", "delete"])
            .empty("note.empty", Some("create"))
            .invalidate_on(["note.*"]))
        .default_sort("updated_at", SortDir::Desc)
        .source(entity_source::<item::Entity>())
}

pub fn get() -> FunctionDef {
    function::<Pick, item::Model>("note.get")
        .kind(Kind::Get)
        .access(Level::User, "note.read")
        .owned::<item::Entity>()
        .handler(|call: Call<Pick>| async move { load(call.db(), &call.params().id).await })
}

pub fn edit() -> FunctionDef {
    function::<NoteEdit, item::Model>("note.edit")
        .kind(Kind::Form)
        .access(Level::User, "note.manage")
        .owned::<item::Entity>()
        .ui(Form::new("note.edit.title")
            .read("note.get")
            .page("main", ["title", "body"])
            .page("owner", ["account"])
            .show_if("$principal.level != 'user'"))
        .handler(|call: Call<NoteEdit>| async move {
            let params = call.params();
            let tx = call.tx()?;
            let current = match &params.id {
                Some(id) => Some(load(tx, id).await?),
                None => None,
            };
            let account = match (params.account, &current) {
                (None, Some(row)) => row.account_id.clone(),
                (requested, _) => call.resolve_account(requested).await?.to_string(),
            };
            let owner = call
                .invoke("keel.account.get", json!({ "id": account }))
                .await?;
            let state: AccountState = serde_json::from_value(owner["state"].clone())?;
            if state != AccountState::Active {
                return Err(KeelError::conflict("note.error.account_suspended")
                    .param("state", owner["state"].clone()));
            }
            let now = call.require::<dyn Clock>()?.now();
            let id = match &current {
                Some(row) => {
                    item::ActiveModel {
                        id: Set(row.id.clone()),
                        account_id: Set(account.clone()),
                        title: Set(params.title.clone()),
                        body: Set(params.body.clone()),
                        updated_at: Set(now),
                        ..Default::default()
                    }
                    .update(tx)
                    .await?;
                    row.id.clone()
                }
                None => {
                    let id = Id::generate().to_string();
                    item::ActiveModel {
                        id: Set(id.clone()),
                        account_id: Set(account.clone()),
                        title: Set(params.title.clone()),
                        body: Set(params.body.clone()),
                        created_at: Set(now),
                        updated_at: Set(now),
                    }
                    .insert(tx)
                    .await?;
                    id
                }
            };
            call.emit("note.saved", &json!({ "id": id, "account_id": account }))?;
            let saved = call.invoke("note.get", json!({ "id": id })).await?;
            Ok(serde_json::from_value(saved)?)
        })
}

pub fn delete() -> FunctionDef {
    function::<Batch, Deleted>("note.delete")
        .kind(Kind::Action)
        .access(Level::User, "note.manage")
        .key("ids")
        .owned::<item::Entity>()
        .handler(|call: Call<Batch>| async move {
            let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
            let deleted = item::Entity::delete_many()
                .filter(item::Column::Id.is_in(ids))
                .exec(call.tx()?)
                .await?
                .rows_affected;
            for id in &call.params().ids {
                call.emit("note.deleted", &json!({ "id": id }))?;
            }
            Ok(Deleted { deleted })
        })
}

async fn load<C: ConnectionTrait>(db: &C, id: &Id) -> Result<item::Model> {
    item::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}
