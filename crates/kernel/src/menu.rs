use keel_sdk::{MenuGroup, MenuNode};
use keel_types::{Badge, BootMenuGroup, BootMenuItem, Principal};

use crate::http::Projection;
use crate::registry::{Function, Registry};

pub(crate) const SYSTEM_GROUP: MenuGroup = MenuGroup {
    id: "system",
    label: "menu.system",
    icon: "settings",
    order: i32::MAX,
};

pub(crate) fn group_of(path: &str) -> &str {
    path.split('/').next().unwrap_or_default()
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
    registry
        .groups()
        .iter()
        .filter_map(|group| {
            let items: Vec<BootMenuItem> = nodes
                .iter()
                .filter(|node| group_of(node.path()) == group.id)
                .filter_map(|node| item(registry, &projection, node))
                .collect();
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

fn item(registry: &Registry, projection: &Projection<'_>, node: &MenuNode) -> Option<BootMenuItem> {
    let (label, kind) = match registry.function(node.func()) {
        Some(function) => {
            projection.admit(function).ok()?;
            (label_of(node, function)?, Some(function.def().kind()))
        }
        None => {
            let view = registry.view(node.func())?;
            projection.admits(view.access()).ok()?;
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_group_of_a_node_is_the_first_path_segment() {
        assert_eq!(group_of("system/accounts"), "system");
        assert_eq!(group_of("dns/zones/records"), "dns");
        assert_eq!(group_of("bare"), "bare");
        assert_eq!(group_of(""), "");
        assert_eq!(SYSTEM_GROUP.id, "system");
        assert_eq!(SYSTEM_GROUP.label, "menu.system");
    }
}
