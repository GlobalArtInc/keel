use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::domains::{apply_after_commit, drop_rows};
use crate::host::{Changed, Mail};
use crate::{STATE_PENDING, domain, mailbox};

pub(crate) fn contribution(mail: Arc<Mail>) -> AccountLifecycle {
    let suspending = Arc::clone(&mail);
    let resuming = Arc::clone(&mail);
    AccountLifecycle::new()
        .on_suspend(move |call: Call<Id>| {
            let mail = Arc::clone(&suspending);
            async move { switch(&call, mail, true).await }
        })
        .on_resume(move |call: Call<Id>| {
            let mail = Arc::clone(&resuming);
            async move { switch(&call, mail, false).await }
        })
        .on_delete(move |call: Call<Id>| {
            let mail = Arc::clone(&mail);
            async move { remove(&call, mail).await }
        })
}

async fn switch(call: &Call<Id>, mail: Arc<Mail>, suspended: bool) -> Result<()> {
    let account = call.params().to_string();
    let tx = call.tx()?;
    let ids: Vec<String> = mailbox::Entity::find()
        .filter(mailbox::Column::AccountId.eq(account.as_str()))
        .filter(mailbox::Column::Suspended.eq(!suspended))
        .all(tx)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let now = call.require::<dyn Clock>()?.now();
    mailbox::Entity::update_many()
        .col_expr(mailbox::Column::Suspended, Expr::value(suspended))
        .col_expr(mailbox::Column::State, Expr::value(STATE_PENDING))
        .col_expr(mailbox::Column::UpdatedAt, Expr::value(now))
        .filter(mailbox::Column::Id.is_in(ids.clone()))
        .exec(tx)
        .await?;
    let changed = Changed {
        mailboxes: ids,
        ..Changed::default()
    };
    call.after_commit(move |ctx| async move { mail.apply(&ctx, changed).await });
    Ok(())
}

async fn remove(call: &Call<Id>, mail: Arc<Mail>) -> Result<()> {
    let account = call.params().to_string();
    let (domains, gone) =
        drop_rows(call.tx()?, domain::Column::AccountId.eq(account.as_str())).await?;
    if domains == 0 {
        return Ok(());
    }
    if let Some(task) = call.task() {
        task.log(&format!(
            "removed {domains} mail domain(s) and {} mailbox(es) of account {account}",
            gone.len()
        ))
        .await?;
    }
    apply_after_commit(call, mail, gone);
    Ok(())
}
