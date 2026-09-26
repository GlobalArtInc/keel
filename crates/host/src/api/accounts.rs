use keel_sdk::prelude::*;

use super::{Dest, Host};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct User {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct Group {
    pub name: String,
    pub gid: u32,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct Created<T> {
    #[serde(flatten)]
    pub item: T,
    pub created: bool,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Removed {
    removed: bool,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct UserList {
    users: Vec<User>,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct GroupList {
    groups: Vec<Group>,
}

pub struct Users<'a> {
    host: &'a Host<'a>,
}

impl<'a> Users<'a> {
    pub(super) fn new(host: &'a Host<'a>) -> Self {
        Self { host }
    }

    pub async fn create(
        &self,
        name: &str,
        shell: Option<&str>,
        home: Option<&Dest>,
    ) -> Result<Created<User>> {
        self.host
            .call(
                "host.user.create",
                json!({ "name": name, "shell": shell, "home": home }),
            )
            .await
    }

    pub async fn delete(&self, name: &str, remove_home: bool) -> Result<bool> {
        let answer: Removed = self
            .host
            .call(
                "host.user.delete",
                json!({ "name": name, "remove_home": remove_home }),
            )
            .await?;
        Ok(answer.removed)
    }

    pub async fn get(&self, name: &str) -> Result<User> {
        self.host
            .call("host.user.get", json!({ "name": name }))
            .await
    }

    pub async fn list(&self) -> Result<Vec<User>> {
        let answer: UserList = self.host.call("host.user.list", json!({})).await?;
        Ok(answer.users)
    }

    pub async fn lock(&self, name: &str) -> Result<User> {
        self.modify(json!({ "name": name, "locked": true })).await
    }

    pub async fn unlock(&self, name: &str) -> Result<User> {
        self.modify(json!({ "name": name, "locked": false })).await
    }

    pub async fn set_shell(&self, name: &str, shell: &str) -> Result<User> {
        self.modify(json!({ "name": name, "shell": shell })).await
    }

    async fn modify(&self, args: Value) -> Result<User> {
        self.host.call("host.user.modify", args).await
    }
}

pub struct Groups<'a> {
    host: &'a Host<'a>,
}

impl<'a> Groups<'a> {
    pub(super) fn new(host: &'a Host<'a>) -> Self {
        Self { host }
    }

    pub async fn create(&self, name: &str) -> Result<Created<Group>> {
        self.host
            .call("host.group.create", json!({ "name": name }))
            .await
    }

    pub async fn delete(&self, name: &str) -> Result<bool> {
        let answer: Removed = self
            .host
            .call("host.group.delete", json!({ "name": name }))
            .await?;
        Ok(answer.removed)
    }

    pub async fn get(&self, name: &str) -> Result<Group> {
        self.host
            .call("host.group.get", json!({ "name": name }))
            .await
    }

    pub async fn list(&self) -> Result<Vec<Group>> {
        let answer: GroupList = self.host.call("host.group.list", json!({})).await?;
        Ok(answer.groups)
    }
}

#[cfg(test)]
mod tests {
    use super::super::HostExecutor;
    use super::super::testing::{Recorder, origin};
    use super::*;

    fn user(name: &str, uid: u32) -> Value {
        json!({ "name": name, "uid": uid, "gid": uid, "home": "/nonexistent", "shell": "/usr/sbin/nologin" })
    }

    #[tokio::test]
    async fn users_and_groups_send_the_arguments_of_the_catalogue_and_read_its_answers() {
        let mut created = user("hp_site", 100000);
        created["created"] = json!(true);
        let recorder = Recorder::answering(vec![
            created,
            json!({ "removed": true }),
            user("hp_site", 100000),
            json!({ "users": [user("hp_a", 100001), user("hp_b", 100002)] }),
            json!({ "name": "hp_web", "gid": 100003, "members": [], "created": false }),
            json!({ "removed": false }),
            json!({ "name": "hp_web", "gid": 100003, "members": ["hp_a"] }),
            json!({ "groups": [] }),
            json!({ "name": "hp_site", "uid": 100000, "gid": 100000, "home": "/nonexistent", "shell": "/usr/sbin/nologin", "locked": true }),
            json!({ "name": "hp_site", "uid": 100000, "gid": 100000, "home": "/nonexistent", "shell": "/usr/sbin/nologin", "locked": false }),
            user("hp_site", 100000),
        ]);
        let executor: &dyn HostExecutor = &recorder;
        let host = executor.on(origin());
        let users = host.users();
        let made = users
            .create(
                "hp_site",
                Some("/bin/sh"),
                Some(&Dest::new("www", "hp_site")),
            )
            .await
            .unwrap();
        assert!(made.created);
        assert_eq!(made.item.uid, 100000);
        assert!(users.delete("hp_site", true).await.unwrap());
        assert_eq!(users.get("hp_site").await.unwrap().name, "hp_site");
        let listed: Vec<String> = users
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|user| user.name)
            .collect();
        assert_eq!(listed, ["hp_a", "hp_b"]);
        let groups = host.groups();
        let group = groups.create("hp_web").await.unwrap();
        assert!(!group.created);
        assert_eq!(group.item.gid, 100003);
        assert!(!groups.delete("hp_web").await.unwrap());
        assert_eq!(groups.get("hp_web").await.unwrap().members, ["hp_a"]);
        assert!(groups.list().await.unwrap().is_empty());
        assert_eq!(users.lock("hp_site").await.unwrap().uid, 100000);
        assert_eq!(users.unlock("hp_site").await.unwrap().name, "hp_site");
        assert_eq!(
            users.set_shell("hp_site", "/bin/bash").await.unwrap().uid,
            100000
        );
        assert_eq!(
            recorder.calls(),
            [
                (
                    "host.user.create".to_owned(),
                    json!({ "name": "hp_site", "shell": "/bin/sh", "home": { "root": "www", "name": "hp_site" } })
                ),
                (
                    "host.user.delete".to_owned(),
                    json!({ "name": "hp_site", "remove_home": true })
                ),
                ("host.user.get".to_owned(), json!({ "name": "hp_site" })),
                ("host.user.list".to_owned(), json!({})),
                ("host.group.create".to_owned(), json!({ "name": "hp_web" })),
                ("host.group.delete".to_owned(), json!({ "name": "hp_web" })),
                ("host.group.get".to_owned(), json!({ "name": "hp_web" })),
                ("host.group.list".to_owned(), json!({})),
                (
                    "host.user.modify".to_owned(),
                    json!({ "name": "hp_site", "locked": true })
                ),
                (
                    "host.user.modify".to_owned(),
                    json!({ "name": "hp_site", "locked": false })
                ),
                (
                    "host.user.modify".to_owned(),
                    json!({ "name": "hp_site", "shell": "/bin/bash" })
                ),
            ]
        );
    }
}
