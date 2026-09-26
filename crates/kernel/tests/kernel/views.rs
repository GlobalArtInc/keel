use keel_kernel::loader::FreezeProblem;
use keel_kernel::{Composition, Registry};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_testkit::{Harness, Who, tokio};
use keel_types::{Boot, Envelope};

const EN: &str = r#"{
    "atlas": { "map": { "title": "Map" }, "board": { "title": "Board" },
               "item": { "list": { "title": "Items" } } },
    "menu": { "atlas": "Atlas" }
}"#;
const BUNDLE_EN: &str = r#"{ "product": { "atlas": { "brand": "Atlas" } } }"#;

#[derive(Deserialize, Serialize, JsonSchema)]
struct Item {
    id: Id,
    title: String,
}

static ATLAS: ModuleDef = ModuleDef {
    id: "atlas",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: atlas_init,
};

fn atlas_init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<ListQuery, Page<Item>>("atlas.item.list")
            .kind(Kind::List)
            .level(Level::User)
            .unowned()
            .ui(List::new("atlas.item.list.title").column(Column::new("title", ColumnType::Text)))
            .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) }),
    )
    .permission("atlas.map.open", &[Level::Reseller])
    .view(
        "atlas.map",
        ViewSpec::new(Level::User)
            .permission("atlas.map.open")
            .menu(
                MenuNode::new("atlas/map", "atlas.map")
                    .label("atlas.map.title")
                    .icon("map")
                    .order(20),
            )
            .needs(["atlas.maps"]),
    )
    .view("atlas.health", ViewSpec::new(Level::Admin))
    .view(
        "atlas.board",
        ViewSpec::new(Level::User).menu(
            MenuNode::new("atlas/board", "atlas.board")
                .label("atlas.board.title")
                .order(10),
        ),
    )
    .menu(MenuNode::new("atlas/items", "atlas.item.list").order(30))
    .i18n("en", EN);
    Ok(())
}

static ATLAS_BUNDLE: ProductManifest = ProductManifest {
    id: "atlas",
    name: "product.atlas.brand",
    version: "0.1.0",
    api: keel_types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: "product.atlas.logo",
        hue: 120,
        chroma: 0.1,
    },
    modules: &[&ATLAS],
    bridges: &[],
    menu: &[MenuGroup {
        id: "atlas",
        label: "menu.atlas",
        icon: "map",
        order: 10,
    }],
    levels: LevelLabels {
        labels: &[],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[Edition {
        id: "standard",
        features: &["atlas.maps"],
    }],
    i18n: &[("en", BUNDLE_EN)],
    assets: &[],
};

