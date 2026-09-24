use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};

use super::Call;
use crate::process;
use crate::protocol::{ERROR_EXEC_FAILED, Failure};

const PACKAGE_LIMIT: usize = 64;
const NAME_LIMIT: usize = 128;
const NONINTERACTIVE: [(&str, &str); 1] = [("DEBIAN_FRONTEND", "noninteractive")];
const SHOW_FORMAT: &str = "--showformat=${Package}\\t${db:Status-Abbrev}\\t${Version}\\n";
const INSTALL: [&str; 10] = [
    "install",
    "--yes",
    "--quiet",
    "--no-install-recommends",
    "-o",
    "APT::Cmd::Pattern-Only=true",
    "-o",
    "Dpkg::Options::=--force-confdef",
    "-o",
    "Dpkg::Options::=--force-confold",
];
const REMOVE: [&str; 5] = [
    "remove",
    "--yes",
    "--quiet",
    "-o",
    "APT::Cmd::Pattern-Only=true",
];

type Versions = BTreeMap<String, Option<String>>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Packages {
    packages: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Get {
    name: String,
}

#[derive(Clone, Copy)]
enum Action {
    Install,
    Remove,
}

pub(super) fn install(call: &Call<'_>, args: Packages) -> Result<Value, Failure> {
    change(call, &args.packages, Action::Install)
}

pub(super) fn remove(call: &Call<'_>, args: Packages) -> Result<Value, Failure> {
    change(call, &args.packages, Action::Remove)
}

pub(super) fn refresh(call: &Call<'_>, (): ()) -> Result<Value, Failure> {
    let apt_get = &call.policy.tools().apt_get;
    process::checked(
        apt_get,
        process::tool(apt_get, ["update", "--quiet"], &NONINTERACTIVE)?,
    )?;
    Ok(json!({ "refreshed": true }))
}

pub(super) fn get(call: &Call<'_>, args: Get) -> Result<Value, Failure> {
    let names = checked_names(std::slice::from_ref(&args.name))?;
    let versions = query(call, &names)?;
    let version = versions.get(&args.name).cloned().flatten();
    Ok(json!({ "name": args.name, "version": version }))
}

fn change(call: &Call<'_>, packages: &[String], action: Action) -> Result<Value, Failure> {
    let names = checked_names(packages)?;
    if let Some(name) = names.iter().find(|name| name.ends_with('-')) {
        return Err(Failure::policy(format!(
            "apt-get reads the trailing - of {name} as a removal"
        )));
    }
    let before = query(call, &names)?;
    for name in names
        .iter()
        .filter(|name| name.ends_with('+') && before.get(*name).is_none_or(Option::is_none))
    {
        let known = match action {
            Action::Install => available(call, name)?,
            Action::Remove => false,
        };
        if !known {
            return Err(Failure::policy(format!(
                "apt-get takes {name} for another package with a trailing + that installs it"
            )));
        }
    }
    let verb: &[&str] = match action {
        Action::Install => &INSTALL,
        Action::Remove => &REMOVE,
    };
    let apt_get = &call.policy.tools().apt_get;
    let argv = verb
        .iter()
        .map(|part| (*part).to_owned())
        .chain(names.iter().cloned());
    process::checked(apt_get, process::tool(apt_get, argv, &NONINTERACTIVE)?)?;
    let after = query(call, &names)?;
    let changes: Vec<Value> = names
        .iter()
        .filter_map(|name| {
            let from = before.get(name).cloned().flatten();
            let to = after.get(name).cloned().flatten();
            (from != to).then(|| json!({ "name": name, "from": from, "to": to }))
        })
        .collect();
    Ok(json!({ "changed": !changes.is_empty(), "changes": changes }))
}

fn checked_names(packages: &[String]) -> Result<Vec<String>, Failure> {
    if packages.is_empty() || packages.len() > PACKAGE_LIMIT {
        return Err(Failure::args(format!(
            "a request names between 1 and {PACKAGE_LIMIT} packages"
        )));
    }
    let mut names = Vec::with_capacity(packages.len());
    for name in packages {
        if !valid_package(name) {
            return Err(Failure::policy(format!("{name} is not a package name")));
        }
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    Ok(names)
}

fn available(call: &Call<'_>, name: &str) -> Result<bool, Failure> {
    let apt_cache = &call.policy.tools().apt_cache;
    let output = process::checked(
        apt_cache,
        process::tool(apt_cache, ["pkgnames", name], &[])?,
    )?;
    Ok(process::stdout(&output).lines().any(|line| line == name))
}

fn query(call: &Call<'_>, names: &[String]) -> Result<Versions, Failure> {
    let dpkg_query = &call.policy.tools().dpkg_query;
    let argv = ["--show".to_owned(), SHOW_FORMAT.to_owned()]
        .into_iter()
        .chain(names.iter().cloned());
    let output = process::tool(dpkg_query, argv, &[])?;
    if !matches!(output.status.code(), Some(0 | 1)) {
        return Err(Failure::new(
            ERROR_EXEC_FAILED,
            process::detail(dpkg_query, &output),
        ));
    }
    let mut versions: Versions = names.iter().map(|name| (name.clone(), None)).collect();
    for line in process::stdout(&output).lines() {
        if let Some((name, version)) = installed(line)
            && let Some(slot) = versions.get_mut(name)
        {
            *slot = Some(version.to_owned());
        }
    }
    Ok(versions)
}

fn installed(line: &str) -> Option<(&str, &str)> {
    let mut fields = line.split('\t');
    let name = fields.next()?;
    let status = fields.next()?;
    let version = fields.next()?;
    (status.as_bytes().get(1) == Some(&b'i') && !version.is_empty()).then_some((name, version))
}

fn valid_package(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() >= 2
        && name.len() <= NAME_LIMIT
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_follow_the_distribution_rules_and_never_read_as_an_option() {
        for good in ["nginx", "php8.3-fpm", "libstdc++6", "g++", "0ad"] {
            assert!(valid_package(good), "{good}");
        }
        for bad in [
            "",
            "a",
            "-y",
            "--purge",
            "Nginx",
            "nginx=1.24",
            "nginx:amd64",
            "a b",
            "../x",
        ] {
            assert!(!valid_package(bad), "{bad}");
        }
    }

    #[test]
    fn a_query_line_names_an_installed_package_only_when_its_status_is_installed() {
        assert_eq!(
            installed("nginx\tii \t1.24.0-2"),
            Some(("nginx", "1.24.0-2"))
        );
        assert_eq!(
            installed("nginx\thi \t1.24.0-2"),
            Some(("nginx", "1.24.0-2"))
        );
        assert_eq!(installed("nginx\trc \t1.24.0-2"), None);
        assert_eq!(installed("nginx\tun \t"), None);
        assert_eq!(installed("nginx"), None);
    }
}
