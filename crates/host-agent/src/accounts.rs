use crate::policy::{Policy, tool};
use crate::process;
use crate::protocol::{ERROR_EXEC_FAILED, Failure};

const NOT_FOUND: i32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct User {
    pub(crate) name: String,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) home: String,
    pub(crate) shell: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    pub(crate) name: String,
    pub(crate) gid: u32,
    pub(crate) members: Vec<String>,
}

pub(crate) struct Accounts<'a> {
    policy: &'a Policy,
}

impl<'a> Accounts<'a> {
    pub(crate) fn new(policy: &'a Policy) -> Self {
        Self { policy }
    }

    pub(crate) fn user(&self, key: &str) -> Result<Option<User>, Failure> {
        self.lookup("passwd", Some(key), parse_user)
            .map(|users| users.into_iter().next())
    }

    pub(crate) fn users(&self) -> Result<Vec<User>, Failure> {
        self.lookup("passwd", None, parse_user)
    }

    pub(crate) fn group(&self, key: &str) -> Result<Option<Group>, Failure> {
        self.lookup("group", Some(key), parse_group)
            .map(|groups| groups.into_iter().next())
    }

    pub(crate) fn groups(&self) -> Result<Vec<Group>, Failure> {
        self.lookup("group", None, parse_group)
    }

    fn lookup<T>(
        &self,
        database: &str,
        key: Option<&str>,
        parse: fn(&str) -> Option<T>,
    ) -> Result<Vec<T>, Failure> {
        let mut args = vec![database, "--"];
        args.extend(key);
        let getent = &self.policy.tools().getent;
        let output = process::run(getent, &args, self.policy.timeout(tool::GETENT))?;
        if !output.status.success() {
            if output.status.code() == Some(NOT_FOUND) && key.is_some() {
                return Ok(Vec::new());
            }
            return Err(Failure::new(
                ERROR_EXEC_FAILED,
                process::detail(getent, &output),
            ));
        }
        Ok(process::stdout(&output).lines().filter_map(parse).collect())
    }
}

fn parse_user(line: &str) -> Option<User> {
    let fields: Vec<&str> = line.split(':').collect();
    if fields.len() < 7 {
        return None;
    }
    Some(User {
        name: fields[0].to_owned(),
        uid: fields[2].parse().ok()?,
        gid: fields[3].parse().ok()?,
        home: fields[5].to_owned(),
        shell: fields[6].to_owned(),
    })
}

fn parse_group(line: &str) -> Option<Group> {
    let fields: Vec<&str> = line.split(':').collect();
    if fields.len() < 3 {
        return None;
    }
    Some(Group {
        name: fields[0].to_owned(),
        gid: fields[2].parse().ok()?,
        members: fields
            .get(3)
            .map(|members| {
                members
                    .split(',')
                    .filter(|member| !member.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwd_and_group_lines_parse_into_their_fields() {
        assert_eq!(
            parse_user("hp_a:x:100001:100001::/nonexistent:/usr/sbin/nologin"),
            Some(User {
                name: "hp_a".to_owned(),
                uid: 100_001,
                gid: 100_001,
                home: "/nonexistent".to_owned(),
                shell: "/usr/sbin/nologin".to_owned(),
            })
        );
        assert_eq!(parse_user("broken:x:one:1::/:/bin/sh"), None);
        assert_eq!(parse_user("short:x:1"), None);
        assert_eq!(
            parse_group("hp_a:x:100001:alice,bob"),
            Some(Group {
                name: "hp_a".to_owned(),
                gid: 100_001,
                members: vec!["alice".to_owned(), "bob".to_owned()],
            })
        );
        assert_eq!(
            parse_group("hp_b:x:100002:").unwrap().members,
            Vec::<String>::new()
        );
        assert_eq!(
            parse_group("hp_b:x:100002").unwrap().members,
            Vec::<String>::new()
        );
        assert_eq!(parse_group("x:y"), None);
    }
}
