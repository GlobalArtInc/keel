use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use keel_types::Level;
use serde_json::Value;

use crate::descriptors::{BOOT_FILE, FUNCTIONS_DIR, PRODUCTS_FILE};

pub const CEILING_PERCENT: f64 = 20.0;
pub const METRIC_LEVEL: Level = Level::Admin;
pub const SHELL_VIEWS: &str = "apps/shell/views.json";
pub const PRODUCTS_DIR: &str = "products";
pub const VIEWS_FILE: &str = "views.json";
pub const COMPOSITION: &str = "composition";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("{}: {source}", path.display())]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{}: expected {expected}", path.display())]
    Shape {
        path: PathBuf,
        expected: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Share {
    pub id: String,
    pub views: usize,
    pub functions: usize,
}

impl Share {
    pub fn percent(&self) -> f64 {
        match (self.views, self.functions) {
            (0, 0) => 0.0,
            (_, 0) => f64::INFINITY,
            (views, functions) => views as f64 * 100.0 / functions as f64,
        }
    }

    pub fn above_ceiling(&self) -> bool {
        self.percent() > CEILING_PERCENT
    }
}

impl fmt::Display for Share {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}/{} {:.1}%",
            self.id,
            self.views,
            self.functions,
            self.percent()
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Shares {
    pub products: Vec<Share>,
    pub composition: Share,
}

impl Shares {
    pub fn lines(&self) -> impl Iterator<Item = &Share> {
        self.products
            .iter()
            .chain(std::iter::once(&self.composition))
    }

    pub fn green(&self) -> bool {
        !self.lines().any(Share::above_ceiling)
    }
}

impl fmt::Display for Shares {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for share in self.lines() {
            writeln!(f, "{share}")?;
        }
        Ok(())
    }
}

struct Product {
    id: String,
    modules: Vec<String>,
}

impl Product {
    fn owns(&self, name: &str) -> bool {
        self.modules.iter().any(|module| {
            name.strip_prefix(module.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
        })
    }
}

struct Function {
    module: String,
}

pub fn share(dump: &Path, frontend: &Path) -> Result<Shares, Error> {
    let level = dump.join(METRIC_LEVEL.as_str());
    let products = products_of(&level.join(PRODUCTS_FILE))?;
    let registered = registered_views(&level.join(BOOT_FILE))?;
    let functions = functions_of(&level.join(FUNCTIONS_DIR))?;
    let shell = names_of(&frontend.join(SHELL_VIEWS))?;
    let mut all: BTreeSet<String> = registered.iter().cloned().collect();
    all.extend(shell);
    let mut shares = Vec::with_capacity(products.len());
    for product in &products {
        let own = optional_names(
            &frontend
                .join(PRODUCTS_DIR)
                .join(&product.id)
                .join(VIEWS_FILE),
        )?;
        let mut views: BTreeSet<String> = registered
            .iter()
            .filter(|name| product.owns(name))
            .cloned()
            .collect();
        views.extend(own.iter().cloned());
        all.extend(own);
        shares.push(Share {
            id: product.id.clone(),
            views: views.len(),
            functions: functions
                .iter()
                .filter(|function| product.modules.contains(&function.module))
                .count(),
        });
    }
    Ok(Shares {
        products: shares,
        composition: Share {
            id: COMPOSITION.to_owned(),
            views: all.len(),
            functions: functions.len(),
        },
    })
}

fn read_json(path: &Path) -> Result<Value, Error> {
    let text = fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn shape(path: &Path, expected: &'static str) -> Error {
    Error::Shape {
        path: path.to_path_buf(),
        expected,
    }
}

fn strings(path: &Path, value: &Value, expected: &'static str) -> Result<Vec<String>, Error> {
    value
        .as_array()
        .ok_or_else(|| shape(path, expected))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| shape(path, expected))
        })
        .collect()
}

fn names_of(path: &Path) -> Result<Vec<String>, Error> {
    strings(path, &read_json(path)?, "an array of view names")
}

fn optional_names(path: &Path) -> Result<Vec<String>, Error> {
    match names_of(path) {
        Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        outcome => outcome,
    }
}

