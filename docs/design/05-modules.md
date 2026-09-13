# Modules, the SDK and system modules

This document specifies the unit of composition in Keel: what a module is, how the composition
assembles modules and product bundles into one binary, what happens between process start and the
first served request, what a module registers and contributes, the isolation rules the toolchain
enforces, the three planes on which compatibility is versioned, the tooling around modules, and the
catalogue of system modules shipped in `keel-system`. The dispatcher that runs registered functions
is specified in [02-functions.md](02-functions.md), accounts and scoping in
[03-access.md](03-access.md), the storage rules a module's tables follow in
[04-storage.md](04-storage.md), and the layers the crates fit into in
[01-overview.md](01-overview.md).

## Module definition

A module is a Rust crate that exports one static value,
`pub static MODULE: ModuleDef { id, version, sdk, requires, migrations, init }`. `ModuleDef` is a
plain struct with static slices; there is no trait to implement and no macro to invoke.

| Field        | Meaning |
|--------------|---------|
| `id`         | Module identifier: a lowercase ASCII identifier, `[a-z][a-z0-9_]*`, because it prefixes every table, function, permission key, setting and i18n key the module owns; the loader refuses any other id. |
| `version`    | Crate version, `env!("CARGO_PKG_VERSION")`. |
| `sdk`        | The `keel_sdk::VERSION` the crate was compiled against; compared with the kernel's at boot. |
| `requires`   | Ids of the modules this one depends on. The list fixes migration order, which `api` modules may be imported and which foreign keys are allowed. |
| `migrations` | Optional embedded migration directory (`migrator::embed!`), applied under the module's own ledger `keel_migration_<module>`. The static names the ledger itself, `.table("keel_migration_<module>")`; the loader refuses a module whose ledger is named otherwise. |
| `init`       | `fn(&mut Registrar) -> keel::Result<()>`; registration only, no I/O. |

### Composition and product bundles

The composition is an explicit builder, `keel::Runtime`, in `bin/keel/src/main.rs` (see the
example at the end of this document): it lists every platform module no bundle names and every
product bundle, and a bundle brings the modules it names. Self-registration through an
`inventory`-style crate is not used: in a static musl build with LTO a registrar crate that
nothing references silently drops out of the binary. The kernel's own module `keel` is always
composed first, by the loader itself; a bundle or a `.module()` call that names it is a
composition error. It has no migrations of its own beyond the kernel's and is recorded in
`keel_module` under the kernel's version.

A product bundle is a `ProductManifest`, a plain struct with static slices exported by the
product's lib crate as `pub static MANIFEST`:

| Field           | Meaning |
|-----------------|---------|
| `id`            | Product id, `[a-z][a-z0-9_]*`: groups modules, prefixes the product's files and OS users, names the product in the lease, in `/boot` and in the handshake. |
| `name`          | The i18n key of the product's name, `product.<id>.brand`, shown in `/boot` and by `keel product list`. |
| `version`       | `env!("CARGO_PKG_VERSION")` of the bundle crate: the workspace version, shown by `keel product list` and `keel.product.list`. |
| `api`           | The descriptor version the bundle is written against; must equal `keel_types::DESCRIPTOR_VERSION`, else the start fails naming the product. |
| `brand`         | `Brand { logo, hue, chroma }` for the shell: the logo is the server-side asset identifier served as `/brand/<id>/logo.svg`; the brand of a single enabled product is the brand of the installation. |
| `modules`       | `&[&ModuleDef]`: the definitions of the modules the product enables, `host` included when its modules need the agent; the composition learns a bundle's modules from here and lists only the platform modules itself. |
| `bridges`       | `Bridge { to, local, remote }`: the pair of modules through which this product reaches product `to`; `local` is loaded when `to` is enabled on the same installation, `remote` otherwise. The field arrives with the first bridge, the `panel` bundle's bridge to `dns` at M5; the M0 manifest has none. |
| `menu`          | Menu groups (`id`, `label`, `icon`, `order`) under which the modules' `MenuNode`s are placed; group ids are unique across enabled bundles. |
| `levels`        | `LevelLabels { labels, disabled }`: level labels and disabled levels ([03-access.md](03-access.md)). |
| `editions`      | Edition ids with their feature sets; the first is the default edition used while no lease is present, and the lease names the edition of each product. |
| `account_kinds` | The account kinds the product creates ([03-access.md](03-access.md)): identifiers, never `system`. |
| `i18n`          | The bundle's own dictionaries, `(lang, json)` pairs: brand name, menu group labels, level labels. |

