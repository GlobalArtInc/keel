use std::io;
use std::path::{Path, PathBuf};

use keel_sdk::chrono::{DateTime, SecondsFormat, Utc};
use keel_sdk::prelude::*;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;

pub const DIRECTORY: &str = "tasks";
pub const LOG_EXTENSION: &str = "log";
pub const OUTPUT_EXTENSION: &str = keel_sdk::task::OUTPUT_EXTENSION;

pub fn directory(data_dir: &Path) -> PathBuf {
    data_dir.join(DIRECTORY)
}

pub fn log_path(data_dir: &Path, id: Id) -> PathBuf {
    directory(data_dir).join(format!("{id}.{LOG_EXTENSION}"))
}

pub fn output_path(data_dir: &Path, id: Id) -> PathBuf {
    directory(data_dir).join(format!("{id}.{OUTPUT_EXTENSION}"))
}

pub fn stamped(at: DateTime<Utc>, line: &str) -> String {
    let mut text = at.to_rfc3339_opts(SecondsFormat::Millis, true);
    text.push(' ');
    text.push_str(line.trim_end_matches(['\r', '\n']));
    text.push('\n');
    text
}

pub async fn append(data_dir: &Path, id: Id, at: DateTime<Utc>, line: &str) -> Result<()> {
    let path = log_path(data_dir, id);
    let failed = |source: io::Error| {
        KeelError::internal("task.error.log")
            .param("path", path.display().to_string())
            .source(source)
    };
    fs::create_dir_all(directory(data_dir))
        .await
        .map_err(failed)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .map_err(failed)?;
    file.write_all(stamped(at, line).as_bytes())
        .await
        .map_err(failed)?;
    file.flush().await.map_err(failed)
}

pub async fn tail(data_dir: &Path, id: Id, lines: usize) -> Result<Vec<String>> {
    let path = log_path(data_dir, id);
    let text = match fs::read_to_string(&path).await {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(KeelError::internal("task.error.log")
                .param("path", path.display().to_string())
                .source(source));
        }
    };
    Ok(last_lines(&text, lines))
}

pub fn last_lines(text: &str, lines: usize) -> Vec<String> {
    let all: Vec<&str> = text.lines().collect();
    let skipped = all.len().saturating_sub(lines);
    all[skipped..]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_stamped_in_utc_with_milliseconds_and_ends_with_one_newline() {
        let at: DateTime<Utc> = "2026-09-15T10:00:00.250Z".parse().expect("an instant");
        assert_eq!(
            stamped(at, "exported alpha\n"),
            "2026-09-15T10:00:00.250Z exported alpha\n"
        );
        assert_eq!(stamped(at, "plain"), "2026-09-15T10:00:00.250Z plain\n");
    }

    #[test]
    fn the_tail_keeps_the_last_lines_and_all_of_them_when_there_are_fewer() {
        assert_eq!(last_lines("a\nb\nc\n", 2), ["b", "c"]);
        assert_eq!(last_lines("a\nb\n", 5), ["a", "b"]);
        assert!(last_lines("", 3).is_empty());
    }

    #[test]
    fn the_files_of_a_job_live_under_the_tasks_directory_of_the_data_dir() {
        let id: Id = "01J9Q3W6B1R8N5X2K4T7M0V9Z3".parse().expect("an id");
        let data_dir = Path::new("/var/lib/keel");
        assert_eq!(
            log_path(data_dir, id),
            Path::new("/var/lib/keel/tasks/01J9Q3W6B1R8N5X2K4T7M0V9Z3.log")
        );
        assert_eq!(
            output_path(data_dir, id),
            Path::new("/var/lib/keel/tasks/01J9Q3W6B1R8N5X2K4T7M0V9Z3.out")
        );
    }

    #[tokio::test]
    async fn appended_lines_are_read_back_by_the_tail_and_a_missing_log_is_empty() {
        let dir = tempfile::tempdir().expect("a directory");
        let id = Id::generate();
        assert!(
            tail(dir.path(), id, 10)
                .await
                .expect("no log yet")
                .is_empty()
        );
        let at: DateTime<Utc> = "2026-09-15T10:00:00Z".parse().expect("an instant");
        append(dir.path(), id, at, "first").await.expect("append");
        append(dir.path(), id, at, "second").await.expect("append");
        assert_eq!(
            tail(dir.path(), id, 1).await.expect("the tail"),
            ["2026-09-15T10:00:00.000Z second"]
        );
        assert_eq!(tail(dir.path(), id, 5).await.expect("the tail").len(), 2);
    }
}
