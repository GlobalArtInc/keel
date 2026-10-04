use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use http::header::ACCEPT;
use keel_types::Event;
use serde::Deserialize;

use crate::FEED_PATH;
use crate::client::{Client, Request, Response};
use crate::error::Error;

const JSON: &str = "application/json";

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct FeedPage {
    pub data: Vec<Event>,
    pub next: u64,
}

type Fetch = Pin<Box<dyn Future<Output = Result<(Response, FeedPage), Error>> + Send>>;

pub struct Feed {
    client: Client,
    types: Vec<String>,
    limit: Option<u32>,
    cursor: u64,
    page_next: u64,
    buffer: VecDeque<Event>,
    pending: Option<Fetch>,
    done: bool,
}

impl Feed {
    pub(crate) fn new(client: Client, after: u64) -> Self {
        Self {
            client,
            types: Vec::new(),
            limit: None,
            cursor: after,
            page_next: after,
            buffer: VecDeque::new(),
            pending: None,
            done: false,
        }
    }

    pub fn types<I, S>(mut self, masks: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.types = masks.into_iter().map(Into::into).collect();
        self
    }

    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    fn fetch(&self) -> Fetch {
        let client = self.client.clone();
        let mut request = Request::get(FEED_PATH)
            .header(ACCEPT, JSON)
            .query("after", &self.cursor.to_string());
        if !self.types.is_empty() {
            request = request.query("types", &self.types.join(","));
        }
        if let Some(limit) = self.limit {
            request = request.query("limit", &limit.to_string());
        }
        Box::pin(async move {
            let response = client.send_as(request, None).await?;
            let page = response.json::<FeedPage>()?;
            Ok((response, page))
        })
    }
}

fn checked(response: &Response, page: FeedPage, after: u64) -> Result<FeedPage, Error> {
    if page.next < after {
        return Err(response.outside(&format!("the feed went back from {after} to {}", page.next)));
    }
    let mut last = after;
    for event in &page.data {
        if event.seq <= last || event.seq > page.next {
            return Err(response.outside(&format!(
                "the event {} has seq {} outside {last} exclusive to {} inclusive, in order",
                event.id, event.seq, page.next
            )));
        }
        last = event.seq;
    }
    Ok(page)
}

impl Stream for Feed {
    type Item = Result<Event, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(event) = this.buffer.pop_front() {
                this.cursor = if this.buffer.is_empty() {
                    this.page_next
                } else {
                    event.seq
                };
                return Poll::Ready(Some(Ok(event)));
            }
            if this.done {
                return Poll::Ready(None);
            }
            if this.pending.is_none() {
                this.pending = Some(this.fetch());
            }
            let Some(pending) = this.pending.as_mut() else {
                continue;
            };
            let outcome = match pending.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(outcome) => outcome,
            };
            this.pending = None;
            let page =
                match outcome.and_then(|(response, page)| checked(&response, page, this.cursor)) {
                    Ok(page) => page,
                    Err(error) => {
                        this.done = true;
                        return Poll::Ready(Some(Err(error)));
                    }
                };
            if page.data.is_empty() {
                if page.next == this.cursor {
                    this.done = true;
                } else {
                    this.cursor = page.next;
                    this.page_next = page.next;
                }
                continue;
            }
            this.page_next = page.next;
            this.buffer.extend(page.data);
        }
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderMap;
    use keel_types::Id;
    use serde_json::json;

    use super::*;

    fn event(seq: u64) -> Event {
        serde_json::from_value(json!({
            "id": Id::generate(), "seq": seq, "type": "keel.account.created",
            "at": "2026-10-04T10:00:00.000Z", "instanceId": Id::generate(), "module": "keel",
        }))
        .unwrap()
    }

    fn response() -> Response {
        Response {
            url: "https://dns.example/api/v1/x/peer/feed?after=10".to_owned(),
            status: 200,
            headers: HeaderMap::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn a_page_moves_forward_with_its_events_in_order_between_the_cursor_and_next() {
        let page = FeedPage {
            data: vec![event(11), event(14)],
            next: 20,
        };
        assert_eq!(checked(&response(), page.clone(), 10).unwrap(), page);
        let empty = FeedPage {
            data: Vec::new(),
            next: 10,
        };
        assert_eq!(checked(&response(), empty.clone(), 10).unwrap(), empty);
        for (data, next) in [
            (vec![], 9),
            (vec![event(10)], 20),
            (vec![event(21)], 20),
            (vec![event(14), event(12)], 20),
            (vec![event(12), event(12)], 20),
        ] {
            let error = checked(&response(), FeedPage { data, next }, 10).unwrap_err();
            assert!(
                matches!(error, Error::Answer { status: 200, .. }),
                "{error}"
            );
        }
    }
}
