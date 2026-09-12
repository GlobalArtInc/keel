use keel_kernel::dispatcher::{CallOptions, Dispatcher, Reply};
use keel_kernel::{Composition, Kernel, Storage};
use keel_sdk::prelude::*;
use keel_testkit::TestDatabase;
use keel_types::{ErrorCode, Scope, Via};

struct Bench {
    database: TestDatabase,
    storage: Storage,
    kernel: Kernel,
    dispatcher: Dispatcher,
    reseller: Id,
    customer: Id,
    other: Id,
}

struct Seeded {
    bench: Bench,
    admin: Id,
    res: Id,
    cust: Id,
    other: Id,
}

fn principal(level: Level, account: Id) -> Principal {
    Principal {
        id: Id::generate(),
        login: format!("{level}-{account}"),
        level,
        account_id: account,
        role: "owner".into(),
        actor: None,
        scope: Scope::Full,
        via: Via::Http,
        external_id: None,
        origin: None,
        lang: "en".into(),
    }
}

fn owner() -> Principal {
    principal(Level::Owner, Id::ROOT_ACCOUNT)
}

fn admin() -> Principal {
    principal(Level::Admin, Id::ROOT_ACCOUNT)
}

fn code(reply: &Reply) -> ErrorCode {
    reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.data()))
        .code
}

fn field(reply: &Reply) -> (String, String) {
    let error = reply.error().expect("an error");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    let field = error.fields.first().expect("a field error");
    (field.path.clone(), field.key.clone().unwrap_or_default())
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn id_of(reply: &Reply) -> Id {
    data(reply)["id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a ulid")
}

fn column<'a>(page: &'a Value, key: &str) -> Vec<&'a str> {
    page["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row[key].as_str().expect(key))
        .collect()
}

async fn bench() -> Bench {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let kernel = Kernel::boot(Composition::new(), &storage)
        .await
        .expect("the bare kernel boots");
    let dispatcher = Dispatcher::new(kernel.registry().clone(), storage.db().clone())
        .expect("every schema compiles");
    let mut bench = Bench {
        database,
        storage,
        kernel,
        dispatcher,
        reseller: Id::ROOT_ACCOUNT,
        customer: Id::ROOT_ACCOUNT,
        other: Id::ROOT_ACCOUNT,
    };
    bench.reseller = bench.account(&owner(), "reseller", None).await;
    bench.customer = bench.account(&owner(), "user", Some(bench.reseller)).await;
    bench.other = bench.account(&owner(), "user", None).await;
    bench
}

async fn seeded() -> Seeded {
    let bench = bench().await;
    let admin = bench.principal(&owner(), "admin", Level::Admin, None).await;
    let res = bench
        .principal(&owner(), "res", Level::Reseller, Some(bench.reseller))
        .await;
    let cust = bench
        .principal(&owner(), "cust", Level::User, Some(bench.customer))
        .await;
    let other = bench
        .principal(&owner(), "other", Level::User, Some(bench.other))
        .await;
    Seeded {
        bench,
        admin,
        res,
        cust,
        other,
    }
}

impl Bench {
    async fn call(&self, principal: &Principal, func: &str, params: Value) -> Reply {
        self.dispatcher
            .call(principal.clone(), func, params, CallOptions::default())
            .await
    }

    async fn account(&self, principal: &Principal, kind: &str, parent: Option<Id>) -> Id {
        let reply = self
            .call(
                principal,
                "keel.account.create",
                json!({ "kind": kind, "parent": parent }),
            )
            .await;
        id_of(&reply)
    }

    async fn principal(
        &self,
        principal: &Principal,
        login: &str,
        level: Level,
        account: Option<Id>,
    ) -> Id {
        let reply = self
            .call(
                principal,
                "keel.principal.create",
                json!({ "login": login, "level": level, "account": account }),
            )
            .await;
        id_of(&reply)
    }

