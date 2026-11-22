use keel_sdk::{MenuGroup, MenuNode, MenuSection};
use keel_types::{Badge, BootMenuGroup, BootMenuItem, Level, Principal};

use crate::http::Projection;
use crate::registry::{Function, Registry};

pub(crate) const PLATFORM_GROUPS: [MenuGroup; 3] = [
    MenuGroup {
        id: "access",
        label: "menu.access",
        icon: "users",
        order: i32::MAX - 2,
    },
    MenuGroup {
        id: "monitor",
        label: "menu.monitor",
        icon: "activity",
        order: i32::MAX - 1,
    },
    MenuGroup {
        id: "system",
        label: "menu.system",
        icon: "settings",
        order: i32::MAX,
    },
];

pub(crate) const SECTION_DEPTH: usize = 2;
pub(crate) const TAB_DEPTH: usize = 3;

pub(crate) fn group_of(path: &str) -> &str {
    path.split('/').next().unwrap_or_default()
}

pub(crate) fn depth_of(path: &str) -> usize {
    path.split('/').count()
}

pub(crate) fn has_empty_segment(path: &str) -> bool {
    path.split('/').any(str::is_empty)
}

pub(crate) fn section_of(path: &str) -> Option<&str> {
    if depth_of(path) != TAB_DEPTH {
        return None;
    }
    path.rsplit_once('/').map(|(section, _)| section)
}

pub(crate) fn label_of<'a>(node: &'a MenuNode, function: &'a Function) -> Option<&'a str> {
    node.label_key().or_else(|| {
        function
            .descriptor()
            .ui
            .as_ref()
            .map(|ui| ui.title.as_str())
    })
}

pub(crate) fn project(registry: &Registry, principal: &Principal) -> Vec<BootMenuGroup> {
    let projection = Projection::new(registry, principal);
    let mut nodes: Vec<&MenuNode> = registry.menu().iter().collect();
    nodes.sort_by(|a, b| {
        a.order_value()
            .cmp(&b.order_value())
            .then_with(|| a.path().cmp(b.path()))
    });
    let level = principal.level;
    registry
        .groups()
        .iter()
        .filter_map(|group| {
            let mut entries: Vec<(i32, &str, BootMenuItem)> = nodes
                .iter()
                .filter(|node| group_of(node.path()) == group.id)
                .filter(|node| depth_of(node.path()) <= SECTION_DEPTH)
                .filter_map(|node| {
                    item(registry, &projection, level, node)
                        .map(|item| (node.order_value(), node.path(), item))
                })
                .collect();
            entries.extend(
                registry
                    .sections()
                    .iter()
                    .filter(|section| group_of(section.path()) == group.id)
                    .filter_map(|section| {
                        section_item(registry, &projection, level, &nodes, section)
                            .map(|item| (section.order_value(), section.path(), item))
                    }),
            );
            entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
            let items: Vec<BootMenuItem> = entries.into_iter().map(|(_, _, item)| item).collect();
            (!items.is_empty()).then(|| BootMenuGroup {
                id: group.id.clone(),
                product: group.product.clone(),
                label: group.label.clone(),
                icon: group.icon.clone(),
                items,
            })
        })
        .collect()
}

fn section_item(
    registry: &Registry,
    projection: &Projection<'_>,
    level: Level,
    nodes: &[&MenuNode],
    section: &MenuSection,
) -> Option<BootMenuItem> {
    let children: Vec<BootMenuItem> = nodes
        .iter()
        .filter(|node| section_of(node.path()) == Some(section.path()))
        .filter_map(|node| item(registry, projection, level, node))
        .collect();
    let first = children.first()?;
    Some(BootMenuItem {
        path: section.path().to_owned(),
        label: section.label_key().to_owned(),
        func: first.func.clone(),
        kind: first.kind,
        icon: Some(section.icon_name().to_owned()),
        badge: None,
        children,
    })
}