Which bundles are enabled is recorded in `keel_kv` under `products`
([04-storage.md](04-storage.md)). `keel product enable <id>` and `keel product disable <id>`
change it — through `keel.product.enable` and `keel.product.disable`, `admin`-level `.unowned()`
functions of the `keel` module that check the lease and end with a restart request, or directly
against the database while the service is stopped — and the change takes effect at the next
start. At M0 the subcommand knows only the direct path and refuses to run while the service
answers on the socket; the functions exist, take `{ id }`, refuse an id the binary does not
carry as a `validation` error on `id`, answer `{ id, state, changed, restart }` with `restart`
true whenever the stored set changed, and neither check a lease nor restart anything until M4
and M5 bring both. `keel.product.list` answers a page of
`{ id, name, version, state, enabledAt, enabledBy }` over every bundle the binary carries,
disabled ones included, with row actions `enable` and `disable`; the compiled-in bundles are a
kernel-private service, not part of the SDK.

The runtime tells the loader two things: every bundle the binary carries, through
`Composition::product`, and the enabled set read from `keel_kv`, through
`Composition::enabled`; a composition without `.enabled()`, as the test harness and CI's full
composition build it, enables every bundle it carries, and an enabled id no bundle carries is a
composition error naming it. The loader computes the loaded set from it: the platform modules,
which no bundle names, plus the modules of every enabled bundle, plus one side of every bridge of
an enabled bundle. A module named by two enabled bundles is loaded once and attributed to the
bundle that comes first in the composition. Modules outside the loaded set are neither migrated
nor initialised; their tables, if any, are orphaned ([04-storage.md](04-storage.md)). `disable`
refuses a product whose module another enabled bundle's module requires, and refuses a product
another enabled bundle bridges to locally unless `--switch-remote` is given
([07-ecosystem.md](07-ecosystem.md)).

The order of `.module()` calls means nothing; the order of `.product()` calls is the order of
the products in `/boot` and in the menu. The kernel builds a graph from `requires` over the
loaded set, sorts it topologically and fails at start with a list of cycles, duplicate ids and
missing dependencies, naming the bundle whose module requires a module no enabled bundle names.
Bundles and module crates depend only on the facade crate `keel` (and on `@globalart/keel` for
the frontend), which is what lets a product's crates move to their own repository later without
changes ([01-overview.md](01-overview.md)).

## Lifecycle

Between process start and the first served request the kernel runs these steps, in this order:

1. Enabled set: after the kernel's own migrations, the runtime reads `keel_kv.products` and hands
   the ids to the loader with `Composition::enabled`; the loader checks every enabled product
   against the lease (a product the lease does not list is a boot error naming it, unless
   `license: None`), refuses an enabled id no bundle carries, chooses the side of every bridge
   and fixes the loaded module set; every later step runs over that set only, and modules of
   disabled bundles are never initialised.
2. Validation: `sdk == keel_sdk::VERSION` for every module, the format and uniqueness of `id`,
   resolvability of `requires` within the loaded set; the topological order is fixed here.
3. Migrations: the kernel's own first, then every loaded module in graph order, before any port is
   opened and under a session lock ([04-storage.md](04-storage.md)).
4. `init(&mut Registrar)` for every module in graph order: registration only, no I/O.
5. Freeze of the registries. The kernel checks that every required service has a provider
   and no service or invoker has two, applies patches to other modules' descriptors (a patch
   cannot weaken `access`; a field added by a patch must carry `store`), compiles condition
   expressions and verifies that every field name they mention exists, validates descriptors
   (every function, action and permission a descriptor or a menu node names is registered,
   `ui.read` names a `get` and `x-keel.options` an `options` function, `x-keel` and `x-ui`
   attributes are members of their enums) and checks that the i18n keys a descriptor or a menu
   node carries exist at least for `en`, that a menu node without a label of its own points at a
   function with a screen title, that every menu node's group is declared by an enabled bundle
   or is the platform group `system`, that no group id is declared by two enabled bundles, that
   every bundle dictionary is a JSON object, that no message uses a placeholder other than
   `{name}` ([08-frontend.md](08-frontend.md)), and that no `ConfigTemplate` id is contributed
   twice across the loaded set. Menu groups are ordered by product in composition order, then by
   `order` within a bundle, with the platform group last. Every problem is collected and reported
   in one error.
6. `start(ctx)`: the start hooks registered in `init` run in graph order; background work is
   spawned through `ctx.spawn`, then transports open. `stop` runs in reverse order: a module's
   stop hook runs first, then the tasks it spawned are cancelled and awaited. A start hook that
   fails stops the modules already started and fails the boot.

There is deliberately no dynamic loading after boot: registries are immutable and every check
runs at startup, so a typo in a condition or a missing translation cannot reach a deployed
instance. Installing a plugin ends with `request_restart`; so does enabling or disabling a
product. CI additionally validates the full composition with every bundle enabled.

## What a module registers

Everything a module adds to the installation goes through `Registrar` inside `init`:

- Functions, built with `keel_sdk::function::<Params, Result>("<module>.<entity>.<verb>")`
  (in the prelude as `function`) and
  registered with `r.function(...)`. Descriptors, the pipeline and the guards that run them are
  in [02-functions.md](02-functions.md).