fn registered_views(path: &Path) -> Result<Vec<String>, Error> {
    let boot = read_json(path)?;
    strings(path, &boot["views"], "a boot payload with views")
}

fn products_of(path: &Path) -> Result<Vec<Product>, Error> {
    let expected = "product rows with an id and their modules";
    read_json(path)?
        .as_array()
        .ok_or_else(|| shape(path, expected))?
        .iter()
        .map(|row| {
            Ok(Product {
                id: row["id"]
                    .as_str()
                    .ok_or_else(|| shape(path, expected))?
                    .to_owned(),
                modules: strings(path, &row["modules"], expected)?,
            })
        })
        .collect()
}

fn functions_of(dir: &Path) -> Result<Vec<Function>, Error> {
    let entries = fs::read_dir(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut functions = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|source| Error::Io {
                path: dir.to_path_buf(),
                source,
            })?
            .path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let descriptor = read_json(&path)?;
        let module = descriptor["module"]
            .as_str()
            .ok_or_else(|| shape(&path, "a descriptor with a module"))?;
        functions.push(Function {
            module: module.to_owned(),
        });
    }
    Ok(functions)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    struct Layout {
        dump: PathBuf,
        frontend: PathBuf,
    }

    fn write(path: &Path, value: &Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    fn layout(root: &Path, functions: &[(&str, &str)], views: &[&str]) -> Layout {
        let dump = root.join("dump");
        let frontend = root.join("frontend");
        let admin = dump.join("admin");
        write(
            &admin.join(PRODUCTS_FILE),
            &json!([{ "id": "demo", "modules": ["note"] }, { "id": "dns", "modules": ["dns", "dns-templates"] }]),
        );
        write(
            &admin.join(BOOT_FILE),
            &json!({ "keel": { "version": "0.1.0", "api": 1 }, "views": views, "meta": "m-0" }),
        );
        for (name, module) in functions {
            write(
                &admin.join(FUNCTIONS_DIR).join(format!("{name}.json")),
                &json!({ "name": name, "module": module }),
            );
        }
        write(&frontend.join(SHELL_VIEWS), &json!([]));
        Layout { dump, frontend }
    }

    const NOTE: [(&str, &str); 8] = [
        ("note.list", "note"),
        ("note.get", "note"),
        ("note.edit", "note"),
        ("note.delete", "note"),
        ("note.recompute", "note"),
        ("note.import", "note"),
        ("note.import.check", "note"),
        ("note.tag.list", "note"),
    ];

    fn functions() -> Vec<(&'static str, &'static str)> {
        let mut all = NOTE.to_vec();
        all.extend([
            ("keel.account.list", "keel"),
            ("keel.product.list", "keel"),
            ("auth.login", "auth"),
            ("settings.set", "settings"),
            ("dns.zone.list", "dns"),
            ("dns.zone.edit", "dns"),
            ("dns-templates.list", "dns-templates"),
        ]);
        all
    }

    fn demo_views(layout: &Layout, names: &[&str]) {
        write(
            &layout
                .frontend
                .join(PRODUCTS_DIR)
                .join("demo")
                .join(VIEWS_FILE),
            &json!(names),
        );
    }

    #[test]
    fn a_ninth_view_over_eight_functions_exceeds_the_ceiling_while_one_over_eight_holds() {
        let temporary = tempfile::tempdir().unwrap();
        let fixture = layout(temporary.path(), &functions(), &[]);
        demo_views(&fixture, &["note.tag.list"]);
        let shares = share(&fixture.dump, &fixture.frontend).unwrap();
        assert_eq!(
            shares.products,
            [
                Share {
                    id: "demo".into(),
                    views: 1,
                    functions: 8
                },
                Share {
                    id: "dns".into(),
                    views: 0,
                    functions: 3
                },
            ]
        );
        assert_eq!(
            shares.composition,
            Share {
                id: COMPOSITION.into(),
                views: 1,
                functions: 15
            }
        );
        assert!(shares.green());
        assert_eq!(
            shares.to_string(),
            "demo 1/8 12.5%\ndns 0/3 0.0%\ncomposition 1/15 6.7%\n"
        );

        demo_views(&fixture, &["note.tag.list", "note.list"]);
        let two = share(&fixture.dump, &fixture.frontend).unwrap();
        assert_eq!(two.products[0].views, 2);
        assert!((two.products[0].percent() - 25.0).abs() < f64::EPSILON);
        assert!(two.products[0].above_ceiling());
        assert!(!two.composition.above_ceiling());
        assert!(!two.green());

        let names: Vec<&str> = NOTE.iter().map(|(name, _)| *name).collect();
        let ninth = layout(
            &temporary.path().join("nine"),
            &functions(),
            &["note.board"],
        );
        demo_views(&ninth, &names);
        let nine = share(&ninth.dump, &ninth.frontend).unwrap();
        assert_eq!(
            nine.products[0],
            Share {
                id: "demo".into(),
                views: 9,
                functions: 8
            }
        );
        assert_eq!(nine.products[0].to_string(), "demo 9/8 112.5%");
        assert!(!nine.green());
    }

    #[test]
    fn registered_views_count_for_the_product_owning_their_module_and_the_shell_views_for_the_composition_alone()
     {
        let temporary = tempfile::tempdir().unwrap();
        let layout = layout(
            temporary.path(),
            &functions(),
            &[
                "note.board",
                "dns.map",
                "dns-templates.editor",
                "other.thing",
            ],
        );
        demo_views(&layout, &["note.board", "note.tag.list"]);
        write(
            &layout.frontend.join(SHELL_VIEWS),
            &json!(["settings.set", "auth.totp.enroll", "note.tag.list"]),
        );
        let shares = share(&layout.dump, &layout.frontend).unwrap();
        assert_eq!(
            shares.products,
            [
                Share {
                    id: "demo".into(),
                    views: 2,
                    functions: 8
                },
                Share {
                    id: "dns".into(),
                    views: 2,
                    functions: 3
                },
            ]
        );
        assert_eq!(shares.composition.views, 7);
        assert_eq!(shares.composition.functions, 15);
        assert!(!shares.green());
        assert!(shares.products[0].above_ceiling());
        assert!(shares.products[1].above_ceiling());
        assert!(shares.composition.above_ceiling());

        let exact = Share {
            id: "x".into(),
            views: 3,
            functions: 15,
        };
        assert!(!exact.above_ceiling());
        assert_eq!(exact.to_string(), "x 3/15 20.0%");
        let empty = Share {
            id: "x".into(),
            views: 0,
            functions: 0,
        };
        assert!(!empty.above_ceiling());
        let orphan = Share {
            id: "x".into(),
            views: 1,
            functions: 0,
        };
        assert!(orphan.above_ceiling());
        assert_eq!(orphan.to_string(), "x 1/0 inf%");
    }

    #[test]
    fn the_share_needs_the_admin_dump_and_the_shell_views_file_and_reads_a_product_without_one_as_zero()
     {
        let temporary = tempfile::tempdir().unwrap();
        let layout = layout(temporary.path(), &functions(), &[]);
        let shares = share(&layout.dump, &layout.frontend).unwrap();
        assert_eq!(shares.products[0].views, 0);
        assert!(shares.green());

        fs::remove_file(layout.frontend.join(SHELL_VIEWS)).unwrap();
        let error = share(&layout.dump, &layout.frontend).unwrap_err();
        assert!(
            matches!(&error, Error::Io { path, .. } if path.ends_with(SHELL_VIEWS)),
            "{error}"
        );
        write(&layout.frontend.join(SHELL_VIEWS), &json!({ "views": [] }));
        let error = share(&layout.dump, &layout.frontend).unwrap_err();
        assert!(matches!(error, Error::Shape { .. }), "{error}");
        write(&layout.frontend.join(SHELL_VIEWS), &json!([]));

        write(
            &layout.dump.join("admin").join(PRODUCTS_FILE),
            &json!([{ "id": "demo", "state": "enabled" }]),
        );
        let error = share(&layout.dump, &layout.frontend).unwrap_err();
        assert!(
            matches!(&error, Error::Shape { path, .. } if path.ends_with(PRODUCTS_FILE)),
            "{error}"
        );

        let error = share(&temporary.path().join("nowhere"), &layout.frontend).unwrap_err();
        assert!(matches!(error, Error::Io { .. }), "{error}");
    }
}
