use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

use super::{Check, Findings, Layout, UNITS};

const KERNEL_UNIT: &str = "keel.service";
const PRIVILEGED_UNIT: &str = "keel-privileged.service";
const UPDATE_SERVICE: &str = "keel-update.service";
const UPDATE_PATH: &str = "keel-update.path";
const SERVICE_USER: &str = "keel";
const ROOT: &str = "root";
const USER_KEY: &str = "User=";
const IS_ENABLED: &str = "is-enabled";
const IS_ACTIVE: &str = "is-active";
const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(10);

const FILES: [(&str, Option<&str>); 4] = [
    (KERNEL_UNIT, Some(SERVICE_USER)),
    (PRIVILEGED_UNIT, Some(ROOT)),
    (UPDATE_SERVICE, Some(ROOT)),
    (UPDATE_PATH, None),
];
const ENABLED: [&str; 3] = [KERNEL_UNIT, PRIVILEGED_UNIT, UPDATE_PATH];

fn user_of(unit: &str) -> String {
    let mut in_service = false;
    for line in unit.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_service = line.eq_ignore_ascii_case("[Service]");
            continue;
        }
        if in_service && let Some(user) = line.strip_prefix(USER_KEY) {
            let user = user.trim();
            return if user.is_empty() {
                ROOT.to_owned()
            } else {
                user.to_owned()
            };
        }
    }
    ROOT.to_owned()
}

enum Answer {
    Yes,
    No(String),
    Unavailable(String),
}

async fn ask(systemctl: &Path, verb: &str, unit: &str) -> Answer {
    let spawned = Command::new(systemctl)
        .arg(verb)
        .arg(unit)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(SYSTEMCTL_TIMEOUT, spawned).await {
        Ok(Ok(output)) => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if output.status.success() {
                Answer::Yes
            } else if text.is_empty() {
                Answer::No(String::from_utf8_lossy(&output.stderr).trim().to_owned())
            } else {
                Answer::No(text)
            }
        }
        Ok(Err(error)) => Answer::Unavailable(format!("{}: {error}", systemctl.display())),
        Err(_) => Answer::Unavailable(format!(
            "{} did not answer within {}s",
            systemctl.display(),
            SYSTEMCTL_TIMEOUT.as_secs()
        )),
    }
}

