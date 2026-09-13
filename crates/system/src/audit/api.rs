use std::sync::Arc;

use keel_sdk::Result;

use super::sink::Sink;

pub struct Flush {
    sink: Arc<Sink>,
}

impl Flush {
    pub(super) fn new(sink: Arc<Sink>) -> Self {
        Self { sink }
    }

    pub async fn flush(&self) -> Result<()> {
        self.sink.flush().await
    }

    pub fn dropped(&self) -> u64 {
        self.sink.dropped()
    }
}
