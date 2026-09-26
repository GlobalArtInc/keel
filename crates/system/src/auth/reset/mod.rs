pub mod functions;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};

use keel_sdk::chrono::{DateTime, Duration, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter};
use keel_sdk::system::{Caller, PrincipalSource};

use super::address;
use super::entity::session;
use super::mail::{self, Purpose, RESET_TEMPLATE};

pub const PER_LOGIN: u32 = 3;
pub const PER_CLIENT: u32 = 10;
const WINDOW_MINUTES: i64 = 60;
pub const MINUTES: u32 = 60;
pub const REASON_RESET: &str = "password_reset";
pub const SETTING_MINUTES: &str = "auth.password.reset_minutes";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Counted {
    Login(String),
    Client(IpAddr),
}

impl Counted {
    const fn limit(&self) -> u32 {
        match self {
            Counted::Login(_) => PER_LOGIN,
            Counted::Client(_) => PER_CLIENT,
        }
    }
}

struct Window {
    count: u32,
    until: DateTime<Utc>,
}

#[derive(Default)]
pub struct ResetLimit {
    windows: Mutex<HashMap<Counted, Window>>,
}

impl ResetLimit {
    pub fn admit(&self, login: &str, ip: Option<IpAddr>, now: DateTime<Utc>) -> bool {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        windows.retain(|_, window| window.until > now);
        let counted: Vec<Counted> = [
            Some(Counted::Login(login.to_owned())),
            ip.map(|ip| Counted::Client(ip.to_canonical())),
        ]
        .into_iter()
        .flatten()
        .collect();
        let full = counted.iter().any(|key| {
            windows
                .get(key)
                .is_some_and(|window| window.count >= key.limit())
        });
        if full {
            return false;
        }
        for key in counted {
            windows
                .entry(key)
                .or_insert(Window {
                    count: 0,
                    until: now + Duration::minutes(WINDOW_MINUTES),
                })
                .count += 1;
        }
        true
    }
}

pub struct Request {
    pub login: String,
    pub minutes: u32,
    pub base: Option<String>,
    pub principals: Arc<dyn PrincipalSource>,
    pub caller: Arc<dyn Caller>,
    pub now: DateTime<Utc>,
}

pub async fn deliver(db: &Db, request: Request) -> Result<()> {
    let Some(principal) = request.principals.by_login(&request.login, db).await? else {
        return Ok(());
    };
    if principal.level >= Level::Owner {
        return Ok(());
    }
    let verified = address::row(db, principal.id)
        .await?
        .is_some_and(|row| row.verified_at.is_some());
    if !verified {
        return Ok(());
    }
    let Some(base) = request.base else {
        tracing::warn!(
            setting = mail::SETTING_SHELL_URL,
            "the shell address is not set; no reset link is mailed"
        );
        return Ok(());
    };
    let token = mail::issue(
        db,
        Purpose::Reset,
        principal.id,
        None,
        Duration::minutes(i64::from(request.minutes)),
        request.now,
    )
    .await?;
    let params = json!({
        "link": mail::link(&base, Purpose::Reset, &token),
        "minutes": request.minutes,
    });
    mail::send(
        request.caller.as_ref(),
        principal.id,
        RESET_TEMPLATE,
        params,
        false,
    )
    .await
}

pub async fn revoke_sessions(db: &Db, principal: Id, now: DateTime<Utc>) -> Result<u64> {
    let principal = principal.to_string();
    let revoked = session::Entity::update_many()
        .col_expr(session::Column::RevokedAt, Expr::value(now))
        .col_expr(session::Column::Reason, Expr::value(REASON_RESET))
        .filter(session::Column::RevokedAt.is_null())
        .filter(
            Condition::any()
                .add(session::Column::PrincipalId.eq(principal.clone()))
                .add(session::Column::ActorPrincipalId.eq(principal)),
        )
        .exec(db)
        .await?;
    Ok(revoked.rows_affected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_counts_a_login_and_a_client_address_apart_and_forgets_after_the_window() {
        let limit = ResetLimit::default();
        let now: DateTime<Utc> = "2026-09-26T10:00:00Z".parse().expect("an instant");
        let ip: IpAddr = "192.0.2.7".parse().expect("an address");
        for _ in 0..PER_LOGIN {
            assert!(limit.admit("alice", Some(ip), now));
        }
        assert!(!limit.admit("alice", Some(ip), now));
        assert!(!limit.admit("alice", None, now));
        let admitted = (0..PER_CLIENT * 2)
            .filter(|n| limit.admit(&format!("login{n}"), Some(ip), now))
            .count();
        assert_eq!(
            admitted,
            usize::try_from(PER_CLIENT - PER_LOGIN).expect("a count")
        );
        assert!(limit.admit("bob", None, now));
        assert!(limit.admit("bob", Some("192.0.2.8".parse().expect("an address")), now));
        let later = now + Duration::minutes(WINDOW_MINUTES);
        assert!(limit.admit("alice", Some(ip), later));
    }
}
