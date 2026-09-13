use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel_sdk::system::{CallExt, SessionChange};

use super::super::entity::session;
use super::{REASON_REVOKED, revoke};

pub const PERMISSION_READ: &str = "auth.session.read";
pub const PERMISSION_MANAGE: &str = "auth.session.manage";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct SessionRow {
    pub id: String,
    pub kind: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub idle_until: DateTime<Utc>,
    pub absolute_until: DateTime<Utc>,
    pub actor: Option<Id>,
    pub current: bool,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Pick {
    #[schemars(length(min = 1, max = 128))]
    pub id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct SessionRevoked {
    pub revoked: bool,
}

fn row_of(row: session::Model, current: Option<&str>) -> Result<SessionRow> {
    let actor = row
        .actor_principal_id
        .as_deref()
        .map(|actor| {
            actor.parse().map_err(|_| {
                KeelError::internal("auth.error.session_actor")
                    .param("id", row.id_hash.as_str())
                    .param("actor", actor)
            })
        })
        .transpose()?;
    Ok(SessionRow {
        current: current == Some(row.id_hash.as_str()),
        id: row.id_hash,
        kind: row.kind,
        ip: row.ip,
        user_agent: row.user_agent,
        created_at: row.created_at,
        idle_until: row.idle_until,
        absolute_until: row.absolute_until,
        actor,
        revoked_at: row.revoked_at,
    })
}

struct SessionSource;

#[async_trait]
impl ListSource for SessionSource {
    type Row = SessionRow;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<SessionRow>> {
        let mut select = call.account_scope().apply(session::Entity::find());
        if call.principal().level < Level::Reseller {
            select =
                select.filter(session::Column::PrincipalId.eq(call.principal().id.to_string()));
        }
        let page = fetch_page(select, call, spec).await?;
        let current = call.client().and_then(|client| client.session.as_deref());
        Ok(Page {
            data: page
                .data
                .into_iter()
                .map(|row| row_of(row, current))
                .collect::<Result<Vec<_>>>()?,
            total: page.total,
            totals: page.totals,
        })
    }
}

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<SessionRow>>("auth.session.list")
        .kind(Kind::List)
        .access(Level::User, PERMISSION_READ)
        .owned::<session::Entity>()
        .ui(List::new("auth.session.list.title")
            .search(["ip", "userAgent"])
            .column(
                Column::new("kind", ColumnType::Text)
                    .sortable()
                    .filter_options(["session", "su"]),
            )
            .column(Column::new("ip", ColumnType::Text).sortable().primary())
            .column(Column::new("userAgent", ColumnType::Text))
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .column(Column::new("idleUntil", ColumnType::Datetime).sortable())
            .column(Column::new("absoluteUntil", ColumnType::Datetime).sortable())
            .column(Column::new("actor", ColumnType::Text))
            .column(Column::new("current", ColumnType::Boolean))
            .column(Column::new("revokedAt", ColumnType::Datetime).sortable())
            .action(
                Action::new("revoke", ActionRole::Button)
                    .function("auth.session.revoke")
                    .params([("id", "$id")])
                    .confirm(),
            )
            .row_actions(["revoke"])
            .empty("auth.session.empty", None)
            .invalidate_on(["auth.session.*", "auth.logout", "auth.su.*"]))
        .default_sort("createdAt", SortDir::Desc)
        .source(SessionSource)
}

pub fn revoke_function() -> FunctionDef {
    function::<Pick, SessionRevoked>("auth.session.revoke")
        .kind(Kind::Action)
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<session::Entity>()
        .handler(|call: Call<Pick>| async move {
            let id = call.params().id.as_str();
            let tx = call.tx()?;
            let row = super::row(tx, id).await?.ok_or_else(not_found)?;
            let caller = call.principal();
            if caller.level < Level::Reseller && row.principal_id != caller.id.to_string() {
                return Err(not_found());
            }
            let now = call.require::<dyn Clock>()?.now();
            let revoked = revoke(tx, id, now, REASON_REVOKED).await?;
            if call.client().and_then(|client| client.session.as_deref()) == Some(id) {
                call.session(SessionChange::Clear);
            }
            Ok(SessionRevoked { revoked })
        })
}

fn not_found() -> KeelError {
    KeelError::not_found("keel.error.not_found")
}
