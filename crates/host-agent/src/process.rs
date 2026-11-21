use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::Pid;

use crate::protocol::{ERROR_EXEC_FAILED, ERROR_IO, ERROR_TIMEOUT, ERROR_UNAVAILABLE, Failure};

pub(crate) const PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

const KILL_WAIT: Duration = Duration::from_secs(5);
const PIPE_GRACE: Duration = Duration::from_secs(2);
const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;

enum Fault {
    Start(io::Error),
    Wait(io::Error),
    Timeout(Duration),
    Flood,
}

#[derive(Clone, Copy)]
enum Stream {
    Out,
    Err,
}

enum Event {
    Chunk(Stream, Vec<u8>),
    Closed(io::Result<()>),
    Exited(io::Result<ExitStatus>),
    Fed(io::Result<()>),
}

#[derive(Default)]
struct Gathered {
    pending: usize,
    exited_at: Option<Instant>,
    status: Option<ExitStatus>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Gathered {
    fn absorb(&mut self, event: Event) -> Result<(), Fault> {
        match event {
            Event::Chunk(stream, bytes) => {
                match stream {
                    Stream::Out => self.stdout.extend_from_slice(&bytes),
                    Stream::Err => self.stderr.extend_from_slice(&bytes),
                }
                if self.stdout.len() + self.stderr.len() > OUTPUT_LIMIT {
                    return Err(Fault::Flood);
                }
            }
            Event::Closed(result) | Event::Fed(result) => {
                self.pending -= 1;
                result.map_err(Fault::Wait)?;
            }
            Event::Exited(status) => {
                self.pending -= 1;
                self.exited_at = Some(Instant::now());
                self.status = Some(status.map_err(Fault::Wait)?);
            }
        }
        Ok(())
    }
}

pub(crate) fn run<I, S>(program: &Path, args: I, timeout: Duration) -> Result<Output, Failure>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    spawn(program, args, &[], timeout).map_err(|fault| failure(program, fault, not_run))
}

pub(crate) fn tool<I, S>(
    program: &Path,
    args: I,
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<Output, Failure>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    spawn(program, args, env, timeout).map_err(|fault| failure(program, fault, unavailable))
}

fn spawn<I, S>(
    program: &Path,
    args: I,
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<Output, Fault>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    execute(
        command(program, args)
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
        None,
        timeout,
    )
}

