use keel_sdk::limit::Unit;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use keel_sdk::types::StatValue;

use super::accounts::SYSTEM_KIND;
use super::entity::account;
use super::stats::ACCOUNTS;

pub(super) const ACCOUNTS_WIDGET: &str = "keel.widget.accounts";

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

pub(super) fn accounts() -> FunctionDef {
    function::<Nothing, StatValue>(ACCOUNTS_WIDGET)
        .kind(Kind::Get)
        .access(Level::User, "keel.account.read")
        .unowned()
        .ui(ObjectScreen::new("keel.widget.accounts.title"))
        .handler(|call: Call<Nothing>| async move {
            let scope = call.account_scope();
            let accounts = scope
                .apply(account::Entity::find())
                .filter(account::Column::Kind.ne(SYSTEM_KIND))
                .count(call.db())
                .await?;
            Ok(StatValue {
                value: accounts as f64,
                unit: Unit::Count.as_str().to_owned(),
                series: (*scope == AccountScope::All).then(|| ACCOUNTS.to_owned()),
                subject: None,
            })
        })
}

pub(super) fn widget() -> DashboardWidget {
    DashboardWidget::new(WidgetKind::Stat, ACCOUNTS_WIDGET).level(Level::Reseller)
}
