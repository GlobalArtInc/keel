use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::http::HttpError;

pub const FRAME_TIMEOUT: Duration = Duration::from_secs(30);
const HOST: &str = "keel.test";
const READ_CHUNK: usize = 8192;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub id: Option<String>,
    pub data: Option<String>,
    pub retry: Option<u64>,
    pub comments: Vec<String>,
}

impl SseFrame {
    pub fn json(&self) -> Value {
        match &self.data {
            Some(data) => match serde_json::from_str(data) {
                Ok(value) => value,
                Err(error) => panic!("the frame data is not JSON ({error}): {data}"),
            },
            None => panic!("the frame carries no data: {self:?}"),
        }
    }

    pub fn is_comment(&self) -> bool {
        self.event.is_none() && self.id.is_none() && self.data.is_none()
    }
}

enum Body {
    Chunked(Chunked),
    Sized(usize),
    UntilClose,
}

enum Chunked {
    Size,
    Data(usize),
    Terminator,
    Trailers,
    Done,
}

pub struct EventStream {
    path: String,
    stream: TcpStream,
    status: u16,
    headers: Vec<(String, String)>,
    body: Body,
    raw: Vec<u8>,
    decoded: Vec<u8>,
    closed: bool,
}

impl EventStream {
    pub(crate) async fn open(
        addr: SocketAddr,
        path: &str,
        headers: &[(&str, &str)],
    ) -> Result<Self, HttpError> {
        let failed = |source| HttpError::Request {
            path: path.to_owned(),
            source,
        };
        let mut request = format!(
            "GET {path} HTTP/1.1\r\nHost: {HOST}\r\nAccept: text/event-stream\r\nConnection: close\r\n"
        );
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        let mut stream = TcpStream::connect(addr).await.map_err(failed)?;
        stream.write_all(request.as_bytes()).await.map_err(failed)?;
        let mut opened = Self {
            path: path.to_owned(),
            stream,
            status: 0,
            headers: Vec::new(),
            body: Body::UntilClose,
            raw: Vec::new(),
            decoded: Vec::new(),
            closed: false,
        };
        opened.read_head().await?;
        Ok(opened)
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub async fn next(&mut self) -> SseFrame {
        match self.next_within(FRAME_TIMEOUT).await {
            Ok(Some(frame)) => frame,
            Ok(None) => panic!("the event stream {} closed", self.path),
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn next_event(&mut self) -> SseFrame {
        loop {
            let frame = self.next().await;
            if !frame.is_comment() {
                return frame;
            }
        }
    }

    pub async fn next_within(&mut self, within: Duration) -> Result<Option<SseFrame>, HttpError> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            if let Some(frame) = self.parse_frame() {
                return Ok(Some(frame));
            }
            if self.closed {
                return Ok(None);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(HttpError::Timeout {
                    path: self.path.clone(),
                });
            }
            match tokio::time::timeout(remaining, self.fill()).await {
                Ok(Ok(())) => {}
                Ok(Err(source)) => {
                    return Err(HttpError::Request {
                        path: self.path.clone(),
                        source,
                    });
                }
                Err(_) => {
                    return Err(HttpError::Timeout {
                        path: self.path.clone(),
                    });
                }
            }
        }
    }

    pub async fn quiet(&mut self, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return true;
            }
            match self.next_within(remaining).await {
                Ok(Some(frame)) if frame.is_comment() => {}
                Ok(Some(_)) => return false,
                Ok(None) => return true,
                Err(HttpError::Timeout { .. }) => return true,
                Err(error) => panic!("{error}"),
            }
        }
    }

    pub async fn closes_within(&mut self, within: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match self.next_within(remaining).await {
                Ok(Some(_)) => {}
                Ok(None) => return true,
                Err(HttpError::Timeout { .. }) => return false,
                Err(error) => panic!("{error}"),
            }
        }
    }

    pub async fn body(&mut self) -> String {
        while !self.closed {
            if let Err(error) = self.fill().await {
                panic!("reading the body of {} failed: {error}", self.path);
            }
        }
        String::from_utf8_lossy(&self.decoded).into_owned()
    }

    pub async fn json(&mut self) -> Value {
        let body = self.body().await;
        match serde_json::from_str(&body) {
            Ok(value) => value,
            Err(error) => panic!("the body is not JSON ({error}): {body}"),
        }
    }

    async fn read_head(&mut self) -> Result<(), HttpError> {
        loop {
            if let Some(split) = self.raw.windows(4).position(|window| window == b"\r\n\r\n") {
                let rest = self.raw.split_off(split + 4);
                let head = String::from_utf8_lossy(&self.raw[..split]).into_owned();
                self.raw = rest;
                self.parse_head(&head)?;
                self.decode();
                return Ok(());
            }
            if self.closed {
                return Err(HttpError::Malformed {
                    path: self.path.clone(),
                    reason: "the connection closed before the end of the headers".to_owned(),
                });
            }
            let read = tokio::time::timeout(FRAME_TIMEOUT, self.read_more())
                .await
                .map_err(|_| HttpError::Timeout {
                    path: self.path.clone(),
                })?;
            read.map_err(|source| HttpError::Request {
                path: self.path.clone(),
                source,
            })?;
        }
    }

    fn parse_head(&mut self, head: &str) -> Result<(), HttpError> {
        let malformed = |reason: String| HttpError::Malformed {
            path: self.path.clone(),
            reason,
        };
        let mut lines = head.split("\r\n");
        let status_line = lines
            .next()
            .ok_or_else(|| malformed("no status line".to_owned()))?;
        self.status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| malformed(format!("bad status line {status_line:?}")))?;
        self.headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        self.body = if self.header("transfer-encoding").is_some_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("chunked"))
        }) {
            Body::Chunked(Chunked::Size)
        } else if let Some(length) = self
            .header("content-length")
            .and_then(|value| value.parse::<usize>().ok())
        {
            Body::Sized(length)
        } else {
            Body::UntilClose
        };
        Ok(())
    }

    async fn fill(&mut self) -> io::Result<()> {
        self.read_more().await?;
        self.decode();
        Ok(())
    }

    async fn read_more(&mut self) -> io::Result<()> {
        let mut chunk = vec![0; READ_CHUNK];
        let read = self.stream.read(&mut chunk).await?;
        if read == 0 {
            self.closed = true;
        } else {
            self.raw.extend_from_slice(&chunk[..read]);
        }
        Ok(())
    }

    fn decode(&mut self) {
        match &mut self.body {
            Body::UntilClose => {
                self.decoded.append(&mut self.raw);
            }
            Body::Sized(remaining) => {
                let take = (*remaining).min(self.raw.len());
                self.decoded.extend(self.raw.drain(..take));
                *remaining -= take;
                if *remaining == 0 {
                    self.closed = true;
                }
            }
            Body::Chunked(state) => loop {
                match state {
                    Chunked::Size => {
                        let Some(end) = self.raw.windows(2).position(|window| window == b"\r\n")
                        else {
                            return;
                        };
                        let text = String::from_utf8_lossy(&self.raw[..end]).into_owned();
                        let size = text.split(';').next().unwrap_or_default().trim();
                        let size = usize::from_str_radix(size, 16).unwrap_or_default();
                        self.raw.drain(..end + 2);
                        *state = if size == 0 {
                            Chunked::Trailers
                        } else {
                            Chunked::Data(size)
                        };
                    }
                    Chunked::Data(remaining) => {
                        let take = (*remaining).min(self.raw.len());
                        self.decoded.extend(self.raw.drain(..take));
                        *remaining -= take;
                        if *remaining > 0 {
                            return;
                        }
                        *state = Chunked::Terminator;
                    }
                    Chunked::Terminator => {
                        if self.raw.len() < 2 {
                            return;
                        }
                        self.raw.drain(..2);
                        *state = Chunked::Size;
                    }
                    Chunked::Trailers => {
                        let Some(end) = self.raw.windows(2).position(|window| window == b"\r\n")
                        else {
                            return;
                        };
                        self.raw.drain(..end + 2);
                        if end == 0 {
                            *state = Chunked::Done;
                            self.closed = true;
                        }
                    }
                    Chunked::Done => {
                        self.raw.clear();
                        return;
                    }
                }
            },
        }
    }

    fn parse_frame(&mut self) -> Option<SseFrame> {
        let text = String::from_utf8_lossy(&self.decoded).into_owned();
        let end = text.find("\n\n")?;
        let block = text[..end].to_owned();
        let consumed = block.len() + 2;
        self.decoded.drain(..consumed);
        Some(parse_block(&block))
    }
}