- SSE streams, `r.stream("dns.zone.log", handler)`.
- Raw routes under `/api/v1/x/<module>/…`, only with a mandatory `Guard` and an audit event.
  They exist for upload, download, the WebSocket terminal and VNC, not as a second way to expose
  functions.
- Custom views, `r.view("hosting.file-manager", ViewSpec { access, menu, needs })`: a screen
  without a descriptor but with access rules for the menu and features
  ([08-frontend.md](08-frontend.md)).
- Migrations, through `ModuleDef.migrations`.
- Menu nodes, `r.menu(MenuNode::new("dns/zones", "dns.zone.list").icon("globe").order(10))`; the
  first path segment names a menu group declared by a bundle or the platform group `system`.
- i18n dictionaries per language, `r.i18n("en", ...)`; key names follow the convention in
  [08-frontend.md](08-frontend.md).
- Permission keys with their default levels,
  `r.permission("dns.zone.manage", &[Level::User, Level::Reseller])`; a level holds a key only
  when listed, `admin` and `owner` aside ([03-access.md](03-access.md)).
- Guards on every call, `r.guard(Arc::new(IpRules::new()))`: a `Guard` sees the function name
  and the call and refuses with an error code; the `auth` module's IP rules and rate limit are
  such guards ([02-functions.md](02-functions.md)).
- Hooks on functions of other modules. `before` runs outside the transaction with a timeout, may
  only read, enrich `params` or veto, and is the kind available to plugins. `within` runs inside the
  transaction without a timeout and is available only to modules compiled into the binary;
  invariant checks such as quotas and limits belong there and nowhere else. A nested
  `call.invoke` reuses the transaction of the call and does not run plugin hooks; plugins see
  events after commit.
- Patches to other modules' descriptors,
  `r.patch(DescriptorPatch::new("hosting.site.edit").field("dnsZone", schema).after("name").store(hooks::store_zone))`,
  within the limits checked at freeze; the `store` mechanics of an added field are in
  [10-contracts.md](10-contracts.md).
- Start and stop hooks, `r.start(|ctx| async move { … })` and `r.stop(|| async { … })`, one of
  each per module. `ctx` is the module's `Context`: `ctx.spawn` for background work,
  `ctx.contributions::<T>()` for the typed bag, `ctx.require::<dyn Trait>()` for services. The
  context carries no database handle in M0; a module that needs one at start gets it with the
  milestone that needs it.
- Services, `r.provide::<dyn Trait>(Arc::new(...))`, consumed elsewhere with
  `call.require::<dyn Trait>()`. Which module provides an implementation is decided by the
  enabled set: `hosting-web` declares `DnsProvider`, and the panel bundle's bridge loads
  `dns-local` or `dns-peer` ([07-ecosystem.md](07-ecosystem.md)).
- Contributions, `r.contribute(T)`, into extension points that modules themselves declare in
  their `pub mod api` and read back with `ctx.contributions::<T>()`. The kernel knows nothing
  about any of them; it only provides the typed bag.
- `r.invoker(Arc<dyn Invoker>)` is reserved in the kernel from M0 for the cluster milestone: an
  `Invoker` selects the node a function runs on by a function attribute
  ([06-host.md](06-host.md)). The kernel has one invoker slot; a composition with two providers
  fails at freeze, and without one the dispatcher runs every function locally.

### Contribution points

| Type | What is contributed | Consumed by |
|------|---------------------|-------------|
| `SettingDef` | A product setting: key, JSON Schema, default, level; rendered as an automatic form | `settings` |
| `CronJob` | A periodic job: id, schedule, handler | `cron` |
| `DashboardWidget` | A widget for the `dashboard` block of `GET /api/v1/boot` | the shell ([08-frontend.md](08-frontend.md)) |
| `ProblemSource` | A source of problems for the problem registry | `notify` |
| `MailTemplate` | A minijinja template for outgoing mail | `notify` |
| `ConfigTemplate` | A configuration file template with its validate command, overridable in `/etc/keel/templates/`; ids are unique across the loaded set | `keel-host` ([06-host.md](06-host.md)) |
| `AgentOp` | A typed host-agent operation: argument and result schemas, handler, policy class | `keel-host` ([06-host.md](06-host.md)) |
| `BackupSource` | The module's own data in a backup archive | `backup` |
| `ConfigSection` | The module's own section of `config.yaml` | configuration loading ([09-operations.md](09-operations.md)) |
| `PermissionSource` | Fine-grained permission keys and their defaults | `auth` |
| `ManifestPatch` | A patch of the effective brand and of the bundle fields served in `/boot`, for example white-label branding by hostname | `settings` |
| `ServiceKind` | A service type for billing: `Params`, `State` and `Account` schemas with `schema_version` | `svc` ([07-ecosystem.md](07-ecosystem.md)) |
| `LimitDef` | An account limit: key, unit, default, level, whether `unlimited` is allowed | `keel_limit` and `call.check_limit` ([03-access.md](03-access.md)) |
| `AccountLifecycle` | `on_suspend`, `on_resume`, `on_delete`, `on_rename` handlers | `keel.account.*` ([03-access.md](03-access.md)) |
| `StatCollector` | A metric: key, unit, collection interval | `stats` |
| `InstanceInfoSource` | Extra fields of `GET /api/v1/instance`, for example `serviceKinds` | the instance endpoint ([07-ecosystem.md](07-ecosystem.md)) |