pub(crate) fn feed(
    program: &Path,
    args: &[String],
    input: &[u8],
    timeout: Duration,
) -> Result<Output, Failure> {
    let output = execute(
        command(program, args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
        Some(input.to_vec()),
        timeout,
    )
    .map_err(|fault| failure(program, fault, unavailable))?;
    checked(program, output)
}

pub(crate) fn from_file(
    program: &Path,
    args: &[String],
    input: File,
    timeout: Duration,
) -> Result<Output, Failure> {
    let output = execute(
        command(program, args)
            .stdin(Stdio::from(input))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
        None,
        timeout,
    )
    .map_err(|fault| failure(program, fault, unavailable))?;
    checked(program, output)
}

pub(crate) fn into_file(
    program: &Path,
    args: &[String],
    out: File,
    timeout: Duration,
) -> Result<Output, Failure> {
    checked(
        program,
        redirected(program, args, Stdio::null(), out, timeout)?,
    )
}

pub(crate) fn redirected(
    program: &Path,
    args: &[String],
    input: impl Into<Stdio>,
    out: impl Into<Stdio>,
    timeout: Duration,
) -> Result<Output, Failure> {
    execute(
        command(program, args)
            .stdin(input)
            .stdout(out)
            .stderr(Stdio::piped()),
        None,
        timeout,
    )
    .map_err(|fault| failure(program, fault, unavailable))
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

fn execute(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<Output, Fault> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| Fault::Wait(io::Error::other("the timeout does not fit the clock")))?;
    let child = command.process_group(0).spawn().map_err(Fault::Start)?;
    let Ok(raw) = i32::try_from(child.id()) else {
        return Err(Fault::Wait(abandoned(child)));
    };
    let group = Pid::from_raw(raw);
    let (sender, inbox) = mpsc::channel();
    let pending = match start(child, input, &sender) {
        Ok(pending) => pending,
        Err(error) => {
            terminate(group, false);
            return Err(Fault::Wait(error));
        }
    };
    drop(sender);
    let mut gathered = Gathered {
        pending,
        ..Gathered::default()
    };
    if let Err(fault) = gather(&inbox, deadline, timeout, &mut gathered) {
        let reaped = gathered.exited_at.is_some();
        terminate(group, reaped);
        if !reaped {
            reap(&inbox);
        }
        return Err(fault);
    }
    if gathered.pending > 0 {
        tracing::warn!(
            program = ?command.get_program(),
            "a process ended but a descendant still holds its pipes; the output read so far is taken"
        );
    }
    match gathered.status {
        Some(status) => Ok(Output {
            status,
            stdout: gathered.stdout,
            stderr: gathered.stderr,
        }),
        None => Err(Fault::Wait(io::Error::other(
            "the process ended without a status",
        ))),
    }
}

fn start(mut child: Child, input: Option<Vec<u8>>, sender: &Sender<Event>) -> io::Result<usize> {
    let mut pending = 1;
    if let Some(pipe) = child.stdout.take() {
        background(sender, move |events| drain(pipe, Stream::Out, events))?;
        pending += 1;
    }
    if let Some(pipe) = child.stderr.take() {
        background(sender, move |events| drain(pipe, Stream::Err, events))?;
        pending += 1;
    }
    if let (Some(pipe), Some(bytes)) = (child.stdin.take(), input) {
        background(sender, move |events| {
            let _ = events.send(Event::Fed(write_all(pipe, &bytes)));
        })?;
        pending += 1;
    }
    background(sender, move |events| {
        let _ = events.send(Event::Exited(child.wait()));
    })?;
    Ok(pending)
}

fn background(
    sender: &Sender<Event>,
    work: impl FnOnce(&Sender<Event>) + Send + 'static,
) -> io::Result<()> {
    let sender = sender.clone();
    thread::Builder::new().spawn(move || work(&sender))?;
    Ok(())
}

fn drain(mut pipe: impl Read, stream: Stream, events: &Sender<Event>) {
    let mut buffer = vec![0; CHUNK];
    let mut listening = true;
    loop {
        let ended = match pipe.read(&mut buffer) {
            Ok(0) => Some(Ok(())),
            Ok(read) => {
                if listening
                    && events
                        .send(Event::Chunk(stream, buffer[..read].to_vec()))
                        .is_err()
                {
                    listening = false;
                }
                None
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => None,
            Err(error) => Some(Err(error)),
        };
        if let Some(result) = ended {
            let _ = events.send(Event::Closed(result));
            return;
        }
    }
}

fn write_all(mut pipe: ChildStdin, bytes: &[u8]) -> io::Result<()> {
    match pipe.write_all(bytes) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

fn gather(
    inbox: &Receiver<Event>,
    deadline: Instant,
    limit: Duration,
    gathered: &mut Gathered,
) -> Result<(), Fault> {
    while gathered.pending > 0 {
        let until = gathered
            .exited_at
            .map_or(deadline, |at| (at + PIPE_GRACE).min(deadline));
        match inbox.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(event) => gathered.absorb(event)?,
            Err(RecvTimeoutError::Timeout) if gathered.exited_at.is_some() => return Ok(()),
            Err(RecvTimeoutError::Timeout) => return Err(Fault::Timeout(limit)),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(Fault::Wait(io::Error::other(
                    "a worker of the process ended without reporting",
                )));
            }
        }
    }
    Ok(())
}

fn terminate(group: Pid, reaped: bool) {
    let mut outcomes = vec![("process group", killpg(group, Signal::SIGKILL))];
    if !reaped {
        outcomes.push(("process", kill(group, Signal::SIGKILL)));
    }
    for (what, outcome) in outcomes {
        if let Err(errno) = outcome
            && errno != Errno::ESRCH
        {
            tracing::error!(pid = group.as_raw(), error = %errno, "could not kill the {what} of a process that did not finish");
        }
    }
}

fn reap(inbox: &Receiver<Event>) {
    let deadline = Instant::now() + KILL_WAIT;
    while let Ok(event) = inbox.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        if matches!(event, Event::Exited(_)) {
            return;
        }
    }
    tracing::error!("a process killed after its timeout did not end within {KILL_WAIT:?}");
}

fn abandoned(mut child: Child) -> io::Error {
    match child.kill().and_then(|()| child.wait().map(drop)) {
        Ok(()) => io::Error::other("the process id does not fit a signal target"),
        Err(error) => error,
    }
}