fn parse_block(block: &str) -> SseFrame {
    let mut frame = SseFrame::default();
    let mut data: Vec<&str> = Vec::new();
    for line in block.split('\n') {
        if let Some(comment) = line.strip_prefix(':') {
            frame.comments.push(comment.trim_start().to_owned());
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => frame.event = Some(value.to_owned()),
            "id" => frame.id = Some(value.to_owned()),
            "data" => data.push(value),
            "retry" => frame.retry = value.parse().ok(),
            _ => {}
        }
    }
    if !data.is_empty() {
        frame.data = Some(data.join("\n"));
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_is_split_into_its_fields_and_data_lines_are_joined() {
        let frame = parse_block(
            "event: data.invalidate\nid: 42\ndata: {\"a\":\ndata: 1}\nretry: 3000\n: keep-alive",
        );
        assert_eq!(frame.event.as_deref(), Some("data.invalidate"));
        assert_eq!(frame.id.as_deref(), Some("42"));
        assert_eq!(frame.data.as_deref(), Some("{\"a\":\n1}"));
        assert_eq!(frame.retry, Some(3000));
        assert_eq!(frame.comments, ["keep-alive"]);
        assert!(!frame.is_comment());
        let comment = parse_block(": keep-alive");
        assert!(comment.is_comment());
        assert_eq!(parse_block("retry: 3000").retry, Some(3000));
        assert!(parse_block("retry: 3000").is_comment());
    }
}
