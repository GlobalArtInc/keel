use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel::sdk::serde_json;

use crate::functions::{apply_after_commit, drop_rows};
use crate::host::{Changed, Ftp};
use crate::{STATE_PENDING, account};

pub(crate) fn contribution(ftp: Arc<Ftp>) -> AccountLifecycle {
    let suspending = Arc::clone(&ftp);
    let resuming = Arc::clone(&ftp);
    AccountLifecycle::new()
        .on_suspend(move |call: Call<Id>| {
            let ftp = Arc::clone(&suspending);
            async move { switch(&call, ftp, true).await }
        })
        .on_resume(move |call: Call<Id>| {
            let ftp = Arc::clone(&resuming);
            async move { switch(&call, ftp, false).await }
        })
        .on_delete(move |call: Call<Id>| {
            let ftp = Arc::clone(&ftp);
            async move { remove(&call, ftp).await }
        })
}

pub(crate) async fn forget_sites(call: Call<Value>, ftp: Arc<Ftp>) -> Result<()> {
    let ids: Vec<Id> = serde_json::from_value(call.params()["ids"].clone())?;
    let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let gone = drop_rows(call.tx()?, account::Column::SiteId.is_in(ids)).await?;
    if !gone.is_empty() {
        apply_after_commit(
            &call,
            ftp,
            Changed {
                gone,
                ..Changed::default()
            },
        );
    }
    Ok(())
}

pub(crate) async fn follow_site(call: Call<Value>, ftp: Arc<Ftp>) -> Result<()> {
    let Ok(site) = serde_json::from_value::<Id>(call.params()["id"].clone()) else {
        return Ok(());
    };
    let accounts: Vec<String> = account::Entity::find()
        .filter(account::Column::SiteId.eq(site.to_string()))
        .all(call.tx()?)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();
    if !accounts.is_empty() {
        apply_after_commit(
            &call,
            ftp,
            Changed {
                accounts,
                ..Changed::default()
            },
        );
    }
    Ok(())
}

async fn switch(call: &Call<Id>, ftp: Arc<Ftp>, suspended: bool) -> Result<()> {
    let tx = call.tx()?;
    let ids: Vec<String> = account::Entity::find()
        .filter(account::Column::AccountId.eq(call.params().to_string()))
        .filter(account::Column::Suspended.eq(!suspended))
        .all(tx)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let now = call.require::<dyn Clock>()?.now();
    account::Entity::update_many()
        .col_expr(account::Column::Suspended, Expr::value(suspended))
        .col_expr(account::Column::State, Expr::value(STATE_PENDING))
        .col_expr(account::Column::UpdatedAt, Expr::value(now))
        .filter(account::Column::Id.is_in(ids.clone()))
        .exec(tx)
        .await?;
    apply_after_commit(
        call,
        ftp,
        Changed {
            accounts: ids,
            ..Changed::default()
        },
    );
    Ok(())
}

async fn remove(call: &Call<Id>, ftp: Arc<Ftp>) -> Result<()> {
    let owner = call.params().to_string();
    let gone = drop_rows(call.tx()?, account::Column::AccountId.eq(owner.as_str())).await?;
    if gone.is_empty() {
        return Ok(());
    }
    if let Some(task) = call.task() {
        task.log(&format!(
            "removed {} FTP account(s) of account {owner}",
            gone.len()
        ))
        .await?;
    }
    apply_after_commit(
        call,
        ftp,
        Changed {
            gone,
            ..Changed::default()
        },
    );
    Ok(())
}