`LimitDef` and `AccountLifecycle` are part of what the SDK gives every module and are described
with the account model; the other points are declared by modules in their `pub mod api`. The last
column names where each contribution is consumed.

## Isolation rules

- A module depends on `keel-sdk` and, optionally, on the `api` modules of the crates named in
  its `requires`. Importing anything else is an error reported by `keel-dev check`. The host
  executor is not part of the SDK: `HostExecutor` lives in `keel_host::api`, so a module that
  manages the host names `host` in `requires` ([06-host.md](06-host.md)).
- The crates the SDK surface is expressed in — `serde`, `serde_json`, `schemars`, `sea-orm`,
  `async-trait` and the migrator — are re-exported by `keel-sdk` under their own names, so one
  version of each is shared by the kernel and every module. Their derives are pointed at the
  re-exports: `#[serde(crate = "keel_sdk::serde")]` and
  `#[schemars(crate = "keel_sdk::schemars")]` on the struct, `use keel_sdk::sea_orm;` in the
  module that defines an entity ([02-functions.md](02-functions.md)). The crate
  `keel-sdk-probe` is such a module, kept in the workspace with `keel-sdk` as its only
  dependency so that CI proves the rule on every build.
- Every table, function, permission key, setting and i18n key of a module carries the module id
  as its prefix.
- Foreign keys between modules exist only inside a declared `requires` relation and towards
  `keel_account` and `keel_principal`. Another module's data is read through `call.invoke` or a
  service, never through SQL ([04-storage.md](04-storage.md)).
- A module does not open transactions: the kernel opens the write transaction in `Call`, and a
  nested `call.invoke` reuses it ([02-functions.md](02-functions.md)).
- Operations on the OS and on peers run only in `after_commit` or as a task; a transaction never
  waits for the agent.

## The SDK

`keel-sdk` is layer L2 together with `keel-testkit`; a hidden `keel_sdk::system` sits beside it.
The SDK is an interface, not an implementation: it knows nothing of axum, the Postgres pool or
cookies; even `ctx.spawn` hands the task to a spawner the kernel implements, so the SDK carries no
runtime. What module code sees is `ModuleDef`, `Registrar`, `Context` and `Call<P>` with `params()`,
`principal()`, `tx()`, `invoke`, `emit`, `after_commit`, `require::<dyn Trait>()`, `check_limit`
and `extra` (values of patched fields); `Principal`, `Account` and `Level`; `KeelError` with an
i18n key, an optional field and a source; `ListQuery`, `Page` and `ListSource`; `MenuNode`;
`Secret`; `Id`; the `Owned` trait with `scope_by_account`, `resolve_account`, `LimitDef` and
`AccountLifecycle`; `ProductManifest` with `Brand`, `MenuGroup`, `LevelLabels` and `Edition`;
and the `Clock` service, `call.require::<dyn Clock>()`, from which the dispatcher, the `keel`
module and every module stamp times, so that a test can freeze it. The crates the surface is
expressed in, `chrono` among them, are re-exported under their own names. The function builder
is specified in [02-functions.md](02-functions.md), the account helpers in
[03-access.md](03-access.md), `Secret` and `Id` in [04-storage.md](04-storage.md).

## keel-system

System modules live in one crate, `keel-system`, with a cargo feature per module. The crate
depends only on `keel-sdk`, which cargo verifies; the kernel imports neither `keel-system` nor
`keel-host`, which CI verifies on the cargo dependency graph. Layer L3 explicitly excludes
operations on the OS: `update` talks to the root unit, not to `keel-host`.

CI runs a matrix over the crate: every module built alone, and the crate built without each
module in turn, plus the test that a composition of the bare kernel starts and answers on the
control socket. This is what keeps every system module replaceable by product composition, for
example SSO in place of `auth`, or a different `notify` for billing.

### Hidden hooks

Hooks needed only by system modules, `Authenticator`, `TransportRegistry`, `StorageSnapshot` and
`OutboxSubscriber`, live in `keel_sdk::system`, marked `#[doc(hidden)]` and exempt from semver
until 1.0. They are the seams behind which authentication, the transports, backup and the outbox
consumers plug in; a product module has no reason to touch them. The public `keel-sdk` is frozen
at 1.0 (M7); until then only `keel_sdk::system` is exempt from semver.

