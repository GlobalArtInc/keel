use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use crate::protocol::{ERROR_EXEC_FAILED, ERROR_UNAVAILABLE, Failure};

pub(crate) const PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

pub(crate) fn run<I, S>(program: &Path, args: I) -> Result<Output, Failure>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    spawn(program, args, &[]).map_err(|error| not_run(program, &error))
}

pub(crate) fn tool<I, S>(program: &Path, args: I, env: &[(&str, &str)]) -> Result<Output, Failure>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    spawn(program, args, env).map_err(|error| unavailable(program, &error))
}

fn spawn<I, S>(program: &Path, args: I, env: &[(&str, &str)]) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    command(program, args)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
}

pub(crate) fn feed(program: &Path, args: &[String], input: &[u8]) -> Result<Output, Failure> {
    let mut child = command(program, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| unavailable(program, &error))?;
    if let Some(mut stdin) = child.stdin.take()
        && let Err(error) = stdin.write_all(input)
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        return Err(not_run(program, &error));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| not_run(program, &error))?;
    checked(program, output)
}

pub(crate) fn into_file(program: &Path, args: &[String], out: File) -> Result<Output, Failure> {
    let output = command(program, args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| unavailable(program, &error))?;
    checked(program, output)
}

fn command<I, S>(program: &Path, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("PATH", PATH)
        .env("LC_ALL", "C");
    command
}

fn unavailable(program: &Path, error: &io::Error) -> Failure {
    if error.kind() == io::ErrorKind::NotFound {
        Failure::new(
            ERROR_UNAVAILABLE,
            format!("{} is not installed on this host", program.display()),
        )
    } else {
        not_run(program, error)
    }
}

fn not_run(program: &Path, error: &io::Error) -> Failure {
    Failure::new(
        ERROR_EXEC_FAILED,
        format!("could not run {}: {error}", program.display()),
    )
}

pub(crate) fn checked(program: &Path, output: Output) -> Result<Output, Failure> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(Failure::new(ERROR_EXEC_FAILED, detail(program, &output)))
    }
}

pub(crate) fn succeed<I, S>(program: &Path, args: I) -> Result<Output, Failure>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    checked(program, run(program, args)?)
}

pub(crate) fn detail(program: &Path, output: &Output) -> String {
    let text = [&output.stderr, &output.stdout]
        .into_iter()
        .map(|bytes| String::from_utf8_lossy(bytes).trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        format!("{} exited with {}", program.display(), output.status)
    } else {
        text
    }
}

pub(crate) fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stub::write_tool;

    #[test]
    fn a_tool_runs_with_a_fixed_argument_vector_and_a_clean_environment() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        write_tool(
            &tool,
            "#!/bin/sh\nprintf '%s|' \"$@\"\nprintf '\\n%s|%s|%s' \"$PATH\" \"$LC_ALL\" \"${SECRET:-unset}\"\n",
        );
        let output = succeed(&tool, ["one", "two words", "$(three)"]).unwrap();
        assert_eq!(
            stdout(&output),
            format!("one|two words|$(three)|\n{PATH}|C|unset")
        );
    }

    #[test]
    fn a_failing_tool_answers_exec_failed_with_its_output_as_the_detail() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        write_tool(
            &tool,
            "#!/bin/sh\necho 'went wrong' >&2\necho 'and said so'\nexit 3\n",
        );
        let failure = succeed(&tool, ["x"]).unwrap_err();
        assert_eq!(failure.key, ERROR_EXEC_FAILED);
        assert_eq!(failure.detail, "went wrong\nand said so");
        write_tool(&tool, "#!/bin/sh\nexit 4\n");
        let failure = succeed(&tool, ["x"]).unwrap_err();
        assert_eq!(
            failure.detail,
            format!("{} exited with exit status: 4", tool.display())
        );
        let failure = run(&dir.path().join("absent"), ["x"]).unwrap_err();
        assert_eq!(failure.key, ERROR_EXEC_FAILED);
        assert!(
            failure.detail.starts_with("could not run"),
            "{}",
            failure.detail
        );
        let failure = super::tool(&dir.path().join("absent"), ["x"], &[]).unwrap_err();
        assert_eq!(failure.key, ERROR_UNAVAILABLE);
        assert!(
            failure.detail.ends_with("is not installed on this host"),
            "{}",
            failure.detail
        );
    }

    #[test]
    fn a_fed_tool_reads_its_input_on_stdin_and_a_redirected_one_writes_into_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        write_tool(&tool, "#!/bin/sh\nprintf '%s|' \"$@\"\ncat\n");
        let output = feed(&tool, &["-x".to_owned()], b"line one\nline two\n").unwrap();
        assert_eq!(stdout(&output), "-x|line one\nline two\n");
        let target = dir.path().join("out");
        let file = File::create(&target).unwrap();
        into_file(&tool, &["a b".to_owned()], file).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "a b|");
        write_tool(&tool, "#!/bin/sh\necho 'refused' >&2\nexit 1\n");
        let failure = feed(&tool, &[], b"x").unwrap_err();
        assert_eq!(failure.detail, "refused");
        let failure = feed(&dir.path().join("absent"), &[], b"x").unwrap_err();
        assert_eq!(failure.key, ERROR_UNAVAILABLE);
    }

    #[test]
    fn a_tool_gets_the_environment_it_names_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let tool_path = dir.path().join("tool");
        write_tool(
            &tool_path,
            "#!/bin/sh\nprintf '%s|%s|%s' \"$DEBIAN_FRONTEND\" \"$LC_ALL\" \"${HOME:-unset}\"\n",
        );
        let output = tool(&tool_path, ["x"], &[("DEBIAN_FRONTEND", "noninteractive")]).unwrap();
        assert_eq!(stdout(&output), "noninteractive|C|unset");
    }
}