fn failure(
    program: &Path,
    fault: Fault,
    missing: impl FnOnce(&Path, &io::Error) -> Failure,
) -> Failure {
    match fault {
        Fault::Start(error) => missing(program, &error),
        Fault::Wait(error) => not_run(program, &error),
        Fault::Timeout(limit) => Failure::new(
            ERROR_TIMEOUT,
            format!(
                "{} did not finish within {} s and was killed",
                program.display(),
                limit.as_secs()
            ),
        ),
        Fault::Flood => Failure::new(
            ERROR_IO,
            format!(
                "{} wrote more than {OUTPUT_LIMIT} bytes of output and was killed",
                program.display()
            ),
        ),
    }
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

pub(crate) fn succeed<I, S>(program: &Path, args: I, timeout: Duration) -> Result<Output, Failure>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    checked(program, run(program, args, timeout)?)
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

    const LIMIT: Duration = Duration::from_secs(30);

    #[test]
    fn a_tool_runs_with_a_fixed_argument_vector_and_a_clean_environment() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        write_tool(
            &tool,
            "#!/bin/sh\nprintf '%s|' \"$@\"\nprintf '\\n%s|%s|%s' \"$PATH\" \"$LC_ALL\" \"${SECRET:-unset}\"\n",
        );
        let output = succeed(&tool, ["one", "two words", "$(three)"], LIMIT).unwrap();
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
        let failure = succeed(&tool, ["x"], LIMIT).unwrap_err();
        assert_eq!(failure.key, ERROR_EXEC_FAILED);
        assert_eq!(failure.detail, "went wrong\nand said so");
        write_tool(&tool, "#!/bin/sh\nexit 4\n");
        let failure = succeed(&tool, ["x"], LIMIT).unwrap_err();
        assert_eq!(
            failure.detail,
            format!("{} exited with exit status: 4", tool.display())
        );
        let failure = run(&dir.path().join("absent"), ["x"], LIMIT).unwrap_err();
        assert_eq!(failure.key, ERROR_EXEC_FAILED);
        assert!(
            failure.detail.starts_with("could not run"),
            "{}",
            failure.detail
        );
        let failure = super::tool(&dir.path().join("absent"), ["x"], &[], LIMIT).unwrap_err();
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
        let output = feed(&tool, &["-x".to_owned()], b"line one\nline two\n", LIMIT).unwrap();
        assert_eq!(stdout(&output), "-x|line one\nline two\n");
        let target = dir.path().join("out");
        let file = File::create(&target).unwrap();
        into_file(&tool, &["a b".to_owned()], file, LIMIT).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "a b|");
        let output = from_file(
            &tool,
            &["-y".to_owned()],
            File::open(&target).unwrap(),
            LIMIT,
        )
        .unwrap();
        assert_eq!(stdout(&output), "-y|a b|");
        write_tool(&tool, "#!/bin/sh\necho 'refused' >&2\nexit 1\n");
        let failure = feed(&tool, &[], b"x", LIMIT).unwrap_err();
        assert_eq!(failure.detail, "refused");
        let failure = feed(&dir.path().join("absent"), &[], b"x", LIMIT).unwrap_err();
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
        let output = tool(
            &tool_path,
            ["x"],
            &[("DEBIAN_FRONTEND", "noninteractive")],
            LIMIT,
        )
        .unwrap();
        assert_eq!(stdout(&output), "noninteractive|C|unset");
    }

    #[test]
    fn output_beyond_what_a_pipe_holds_is_read_to_the_end_while_the_tool_runs() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        write_tool(
            &tool,
            "#!/bin/sh\nhead -c 300000 /dev/zero | tr '\\0' x\nhead -c 300000 /dev/zero | tr '\\0' y >&2\n",
        );
        let output = succeed(&tool, ["x"], LIMIT).unwrap();
        assert_eq!(output.stdout.len(), 300_000);
        assert_eq!(output.stderr.len(), 300_000);
    }

    #[test]
    fn a_fed_tool_that_never_reads_is_killed_at_its_timeout_whatever_it_was_sent() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        write_tool(&tool, "#!/bin/sh\nsleep 30\n");
        let started = Instant::now();
        let failure = feed(&tool, &[], &vec![b'x'; 1 << 20], Duration::from_secs(1)).unwrap_err();
        assert_eq!(failure.key, ERROR_TIMEOUT);
        assert_eq!(
            failure.detail,
            format!(
                "{} did not finish within 1 s and was killed",
                tool.display()
            )
        );
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    fn alive(pid: &str) -> bool {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        !state.is_empty() && !state.starts_with('Z')
    }

    #[test]
    fn a_tool_that_exits_is_no_timeout_because_a_descendant_still_holds_its_pipes() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        write_tool(&tool, "#!/bin/sh\nsleep 6 &\necho done\n");
        let started = Instant::now();
        let output = succeed(&tool, ["x"], LIMIT).unwrap();
        assert_eq!(stdout(&output), "done\n");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_tool_writing_past_the_output_cap_is_killed_and_refused_without_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        let pid_file = dir.path().join("pid");
        write_tool(
            &tool,
            "#!/bin/sh\necho $$ > \"$1\"\nhead -c 20000000 /dev/zero | tr '\\0' x\nsleep 30\n",
        );
        let failure = succeed(&tool, [pid_file.as_os_str()], LIMIT).unwrap_err();
        assert_eq!(failure.key, ERROR_IO);
        assert_eq!(
            failure.detail,
            format!(
                "{} wrote more than 16777216 bytes of output and was killed",
                tool.display()
            )
        );
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        assert!(!alive(pid.trim()), "{pid} survived the cap");
    }

    #[test]
    fn a_descendant_past_the_grace_survives_a_later_write_to_the_closed_pipes() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        let marker = dir.path().join("marker");
        write_tool(
            &tool,
            "#!/bin/sh\n( sleep 3; echo first; echo first >&2; sleep 1; echo second; echo second >&2; : > \"$1\" ) &\nexit 0\n",
        );
        let started = Instant::now();
        let output = succeed(&tool, [marker.as_os_str()], LIMIT).unwrap();
        assert_eq!(stdout(&output), "");
        assert!(!marker.exists(), "the call waited for the descendant");
        let deadline = started + Duration::from_secs(7);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            marker.exists(),
            "the descendant died of a write to a closed pipe"
        );
    }
}