fn atlas_items(boot: &Boot) -> Vec<(&str, &str, Option<Kind>, Option<&str>)> {
    boot.menu
        .iter()
        .filter(|group| group.id == "atlas")
        .flat_map(|group| group.items.iter())
        .map(|item| {
            (
                item.path.as_str(),
                item.label.as_str(),
                item.kind,
                item.icon.as_deref(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_registered_view_is_listed_in_boot_for_the_levels_its_access_admits_and_menus_without_a_kind()
 {
    let harness = Harness::new().product(&ATLAS_BUNDLE).boot().await;
    let registry: &Registry = harness.registry();
    let map = registry.view("atlas.map").expect("the view is frozen");
    assert_eq!(map.module(), "atlas");
    assert_eq!(map.needs(), ["atlas.maps"]);
    assert!(registry.function("atlas.map").is_none());

    let http = harness.http().await;
    let guest = http.get(Who::Guest, "/api/v1/boot").await;
    assert_eq!(guest.status, 200, "{}", guest.body);
    let guest: Boot = serde_json::from_value(guest.json()).expect("a boot payload");
    assert!(guest.views.is_empty());
    assert!(atlas_items(&guest).is_empty());

    let user = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(user.status, 200, "{}", user.body);
    let raw = user.json();
    let user: Boot = serde_json::from_value(raw.clone()).expect("a boot payload");
    assert_eq!(user.views, ["atlas.board"]);
    assert_eq!(
        atlas_items(&user),
        [
            ("atlas/board", "atlas.board.title", None, None),
            (
                "atlas/items",
                "atlas.item.list.title",
                Some(Kind::List),
                None
            ),
        ]
    );
    let items = raw["menu"][0]["items"].as_array().expect("the atlas items");
    assert_eq!(raw["menu"][0]["id"], json!("atlas"));
    assert_eq!(
        items[0],
        json!({ "path": "atlas/board", "label": "atlas.board.title", "func": "atlas.board" })
    );
    assert_eq!(items[1]["kind"], json!("list"));

    let reseller = http.get(Who::Reseller, "/api/v1/boot").await;
    let reseller: Boot = serde_json::from_value(reseller.json()).expect("a boot payload");
    assert_eq!(reseller.views, ["atlas.board", "atlas.map"]);
    assert_eq!(
        atlas_items(&reseller),
        [
            ("atlas/board", "atlas.board.title", None, None),
            ("atlas/map", "atlas.map.title", None, Some("map")),
            (
                "atlas/items",
                "atlas.item.list.title",
                Some(Kind::List),
                None
            ),
        ]
    );

    let admin = http.get(Who::Admin, "/api/v1/boot").await;
    let admin: Boot = serde_json::from_value(admin.json()).expect("a boot payload");
    assert_eq!(admin.views, ["atlas.board", "atlas.health", "atlas.map"]);
    assert_eq!(atlas_items(&admin), atlas_items(&reseller));
    http.stop().await.expect("the server stops");

    let socket = harness.socket().await;
    let Envelope::Ok { data: owner, .. } = socket.boot().await else {
        panic!("boot over the socket failed");
    };
    assert_eq!(owner.views, admin.views);
    assert_eq!(atlas_items(&owner), atlas_items(&admin));
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}

static STRAY: ModuleDef = ModuleDef {
    id: "stray",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: stray_init,
};

fn stray_init(r: &mut Registrar) -> Result<()> {
    r.view("other.map", ViewSpec::new(Level::User));
    Ok(())
}

static LOOSE: ModuleDef = ModuleDef {
    id: "loose",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: loose_init,
};

fn loose_init(r: &mut Registrar) -> Result<()> {
    r.view(
        "loose.map",
        ViewSpec::new(Level::User)
            .permission("loose.map.open")
            .menu(MenuNode::new("nowhere/map", "loose.map").label("loose.map.title")),
    )
    .i18n("en", r#"{ "loose": { "map": { "title": "Map" } } }"#);
    Ok(())
}

#[test]
fn a_view_with_a_foreign_prefix_an_unknown_permission_or_an_unknown_group_fails_freeze() {
    let stray = Composition::new()
        .module(&STRAY)
        .resolve()
        .expect("the composition resolves")
        .init()
        .expect_err("the prefix is refused before the freeze");
    assert_eq!(
        stray.to_string(),
        "module stray: init registered invalid entries:\n  - module stray: view \"other.map\" does not carry the module prefix"
    );
    let loose = Composition::new()
        .module(&LOOSE)
        .resolve()
        .expect("the composition resolves")
        .init()
        .expect("init registers")
        .freeze()
        .expect_err("the freeze fails");
    assert!(
        matches!(
            loose.problems(),
            [
                FreezeProblem::ViewPermissionUnknown { module, view, permission },
                FreezeProblem::MenuGroupUnknown { path, group, .. },
            ] if module == "loose"
                && view == "loose.map"
                && permission == "loose.map.open"
                && path == "nowhere/map"
                && group == "nowhere"
        ),
        "{loose}"
    );
    assert_eq!(
        loose.to_string(),
        "the registries cannot be frozen:\n  - module loose: view loose.map names the permission loose.map.open, which no module registers\n  - module loose: menu node nowhere/map names the group nowhere, which no enabled product declares"
    );
}
