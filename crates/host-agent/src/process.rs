use std::ffi::OsStr;
use std::io;
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
    spawn(program, args, env).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            Failure::new(
                ERROR_UNAVAILABLE,
                format!("{} is not installed on this host", program.display()),
            )
        } else {
            not_run(program, &error)
        }
    })
}

fn spawn<I, S>(program: &Path, args: I, env: &[(&str, &str)]) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", PATH)
        .env("LC_ALL", "C")
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn a_tool_runs_with_a_fixed_argument_vector_and_a_clean_environment() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        fs::write(
            &tool,
            "#!/bin/sh\nprintf '%s|' \"$@\"\nprintf '\\n%s|%s|%s' \"$PATH\" \"$LC_ALL\" \"${SECRET:-unset}\"\n",
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
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
        fs::write(
            &tool,
            "#!/bin/sh\necho 'went wrong' >&2\necho 'and said so'\nexit 3\n",
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let failure = succeed(&tool, ["x"]).unwrap_err();
        assert_eq!(failure.key, ERROR_EXEC_FAILED);
        assert_eq!(failure.detail, "went wrong\nand said so");
        fs::write(&tool, "#!/bin/sh\nexit 4\n").unwrap();
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
    fn a_tool_gets_the_environment_it_names_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let tool_path = dir.path().join("tool");
        fs::write(
            &tool_path,
            "#!/bin/sh\nprintf '%s|%s|%s' \"$DEBIAN_FRONTEND\" \"$LC_ALL\" \"${HOME:-unset}\"\n",
        )
        .unwrap();
        fs::set_permissions(&tool_path, fs::Permissions::from_mode(0o755)).unwrap();
        let output = tool(&tool_path, ["x"], &[("DEBIAN_FRONTEND", "noninteractive")]).unwrap();
        assert_eq!(stdout(&output), "noninteractive|C|unset");
    }
}
