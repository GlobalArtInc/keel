use std::sync::OnceLock;
use std::time::Duration;

use keel::prelude::*;
use keel::sdk::StatCollector;
use keel::sdk::chrono::{DateTime, Utc};
use keel::sdk::limit::Unit;
use keel_host::api::{HostExecutor, Origin, ServiceStatus};
use keel_system::notify::api::{ProblemSource, Raise, Severity};

use crate::PRODUCT;

pub(crate) const DOWN: &str = "hosting.service.down";
pub(crate) const ACTIVE: &str = "active";
pub(crate) const INACTIVE: &str = "inactive";
pub(crate) const FAILED: &str = "failed";

const MEMORY: &str = "hosting.service.memory";
const UPTIME: &str = "hosting.service.uptime";
const INTERVAL: Duration = Duration::from_secs(300);
const DETACHED: &str = "hosting.service.error.detached";

#[derive(Serialize)]
#[serde(crate = "keel::sdk::serde")]
struct Down {
    name: String,
    state: String,
}

pub(crate) struct Watch {
    host: OnceLock<Arc<dyn HostExecutor>>,
    clock: OnceLock<Arc<dyn Clock>>,
}

impl Watch {
    pub(crate) fn new() -> Self {
        Self {
            host: OnceLock::new(),
            clock: OnceLock::new(),
        }
    }

    pub(crate) fn attach(&self, ctx: &Context) -> Result<()> {
        let _ = self.host.set(ctx.require::<dyn HostExecutor>()?);
        let _ = self.clock.set(ctx.require::<dyn Clock>()?);
        Ok(())
    }

    async fn status(&self) -> Result<Vec<ServiceStatus>> {
        let host = self
            .host
            .get()
            .ok_or_else(|| KeelError::unavailable(DETACHED))?;
        let request = Id::generate().to_string();
        host.on(Origin::installation(PRODUCT, &request))
            .services()
            .status()
            .await
    }

    fn now(&self) -> Result<DateTime<Utc>> {
        self.clock
            .get()
            .map(|clock| clock.now())
            .ok_or_else(|| KeelError::unavailable(DETACHED))
    }
}

pub(crate) fn problems(watch: Arc<Watch>) -> ProblemSource {
    ProblemSource::new(DOWN, move |_| {
        let watch = Arc::clone(&watch);
        async move { down(watch.status().await?) }
    })
}

pub(crate) fn memory(watch: Arc<Watch>) -> StatCollector {
    StatCollector::per_subject(MEMORY, Unit::Bytes, INTERVAL, move |_| {
        let watch = Arc::clone(&watch);
        async move { Ok(memory_of(watch.status().await?)) }
    })
}

pub(crate) fn uptime(watch: Arc<Watch>) -> StatCollector {
    StatCollector::per_subject(UPTIME, Unit::Seconds, INTERVAL, move |_| {
        let watch = Arc::clone(&watch);
        async move {
            let services = watch.status().await?;
            Ok(uptime_of(services, watch.now()?))
        }
    })
}

fn down(services: Vec<ServiceStatus>) -> Result<Vec<Raise>> {
    let mut raises = Vec::new();
    for service in services.into_iter().filter(|service| service.installed) {
        let severity = match service.state.as_str() {
            FAILED => Severity::Critical,
            INACTIVE => Severity::Warning,
            _ => continue,
        };
        raises.push(Raise::new(DOWN, &service.name, severity).params(Down {
            name: service.name.clone(),
            state: service.state,
        })?);
    }
    Ok(raises)
}

fn memory_of(services: Vec<ServiceStatus>) -> Vec<(String, f64)> {
    services
        .into_iter()
        .filter(|service| service.state == ACTIVE)
        .filter_map(|service| service.memory.map(|bytes| (service.name, bytes as f64)))
        .collect()
}

fn uptime_of(services: Vec<ServiceStatus>, now: DateTime<Utc>) -> Vec<(String, f64)> {
    let now = now.timestamp();
    services
        .into_iter()
        .filter(|service| service.state == ACTIVE)
        .filter_map(|service| {
            service
                .since
                .map(|since| (service.name, now.saturating_sub(since).max(0) as f64))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, state: &str, installed: bool) -> ServiceStatus {
        ServiceStatus {
            name: name.to_owned(),
            state: state.to_owned(),
            installed,
            memory: (state == ACTIVE).then_some(7_340_032),
            since: (state == ACTIVE).then_some(1_790_000_000),
        }
    }

    fn services() -> Vec<ServiceStatus> {
        vec![
            service("nginx", ACTIVE, true),
            service("postfix", FAILED, true),
            service("dovecot", INACTIVE, true),
            service("vsftpd", INACTIVE, false),
            service("named", "activating", true),
        ]
    }

    #[test]
    fn a_failed_service_is_critical_a_stopped_one_a_warning_and_a_missing_or_busy_one_no_problem() {
        let raised: Vec<(String, Severity)> = down(services())
            .unwrap()
            .into_iter()
            .map(|raise| (raise.subject.clone(), raise.severity))
            .collect();
        assert_eq!(
            raised,
            [
                ("postfix".to_owned(), Severity::Critical),
                ("dovecot".to_owned(), Severity::Warning)
            ]
        );
    }

    #[test]
    fn memory_and_uptime_are_sampled_for_the_active_services_alone() {
        assert_eq!(memory_of(services()), [("nginx".to_owned(), 7_340_032.0)]);
        let now = DateTime::from_timestamp(1_790_000_600, 0).unwrap();
        assert_eq!(uptime_of(services(), now), [("nginx".to_owned(), 600.0)]);
        let earlier = DateTime::from_timestamp(1_789_999_000, 0).unwrap();
        assert_eq!(uptime_of(services(), earlier), [("nginx".to_owned(), 0.0)]);
    }
}
