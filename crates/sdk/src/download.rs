use std::fmt;
use std::io;
use std::path::PathBuf;

use bytes::Bytes;
use futures::stream::{self, BoxStream, StreamExt};
use keel_types::{DownloadFile, TaskRef};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

pub const FILE_CHUNK: usize = 64 * 1024;

pub type Body = BoxStream<'static, io::Result<Bytes>>;

pub struct Download {
    pub filename: String,
    pub content_type: String,
    pub body: Body,
}

pub enum DownloadOutcome {
    Body(Download),
    Stored {
        filename: String,
        content_type: String,
    },
    Task(TaskRef),
}

impl Download {
    pub fn new(
        filename: impl Into<String>,
        content_type: impl Into<String>,
        body: impl futures::Stream<Item = io::Result<Bytes>> + Send + 'static,
    ) -> Self {
        Self {
            filename: filename.into(),
            content_type: content_type.into(),
            body: body.boxed(),
        }
    }

    pub fn bytes(
        filename: impl Into<String>,
        content_type: impl Into<String>,
        bytes: impl Into<Bytes>,
    ) -> Self {
        let bytes: Bytes = bytes.into();
        Self::new(
            filename,
            content_type,
            stream::once(async move { Ok(bytes) }),
        )
    }

    pub fn file(
        filename: impl Into<String>,
        content_type: impl Into<String>,
        path: PathBuf,
    ) -> Self {
        Self::new(filename, content_type, file_stream(path))
    }

    pub fn info(&self) -> DownloadFile {
        DownloadFile::new(self.filename.clone(), self.content_type.clone())
    }

    pub async fn collect(self) -> io::Result<Vec<u8>> {
        let mut body = self.body;
        let mut collected = Vec::new();
        while let Some(chunk) = body.next().await {
            collected.extend_from_slice(&chunk?);
        }
        Ok(collected)
    }
}

enum Reading {
    Opening(PathBuf),
    Open(File),
}

fn file_stream(path: PathBuf) -> impl futures::Stream<Item = io::Result<Bytes>> + Send + 'static {
    stream::try_unfold(Some(Reading::Opening(path)), |state| async move {
        let mut file = match state {
            None => return Ok(None),
            Some(Reading::Opening(path)) => File::open(path).await?,
            Some(Reading::Open(file)) => file,
        };
        let mut chunk = vec![0; FILE_CHUNK];
        let read = file.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        chunk.truncate(read);
        Ok(Some((Bytes::from(chunk), Some(Reading::Open(file)))))
    })
}

impl DownloadOutcome {
    pub fn stored(filename: impl Into<String>, content_type: impl Into<String>) -> Self {
        DownloadOutcome::Stored {
            filename: filename.into(),
            content_type: content_type.into(),
        }
    }
}

impl From<Download> for DownloadOutcome {
    fn from(download: Download) -> Self {
        DownloadOutcome::Body(download)
    }
}

impl From<TaskRef> for DownloadOutcome {
    fn from(reference: TaskRef) -> Self {
        DownloadOutcome::Task(reference)
    }
}

impl fmt::Debug for Download {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Download")
            .field("filename", &self.filename)
            .field("content_type", &self.content_type)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for DownloadOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DownloadOutcome::Body(download) => f.debug_tuple("Body").field(download).finish(),
            DownloadOutcome::Stored {
                filename,
                content_type,
            } => f
                .debug_struct("Stored")
                .field("filename", filename)
                .field("content_type", content_type)
                .finish(),
            DownloadOutcome::Task(reference) => f.debug_tuple("Task").field(reference).finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use keel_types::Id;

    use super::*;

    #[tokio::test]
    async fn a_body_of_bytes_and_a_file_body_collect_to_their_content() {
        let download = Download::bytes("a.csv", "text/csv", "a,b\n");
        assert_eq!(download.info(), DownloadFile::new("a.csv", "text/csv"));
        assert!(format!("{download:?}").starts_with("Download { filename: \"a.csv\""));
        assert_eq!(download.collect().await.unwrap(), b"a,b\n");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        let content: Vec<u8> = (0..FILE_CHUNK * 2 + 17).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &content).unwrap();
        let download = Download::file("big.bin", "application/octet-stream", path);
        assert_eq!(download.collect().await.unwrap(), content);

        let missing = Download::file("gone", "text/plain", dir.path().join("gone"));
        let error = missing.collect().await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn an_outcome_is_built_from_a_download_a_stored_file_or_a_task_ref() {
        let body: DownloadOutcome = Download::bytes("a", "b", "c").into();
        assert!(matches!(body, DownloadOutcome::Body(_)));
        let stored = DownloadOutcome::stored("a.csv", "text/csv");
        assert_eq!(
            format!("{stored:?}"),
            "Stored { filename: \"a.csv\", content_type: \"text/csv\" }"
        );
        let reference = TaskRef::new(Id::generate());
        let task: DownloadOutcome = reference.into();
        assert!(matches!(task, DownloadOutcome::Task(_)));
        assert!(format!("{task:?}").starts_with("Task("));
    }
}