The authentication seam is a chain, not a single service: a module registers its authenticator
through the contribution bag, `r.contribute(Arc<dyn Authenticator>)`, so that several modules
answer for their own token prefixes, and the transport asks them in module order with the
`Credential` and the `Client` of the request. `Ok(None)` means "not my credential" and the next
one is asked; `Ok(Some(Authenticated { principal, session }))` ends the chain, `session` being the
`id_hash` of the session row for a cookie and `None` for a token; `Err` ends it with that error;
a credential none recognises answers `401 unauthenticated`. `Client { ip, user_agent, session }`
is what the transport learned about the request, read from the call through
`CallExt::client()`, `None` on the socket. A handler records `SessionChange { Set(Secret) |
Clear }` through `CallExt::session()`, and the dispatcher carries it to the transport only for a
write that committed or a read that completed, so a call that fails after recording it answers
without `Set-Cookie`. `PrincipalSource { by_id, by_login }` is the kernel's lookup over
`keel_principal`, answering the stored principal with `via: internal`, no actor and the full
scope, so that `auth` resolves logins and session rows to principals without SQL on a kernel
table.

## Versioning

Compatibility is tracked on three planes:

| Plane | Carrier | Rule |
|-------|---------|------|
| Compile-time | `keel-sdk` semver | The kernel and the SDK release under one version; `ModuleDef.sdk` is checked at boot; `cargo-semver-checks` runs on the facade from the first commit; `keel_sdk::system` is semver-exempt until 1.0. |
| Wire | `PROTOCOL` in `GET /api/v1/instance` and in the plugin manifest | Versions N and N-1 are supported. |
| Data | Descriptor `"keel": 1` and `/api/v1` | A breaking change is `/api/v2` next to `/api/v1`; the major version of `@globalart/keel` equals the descriptor version. |

The storage counterpart of the data plane, additive-only migrations inside a major version, is in
[04-storage.md](04-storage.md).

## Tooling

| Crate | Layer | Purpose |
|-------|-------|---------|
| `keel-testkit` | L2 | Test harness for a module in isolation |
| `keel-dev` | L7 | Developer CLI: scaffolding, descriptors, types, i18n, lints, docs |
| `keel-conformance` | L7 | Contract suite against the running `keel` binary with any set of products enabled |
| `keel-client` | L4 | Rust client for calling a peer |

`keel-testkit` boots a module with `Harness::new().module(&M).boot()`; `.product(&MANIFEST)`
boots a bundle with the modules it names. The harness links `keel-kernel` to boot the module in
process, which is why it is the one L2 crate with a kernel dependency. Every test gets a
temporary Postgres database created from a template with `CREATE DATABASE`
(`keel_testkit::TestDatabase`), taken from the server `KEEL_TEST_DATABASE_URL` names; CI
provides the server as a service until testcontainers replace it in M1, and
`hack/check-test-databases.sh` asserts that no test database remains. The harness seeds a
fixture account tree and the principals `Who::{Guest, Service, Owner, Admin, Reseller, A1, A2,
Unrelated}`, and offers `call_as(Level::Reseller, "dns.zone.list", json!({}))`, which picks the
fixture of that level (`A1` for `user`), `call(Who, ..)` for a named fixture, a clock frozen at
boot (`harness.clock()`), and `snapshot_descriptors(level, dir)`, one snapshot file per function
and level, kernel functions excluded. For HTTP tests the harness registers a test
`Authenticator` that resolves the header `X-Keel-Test-Principal` to a fixture principal, a
test-only arrangement; `harness.socket()` and `TestSocket::at(path)` drive the control socket
with `call`, `boot`, `ctl_json` and `ctl_table`. The two standard tests of the module template
are the helpers `assert_reseller_sees_only_subtree(func_list, &fixture)` and
`assert_user_cannot_edit_foreign_row(func_edit, &fixture)`, where a `Fixture` creates a row in
a given account; a module's permission defaults must list `reseller` for the reseller to hold
the key under the default source ([03-access.md](03-access.md)). `FakeHost` in place of the host
agent and fixture bundles joined by a bridge arrive with the milestones that need them.

`keel-dev` provides `new-module` and `new-product` (a bundle lib crate), `migrate create`,
`descriptors dump | check | diff`, `types` (TypeScript from the schemas), `i18n extract | pull |
missing`, `check` and `docs`. `check` lints prefixes, foreign keys, imports, `.owned()` /
`.unowned()` on every function with a key, additivity of migrations, `ldd` cleanliness of the
binary, that every module crate under `products/` is named by its bundle, and that the facade
depends on no product crate. `docs` generates the catalogue of functions, permissions, events and
settings from the registry.

`keel-conformance` runs against the started `keel` binary with any set of products enabled: boot,
auth, the list contract, errors, idempotency, webhooks and `svc.*`, plus the check that a field
shown by the descriptor is accepted by the server. The skeleton exists from M1 and is a mandatory
CI job, run with each first-party bundle alone and with all of them.

`keel-client` is the Rust client of a peer, typed with `keel-types`; it retries only requests that
carry an `Idempotency-Key`. Other languages use the OpenAPI document generated from the function
registry.

## System modules

