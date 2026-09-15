use std::fmt;

use keel_sdk::system::{Frame, LiveEvents};
use tokio::sync::broadcast;

pub const LIVE_CAPACITY: usize = 1024;

pub struct LiveBus {
    sender: broadcast::Sender<Frame>,
}

impl Default for LiveBus {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(LIVE_CAPACITY);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Frame> {
        self.sender.subscribe()
    }

    pub fn listeners(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl LiveEvents for LiveBus {
    fn publish(&self, frame: Frame) {
        if self.sender.send(frame).is_err() {
            tracing::trace!("a live frame was published without a listener and dropped");
        }
    }
}

impl fmt::Debug for LiveBus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveBus")
            .field("listeners", &self.listeners())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use keel_types::Id;
    use serde_json::json;
    use tokio::sync::broadcast::error::TryRecvError;

    use super::*;

    fn frame(kind: &str) -> Frame {
        Frame {
            principal: Id::generate(),
            account: Id::generate(),
            kind: kind.to_owned(),
            data: json!({ "n": 1 }),
        }
    }

    #[tokio::test]
    async fn every_listener_gets_a_frame_and_a_frame_without_one_is_dropped() {
        let bus = LiveBus::new();
        assert_eq!(bus.listeners(), 0);
        bus.publish(frame("dropped"));
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();
        assert_eq!(bus.listeners(), 2);
        let published = frame("task.progress");
        bus.publish(published.clone());
        assert_eq!(first.recv().await.unwrap(), published);
        assert_eq!(second.recv().await.unwrap(), published);
        assert_eq!(first.try_recv().unwrap_err(), TryRecvError::Empty);
        assert_eq!(format!("{bus:?}"), "LiveBus { listeners: 2 }");
    }
}
