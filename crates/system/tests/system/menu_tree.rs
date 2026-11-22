use keel_sdk::prelude::*;
use keel_system::{
    audit, auth, backup, cron, license, notify, peers, plugins, settings, tasks, update, webhooks,
};
use keel_testkit::license::Issuer;
use keel_testkit::{Harness, Who, tokio};

type Item = (String, Vec<String>);
type Tree = Vec<(String, Vec<Item>)>;
type Tabs<'a> = &'a [&'a str];
type Section<'a> = (&'a str, Tabs<'a>);
type Spec<'a> = &'a [(&'a str, &'a [Section<'a>])];

fn tree(boot: &Value) -> Tree {
    let text = |value: &Value| value.as_str().expect("a string").to_owned();
    boot["menu"]
        .as_array()
        .expect("groups")
        .iter()
        .map(|group| {
            let items = group["items"]
                .as_array()
                .expect("items")
                .iter()
                .map(|item| {
                    let tabs = item["children"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|tab| text(&tab["path"]))
                        .collect();
                    (text(&item["path"]), tabs)
                })
                .collect();
            (text(&group["id"]), items)
        })
        .collect()
}

fn expected(groups: Spec<'_>) -> Tree {
    groups
        .iter()
        .map(|(group, items)| {
            let items = items
                .iter()
                .map(|(path, tabs)| {
                    (
                        (*path).to_owned(),
                        tabs.iter().map(|tab| (*tab).to_owned()).collect(),
                    )
                })
                .collect();
            ((*group).to_owned(), items)
        })
        .collect()
}

#[tokio::test]
async fn the_platform_menu_is_three_groups_of_sections_cut_by_level_and_a_section_without_tabs_is_dropped()
 {
    let issuer = Issuer::generate().expect("a throwaway license key");
    let harness = Harness::new()
        .module(&auth::MODULE)
        .module(&settings::MODULE)
        .module(&audit::MODULE)
        .module(&tasks::MODULE)
        .module(&cron::MODULE)
        .module(&notify::MODULE)
        .module(&webhooks::MODULE)
        .module(&license::MODULE)
        .module(&update::MODULE)
        .module(&backup::MODULE)
        .module(&peers::MODULE)
        .module(&plugins::MODULE)
        .license_key(issuer.key())
        .boot()
        .await;
    let http = harness.http().await;
    let mut seen = Vec::new();
    for who in [Who::Admin, Who::Reseller, Who::A1] {
        let answer = http.get(who, "/api/v1/boot").await;
        assert_eq!(answer.status, 200, "{}", answer.body);
        seen.push(tree(&answer.json()));
    }
    http.stop().await.expect("the server stops");
    harness.finish().await;
    let [admin, reseller, user]: [Tree; 3] = seen.try_into().expect("three trees");
    assert_eq!(
        admin,
        expected(&[
            (
                "access",
                &[(
                    "access/users",
                    &[
                        "access/users/principals",
                        "access/users/accounts",
                        "access/users/tokens",
                        "access/users/sessions",
                        "access/users/ip-rules",
                        "access/users/grants",
                    ]
                )]
            ),
            (
                "monitor",
                &[
                    ("monitor/problems", &[]),
                    (
                        "monitor/tasks",
                        &["monitor/tasks/queue", "monitor/tasks/cron"]
                    ),
                    (
                        "monitor/logs",
                        &["monitor/logs/audit", "monitor/logs/server"]
                    ),
                    ("monitor/health", &[]),
                ]
            ),
            (
                "system",
                &[
                    (
                        "system/settings",
                        &[
                            "system/settings/general",
                            "system/settings/brand",
                            "system/settings/channels",
                            "system/settings/webhooks",
                            "system/settings/peers",
                        ]
                    ),
                    ("system/backups", &[]),
                    (
                        "system/install",
                        &[
                            "system/install/products",
                            "system/install/license",
                            "system/install/updates",
                            "system/install/plugins",
                        ]
                    ),
                ]
            ),
        ])
    );
    assert_eq!(
        reseller,
        expected(&[
            (
                "access",
                &[(
                    "access/users",
                    &[
                        "access/users/principals",
                        "access/users/accounts",
                        "access/users/tokens",
                        "access/users/sessions",
                        "access/users/grants",
                    ]
                )]
            ),
            (
                "monitor",
                &[
                    ("monitor/problems", &[]),
                    ("monitor/tasks", &["monitor/tasks/queue"]),
                    ("monitor/logs", &["monitor/logs/audit"]),
                ]
            ),
            (
                "system",
                &[(
                    "system/settings",
                    &["system/settings/general", "system/settings/webhooks"]
                )]
            ),
        ])
    );
    assert_eq!(
        user,
        expected(&[
            (
                "access",
                &[(
                    "access/users",
                    &[
                        "access/users/principals",
                        "access/users/tokens",
                        "access/users/sessions",
                    ]
                )]
            ),
            (
                "monitor",
                &[
                    ("monitor/problems", &[]),
                    ("monitor/tasks", &["monitor/tasks/queue"]),
                    ("monitor/logs", &["monitor/logs/audit"]),
                ]
            ),
            (
                "system",
                &[("system/settings", &["system/settings/webhooks"])]
            ),
        ])
    );
}