A subsystem belongs to the kernel only if the installation does not start and does not answer on
the local socket without it, and if it cannot be replaced by composition without touching its
neighbours. By this test the kernel keeps the function dispatcher, descriptors, principal and
account, storage with migrations, the outbox, the message catalogue, the manifest and the
transports. Everything else is a system module: an ordinary `ModuleDef` on the public SDK, replaced
by product composition when a product needs something else. The kernel imports none of them.

The names below are the cargo features and directories of `keel-system`. The module id — the
prefix of the module's functions, tables, permission keys and i18n keys — is the singular form
where the feature is plural: `task` (`task.list`, `task_job`), `plugin` (`plugin.kv.*`,
`plugin_state`), `peer` (`peer.pair_code`, `peer_inbox`), `webhook`.

| Module | Responsibility | Milestone |
|--------|----------------|-----------|
| `auth` | Passwords, sessions, API tokens, one-time login keys, TOTP, `su`, IP rules, rate limit, login methods | M1 |
| `settings` | Product settings, per-principal preferences, brand | M1 |
| `audit` | Who, when, what, with which parameters, with what outcome | M1 |
| `tasks`, `cron` | Long-running operations and periodic jobs | M3 |
| `notify` | In-app notifications, problem registry, delivery channels | M3 |
| `webhooks` | Outgoing subscriptions to events | M3 |
| `license` | Editions, features, limits, offline verification | M5 |
| `update`, `backup` | Update, rollback, backup, restore, `doctor` | M4 |
| `plugins` | Extensions without a rebuild, in any language | M7 |
| `peers` | Neighbouring products | M6 |
| `svc` | Services for billing | M6 |
| `stats` | Metrics and reports | M5 |

M8 adds `keel-system::cluster` with the node registry and the `Invoker` that selects a node by a
function attribute ([06-host.md](06-host.md)).

### auth

- Tokens are opaque, prefixed `ks_` (session), `kat_` (API token), `kpt_` (peer token) and
  `kok_` (one-time login key); only their sha256 is stored, so revocation is immediate and a
  backup contains no usable keys.
- The session cookie is `__Host-keel_session`, one for the installation whatever products are
  enabled; the `__Host-` prefix is why the installation listens on TLS only from its first start
  ([09-operations.md](09-operations.md)).
- `auth.login` is a function of level `guest`, so rate limiting, audit and hooks apply to it
  like to any other function.
- Sessions can be listed and revoked; there is a password policy, lockout after N failed
  attempts, and password reset through `notify`.
- OIDC is a login method for corporate installations and the single sign-on path between
  products ([07-ecosystem.md](07-ecosystem.md)).
- Fine-grained permissions, keys such as `dns.zone.manage` with defaults per level, are stored by
  `auth` through `PermissionSource`. `su` goes only down the ladder, as a separate session with
  an `actor`; audit records both sides ([03-access.md](03-access.md)).
- One-time login keys: `auth.key.issue { external_id, ttl <= 300, redirect }` returns a `kok_`
  key, redeemed by a same-origin `POST auth.key.redeem`; the key dies on first use
  ([07-ecosystem.md](07-ecosystem.md)).
- M1 scope: argon2id, opaque sessions, `__Host-` cookie with CSRF checks, Bearer, TOTP, tokens
  with scope and IP restrictions, `su`, login keys, IP rules, rate limit, session management,
  password policy, lockout.

### settings

- `SettingDef` contributions turn into an automatic form.
- `settings_pref` holds per-principal preferences: columns, widget order, language, time zone,
  theme.
- Values inherit from admin to reseller.
- White-label by hostname through `ManifestPatch`; the brand goes out in `GET /api/v1/boot`.
- Everything editable from the UI lives here; `config.yaml` holds only what the process needs to
  start ([09-operations.md](09-operations.md)).

### audit

- Subscribes to the in-process audit event emitted after commit or rollback and writes in
  batches from a background worker, so a call's response is not lengthened. Functions with
  `mode: read` are not audited by default.
- Masking follows the `secret` attribute in the schema; the same flag drives logs and the
  support bundle. Failed calls are recorded too, with the password masked.
- An optional `subject` and before/after snapshots for form functions feed the "object history"
  screen.
- For `su`, both the principal and the actor are recorded.

### tasks and cron

- A persistent queue with worker classes; progress, log, cancellation and checkpoints. Jobs
  found running at start are marked `interrupted`, and only resumable ones are restarted. The
  queue drains before an update.
- `ui.result = task` opens the progress modal in the frontend; progress travels as the
  `task.progress` SSE event ([08-frontend.md](08-frontend.md)).
- A batch above `batch_threshold` becomes a task; so does `keel.export` above its row threshold,
  with a `download` result; `keel.account.delete` runs as a task with a checkpoint
  ([02-functions.md](02-functions.md), [03-access.md](03-access.md)).
- Cron leadership and the single task worker are held through `pg_advisory_lock`
  ([04-storage.md](04-storage.md)); task logs live under `/var/lib/keel/tasks/`.
