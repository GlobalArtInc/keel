use keel_sdk::prelude::*;
use keel_sdk::serde_json::{self, Map};

use super::Host;
use crate::frame::{ERROR_ARGS, failure};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct Dest {
    root: String,
    name: String,
}

impl Dest {
    pub fn new(root: &str, name: &str) -> Self {
        Self {
            root: root.to_owned(),
            name: name.to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct Attributes {
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
}

impl Attributes {
    pub fn mode(mut self, mode: &str) -> Self {
        self.mode = Some(mode.to_owned());
        self
    }

    pub fn owner(mut self, owner: &str) -> Self {
        self.owner = Some(owner.to_owned());
        self
    }

    pub fn group(mut self, group: &str) -> Self {
        self.group = Some(group.to_owned());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct FileContent {
    pub content: String,
    pub mode: String,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub struct Entry {
    pub name: String,
    pub kind: String,
    pub size: u64,
    pub mode: String,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Changed {
    changed: bool,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Removed {
    removed: bool,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Made {
    created: bool,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Listing {
    entries: Vec<Entry>,
}

fn with(mut args: Value, attributes: &Attributes) -> Result<Value> {
    let extra = serde_json::to_value(attributes).map_err(|error| failure(ERROR_ARGS, error))?;
    if let (Value::Object(args), Value::Object(extra)) = (&mut args, extra) {
        args.extend(extra);
    }
    Ok(args)
}

pub struct Files<'a> {
    host: &'a Host<'a>,
}

impl<'a> Files<'a> {
    pub(super) fn new(host: &'a Host<'a>) -> Self {
        Self { host }
    }

    pub async fn write(&self, dest: &Dest, content: &str, attributes: &Attributes) -> Result<bool> {
        let args = with(json!({ "dest": dest, "content": content }), attributes)?;
        let answer: Changed = self.host.call("host.fs.write", args).await?;
        Ok(answer.changed)
    }

    pub async fn read(&self, dest: &Dest) -> Result<FileContent> {
        self.host
            .call("host.fs.read", json!({ "dest": dest }))
            .await
    }

    pub async fn remove(&self, dest: &Dest, recursive: bool) -> Result<bool> {
        let answer: Removed = self
            .host
            .call(
                "host.fs.remove",
                json!({ "dest": dest, "recursive": recursive }),
            )
            .await?;
        Ok(answer.removed)
    }

    pub async fn mkdir(&self, dest: &Dest, attributes: &Attributes) -> Result<bool> {
        let args = with(json!({ "dest": dest }), attributes)?;
        let answer: Made = self.host.call("host.fs.mkdir", args).await?;
        Ok(answer.created)
    }

    pub async fn list(&self, root: &str) -> Result<Vec<Entry>> {
        let answer: Listing = self
            .host
            .call("host.fs.list", json!({ "root": root }))
            .await?;
        Ok(answer.entries)
    }
}

pub struct Template<'a> {
    host: &'a Host<'a>,
    id: String,
    vars: Result<Map<String, Value>>,
    dest: Option<Dest>,
    attributes: Attributes,
    validate: Option<String>,
    reload: Option<String>,
}

impl<'a> Template<'a> {
    pub(super) fn new(host: &'a Host<'a>, id: &str) -> Self {
        Self {
            host,
            id: id.to_owned(),
            vars: Ok(Map::new()),
            dest: None,
            attributes: Attributes::default(),
            validate: None,
            reload: None,
        }
    }

    pub fn vars(mut self, vars: &impl Serialize) -> Self {
        self.vars = match serde_json::to_value(vars) {
            Ok(Value::Object(map)) => Ok(map),
            Ok(other) => Err(failure(
                ERROR_ARGS,
                format!("the variables of {} are not an object: {other}", self.id),
            )),
            Err(error) => Err(failure(ERROR_ARGS, error)),
        };
        self
    }

    pub fn dest(mut self, root: &str, name: &str) -> Self {
        self.dest = Some(Dest::new(root, name));
        self
    }

    pub fn mode(mut self, mode: &str) -> Self {
        self.attributes = self.attributes.mode(mode);
        self
    }

    pub fn owner(mut self, owner: &str) -> Self {
        self.attributes = self.attributes.owner(owner);
        self
    }

    pub fn group(mut self, group: &str) -> Self {
        self.attributes = self.attributes.group(group);
        self
    }

    pub fn validate(mut self, check: &str) -> Self {
        self.validate = Some(check.to_owned());
        self
    }

    pub fn reload(mut self, service: &str) -> Self {
        self.reload = Some(service.to_owned());
        self
    }

    pub async fn apply(self) -> Result<bool> {
        let vars = self.vars?;
        let dest = self.dest.ok_or_else(|| {
            failure(
                ERROR_ARGS,
                format!("the template {} is applied without a dest", self.id),
            )
        })?;
        let mut args = json!({ "template": self.id, "vars": vars, "dest": dest });
        if let Some(check) = self.validate {
            args["validate"] = json!(check);
        }
        if let Some(service) = self.reload {
            args["reload"] = json!(service);
        }
        let args = with(args, &self.attributes)?;
        let answer: Changed = self.host.call("host.template.apply", args).await?;
        Ok(answer.changed)
    }
}

#[cfg(test)]
mod tests {
    use super::super::HostExecutor;
    use super::super::testing::{Recorder, origin};
    use super::*;

    #[tokio::test]
    async fn a_template_apply_carries_the_design_s_arguments_and_refuses_a_missing_dest_before_the_agent()
     {
        let recorder = Recorder::answering(vec![json!({ "changed": true })]);
        let executor: &dyn HostExecutor = &recorder;
        let host = executor.on(origin());
        let changed = host
            .template("bind/zone.j2")
            .vars(&json!({ "origin": "a.org" }))
            .dest("zones", "keel-dns-a.org.zone")
            .mode("0640")
            .owner("bind")
            .validate("named-checkzone")
            .reload("named")
            .apply()
            .await
            .unwrap();
        assert!(changed);
        let missing = host.template("bind/zone.j2").apply().await.unwrap_err();
        assert_eq!(missing.key(), "host.error.args");
        let scalar = host
            .template("bind/zone.j2")
            .vars(&3)
            .dest("zones", "keel-dns-a.org.zone")
            .apply()
            .await
            .unwrap_err();
        assert_eq!(scalar.key(), "host.error.args");
        assert_eq!(
            recorder.calls(),
            [(
                "host.template.apply".to_owned(),
                json!({
                    "template": "bind/zone.j2",
                    "vars": { "origin": "a.org" },
                    "dest": { "root": "zones", "name": "keel-dns-a.org.zone" },
                    "mode": "0640",
                    "owner": "bind",
                    "validate": "named-checkzone",
                    "reload": "named",
                })
            )]
        );
    }
}
