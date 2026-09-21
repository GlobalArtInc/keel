use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::net::UnixDatagram;
use tokio::task::JoinHandle;

const SOCKET_VARIABLE: &str = "NOTIFY_SOCKET";
const WATCHDOG_VARIABLE: &str = "WATCHDOG_USEC";
const WATCHDOG_PID_VARIABLE: &str = "WATCHDOG_PID";
const READY: &[u8] = b"READY=1\n";
const WATCHDOG: &[u8] = b"WATCHDOG=1\n";
const STOPPING: &[u8] = b"STOPPING=1\n";
const ABSTRACT_PREFIX: char = '@';

pub(crate) struct Notifier {
    socket: PathBuf,
    watchdog: Option<Duration>,
}

pub(crate) struct Watchdog(JoinHandle<()>);

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Notifier {
    pub(crate) fn from_env() -> Option<Self> {
        let socket = std::env::var_os(SOCKET_VARIABLE)?;
        let socket = PathBuf::from(socket);
        if socket.to_string_lossy().starts_with(ABSTRACT_PREFIX) {
            tracing::warn!(
                socket = %socket.display(),
                "the notify socket is an abstract name, which the service does not speak; readiness is not reported"
            );
            return None;
        }
        let watchdog = watchdog_interval(
            std::env::var(WATCHDOG_VARIABLE).ok().as_deref(),
            std::env::var(WATCHDOG_PID_VARIABLE).ok().as_deref(),
            std::process::id(),
        );
        tracing::debug!(
            socket = %socket.display(),
            watchdog = ?watchdog,
            "the service manager listens for readiness"
        );
        Some(Self { socket, watchdog })
    }

    pub(crate) async fn ready(&self) {
        self.send(READY).await;
    }

    pub(crate) async fn stopping(&self) {
        self.send(STOPPING).await;
    }

    pub(crate) fn watchdog(&self) -> Option<Watchdog> {
        let interval = self.watchdog?;
        let socket = self.socket.clone();
        Some(Watchdog(tokio::spawn(async move {
            let mut ticks = tokio::time::interval(interval);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticks.tick().await;
            loop {
                ticks.tick().await;
                send_to(&socket, WATCHDOG).await;
            }
        })))
    }

    async fn send(&self, message: &[u8]) {
        send_to(&self.socket, message).await;
    }
}

fn watchdog_interval(usec: Option<&str>, pid: Option<&str>, own_pid: u32) -> Option<Duration> {
    let micros: u64 = usec?.trim().parse().ok().filter(|micros| *micros > 0)?;
    if let Some(pid) = pid
        && pid.trim().parse::<u32>().ok() != Some(own_pid)
    {
        return None;
    }
    Some(Duration::from_micros(micros / 2))
}

async fn send_to(socket: &Path, message: &[u8]) {
    let sent = async {
        let datagram = UnixDatagram::unbound()?;
        datagram.send_to(message, socket).await
    }
    .await;
    if let Err(error) = sent {
        tracing::debug!(
            socket = %socket.display(),
            message = %String::from_utf8_lossy(message).trim_end(),
            error = %error,
            "the notification did not reach the service manager"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_watchdog_pings_at_half_the_interval_for_this_process_alone() {
        assert_eq!(
            watchdog_interval(Some("60000000"), None, 7),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            watchdog_interval(Some("60000000"), Some("7"), 7),
            Some(Duration::from_secs(30))
        );
        assert_eq!(watchdog_interval(Some("60000000"), Some("8"), 7), None);
        assert_eq!(watchdog_interval(Some("0"), None, 7), None);
        assert_eq!(watchdog_interval(Some("soon"), None, 7), None);
        assert_eq!(watchdog_interval(None, None, 7), None);
    }
}
