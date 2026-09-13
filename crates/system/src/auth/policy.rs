use keel_sdk::chrono::Duration;
use keel_sdk::{Call, Db, Id, Result, SettingsSource};

use super::limits::LockoutPolicy;
use super::session::Lifetime;

pub const SETTING_PASSWORD_MIN_LENGTH: &str = "auth.password.min_length";
pub const SETTING_SESSION_IDLE_MINUTES: &str = "auth.session.idle_minutes";
pub const SETTING_SESSION_ABSOLUTE_HOURS: &str = "auth.session.absolute_hours";
pub const SETTING_LOCKOUT_THRESHOLD: &str = "auth.lockout.threshold";
pub const SETTING_LOCKOUT_MINUTES: &str = "auth.lockout.minutes";
pub const SETTING_RATE_GUEST_PER_MINUTE: &str = "auth.rate.guest_per_minute";

pub async fn min_length<P>(call: &Call<P>) -> Result<u32> {
    call.setting(SETTING_PASSWORD_MIN_LENGTH).await
}

pub async fn lockout_policy<P>(call: &Call<P>) -> Result<LockoutPolicy> {
    let threshold: u32 = call.setting(SETTING_LOCKOUT_THRESHOLD).await?;
    let minutes: u32 = call.setting(SETTING_LOCKOUT_MINUTES).await?;
    Ok(LockoutPolicy::new(threshold, i64::from(minutes)))
}

pub async fn lifetime<P>(call: &Call<P>) -> Result<Lifetime> {
    let idle: u32 = call.setting(SETTING_SESSION_IDLE_MINUTES).await?;
    let absolute: u32 = call.setting(SETTING_SESSION_ABSOLUTE_HOURS).await?;
    Ok(Lifetime {
        idle: Duration::minutes(i64::from(idle)),
        absolute: Duration::hours(i64::from(absolute)),
    })
}

pub async fn idle_of(settings: &dyn SettingsSource, account: Id, db: &Db) -> Result<Duration> {
    let minutes = settings
        .resolve(SETTING_SESSION_IDLE_MINUTES, account, db)
        .await?
        .and_then(|value| value.as_u64())
        .and_then(|minutes| i64::try_from(minutes).ok())
        .unwrap_or(super::session::IDLE_MINUTES);
    Ok(Duration::minutes(minutes))
}