    async fn acl_gen(&self, id: Id) -> i64 {
        let reply = self
            .call(&owner(), "keel.principal.get", json!({ "id": id }))
            .await;
        data(&reply)["aclGen"].as_i64().expect("aclGen")
    }

    async fn path(&self, id: Id) -> String {
        let reply = self
            .call(&owner(), "keel.account.get", json!({ "id": id }))
            .await;
        data(&reply)["path"].as_str().expect("path").to_owned()
    }

    async fn accounts(&self, principal: &Principal) -> Vec<String> {
        let reply = self.call(principal, "keel.account.list", json!({})).await;
        column(data(&reply), "id")
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    async fn logins(&self, principal: &Principal, query: Value) -> Vec<String> {
        let reply = self.call(principal, "keel.principal.list", query).await;
        column(data(&reply), "login")
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    async fn finish(self) {
        self.kernel.stop().await.expect("the kernel stops");
        self.storage.close().await.expect("the pool closes");
        self.database.finish().await;
    }
}

fn ids(ids: &[Id]) -> Vec<String> {
    let mut ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn account_list_shows_the_subtree_to_a_reseller_the_own_account_to_a_user_and_everything_to_the_owner()
 {
    let bench = bench().await;
    let root = Id::ROOT_ACCOUNT;
    let everything = ids(&[root, bench.reseller, bench.customer, bench.other]);
    assert_eq!(bench.accounts(&owner()).await, everything);
    assert_eq!(bench.accounts(&admin()).await, everything);
    assert_eq!(
        bench
            .accounts(&principal(Level::Reseller, bench.reseller))
            .await,
        ids(&[bench.reseller, bench.customer])
    );
    assert_eq!(
        bench
            .accounts(&principal(Level::User, bench.customer))
            .await,
        ids(&[bench.customer])
    );
    let service = bench
        .call(
            &principal(Level::Service, bench.other),
            "keel.account.list",
            json!({}),
        )
        .await;
    assert_eq!(code(&service), ErrorCode::Forbidden);
    assert_eq!(service.error().unwrap().key, "keel.error.level");
    let guest = bench
        .call(
            &principal(Level::Guest, root),
            "keel.account.list",
            json!({}),
        )
        .await;
    assert_eq!(code(&guest), ErrorCode::Unauthenticated);
    assert_eq!(guest.status(), 401);

    let as_user = bench
        .call(
            &principal(Level::User, bench.customer),
            "keel.account.list",
            json!({}),
        )
        .await;
    let row = &data(&as_user)["data"][0];
    assert_eq!(row["kind"], json!("user"));
    assert_eq!(row["state"], json!("active"));
    assert!(row.get("parentId").is_none(), "{row}");
    assert!(row.get("path").is_none(), "{row}");
    assert!(row.get("origin").is_none(), "{row}");
    let as_reseller = bench
        .call(
            &principal(Level::Reseller, bench.reseller),
            "keel.account.list",
            json!({ "filters": [ { "key": "parentId", "value": bench.reseller } ] }),
        )
        .await;
    let row = &data(&as_reseller)["data"][0];
    assert_eq!(row["id"], json!(bench.customer.to_string()));
    assert_eq!(row["parentId"], json!(bench.reseller.to_string()));
    assert_eq!(
        row["path"],
        json!(format!("/{}/{}/", bench.reseller, bench.customer))
    );
    assert!(row.get("origin").is_none(), "{row}");
    let as_admin = bench
        .call(&admin(), "keel.account.get", json!({ "id": root }))
        .await;
    let row = data(&as_admin);
    assert_eq!(row["kind"], json!("system"));
    assert_eq!(row["path"], json!("/"));
    assert_eq!(row["origin"], Value::Null);
    bench.finish().await;
}

#[tokio::test]
async fn principal_list_is_scoped_searched_sorted_and_filtered_by_the_declared_columns() {
    let seeded = seeded().await;
    let bench = &seeded.bench;
    assert_eq!(
        bench.logins(&owner(), json!({})).await,
        ["admin", "cust", "other", "res"]
    );
    assert_eq!(
        bench
            .logins(&principal(Level::Reseller, bench.reseller), json!({}))
            .await,
        ["cust", "res"]
    );
    assert_eq!(
        bench
            .logins(&principal(Level::User, bench.customer), json!({}))
            .await,
        ["cust"]
    );
    assert_eq!(
        bench
            .logins(&principal(Level::User, bench.other), json!({}))
            .await,
        ["other"]
    );
    assert_eq!(
        bench.logins(&owner(), json!({ "search": "RE" })).await,
        ["res"]
    );
    assert_eq!(
        bench
            .logins(
                &owner(),
                json!({ "sort": { "key": "login", "dir": "desc" }, "pageSize": 2 })
            )
            .await,
        ["res", "other"]
    );
    assert_eq!(
        bench
            .logins(
                &owner(),
                json!({ "filters": [ { "key": "level", "value": ["user", "admin"] } ] })
            )
            .await,
        ["admin", "cust", "other"]
    );
    assert_eq!(
        bench
            .logins(
                &owner(),
                json!({ "filters": [ { "key": "accountId", "value": bench.customer } ] })
            )
            .await,
        ["cust"]
    );
    let unsortable = bench
        .call(
            &owner(),
            "keel.principal.list",
            json!({ "sort": { "key": "aclGen", "dir": "asc" } }),
        )
        .await;
    assert_eq!(field(&unsortable).0, "sort.key");
    let unfilterable = bench
        .call(
            &owner(),
            "keel.principal.list",
            json!({ "filters": [ { "key": "aclGen", "value": "0" } ] }),
        )
        .await;
    assert_eq!(field(&unfilterable).0, "filters[0].key");

    let as_user = bench
        .call(
            &principal(Level::User, bench.customer),
            "keel.principal.list",
            json!({}),
        )
        .await;
    let row = &data(&as_user)["data"][0];
    assert_eq!(row["login"], json!("cust"));
    assert_eq!(row["level"], json!("user"));
    assert_eq!(row["role"], json!("owner"));
    assert_eq!(row["lang"], json!("en"));
    assert!(row.get("accountId").is_none(), "{row}");
    assert!(row.get("origin").is_none(), "{row}");
    let as_reseller = bench
        .call(
            &principal(Level::Reseller, bench.reseller),
            "keel.principal.list",
            json!({ "search": "cust" }),
        )
        .await;
    let row = &data(&as_reseller)["data"][0];
    assert_eq!(row["accountId"], json!(bench.customer.to_string()));
    assert!(row.get("origin").is_none(), "{row}");
    let as_admin = bench
        .call(&admin(), "keel.principal.get", json!({ "id": seeded.res }))
        .await;
    assert_eq!(data(&as_admin)["origin"], Value::Null);
    seeded.bench.finish().await;
}

#[tokio::test]
async fn get_and_update_answer_not_found_outside_the_callers_scope() {
    let seeded = seeded().await;
    let bench = &seeded.bench;
    let user = principal(Level::User, bench.customer);
    let reseller = principal(Level::Reseller, bench.reseller);

    let own = bench
        .call(&user, "keel.account.get", json!({ "id": bench.customer }))
        .await;
    assert_eq!(data(&own)["id"], json!(bench.customer.to_string()));
    for foreign in [
        bench.reseller,
        bench.other,
        Id::ROOT_ACCOUNT,
        Id::generate(),
    ] {
        let reply = bench
            .call(&user, "keel.account.get", json!({ "id": foreign }))
            .await;
        assert_eq!(code(&reply), ErrorCode::NotFound, "{foreign}");
        assert_eq!(reply.status(), 404);
    }
    let below = bench
        .call(
            &reseller,
            "keel.account.get",
            json!({ "id": bench.customer }),
        )
        .await;
    assert_eq!(data(&below)["parentId"], json!(bench.reseller.to_string()));
    let outside = bench
        .call(&reseller, "keel.account.get", json!({ "id": bench.other }))
        .await;
    assert_eq!(code(&outside), ErrorCode::NotFound);
    let moved = bench
        .call(
            &reseller,
            "keel.account.update",
            json!({ "id": bench.other, "kind": "client" }),
        )
        .await;
    assert_eq!(code(&moved), ErrorCode::NotFound);
    let renamed = bench
        .call(
            &reseller,
            "keel.account.update",
            json!({ "id": bench.customer, "kind": "client" }),
        )
        .await;
    assert_eq!(id_of(&renamed), bench.customer);
    let kind = bench
        .call(
            &reseller,
            "keel.account.get",
            json!({ "id": bench.customer }),
        )
        .await;
    assert_eq!(data(&kind)["kind"], json!("client"));
    let level = bench
        .call(
            &user,
            "keel.account.update",
            json!({ "id": bench.customer, "kind": "client" }),
        )
        .await;
    assert_eq!(code(&level), ErrorCode::Forbidden);
    assert_eq!(level.error().unwrap().key, "keel.error.level");

    let mine = bench
        .call(&user, "keel.principal.get", json!({ "id": seeded.cust }))
        .await;
    assert_eq!(data(&mine)["login"], json!("cust"));
    let theirs = bench
        .call(&user, "keel.principal.get", json!({ "id": seeded.res }))
        .await;
    assert_eq!(code(&theirs), ErrorCode::NotFound);
    let stolen = bench
        .call(
            &user,
            "keel.principal.update",
            json!({ "id": seeded.other, "role": "member" }),
        )
        .await;
    assert_eq!(code(&stolen), ErrorCode::NotFound);
    let subtree = bench
        .call(
            &reseller,
            "keel.principal.get",
            json!({ "id": seeded.cust }),
        )
        .await;
    assert_eq!(data(&subtree)["login"], json!("cust"));
    let above = bench
        .call(
            &reseller,
            "keel.principal.get",
            json!({ "id": seeded.admin }),
        )
        .await;
    assert_eq!(code(&above), ErrorCode::NotFound);
    seeded.bench.finish().await;
}

#[tokio::test]
async fn the_account_parameter_is_resolved_inside_the_callers_scope() {
    let bench = bench().await;
    let reseller = principal(Level::Reseller, bench.reseller);
    let user = principal(Level::User, bench.customer);

    let inside = bench
        .call(
            &reseller,
            "keel.principal.create",
            json!({ "login": "inside", "level": "user", "account": bench.customer }),
        )
        .await;
    let inside = bench
        .call(
            &owner(),
            "keel.principal.get",
            json!({ "id": id_of(&inside) }),
        )
        .await;
    assert_eq!(
        data(&inside)["accountId"],
        json!(bench.customer.to_string())
    );
    let outside = bench
        .call(
            &reseller,
            "keel.principal.create",
            json!({ "login": "outside", "level": "user", "account": bench.other }),
        )
        .await;
    assert_eq!(outside.status(), 422);
    assert_eq!(
        field(&outside),
        (
            "account".to_owned(),
            "keel.error.account_not_found".to_owned()
        )
    );
    let missing = bench
        .call(
            &reseller,
            "keel.principal.create",
            json!({ "login": "missing", "level": "user", "account": Id::generate() }),
        )
        .await;
    assert_eq!(field(&missing).0, "account");
    let root = bench
        .call(
            &reseller,
            "keel.principal.create",
            json!({ "login": "root", "level": "user", "account": Id::ROOT_ACCOUNT }),
        )
        .await;
    assert_eq!(field(&root).0, "account");
    let default = bench
        .call(
            &reseller,
            "keel.principal.create",
            json!({ "login": "default", "level": "user" }),
        )
        .await;
    let default = bench
        .call(
            &owner(),
            "keel.principal.get",
            json!({ "id": id_of(&default) }),
        )
        .await;
    assert_eq!(
        data(&default)["accountId"],
        json!(bench.reseller.to_string())
    );

    let own = bench
        .call(
            &user,
            "keel.principal.create",
            json!({ "login": "colleague", "level": "user" }),
        )
        .await;
    let own = bench
        .call(&owner(), "keel.principal.get", json!({ "id": id_of(&own) }))
        .await;
    assert_eq!(data(&own)["accountId"], json!(bench.customer.to_string()));
    for named in [bench.other, bench.customer] {
        let reply = bench
            .call(
                &user,
                "keel.principal.create",
                json!({ "login": "named", "level": "user", "account": named }),
            )
            .await;
        assert_eq!(code(&reply), ErrorCode::ForbiddenField, "{named}");
        assert_eq!(reply.status(), 403);
        assert_eq!(reply.error().unwrap().fields[0].path, "account");
    }
    let admin_anywhere = bench
        .call(
            &admin(),
            "keel.principal.create",
            json!({ "login": "anywhere", "level": "user", "account": bench.other }),
        )
        .await;
    assert!(
        admin_anywhere.data().is_some(),
        "{:?}",
        admin_anywhere.error()
    );

    let nested = bench
        .call(
            &reseller,
            "keel.account.create",
            json!({ "kind": "user", "parent": bench.customer }),
        )
        .await;
    let nested = id_of(&nested);
    assert_eq!(
        bench.path(nested).await,
        format!("/{}/{}/{nested}/", bench.reseller, bench.customer)
    );
    let foreign_parent = bench
        .call(
            &reseller,
            "keel.account.create",
            json!({ "kind": "user", "parent": bench.other }),
        )
        .await;
    assert_eq!(
        field(&foreign_parent),
        (
            "parent".to_owned(),
            "keel.error.account_not_found".to_owned()
        )
    );
    let under_reseller = bench.account(&reseller, "user", None).await;
    assert_eq!(
        bench.path(under_reseller).await,
        format!("/{}/{under_reseller}/", bench.reseller)
    );
    let under_root = bench.account(&admin(), "user", None).await;
    assert_eq!(bench.path(under_root).await, format!("/{under_root}/"));
    assert_eq!(
        bench.accounts(&reseller).await,
        ids(&[bench.reseller, bench.customer, nested, under_reseller])
    );
    bench.finish().await;
}

#[tokio::test]
async fn changing_a_principals_level_role_or_account_bumps_its_acl_gen() {
    let bench = bench().await;
    let id = bench
        .principal(&owner(), "p", Level::User, Some(bench.customer))
        .await;
    assert_eq!(bench.acl_gen(id).await, 0);

    let update = |params: Value| async {
        let mut params = params;
        params["id"] = json!(id);
        let reply = bench.call(&owner(), "keel.principal.update", params).await;
        assert!(reply.data().is_some(), "{:?}", reply.error());
    };
    update(json!({ "lang": "ru" })).await;
    assert_eq!(bench.acl_gen(id).await, 0);
    update(json!({ "login": "p2" })).await;
    assert_eq!(bench.acl_gen(id).await, 0);
    update(json!({ "role": "member" })).await;
    assert_eq!(bench.acl_gen(id).await, 1);
    update(json!({ "role": "member" })).await;
    assert_eq!(bench.acl_gen(id).await, 1);
    update(json!({ "account": bench.reseller })).await;
    assert_eq!(bench.acl_gen(id).await, 2);
    update(json!({ "level": "service" })).await;
    assert_eq!(bench.acl_gen(id).await, 3);
    update(json!({ "level": "service", "role": "member", "account": bench.reseller })).await;
    assert_eq!(bench.acl_gen(id).await, 3);

    let reply = bench
        .call(&owner(), "keel.principal.get", json!({ "id": id }))
        .await;
    let row = data(&reply);
    assert_eq!(row["login"], json!("p2"));
    assert_eq!(row["lang"], json!("ru"));
    assert_eq!(row["role"], json!("member"));
    assert_eq!(row["level"], json!("service"));
    assert_eq!(row["accountId"], json!(bench.reseller.to_string()));
    assert_ne!(row["updatedAt"], row["createdAt"]);
    bench.finish().await;
}

#[tokio::test]
async fn changing_a_reseller_account_bumps_acl_gen_of_every_principal_in_its_subtree() {
    let seeded = seeded().await;
    let bench = &seeded.bench;
    let gens = || async {
        (
            bench.acl_gen(seeded.res).await,
            bench.acl_gen(seeded.cust).await,
            bench.acl_gen(seeded.other).await,
            bench.acl_gen(seeded.admin).await,
        )
    };
    assert_eq!(gens().await, (0, 0, 0, 0));

    let moved = bench
        .call(
            &owner(),
            "keel.account.update",
            json!({ "id": bench.reseller, "parent": bench.other }),
        )
        .await;
    assert!(moved.data().is_some(), "{:?}", moved.error());
    assert_eq!(gens().await, (1, 1, 0, 0));
    assert_eq!(
        bench.path(bench.reseller).await,
        format!("/{}/{}/", bench.other, bench.reseller)
    );
    assert_eq!(
        bench.path(bench.customer).await,
        format!("/{}/{}/{}/", bench.other, bench.reseller, bench.customer)
    );
    let parent = bench
        .call(
            &owner(),
            "keel.account.get",
            json!({ "id": bench.reseller }),
        )
        .await;
    assert_eq!(data(&parent)["parentId"], json!(bench.other.to_string()));
    let moved_rows = bench
        .database
        .count(
            &bench.storage,
            &format!(
                "SELECT count(*) FROM keel_account WHERE path LIKE '/{}/{}/%'",
                bench.other, bench.reseller
            ),
        )
        .await;
    assert_eq!(moved_rows, 2);
    assert_eq!(
        bench
            .accounts(&principal(Level::Reseller, bench.reseller))
            .await,
        ids(&[bench.reseller, bench.customer])
    );
    assert_eq!(
        bench.accounts(&principal(Level::User, bench.other)).await,
        ids(&[bench.other])
    );

    let rekinded = bench
        .call(
            &owner(),
            "keel.account.update",
            json!({ "id": bench.reseller, "kind": "agency" }),
        )
        .await;
    assert!(rekinded.data().is_some(), "{:?}", rekinded.error());
    assert_eq!(gens().await, (2, 2, 0, 0));
    let unchanged = bench
        .call(
            &owner(),
            "keel.account.update",
            json!({ "id": bench.reseller, "kind": "agency", "parent": bench.other }),
        )
        .await;
    assert!(unchanged.data().is_some(), "{:?}", unchanged.error());
    assert_eq!(gens().await, (2, 2, 0, 0));
    let leaf = bench
        .call(
            &owner(),
            "keel.account.update",
            json!({ "id": bench.customer, "kind": "client" }),
        )
        .await;
    assert!(leaf.data().is_some(), "{:?}", leaf.error());
    assert_eq!(gens().await, (2, 3, 0, 0));

    for cycle in [bench.reseller, bench.customer] {
        let reply = bench
            .call(
                &owner(),
                "keel.account.update",
                json!({ "id": bench.reseller, "parent": cycle }),
            )
            .await;
        assert_eq!(
            field(&reply),
            (
                "parent".to_owned(),
                "keel.account.error.parent_cycle".to_owned()
            ),
            "{cycle}"
        );
    }
    let root = bench
        .call(
            &owner(),
            "keel.account.update",
            json!({ "id": Id::ROOT_ACCOUNT, "kind": "agency" }),
        )
        .await;
    assert_eq!(code(&root), ErrorCode::Conflict);
    assert_eq!(
        root.error().unwrap().key,
        "keel.account.error.root_immutable"
    );
    let reserved = bench
        .call(
            &owner(),
            "keel.account.update",
            json!({ "id": bench.reseller, "kind": "system" }),
        )
        .await;
    assert_eq!(
        field(&reserved),
        (
            "kind".to_owned(),
            "keel.account.error.kind_reserved".to_owned()
        )
    );
    let reserved = bench
        .call(&owner(), "keel.account.create", json!({ "kind": "system" }))
        .await;
    assert_eq!(field(&reserved).0, "kind");
    let malformed = bench
        .call(
            &owner(),
            "keel.account.create",
            json!({ "kind": "9 lives" }),
        )
        .await;
    assert_eq!(
        field(&malformed),
        ("kind".to_owned(), "keel.check.name".to_owned())
    );
    assert_eq!(gens().await, (2, 3, 0, 0));
    seeded.bench.finish().await;
}

#[tokio::test]
async fn principal_writes_enforce_login_uniqueness_and_the_level_rules() {
    let seeded = seeded().await;
    let bench = &seeded.bench;
    let reseller = principal(Level::Reseller, bench.reseller);

    let duplicate = bench
        .call(
            &owner(),
            "keel.principal.create",
            json!({ "login": "cust", "level": "user", "account": bench.other }),
        )
        .await;
    assert_eq!(duplicate.status(), 422);
    assert_eq!(
        field(&duplicate),
        (
            "login".to_owned(),
            "keel.principal.error.login_exists".to_owned()
        )
    );
    let renamed = bench
        .call(
            &owner(),
            "keel.principal.update",
            json!({ "id": seeded.other, "login": "cust" }),
        )
        .await;
    assert_eq!(field(&renamed).0, "login");
    let blank = bench
        .call(
            &owner(),
            "keel.principal.create",
            json!({ "login": "", "level": "user" }),
        )
        .await;
    assert_eq!(
        field(&blank),
        ("login".to_owned(), "keel.schema.minLength".to_owned())
    );

    let cases = [
        (
            &reseller,
            json!({ "login": "a1", "level": "admin" }),
            "keel.principal.error.level_above_caller",
        ),
        (
            &reseller,
            json!({ "login": "a2", "level": "reseller", "account": bench.customer }),
            "keel.principal.error.level_account",
        ),
        (
            &admin(),
            json!({ "login": "a3", "level": "owner" }),
            "keel.principal.error.level_above_caller",
        ),
        (
            &owner(),
            json!({ "login": "a4", "level": "admin", "account": bench.customer }),
            "keel.principal.error.level_account",
        ),
        (
            &owner(),
            json!({ "login": "a5", "level": "guest" }),
            "keel.principal.error.level_guest",
        ),
    ];
    for (caller, params, key) in cases {
        let reply = bench.call(caller, "keel.principal.create", params).await;
        assert_eq!(field(&reply), ("level".to_owned(), key.to_owned()));
    }
    let peer = bench
        .principal(&reseller, "peer", Level::Reseller, None)
        .await;
    let socket_owner = bench
        .principal(&owner(), "socket", Level::Owner, None)
        .await;
    assert_eq!(
        bench
            .logins(
                &owner(),
                json!({ "filters": [ { "key": "level", "value": "owner" } ] })
            )
            .await,
        ["socket"]
    );

    let promoted = bench
        .call(
            &reseller,
            "keel.principal.update",
            json!({ "id": seeded.cust, "level": "reseller" }),
        )
        .await;
    assert_eq!(
        field(&promoted),
        (
            "level".to_owned(),
            "keel.principal.error.level_account".to_owned()
        )
    );
    let escalated = bench
        .call(
            &reseller,
            "keel.principal.update",
            json!({ "id": seeded.cust, "level": "admin" }),
        )
        .await;
    assert_eq!(
        field(&escalated).1,
        "keel.principal.error.level_above_caller"
    );
    let moved_up = bench
        .call(
            &reseller,
            "keel.principal.update",
            json!({ "id": seeded.cust, "level": "reseller", "account": bench.reseller }),
        )
        .await;
    assert!(moved_up.data().is_some(), "{:?}", moved_up.error());
    assert_eq!(bench.acl_gen(seeded.cust).await, 1);

    let member = bench
        .principal(&owner(), "member", Level::User, Some(bench.reseller))
        .await;
    let as_member = principal(Level::User, bench.reseller);
    let above = bench
        .call(
            &as_member,
            "keel.principal.update",
            json!({ "id": peer, "role": "member" }),
        )
        .await;
    assert_eq!(code(&above), ErrorCode::Forbidden);
    assert_eq!(
        above.error().unwrap().key,
        "keel.principal.error.level_above_caller"
    );
    let sideways = bench
        .call(
            &as_member,
            "keel.principal.update",
            json!({ "id": member, "role": "member", "lang": "ru" }),
        )
        .await;
    assert!(sideways.data().is_some(), "{:?}", sideways.error());
    let outside = bench
        .call(
            &as_member,
            "keel.principal.update",
            json!({ "id": socket_owner, "lang": "ru" }),
        )
        .await;
    assert_eq!(code(&outside), ErrorCode::NotFound);
    seeded.bench.finish().await;
}

#[tokio::test]
async fn account_options_list_only_visible_accounts_and_page_by_cursor() {
    let bench = bench().await;
    let reseller = principal(Level::Reseller, bench.reseller);
    let choices = |reply: &Reply| -> (Vec<(String, String, String)>, bool) {
        let page = data(reply);
        let items = page["data"]
            .as_array()
            .expect("choices")
            .iter()
            .map(|choice| {
                (
                    choice["value"].as_str().expect("value").to_owned(),
                    choice["label"].as_str().expect("label").to_owned(),
                    choice["hint"].as_str().expect("hint").to_owned(),
                )
            })
            .collect();
        (items, page["more"].as_bool().expect("more"))
    };
    let (subtree, more) = choices(
        &bench
            .call(&reseller, "keel.account.options", json!({}))
            .await,
    );
    let reseller_id = bench.reseller.to_string();
    let customer_id = bench.customer.to_string();
    assert_eq!(
        subtree,
        [
            (reseller_id.clone(), reseller_id.clone(), "reseller".into()),
            (customer_id.clone(), customer_id.clone(), "user".into()),
        ]
    );
    assert!(!more);

    let (first, more) = choices(
        &bench
            .call(&reseller, "keel.account.options", json!({ "limit": 1 }))
            .await,
    );
    assert_eq!(first[0].0, reseller_id);
    assert!(more);
    let (second, more) = choices(
        &bench
            .call(
                &reseller,
                "keel.account.options",
                json!({ "limit": 1, "cursor": first[0].0 }),
            )
            .await,
    );
    assert_eq!(second[0].0, customer_id);
    assert!(!more);
    let (searched, _) = choices(
        &bench
            .call(
                &reseller,
                "keel.account.options",
                json!({ "search": "resell" }),
            )
            .await,
    );
    assert_eq!(searched.len(), 1);
    assert_eq!(searched[0].0, reseller_id);
    let (escaped, _) = choices(
        &bench
            .call(
                &reseller,
                "keel.account.options",
                json!({ "search": "%\\_" }),
            )
            .await,
    );
    assert!(escaped.is_empty());

    let (own, _) = choices(
        &bench
            .call(
                &principal(Level::User, bench.customer),
                "keel.account.options",
                json!({}),
            )
            .await,
    );
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].0, customer_id);
    let (everything, _) = choices(
        &bench
            .call(&owner(), "keel.account.options", json!({}))
            .await,
    );
    assert_eq!(everything.len(), 4);
    assert_eq!(everything[0].0, Id::ROOT_ACCOUNT.to_string());
    assert_eq!(everything[0].2, "system");
    let guest = bench
        .call(
            &principal(Level::Guest, Id::ROOT_ACCOUNT),
            "keel.account.options",
            json!({}),
        )
        .await;
    assert_eq!(code(&guest), ErrorCode::Unauthenticated);
    bench.finish().await;
}
