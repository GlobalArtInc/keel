use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::{Pid, Uid};

use super::user::inside;
use crate::policy::UidRange;
use crate::protocol::{ERROR_EXEC_FAILED, Failure};

const PROC: &str = "/proc";

#[derive(Clone, Copy)]
pub(super) struct Grace {
    term: Duration,
    kill: Duration,
    step: Duration,
}

pub(super) const GRACE: Grace = Grace {
    term: Duration::from_secs(3),
    kill: Duration::from_secs(2),
    step: Duration::from_millis(50),
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Stopped {
    pub(super) terminated: usize,
    pub(super) killed: usize,
}

pub(super) fn stop_user(
    range: UidRange,
    uid: u32,
    name: &str,
    stopped: &mut Stopped,
) -> Result<(), Failure> {
    inside(range, uid, name)?;
    if uid == Uid::current().as_raw() {
        return Err(Failure::policy(format!(
            "{name} has the id {uid} of the agent itself, whose processes the agent does not stop"
        )));
    }
    let own = Pid::this().as_raw();
    let proc_root = Path::new(PROC);
    escalate(
        GRACE,
        &mut || processes_of(proc_root, uid, own),
        &mut |pid, signal| {
            if eligible(pid.as_raw(), own) && is_target(proc_root, pid.as_raw(), uid) {
                deliver(pid, signal)
            } else {
                Ok(false)
            }
        },
        stopped,
    )
}

fn escalate<F, S>(
    grace: Grace,
    find: &mut F,
    send: &mut S,
    stopped: &mut Stopped,
) -> Result<(), Failure>
where
    F: FnMut() -> Result<Vec<Pid>, Failure>,
    S: FnMut(Pid, Signal) -> Result<bool, Failure>,
{
    let first = find()?;
    if first.is_empty() {
        return Ok(());
    }
    let mut termed = BTreeSet::new();
    let sent = signal_all(&first, Signal::SIGTERM, send, &mut termed);
    stopped.terminated += termed.len();
    sent?;
    let deadline = Instant::now() + grace.term;
    loop {
        let alive = find()?;
        if alive.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(
            grace
                .step
                .min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    let mut killed = BTreeSet::new();
    let outcome = kill_until_gone(grace, find, send, &mut killed);
    stopped.killed += killed.len();
    outcome
}

fn kill_until_gone<F, S>(
    grace: Grace,
    find: &mut F,
    send: &mut S,
    killed: &mut BTreeSet<i32>,
) -> Result<(), Failure>
where
    F: FnMut() -> Result<Vec<Pid>, Failure>,
    S: FnMut(Pid, Signal) -> Result<bool, Failure>,
{
    let deadline = Instant::now() + grace.kill;
    loop {
        let alive = find()?;
        if alive.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Failure::new(
                ERROR_EXEC_FAILED,
                format!(
                    "{} process(es) of the account are still alive {:?} after SIGKILL",
                    alive.len(),
                    grace.kill
                ),
            ));
        }
        signal_all(&alive, Signal::SIGKILL, send, killed)?;
        thread::sleep(
            grace
                .step
                .min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

fn deliver(pid: Pid, signal: Signal) -> Result<bool, Failure> {
    match kill(pid, signal) {
        Ok(()) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(errno) => Err(Failure::new(
            ERROR_EXEC_FAILED,
            format!("could not send {signal} to process {pid}: {errno}"),
        )),
    }
}

fn signal_all<S>(
    pids: &[Pid],
    signal: Signal,
    send: &mut S,
    sent: &mut BTreeSet<i32>,
) -> Result<(), Failure>
where
    S: FnMut(Pid, Signal) -> Result<bool, Failure>,
{
    for pid in pids {
        if pid.as_raw() > 1 && send(*pid, signal)? {
            sent.insert(pid.as_raw());
        }
    }
    Ok(())
}

fn processes_of(proc_root: &Path, uid: u32, own: i32) -> Result<Vec<Pid>, Failure> {
    let entries = match fs::read_dir(proc_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            tracing::warn!(
                path = %proc_root.display(),
                "the process table is not mounted, so no process of the account is stopped"
            );
            return Ok(Vec::new());
        }
        Err(error) => return Err(Failure::io(proc_root.display(), &error)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| Failure::io(proc_root.display(), &error))?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        if eligible(pid, own) && is_target(proc_root, pid, uid) {
            found.push(Pid::from_raw(pid));
        }
    }
    found.sort();
    Ok(found)
}

fn is_target(proc_root: &Path, pid: i32, uid: u32) -> bool {
    let path = proc_root.join(pid.to_string()).join("status");
    match fs::read_to_string(&path) {
        Ok(status) => real_uid(&status) == Some(uid) && !defunct(&status),
        Err(error) if gone(&error) => false,
        Err(error) => {
            tracing::debug!(pid, error = %error, "the status of a process could not be read");
            false
        }
    }
}

fn gone(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(Errno::ESRCH as i32)
}

fn eligible(pid: i32, own: i32) -> bool {
    pid > 1 && pid != own
}

fn real_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|ids| ids.split_whitespace().next())
        .and_then(|real| real.parse().ok())
}

fn defunct(status: &str) -> bool {
    let zombie = status
        .lines()
        .find_map(|line| line.strip_prefix("State:"))
        .is_some_and(|state| state.trim_start().starts_with('Z'));
    let threads: u32 = status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(1);
    zombie && threads <= 1
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};

    use super::*;
    use crate::protocol::ERROR_POLICY;

    const RANGE: UidRange = UidRange {
        from: 4_000_000_000,
        to: 4_000_000_099,
    };
    const TARGET: u32 = 4_000_000_001;

    fn fabricate(root: &Path, pid: &str, status: Option<&str>) {
        let dir = root.join(pid);
        fs::create_dir_all(&dir).unwrap();
        if let Some(status) = status {
            fs::write(dir.join("status"), status).unwrap();
        }
    }

    fn status(real: u32, effective: u32, state: &str, threads: u32) -> String {
        format!(
            "Name:\tphp-fpm\nState:\t{state}\nPid:\t1\nUid:\t{real}\t{effective}\t{real}\t{effective}\nGid:\t{real}\t{real}\t{real}\t{real}\nThreads:\t{threads}\n"
        )
    }

    fn stubborn() -> Child {
        let mut child = Command::new("sh")
            .args(["-c", "trap '' TERM; echo ready; exec sleep 30"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "ready");
        child
    }

    fn visible(children: &RefCell<Vec<Child>>, count: usize) -> Vec<Pid> {
        children
            .borrow_mut()
            .iter_mut()
            .take(count)
            .filter_map(|child| {
                child
                    .try_wait()
                    .unwrap()
                    .is_none()
                    .then(|| Pid::from_raw(child.id() as i32))
            })
            .collect()
    }

    fn grace(term: Duration) -> Grace {
        Grace {
            term,
            kill: Duration::from_secs(5),
            step: Duration::from_millis(10),
        }
    }

    #[test]
    fn the_scan_selects_the_real_uid_and_never_pid_1_or_the_agent() {
        let root = tempfile::tempdir().unwrap();
        let alive = status(TARGET, TARGET, "S (sleeping)", 1);
        fabricate(root.path(), "1", Some(&alive));
        fabricate(root.path(), "20", Some(&alive));
        fabricate(
            root.path(),
            "21",
            Some(&status(TARGET, 0, "R (running)", 1)),
        );
        fabricate(
            root.path(),
            "22",
            Some(&status(0, TARGET, "S (sleeping)", 1)),
        );
        fabricate(
            root.path(),
            "23",
            Some(&status(TARGET + 1, TARGET + 1, "S (sleeping)", 1)),
        );
        fabricate(
            root.path(),
            "24",
            Some(&status(TARGET, TARGET, "Z (zombie)", 1)),
        );
        fabricate(root.path(), "25", Some("Name:\tkthreadd\nState:\tS\n"));
        fabricate(root.path(), "26", None);
        fabricate(
            root.path(),
            "27",
            Some(&status(TARGET, TARGET, "Z (zombie)", 3)),
        );
        fabricate(root.path(), "99", Some(&alive));
        fabricate(root.path(), "self", Some(&alive));
        fabricate(root.path(), "28", None);
        fs::create_dir(root.path().join("28/status")).unwrap();
        let found = processes_of(root.path(), TARGET, 99).unwrap();
        let found: Vec<i32> = found.iter().map(|pid| pid.as_raw()).collect();
        assert_eq!(found, [20, 21, 27]);
        assert_eq!(real_uid("Uid:\t7\t8\t9\t10\n"), Some(7));
        assert_eq!(real_uid("Name:\tx\n"), None);
        assert!(eligible(2, 99));
        assert!(!eligible(1, 99));
        assert!(!eligible(0, 99));
        assert!(!eligible(99, 99));
        assert!(!eligible(-5, 99));
        assert_eq!(
            processes_of(&root.path().join("absent"), TARGET, 99).unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn a_signal_is_sent_only_while_the_process_still_has_the_target_uid() {
        let root = tempfile::tempdir().unwrap();
        let mut child = stubborn();
        let raw = child.id() as i32;
        let own = Pid::this().as_raw();
        let guarded = |pid: Pid| {
            if eligible(pid.as_raw(), own) && is_target(root.path(), pid.as_raw(), TARGET) {
                deliver(pid, Signal::SIGKILL)
            } else {
                Ok(false)
            }
        };
        fabricate(
            root.path(),
            &raw.to_string(),
            Some(&status(TARGET + 1, TARGET + 1, "S (sleeping)", 1)),
        );
        assert!(!guarded(Pid::from_raw(raw)).unwrap());
        assert!(child.try_wait().unwrap().is_none());
        fabricate(
            root.path(),
            &raw.to_string(),
            Some(&status(TARGET, TARGET, "S (sleeping)", 1)),
        );
        assert!(guarded(Pid::from_raw(raw)).unwrap());
        child.wait().unwrap();
        let mut sent = BTreeSet::new();
        signal_all(
            &[Pid::from_raw(1), Pid::from_raw(0), Pid::from_raw(-1)],
            Signal::SIGKILL,
            &mut |_, _| panic!("a pid at or below 1 was signalled"),
            &mut sent,
        )
        .unwrap();
        assert!(sent.is_empty());
    }

    #[test]
    fn a_process_that_ignores_sigterm_is_killed_after_the_grace() {
        let children = RefCell::new(vec![stubborn()]);
        let mut stopped = Stopped::default();
        escalate(
            grace(Duration::from_secs(1)),
            &mut || Ok(visible(&children, 1)),
            &mut |pid, signal| deliver(pid, signal),
            &mut stopped,
        )
        .unwrap();
        assert_eq!(
            stopped,
            Stopped {
                terminated: 1,
                killed: 1
            }
        );
        assert!(children.borrow_mut()[0].try_wait().unwrap().is_some());
    }

    #[test]
    fn a_process_forked_during_the_kill_wait_is_killed_on_the_next_poll() {
        let children = RefCell::new(vec![stubborn(), stubborn()]);
        let forked = Cell::new(1);
        let mut stopped = Stopped::default();
        escalate(
            grace(Duration::from_millis(200)),
            &mut || Ok(visible(&children, forked.get())),
            &mut |pid, signal| {
                if signal == Signal::SIGKILL {
                    forked.set(2);
                }
                deliver(pid, signal)
            },
            &mut stopped,
        )
        .unwrap();
        assert_eq!(
            stopped,
            Stopped {
                terminated: 1,
                killed: 2
            }
        );
        assert!(
            children
                .borrow_mut()
                .iter_mut()
                .all(|child| child.try_wait().unwrap().is_some())
        );
    }

    #[test]
    fn a_process_that_obeys_sigterm_is_not_killed() {
        let children = RefCell::new(vec![Command::new("sleep").arg("30").spawn().unwrap()]);
        let mut stopped = Stopped::default();
        escalate(
            grace(Duration::from_secs(10)),
            &mut || Ok(visible(&children, 1)),
            &mut |pid, signal| deliver(pid, signal),
            &mut stopped,
        )
        .unwrap();
        assert_eq!(
            stopped,
            Stopped {
                terminated: 1,
                killed: 0
            }
        );
    }

    #[test]
    fn an_account_without_processes_costs_one_scan_and_no_wait() {
        let scans = Cell::new(0);
        let mut stopped = Stopped::default();
        escalate(
            GRACE,
            &mut || {
                scans.set(scans.get() + 1);
                Ok(Vec::new())
            },
            &mut |_, _| panic!("nothing is signalled"),
            &mut stopped,
        )
        .unwrap();
        assert_eq!(scans.get(), 1);
        assert_eq!(stopped, Stopped::default());
    }

    #[test]
    fn a_uid_outside_the_range_or_the_agents_own_is_refused_before_any_scan() {
        let mut stopped = Stopped::default();
        let failure = stop_user(RANGE, 1000, "hp_a", &mut stopped).unwrap_err();
        assert_eq!(failure.key, ERROR_POLICY);
        assert!(failure.detail.contains("1000"), "{}", failure.detail);
        let failure = stop_user(RANGE, 0, "hp_a", &mut stopped).unwrap_err();
        assert_eq!(failure.key, ERROR_POLICY);
        let own = Uid::current().as_raw();
        let wide = UidRange {
            from: 0,
            to: u32::MAX - 1,
        };
        let failure = stop_user(wide, own, "hp_a", &mut stopped).unwrap_err();
        assert_eq!(failure.key, ERROR_POLICY);
        assert!(
            failure.detail.contains("agent itself"),
            "{}",
            failure.detail
        );
        assert_eq!(stopped, Stopped::default());
    }
}