pub(super) async fn check(layout: &Layout) -> Check {
    let mut findings = Findings::new(UNITS);
    let mut installed = Vec::new();
    for (name, expected_user) in FILES {
        let path = layout.unit_dir.join(name);
        match fs::read_to_string(&path) {
            Ok(text) => {
                if let Some(expected) = expected_user {
                    let user = user_of(&text);
                    if user != expected {
                        findings.fail(format!(
                            "{name} runs as {user}; the design runs it as {expected}"
                        ));
                    }
                    installed.push(format!("{name} ({user})"));
                } else {
                    installed.push(name.to_owned());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                findings.fail(format!(
                    "{name} is missing from {}",
                    layout.unit_dir.display()
                ));
            }
            Err(error) => findings.fail(format!("{} cannot be read: {error}", path.display())),
        }
    }
    let mut asked = true;
    'units: for unit in ENABLED {
        for verb in [IS_ENABLED, IS_ACTIVE] {
            match ask(&layout.systemctl, verb, unit).await {
                Answer::Yes => {}
                Answer::No(state) => {
                    let state = if state.is_empty() {
                        format!("not {}", verb.trim_start_matches("is-"))
                    } else {
                        state
                    };
                    if verb == IS_ACTIVE && unit == KERNEL_UNIT {
                        findings.warn(format!("{unit} is {state}"));
                    } else {
                        findings.fail(format!("{unit} is {state}"));
                    }
                }
                Answer::Unavailable(reason) => {
                    findings.warn(format!(
                        "the service manager could not be asked about the units: {reason}"
                    ));
                    asked = false;
                    break 'units;
                }
            }
        }
    }
    let state = if asked {
        "enabled and active"
    } else {
        "installed"
    };
    findings.finish(format!(
        "{} are installed; {}, {} and {} are {state}",
        installed.join(", "),
        KERNEL_UNIT,
        PRIVILEGED_UNIT,
        UPDATE_PATH
    ))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::super::Status;
    use super::*;

    const SHIPPED: [(&str, &str); 4] = [
        (
            KERNEL_UNIT,
            include_str!("../../../../deploy/systemd/keel.service"),
        ),
        (
            PRIVILEGED_UNIT,
            include_str!("../../../../deploy/systemd/keel-privileged.service"),
        ),
        (
            UPDATE_SERVICE,
            include_str!("../../../../deploy/systemd/keel-update.service"),
        ),
        (
            UPDATE_PATH,
            include_str!("../../../../deploy/systemd/keel-update.path"),
        ),
    ];

    fn stub(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("systemctl");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn layout(dir: &Path, systemctl: PathBuf) -> Layout {
        Layout {
            opt: dir.join("opt"),
            unit_dir: dir.join("units"),
            systemctl,
            agent_socket: dir.join("agent.sock"),
        }
    }

    fn install_units(dir: &Path) {
        fs::create_dir_all(dir.join("units")).unwrap();
        for (name, text) in SHIPPED {
            fs::write(dir.join("units").join(name), text).unwrap();
        }
    }

    #[test]
    fn the_user_of_a_unit_comes_from_its_service_section_and_defaults_to_root() {
        assert_eq!(user_of(SHIPPED[0].1), "keel");
        assert_eq!(user_of(SHIPPED[1].1), "root");
        assert_eq!(
            user_of("[Unit]\nUser=nobody\n[Service]\nExecStart=/bin/true\n"),
            "root"
        );
        assert_eq!(user_of("[Service]\nUser=\n"), "root");
        assert_eq!(user_of("[service]\n  User = alice \n"), "root");
        assert_eq!(user_of("[service]\nUser=alice\n"), "alice");
    }

    #[tokio::test]
    async fn the_shipped_units_with_a_stub_that_says_enabled_and_active_are_green() {
        let dir = tempfile::tempdir().unwrap();
        install_units(dir.path());
        let log = dir.path().join("systemctl.log");
        let systemctl = stub(
            dir.path(),
            &format!(
                "printf '%s\\n' \"$*\" >> {}\ncase \"$1\" in is-enabled) echo enabled ;; is-active) echo active ;; esac",
                log.display()
            ),
        );
        let check = check(&layout(dir.path(), systemctl)).await;
        assert_eq!(check.status, Status::Ok, "{}", check.message);
        assert_eq!(
            check.message,
            "keel.service (keel), keel-privileged.service (root), keel-update.service (root), keel-update.path are installed; keel.service, keel-privileged.service and keel-update.path are enabled and active"
        );
        let calls: Vec<String> = fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            calls,
            [
                "is-enabled keel.service",
                "is-active keel.service",
                "is-enabled keel-privileged.service",
                "is-active keel-privileged.service",
                "is-enabled keel-update.path",
                "is-active keel-update.path",
            ]
        );
    }

    #[tokio::test]
    async fn a_missing_unit_a_wrong_user_and_a_disabled_unit_are_named() {
        let dir = tempfile::tempdir().unwrap();
        install_units(dir.path());
        fs::remove_file(dir.path().join("units").join(UPDATE_PATH)).unwrap();
        fs::write(
            dir.path().join("units").join(PRIVILEGED_UNIT),
            "[Service]\nUser=keel\nExecStart=/opt/keel/bin/current privileged\n",
        )
        .unwrap();
        let systemctl = stub(
            dir.path(),
            "case \"$1 $2\" in 'is-enabled keel.service') echo disabled; exit 1 ;; 'is-active keel.service') echo inactive; exit 3 ;; *) echo yes ;; esac",
        );
        let check = check(&layout(dir.path(), systemctl)).await;
        assert_eq!(check.status, Status::Fail);
        for expected in [
            "keel-privileged.service runs as keel; the design runs it as root",
            "keel-update.path is missing from",
            "keel.service is disabled",
            "keel.service is inactive",
        ] {
            assert!(check.message.contains(expected), "{}", check.message);
        }
    }

    #[tokio::test]
    async fn a_stopped_kernel_unit_is_a_warning_and_an_absent_service_manager_too() {
        let dir = tempfile::tempdir().unwrap();
        install_units(dir.path());
        let systemctl = stub(
            dir.path(),
            "case \"$1 $2\" in 'is-active keel.service') exit 3 ;; *) echo yes ;; esac",
        );
        let stopped = check(&layout(dir.path(), systemctl)).await;
        assert_eq!(stopped.status, Status::Warn, "{}", stopped.message);
        assert_eq!(stopped.message, "keel.service is not active");
        let absent = check(&layout(dir.path(), dir.path().join("no-systemctl"))).await;
        assert_eq!(absent.status, Status::Warn, "{}", absent.message);
        assert!(
            absent
                .message
                .starts_with("the service manager could not be asked about the units:"),
            "{}",
            absent.message
        );
        let unit_dir_missing = check(&layout(
            &dir.path().join("elsewhere"),
            dir.path().join("no-systemctl"),
        ))
        .await;
        assert_eq!(unit_dir_missing.status, Status::Fail);
        assert_eq!(
            unit_dir_missing.message.matches("is missing from").count(),
            4
        );
    }
}