- `cron` runs `CronJob` contributions and the `cron` sections of plugin manifests.

### notify

- `notify_problem` with idempotent `raise` and `clear`; sources include disk, TLS, license,
  migrations, the outbox and nodes. Open problems appear on the dashboard, as menu badges and in
  the counters of `GET /api/v1/boot`.
- Channels: email over lettre, telegram, webhook; delivery goes through the outbox.
- Templates are minijinja, taken from `MailTemplate` contributions.
- In-app notifications reach the shell as the `notification` SSE event.

### webhooks

- One module serves both peers and user integrations.
- Signatures are HMAC-SHA256; retries at 1m, 5m, 30m, 2h and 12h; a dead letter is raised as a
  problem.
- Delivery reads the outbox at-least-once with its own cursor ([02-functions.md](02-functions.md)).

### license

- The guard on `access.feature`: a function whose feature is not in the edition the lease grants
  the product that owns the feature is hidden from the menu and answers 402.
- The lease is an Ed25519-signed JSON document bound to `instance_id`, with a grace period and
  degradation to read-only (mutating functions answer 402; reads and login keep working).
- The lease lists the products of the installation and the edition of each; the features of an
  edition are the ones the bundle declares for it, and `/boot.features` is their union.
- `keel.product.enable` refuses a product the lease does not list; with `license: None` every
  compiled-in bundle may be enabled. An enabled product that a later lease no longer lists is
  treated like an expired lease for its modules — reads keep working, writes answer `402` — and
  a problem is raised; the rest of the installation is unaffected.
- Monotonic time in `keel_kv` protects against clock changes.
- `license: None` switches the mechanism off, which is how internal builds run without a key.
  Until M5 the feature guard runs without a lease.
- The edition ceiling for account limits is per product and comes from here
  ([03-access.md](03-access.md)).
- The first releases are expected to run on long offline leases issued by hand; when the license
  server is written as a product on Keel and where its signing key is kept are open questions in
  [11-decisions.md](11-decisions.md).

### update and backup

- The kernel writes `update.request`; the root unit can do exactly one thing, install signed
  version X. Anti-rollback uses the minimum version in the channel manifest. Before an update the
  queue drains and a logical dump of the database is taken.
- An update moves every enabled product at once, because there is one binary and one version.
- A backup is `tar.zst` with a manifest, `BackupSource` contributions from modules, targets
  through `keel-storage`, and `--with-secret-key` under a passphrase.
- `doctor` belongs here as well. The full procedure, restore and support bundle are in
  [09-operations.md](09-operations.md).

### plugins

- A plugin manifest is registered through the same `Registrar`, so the frontend, the CLI, audit
  and license do not distinguish a module from a plugin.
