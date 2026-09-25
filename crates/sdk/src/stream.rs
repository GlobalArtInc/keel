use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::stream::{BoxStream, Stream, StreamExt};
use keel_types::LogLine;

pub struct Lines(BoxStream<'static, LogLine>);

impl Lines {
    pub fn new(lines: impl Stream<Item = LogLine> + Send + 'static) -> Self {
        Self(lines.boxed())
    }
}

impl Stream for Lines {
    type Item = LogLine;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<LogLine>> {
        self.0.poll_next_unpin(cx)
    }
}

impl fmt::Debug for Lines {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Lines")
    }
}

#[cfg(test)]
mod tests {
    use futures::stream;
    use keel_types::LogLevel;

    use super::*;

    #[tokio::test]
    async fn lines_yield_what_they_wrap() {
        let lines = Lines::new(stream::iter([
            LogLine::new(LogLevel::Info, "one"),
            LogLine::new(LogLevel::Error, "two"),
        ]));
        assert_eq!(format!("{lines:?}"), "Lines");
        let collected: Vec<LogLine> = lines.collect().await;
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[1].text, "two");
    }
}