fn item(
    registry: &Registry,
    projection: &Projection<'_>,
    level: Level,
    node: &MenuNode,
) -> Option<BootMenuItem> {
    if node.min_level().is_some_and(|minimum| level < minimum) {
        return None;
    }
    let (label, kind) = match registry.function(node.func()) {
        Some(function) => {
            projection.admit(function).ok()?;
            (label_of(node, function)?, Some(function.def().kind()))
        }
        None => {
            let view = registry.view(node.func())?;
            projection.admits_view(view).ok()?;
            (node.label_key()?, None)
        }
    };
    Some(BootMenuItem {
        path: node.path().to_owned(),
        label: label.to_owned(),
        func: node.func().to_owned(),
        kind,
        icon: node.icon_name().map(str::to_owned),
        badge: node.badge_problem().map(|problem| Badge {
            problem: problem.to_owned(),
        }),
        children: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use keel_sdk::prelude::*;
    use keel_types::{Id, Scope, Via};

    use super::*;
    use crate::loader::Composition;
    use crate::loader::testing::{Nothing, module};

    fn someone(level: Level) -> Principal {
        Principal {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9A1".parse().unwrap(),
            login: "alice".into(),
            level,
            account_id: Id::ROOT_ACCOUNT,
            role: "owner".into(),
            actor: None,
            scope: Scope::Full,
            via: Via::Http,
            external_id: None,
            origin: None,
            lang: "en".into(),
            grants: BTreeMap::new(),
        }
    }

    fn reading(name: &str, level: Level) -> FunctionDef {
        function::<Nothing, Nothing>(name)
            .kind(Kind::Get)
            .level(level)
            .handler(|_: Call<Nothing>| async { Ok(Nothing {}) })
    }

    fn board_init(r: &mut Registrar) -> Result<()> {
        r.function(reading("board.plain", Level::User))
            .function(reading("board.leveled", Level::User))
            .function(reading("board.last", Level::User))
            .function(reading("board.first", Level::User))
            .function(reading("board.mid", Level::Admin))
            .function(reading("board.late", Level::User))
            .function(reading("board.closed", Level::Admin))
            .section(MenuSection::new("system/board", "board.menu.board", "users").order(5))
            .section(MenuSection::new("system/closed", "board.menu.closed", "users").order(6))
            .menu(MenuNode::new("system/plain", "board.plain").label("board.plain").order(1))
            .menu(
                MenuNode::new("system/leveled", "board.leveled")
                    .label("board.leveled")
                    .order(9)
                    .level(Level::Admin),
            )
            .menu(MenuNode::new("system/last", "board.last").label("board.last").order(99))
            .menu(
                MenuNode::new("system/board/first", "board.first")
                    .label("board.first")
                    .order(10)
                    .level(Level::Reseller),
            )
            .menu(MenuNode::new("system/board/mid", "board.mid").label("board.mid").order(15))
            .menu(MenuNode::new("system/board/late", "board.late").label("board.late").order(20))
            .menu(MenuNode::new("system/closed/only", "board.closed").label("board.closed"))
            .i18n(
                "en",
                r#"{ "board": { "plain": "Plain", "leveled": "Leveled", "last": "Last", "first": "First",
                                "mid": "Mid", "late": "Late", "closed": "Closed",
                                "menu": { "board": "Board", "closed": "Closed" } } }"#,
            );
        Ok(())
    }

    module!(BOARD, "board", [], board_init);

    fn shown(level: Level) -> Vec<BootMenuItem> {
        let frozen = Composition::new()
            .module(&BOARD)
            .resolve()
            .unwrap()
            .init()
            .unwrap()
            .freeze()
            .unwrap();
        project(frozen.registry(), &someone(level))
            .into_iter()
            .filter(|group| group.id == "system")
            .flat_map(|group| group.items)
            .filter(|item| item.func.starts_with("board."))
            .collect()
    }

    fn tabs(item: &BootMenuItem) -> Vec<&str> {
        item.children.iter().map(|tab| tab.path.as_str()).collect()
    }

    #[test]
    fn a_section_is_one_item_among_its_group_ordered_by_its_own_order_with_its_tabs_as_children() {
        let items = shown(Level::Admin);
        let paths: Vec<&str> = items.iter().map(|item| item.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "system/plain",
                "system/board",
                "system/closed",
                "system/leveled",
                "system/last"
            ]
        );
        let section = &items[1];
        assert_eq!(section.label, "board.menu.board");
        assert_eq!(section.icon.as_deref(), Some("users"));
        assert!(section.badge.is_none());
        assert_eq!(section.func, "board.first");
        assert_eq!(section.kind, Some(Kind::Get));
        assert_eq!(
            tabs(section),
            [
                "system/board/first",
                "system/board/mid",
                "system/board/late"
            ]
        );
        assert!(section.children.iter().all(|tab| tab.children.is_empty()));
        assert!(items[0].children.is_empty());
    }

    #[test]
    fn a_node_level_hides_the_node_from_the_menu_below_it_and_the_section_takes_the_first_admitted_tab()
     {
        let reseller = shown(Level::Reseller);
        let paths: Vec<&str> = reseller.iter().map(|item| item.path.as_str()).collect();
        assert_eq!(paths, ["system/plain", "system/board", "system/last"]);
        assert_eq!(
            tabs(&reseller[1]),
            ["system/board/first", "system/board/late"]
        );
        let user = shown(Level::User);
        assert_eq!(tabs(&user[1]), ["system/board/late"]);
        assert_eq!(user[1].func, "board.late");
    }

    #[test]
    fn a_section_without_an_admitted_tab_is_left_out() {
        let user = shown(Level::User);
        assert!(user.iter().all(|item| item.path != "system/closed"));
    }

    #[test]
    fn the_group_of_a_node_is_the_first_path_segment() {
        assert_eq!(group_of("system/accounts"), "system");
        assert_eq!(group_of("dns/zones/records"), "dns");
        assert_eq!(group_of("bare"), "bare");
        assert_eq!(group_of(""), "");
    }

    #[test]
    fn only_a_three_segment_path_belongs_to_a_section_and_the_section_is_its_first_two_segments() {
        assert_eq!(section_of("access/users/tokens"), Some("access/users"));
        assert_eq!(section_of("access/users"), None);
        assert_eq!(section_of("access"), None);
        assert_eq!(section_of("a/b/c/d"), None);
        assert_eq!(depth_of("a/b/c"), 3);
        assert_eq!(depth_of(""), 1);
    }

    #[test]
    fn the_platform_groups_are_access_monitor_and_system_in_that_order_with_the_system_id_kept() {
        let ids: Vec<&str> = PLATFORM_GROUPS.iter().map(|group| group.id).collect();
        assert_eq!(ids, ["access", "monitor", "system"]);
        let labels: Vec<&str> = PLATFORM_GROUPS.iter().map(|group| group.label).collect();
        assert_eq!(labels, ["menu.access", "menu.monitor", "menu.system"]);
        assert!(
            PLATFORM_GROUPS
                .windows(2)
                .all(|pair| pair[0].order < pair[1].order)
        );
    }
}