- Three rights: `observe` (after-only, any events, asynchronously from the outbox), `hook`
  (`before` with veto, not above the plugin's own level, never on `keel.*` or `auth.*`) and
  `patch`. A hook on `auth.*` is rejected at start.
- Storage `plugin.kv.*` with a namespace and a binding to a subject.
- Manifest sections `settings`, `cron`, `problems` and `widgets`.
- A separate uid, timeouts and a circuit breaker; a crashing daemon plugin does not take the
  kernel down. Run modes are `daemon` and `exec`.
- A field added by a plugin's patch is stored through the `keel.patch.stored` event from the
  outbox after commit, at-least-once, and the plugin is idempotent by id. The manifest and the
  frame protocol are in [10-contracts.md](10-contracts.md).

### peers

- Peers link installations on different hosts; products enabled on one installation share the
  kernel, the account tree and the outbox and need no pairing ([07-ecosystem.md](07-ecosystem.md)).
- Pairing with a one-time code; mutual `kpt_` tokens with scopes; trust on first use by the
  instance's Ed25519 key, so a TLS rotation does not break the link.
- A pull feed `/api/v1/x/peers/feed?after=seq` for instances behind NAT; the inbox deduplicates
  by event id.
- On-behalf calls are allowed only for principals whose `origin` is this peer.
- `keel.account.*` and `keel.principal.*` events from an identity peer are applied by a handler
  in this module. The trust model and delivery are in [07-ecosystem.md](07-ecosystem.md).

### svc

- A module, not the kernel: it collects `ServiceKind` contributions from product modules and
  registers `svc.<kind>.open`, `suspend`, `resume`, `close`, `set_params`, `stat` and `login`.
- `serviceKinds` in `GET /api/v1/instance` are supplied through `InstanceInfoSource`.
- Service semantics (account upsert on `open`, retention on `close`) are in
  [07-ecosystem.md](07-ecosystem.md).

### stats

- `StatCollector` contributions: key, unit, interval.
- `stats_sample` and `stats_agg` with rollover; aggregation runs as a cron job.
- `stats.series` functions serve dashboard widgets and `report` screens
  ([08-frontend.md](08-frontend.md)).

## Example: module, bundle and composition

```rust
pub static MODULE: ModuleDef = ModuleDef {
    id: "dns", version: env!("CARGO_PKG_VERSION"), sdk: keel_sdk::VERSION,
    requires: &["auth", "tasks", "host"],
    migrations: Some(&MIGRATIONS),           // migrator::embed!("$CARGO_MANIFEST_DIR/migrations")
    init,
};

fn init(r: &mut Registrar) -> keel::Result<()> {
    r.function(zone::list()).function(zone::get()).function(zone::edit()).function(zone::delete());
    r.stream("dns.zone.log", logs::tail);
    r.menu(MenuNode::new("dns/zones", "dns.zone.list").icon("globe").order(10));
    r.i18n("en", include_str!("../i18n/en.json")).i18n("ru", include_str!("../i18n/ru.json"));
    r.permission("dns.zone.read", &[Level::User, Level::Reseller]).permission("dns.zone.manage", &[Level::User, Level::Reseller]);
    r.within("keel.account.delete", hooks::forbid_delete_with_zones);
    r.contribute(LimitDef::new("dns.zone.max", Unit::Count).default(50).unlimited(true));
    r.contribute(AccountLifecycle { on_delete: Some(lifecycle::drop_zones), ..Default::default() });
    r.contribute(SettingDef::new("dns.default_ttl", schema_for!(u32)).default(3600).level(Level::Admin));
    r.contribute(CronJob::new("dns.serial_check", "0 */6 * * *", jobs::serial_check));
    r.contribute(ConfigTemplate::new("bind/zone.j2", include_str!("../templates/zone.j2")).validate("named-checkzone"));
    r.contribute(BackupSource::new("dns.zonefiles", backup::zonefiles));
    r.provide::<dyn DnsProvider>(Arc::new(BindProvider::new()));
    r.start(|ctx| async move { ctx.spawn(jobs::serial_watch(ctx)); Ok(()) });
    Ok(())
}

// products/dns/src/lib.rs
pub static MANIFEST: ProductManifest = ProductManifest {
    id: "dns",
    name: "product.dns.brand",
    version: env!("CARGO_PKG_VERSION"),
    api: keel::types::DESCRIPTOR_VERSION,
    brand: Brand { logo: "product.dns.logo", hue: 264, chroma: 0.12 },
    modules: &[&keel_host::MODULE, &dns_zones::MODULE, &dns_records::MODULE, &dns_templates::MODULE],
    menu: &[MenuGroup { id: "dns", label: "menu.dns", icon: "globe", order: 10 }],
    levels: LevelLabels { labels: &[], disabled: &[] },
    editions: &[
        Edition { id: "lite", features: &["dns.zones"] },
        Edition { id: "pro", features: &["dns.zones", "dns.dnssec", "dns.secondary"] },
    ],
    account_kinds: &["user"],
    i18n: &[("en", include_str!("../i18n/en.json")), ("ru", include_str!("../i18n/ru.json"))],
};

// bin/keel/src/main.rs
fn main() -> keel::ExitCode {
    let runtime = keel::Runtime::new()
        .module(&keel_system::auth::MODULE).module(&keel_system::settings::MODULE).module(&keel_system::audit::MODULE)
        .module(&keel_system::tasks::MODULE).module(&keel_system::cron::MODULE).module(&keel_system::notify::MODULE)
        .module(&keel_system::webhooks::MODULE).module(&keel_system::license::MODULE).module(&keel_system::update::MODULE)
        .module(&keel_system::backup::MODULE).module(&keel_system::plugins::MODULE).module(&keel_system::peers::MODULE)
        .module(&keel_system::svc::MODULE).module(&keel_system::stats::MODULE)
        .product(&keel_dns::MANIFEST).product(&keel_panel::MANIFEST);
    #[cfg(feature = "demo")]
    let runtime = runtime.product(&keel_demo::MANIFEST);
    runtime.frontend(keel::embed_frontend!("../../frontend/dist/shell")).run()
}
```

The panel bundle differs in its bridge, `bridges: &[Bridge { to: "dns", local: "dns_local",
remote: "dns_peer" }]`, which is how `DnsProvider` is served in-process when `dns` is enabled on
the same installation and through a peer otherwise ([07-ecosystem.md](07-ecosystem.md)); the
field joins `ProductManifest` with that bundle. The ids of the `dns-records` and `dns-templates`
modules above are illustrative. The composition lists the system modules, which no bundle
names, and every first-party bundle; each bundle names its own module crates, `keel-host`
included where a product manages the host, so `.module()` never repeats what a manifest says.
The embedded frontend is the one shell application built in the repository
([08-frontend.md](08-frontend.md)); at M0, before any system module or frontend exists, the
whole of `main.rs` is `keel::Runtime::new().product(&keel_demo::MANIFEST).run()` behind the
`demo` feature. The function descriptor this module produces and the envelope of a call are in
[10-contracts.md](10-contracts.md).
