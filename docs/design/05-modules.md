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
| `id`         | Module identifier: a lowercase ASCII identifier, `[a-z][a-z0-9_]*`, because it prefixes every table, function, permission key, setting and i18n key the module owns; the loader refuses any other id. A dotted name may also spell each underscore of the id as a dot: `dns_record` registers `dns.record.*` and owns the table `dns_record`, while `dns.zone.*` stays foreign to it. The owning module of a function is the module that registered it, not the first segment of its name, and a raw route keeps one path per function, `/api/v1/x/<module>/<rest>`. |
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
| `brand`         | `Brand { logo, hue, chroma }` for the shell: the logo is the id of an entry of `assets`, served as `/brand/<id>/logo.svg`; the brand of a single enabled product is the brand of the installation. |
| `assets`        | `&[Asset { id, content_type, bytes }]`: the files the bundle ships, embedded at compile time, the logo `Brand.logo` names among them; `ProductManifest::asset(id)` and `logo()` read them, the kernel serves the logo at `/brand/<id>/logo.svg` for every compiled-in bundle, enabled or not, since the products screen lists disabled bundles with their logo, and the freeze refuses an id shipped twice. A single enabled product without the asset its brand names answers `404` on `/brand/logo.svg`; the kernel's own mark stands for the platform brand alone. |
| `modules`       | `&[&ModuleDef]`: the definitions of the modules the product enables, `host` included when its modules need the agent; the composition learns a bundle's modules from here and lists only the platform modules itself. |
| `bridges`       | `Bridge { to, local, remote }`: the pair of modules through which this product reaches product `to`; `local` is loaded when `to` is enabled on the same installation, `remote` otherwise. The sides are `&ModuleDef`s rather than ids, so a side cannot name a module the binary lacks, and they stay out of `modules`; the loader attributes the chosen side to the bridging bundle. The first bridge is the `panel` bundle's bridge to `dns`, since M5. |
| `menu`          | Menu groups (`id`, `label`, `icon`, `order`) under which the modules' `MenuNode`s are placed; group ids are unique across enabled bundles and differ from the platform groups `access`, `monitor` and `system`. |
| `levels`        | `LevelLabels { labels, disabled }`: level labels and disabled levels ([03-access.md](03-access.md)). |
| `editions`      | Edition ids with their feature sets; the first is the default edition used while no lease is present, and the lease names the edition of each product. |
| `account_kinds` | The account kinds the product creates ([03-access.md](03-access.md)): identifiers, never `system`. |
| `i18n`          | The bundle's own dictionaries, `(lang, json)` pairs: brand name, menu group labels, level labels. |

Which bundles are enabled is recorded in `keel_kv` under `products`
([04-storage.md](04-storage.md)). `keel product enable <id>` and `keel product disable <id>`
change it — through `keel.product.enable` and `keel.product.disable`, `admin`-level `.unowned()`
functions of the `keel` module that check the lease and end with a restart request, or directly
against the database while the service is stopped — and the change takes effect at the next
start. The functions take `{ id }`, refuse an id the binary does not carry as a `validation`
error on `id`, and answer `{ id, state, changed, restart }` with `restart` true whenever the
stored set changed. Since M4 the subcommand calls them over the socket while the service runs,
and a change queues the restart: the kernel drains, takes the pre-change dump and exits with the
status the unit restarts on ([09-operations.md](09-operations.md)); the screen's two actions
carry confirmations of their own that name the restart. The lease check waits for M5; until then
every bundle the binary carries may be enabled. `keel.product.list` answers a page of
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
5. Discovery: every `ModuleSource` contribution receives a `Landscape` (the platform version, the
   data directory, the loaded modules with their versions, the level of every registered function
   and the database) and answers data modules, each an id, a version and an `init` over a
   `Registrar`, and the ones it refused. This is the only step between migrations and start that
   does I/O. A data module whose id is not a module id, is the id of a composed module or is
   discovered twice is refused; the others are initialised after the compiled modules. When their
   `init` or the freeze fails, each data module is tried alone on top of the ones already
   accepted, and a failing one is refused with the error as its reason while the boot goes on.
   The outcome of every data module is served as the `Extensions` service, and the accepted ones
   join the module list of the registry.
6. Freeze of the registries. The kernel checks that every required service has a provider
   and no service or invoker has two, applies patches to other modules' descriptors (a patch
   cannot weaken `access`; a field added by a patch must carry `store`), compiles condition
   expressions and verifies that every field name they mention exists, validates descriptors
   (every function, action and permission a descriptor or a menu node names is registered,
   `ui.read` names a `get` and `x-keel.options` an `options` function, `x-keel` and `x-ui`
   attributes are members of their enums) and checks that the i18n keys a descriptor or a menu
   node carries exist at least for `en`, that a menu node without a label of its own points at a
   function with a screen title, that every menu node's group is declared by an enabled bundle
   or is one of the platform groups `access`, `monitor` and `system`, that no group id is declared by two enabled bundles, that
   every bundle dictionary is a JSON object, that no message uses a placeholder other than
   `{name}` ([08-frontend.md](08-frontend.md)), that every icon a descriptor action, a menu node
   or a menu group names is in the embedded icon set of `contracts/icons.json`, that a view's
   permission is registered and its menu node carries a label of its own and a declared group,
   that no bundle ships an asset id twice, and that no `ConfigTemplate` id is contributed twice
   across the loaded set. Menu groups are ordered by product in composition order, then by
   `order` within a bundle, with the platform groups last, in the order `access`, `monitor`, `system`. A menu section is checked too: it is declared exactly once, its path is exactly `group/section` under an existing group, its icon is in the embedded set, its label exists at least for `en`, and a node of three segments `group/section/item` names a declared section; a node path has no empty segment and no more than three segments. Every problem is collected and reported
   in one error, which fails the boot when it comes from a compiled module.
7. `attach(ctx)` and `start(ctx)`: in graph order each module's attach hook runs before its start
   hook. An attach hook only records what its module needs later (its configuration section, the
   services it requires) and spawns nothing; `keel backup` runs the attach hooks of the loaded
   modules and nothing else, so a backup source finds that state with the service stopped. The
   start hooks registered in `init` run in graph order; background work is spawned through
   `ctx.spawn`, then transports open. `stop` runs in reverse order: a module's stop hook runs
   first, then the tasks it spawned are cancelled and awaited. An attach or start hook that fails
   stops the modules already started and fails the boot.

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
- Streams, `r.stream(function::<P, LogLine>("dns.zone.log")…, handler)`: the builder carries the
  access, ownership and screen of any function, and the handler answers `Lines`, a stream of
  `LogLine { at?, level, text }`. The dispatcher runs the whole pipeline and audits the open
  whatever the mode; the lines are served as server-sent events on the raw route
  `GET /api/v1/x/<module>/<name>` ([10-contracts.md](10-contracts.md)), and a call of a stream
  over the call route, the socket or a nested `invoke` answers `keel.error.stream_route`.
- Raw routes under `/api/v1/x/<module>/…`, only with a mandatory `Guard` and an audit event.
  They exist for upload, download, the WebSocket terminal and VNC, not as a second way to expose
  functions. The raw routes built so far are the kernel's own projection: a function of kind
  `download` or `upload` registered with `r.function` is served at `/api/v1/x/<module>/<name>`,
  `<name>` being the function name without its module prefix, behind authentication and through
  the full pipeline with its audit event; a free-form raw route a module registers itself,
  `r.raw`, waits for the terminal and VNC of `panel`, which M5 left out: the two custom views of
  `panel`, the file manager and the configuration editor, are served by the `upload` and
  `download` kinds and ordinary functions. The one exception to the guard and the audit event is
  the hidden `Endpoint` of `keel_sdk::system`, contributed with `r.contribute(Endpoint::get(name,
  handler))` or `Endpoint::post` and served at `/api/v1/x/<module>/<name>`: the kernel resolves the
  credential as on any route and hands the handler an `EndpointRequest` with the principal, the raw
  body, the query and the headers without the credential, and runs neither the guards nor the
  audit of the pipeline, so the endpoint authenticates and checks access itself and may answer a
  guest. It exists for traffic between installations that a function cannot carry: the hook of
  `peers`, which a delivery's HMAC signature authenticates, and its pull feed, which answers the
  peer token of a link alone ([07-ecosystem.md](07-ecosystem.md)).
- Custom views, `r.view("hosting.file-manager", ViewSpec { access, menu, needs })`: a screen
  without a descriptor but with access rules for the menu and features; the registrar checks the
  module prefix of the name and refuses a second view or function under it, and `menu`, an
  optional `MenuNode` pointing at the view, joins the module's menu
  ([08-frontend.md](08-frontend.md)).
- Migrations, through `ModuleDef.migrations`.
- Menu nodes, `r.menu(MenuNode::new("dns/zones", "dns.zone.list").icon("globe").order(10))`; the
  first path segment names a menu group declared by a bundle or one of the platform groups
  `access`, `monitor` and `system`. `.level(Level::Reseller)` hides the node from the menu of a
  lower level and changes nothing about who may call its function. A node of three segments,
  `system/settings/webhooks`, is a tab of the menu section `system/settings` declared with
  `r.section(MenuSection::new("system/settings", "keel.menu.settings", "sliders-horizontal").order(10))`;
  the shell shows a section as one sidebar item and its nodes as the tabs of one screen. The label
  key of a section carries the declaring module's prefix.
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
- Attach, start and stop hooks, `r.attach(|ctx| async move { … })`, `r.start(|ctx| async move { … })`
  and `r.stop(|| async { … })`, one of each per module; the attach hook is for state that a
  backup source or the start needs, and a task it spawns is refused when the hook runs outside a
  start. `ctx` is the module's `Context`: `ctx.spawn` for background work,
  `ctx.contributions::<T>()` for the typed bag, `ctx.require::<dyn Trait>()` for services,
  `ctx.config::<T>()` for the module's own `ConfigSection`. The public context carries no
  database handle; a system module that needs one outside a call, the `audit` worker being the
  first, reads it through the hidden `ContextExt::db()` of `keel_sdk::system`, and one that keeps
  files, the task output and the uploads, reads the data directory through `ContextExt::data_dir()`
  next to it.
- Services, `r.provide::<dyn Trait>(Arc::new(...))`, consumed elsewhere with
  `call.require::<dyn Trait>()`. Which module provides an implementation is decided by the
  enabled set: `hosting-web` declares `DnsProvider`, and the panel bundle's bridge loads
  `dns-local` or `dns-peer` ([07-ecosystem.md](07-ecosystem.md)).
- Contributions, `r.contribute(T)`, into extension points that modules themselves declare in
  their `pub mod api` and read back with `ctx.contributions::<T>()`. The kernel knows nothing
  about any of them; it only provides the typed bag.
- Derived registrations, the hidden `r.derive::<T>(hook)`, for a module whose functions follow
  from what other modules contribute: a module's `init` runs before the contributions of the
  modules after it are known and functions are fixed at the freeze, so after every `init` the
  loader hands the hook the contributions of type `T` of every module, as `init` left them, and the
  hook registers into its own module's registrar, where the prefix and duplicate checks apply;
  contributions another hook adds are not visible, a module takes one hook, and a failing hook
  fails the start naming its module. `svc` derives its functions, its catalogue and its
  `serviceKinds` that way ([07-ecosystem.md](07-ecosystem.md)).
- `r.invoker(Arc<dyn Invoker>)` is reserved in the kernel from M0 for the cluster milestone: an
  `Invoker` selects the node a function runs on by a function attribute
  ([06-host.md](06-host.md)). The kernel has one invoker slot; a composition with two providers
  fails at freeze, and without one the dispatcher runs every function locally.

### Contribution points

| Type | What is contributed | Consumed by |
|------|---------------------|-------------|
| `SettingDef` | A product setting: key, JSON Schema, default, level; listed with its schema by `settings.list`, from which the shell renders the settings form | `settings` |
| `CronJob` | A periodic job: id, schedule, handler | `cron` |
| `DashboardWidget` | A widget for the `dashboard` block of `GET /api/v1/boot` | the shell ([08-frontend.md](08-frontend.md)) |
| `ProblemSource` | A source of problems for the problem registry | `notify` |
| `MailTemplate` | A minijinja template for outgoing mail; the type lives in `keel-sdk` since M5 and `notify::api` re-exports it, so `auth` contributes the templates of the password reset without depending on `notify` | `notify` |
| `ConfigTemplate` | A configuration file template with its validate command, overridable in `/etc/keel/templates/`; ids are unique across the loaded set | `keel-host` ([06-host.md](06-host.md)) |
| `AgentOp` | A typed host-agent operation: argument and result schemas, handler, policy class | `keel-host` ([06-host.md](06-host.md)) |
| `BackupSource` | The module's own data in a backup archive | `backup` |
| `ConfigSection` | The module's own section of `config.yaml`, validated against the type's schema and typed at freeze, `T::default()` when absent | configuration loading ([09-operations.md](09-operations.md)) |
| `PermissionSource` | The stored grants of a principal, `grants(principal, db)` answering key → held; the transports overlay the answer on `Principal.grants` once per request and the guards read a stored grant over the registered default ([03-access.md](03-access.md)) | `auth` |
| `ManifestSource` | `patch(host, db)` answering the `ManifestPatch { name?, logo?, hue?, chroma? }` of the effective brand for a request host, every member optional: white-label branding by hostname, applied to `/boot.brand` and `/brand/logo.svg` and never to the socket's boot | `settings` |
| `ServiceKindDef` | A service type for billing, `ServiceKindDef::new(kind)` over a `ServiceKind` of `keel_sdk::svc` with its `KIND`, `TITLE`, `SCHEMA_VERSION` and its `Params`, `State` and `Account` schemas | `svc` ([07-ecosystem.md](07-ecosystem.md)) |
| `LimitDef` | An account limit: key, unit, default, level, whether `unlimited` is allowed | `keel_limit` and `call.check_limit` ([03-access.md](03-access.md)) |
| `AccountLifecycle` | `on_suspend`, `on_resume`, `on_delete`, `on_rename` handlers | `keel.account.*` ([03-access.md](03-access.md)) |
| `StatCollector` | A metric: key, unit (`count`, `bytes` or `seconds`), an interval of whole minutes and the collector; `new` samples one installation-wide value, `per_account` a value per account id and `per_subject` a value per named subject of an installation-wide key, such as a service | `stats` |
| `InstanceInfoSource` | Extra `features` and `serviceKinds` of `GET /api/v1/instance`, which the kernel unions, sorts and serves without knowing what they mean: `peers` contributes the feature `peers`, `svc` the feature `svc` and the name of every kind | the instance endpoint ([07-ecosystem.md](07-ecosystem.md)) |

`LimitDef`, `AccountLifecycle`, `SettingDef`, `ConfigSection`, `StatCollector` and `MailTemplate`
are part of what the SDK gives every module: a product module contributes a setting or claims a section of `config.yaml` without
naming `settings` in `requires`, as the `dns` example at the end of this document does, and the
resolution of a setting is the public trait `SettingsSource` behind `call.setting::<T>(key)`. The
other points are declared by modules in their `pub mod api`. The last column names where each
contribution is consumed.

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
  as its prefix, literally or with its underscores read as dots.
- A module crate of a product may depend on the `api` module of another module crate of the same
  bundle it names in `requires`, which is how `dns-records` reaches `dns-zones`, and on
  `keel-client`, a library of L4 like `keel-host`, which is how `dns-peer` reaches a neighbouring
  installation; `hack/check-deps.sh` admits both.
- Foreign keys between modules exist only inside a declared `requires` relation and towards
  `keel_account` and `keel_principal`. Another module's data is read through `call.invoke` or a
  service, never through SQL ([04-storage.md](04-storage.md)).
- A module does not open transactions: the kernel opens the write transaction in `Call`, and a
  nested `call.invoke` reuses it ([02-functions.md](02-functions.md)).
- Operations on the OS and on peers run only in `after_commit` or as a task, and a call to a peer
  may also run in a `before` hook; a transaction never waits for the agent or the network.

## The SDK

`keel-sdk` is layer L2 together with `keel-testkit`; a hidden `keel_sdk::system` sits beside it. The
SDK is an interface, not an implementation: it knows nothing of axum, the Postgres pool or cookies;
even `ctx.spawn` hands the task to a spawner the kernel implements, so the SDK carries no runtime.
What module code sees is `ModuleDef`, `Registrar`, `Context` and `Call<P>` with `params()`,
`principal()`, `tx()`, `invoke`, `emit`, `emit_about` (the event with an explicit subject,
[07-ecosystem.md](07-ecosystem.md)), `after_commit`, `require::<dyn Trait>()`, `check_limit`,
`extra` (values of patched fields), `setting::<T>(key)`, `config::<T>()`, `tz()`, `task()` (the
`TaskHandle` of a call running as a job, `None` otherwise) and `spawn_self_as_task()` (the call
re-entered as a job, answering its `TaskRef`); `Principal`, `Account` and `Level`; `KeelError` with
an i18n key, an optional field and a source; `ListQuery`, `Page` and `ListSource`, with `fetch_page`
for a module source that narrows its own query; `MenuNode`; `Secret`; `Id`; the `Owned` trait with
`scope_by_account`, `resolve_account`, `LimitDef` and `AccountLifecycle`; `SettingDef`,
`SettingsSource` and `ConfigSection`; `ProductManifest` with `Brand`, `MenuGroup`, `LevelLabels` and
`Edition`; `Download` and `DownloadOutcome` for the handler of a `download` function and
`keel_sdk::multipart` with `FileAttrs` for the file fields of an `upload`; `DashboardWidget`, the
contribution behind `/boot.dashboard`; `Idempotency`, which `.idempotent(Idempotency::Required)` on
the function builder declares ([02-functions.md](02-functions.md)); `keel_sdk::svc` with
`ServiceKind`, `ServiceKindDef`, `ServiceAccount` and the helpers `peak` and `login`
([07-ecosystem.md](07-ecosystem.md)); `InstanceInfoSource`; and the `Clock` service,
`call.require::<dyn Clock>()`, from which the dispatcher, the `keel` module and every module stamp
times, so that a test can freeze it. The crates the surface is expressed in, `chrono` and
`chrono-tz` among them, are re-exported under their own names. The function builder is specified in
[02-functions.md](02-functions.md), the account helpers in [03-access.md](03-access.md), `Secret`
and `Id` in [04-storage.md](04-storage.md).

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

Hooks needed only by system modules — `Authenticator` with `Client`, `Authenticated` and
`SessionChange`, `OnBehalf`, `PeerReach`, `PeerLinks` with `PeerLink`, `PrincipalSource`,
`AccountSource`, `LimitSource`, `SecretCipher`, `SecondFactorSource`,
`PreferenceSource`, `LanguageSource`, `PermissionSource` with `PermissionCatalogue`,
`ManifestSource` with `ManifestPatch`, `ContextExt`, `AuditSink` with `AuditEvent`, `TaskRunner`
with `TaskSpec` and `TaskSink`, `Caller` with `CallerOptions`, `LiveEvents` with `Frame`,
`AdvisoryLock` with `LockGuard`, `ProblemCounter`, `OutboxSubscriber`, `EventRelay`, `Endpoint`
with `EndpointRequest`, `ModuleSource` with `DataModule`, `Discovery`, `Landscape` and
`Extensions`, `PluginLauncher` with `PluginProcess`, the function `peer_sees`, the registrar's
`derive`, and later
`TransportRegistry` and `StorageSnapshot` — live in `keel_sdk::system`,
marked `#[doc(hidden)]` and exempt from semver until 1.0. They are the seams behind which
authentication, the transports, backup and the outbox consumers plug in; a product module has no
reason to touch them. The public `keel-sdk` is frozen at 1.0 (M7); until then only
`keel_sdk::system` is exempt from semver.

The authentication seam is a chain, not a single service: a module registers its authenticator
through the contribution bag, `r.contribute(Arc<dyn Authenticator>)`, so that several modules
answer for their own token prefixes, and the transport asks them in module order with the
`Credential` and the `Client` of the request. `Ok(None)` means "not my credential" and the next
one is asked; `Ok(Some(Authenticated { principal, session, token }))` ends the chain, `session`
being the `id_hash` of the session row for a cookie and `None` for a token, and `token` the hash a
module keeps the bearer token of the call under (`token_hash` of an API token,
`inbound_token_hash` of a peer token), `None` for a session cookie; `Err` ends it with that error;
a credential none recognises answers `401 unauthenticated`. `auth` answers for `ks_` and `kat_`,
`peers` for `kpt_`. `Client { ip, user_agent, session, token }` is what the transport learned
about the request, read from the call through `CallExt::client()`, `None` on the socket. Once the
credential is authenticated, the transport hands an `X-Keel-On-Behalf-Of` header as sent to the
`OnBehalf { act_for(caller, header, db) }` contributions, and the first that answers a principal
supplies the principal of the call; a header none accepts answers `403 keel.error.on_behalf`
([07-ecosystem.md](07-ecosystem.md)). A handler records `SessionChange { Set(Secret) |
Clear }` through `CallExt::session()`, and the dispatcher carries it to the transport only for a
write that committed or a read that completed, so a call that fails after recording it answers
without `Set-Cookie`. `PrincipalSource { by_id, by_login, by_external, of_external_id, create }`
is the kernel's lookup over `keel_principal`, answering the stored principal with `via: internal`,
no actor and the full scope, so that `auth` resolves logins and session rows to principals and
`peers` creates the principal of a link and finds a customer by its external identity without SQL
on a kernel table.

The kernel provides its own hidden services under the name of the platform before the requirements
of the modules are checked at freeze, so a module may `r.require` one at `init` and a module
providing one of them is reported as a second provider rather than replacing the kernel:
`PrincipalSource`; `AccountSource { ancestry, by_id, of_external_id }`, the chain from an account up
to the root read from `keel_account.path`, an account by its id and the accounts of an external id;
`LimitSource { effective, store }`, the effective limit of an account and the write of its own row
of `keel_limit` the service kinds make, refused for an undeclared key and for `null` where the
definition admits no unlimited; `InstanceInfo`, the handshake the kernel folds at freeze;
`SecretCipher { seal, open }` over the data key the kernel opens from `keel_kv.secret_key`, whose
sealed form is the same JSON text as the wrapped key, so a `TEXT` column holds nothing the key does
not unlock and a composition without a key provides no cipher; `LanguageSource`, the languages the
frozen catalogue serves; `Caller { call }`, the dispatcher entered outside a request with an
explicit principal, through which a module runs an internal function of its own or a job's call, so
the outbox and the audit trail see it as any call; `LiveEvents { publish }`, the in-memory bus of
the frames the event stream carries beside the outbox, `task.progress` and `notification` among
them; `AdvisoryLock { take }`, the session lock on a pinned connection; `OutboxSubscriber { after,
newest, latest }`, the only reader of `keel_outbox` ([04-storage.md](04-storage.md)); and
`EventRelay { relay }`, which enters an event of another installation into `keel_outbox` under its
own id with its `instance_id`, answers a repeat as nothing new and refuses an event of this
installation and an id the outbox holds for another one. `PrincipalSource` also answers
`at_or_above(level)` and `of_account(account)`, the recipient lookups of `notify`. A module provides
the others: `TaskRunner { spawn, enqueue, wake, progress, log, checkpoint, cancel, state }` from
`tasks`, behind which the dispatcher turns a call into a job, `enqueue(db, spec)` inserting the job
through the caller's transaction and `wake(id)` publishing its state and waking the worker once that
transaction committed ([02-functions.md](02-functions.md)); `PeerReach { reaches, customer, admits,
holds }` and `PeerLinks { offering }` from `peers`, through which `auth`, `svc`, `webhooks` and
`dns-peer` learn whether a principal reaches through a link, which customer an external id names for
it, whether a link's principal may see a record of an origin, whether a link still holds a token,
and which link offers a product with its address and its outbound token opened, each asked only when
`peers` provides it ([07-ecosystem.md](07-ecosystem.md)); `ProblemCounter { counts }` from `notify`,
from which `GET /api/v1/boot` fills `problems`; `SecondFactorSource { enrolled }` from `auth`, from
which `GET /api/v1/boot` fills `principal.totp`, `false` for a guest, for the owner and while none
is provided; `PreferenceSource { preferences }` from `settings`, from which the HTTP transport
resolves once per request the language it overlays on the principal and the zone it hands to
`Call::tz()`; and `AuditSink { record }` from `audit`, fed the `AuditEvent` of every write call.
`ContextExt::db()` is the pool handle a module reads outside any call, the `audit` worker being its
first consumer.

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
| `keel-dev` | L7 | Developer CLI, `crates/dev`: `types`, `descriptors dump \| diff \| check`, `views share` and `openapi` exist from M2; scaffolding, i18n, lints and docs follow with the milestones that need them |
| `keel-conformance` | L7 | Contract suite against the running `keel` binary with any set of products enabled |
| `keel-client` | L4 | Rust client of the wire contract, for calling a peer and for the conformance suite |

`keel-testkit` boots a module with `Harness::new().module(&M).boot()`; `.product(&MANIFEST)`
boots a bundle with the modules it names. The harness links `keel-kernel` to boot the module in
process, which is why it is the one L2 crate with a kernel dependency. Every test gets a
temporary Postgres database created from a template with `CREATE DATABASE`
(`keel_testkit::TestDatabase`), taken from the server `KEEL_TEST_DATABASE_URL` names or, when
the variable is unset, from the `postgres:16` container `keel-testkit-postgres` the testkit
starts through testcontainers and keeps running for the next test process, printing the URL it
uses once. CI keeps its Postgres service, because `hack/prove-binary.sh`, `hack/conformance.sh`
and `hack/check-test-databases.sh` need a server that outlives one process, proves the container
path by running the testkit's own tests with the variable unset, and
`hack/check-test-databases.sh` asserts that no test database remains. The harness seeds a
fixture account tree and the principals `Who::{Guest, Service, Owner, Admin, Reseller, A1, A2,
Unrelated}`, and offers `call_as(Level::Reseller, "dns.zone.list", json!({}))`, which picks the
fixture of that level (`A1` for `user`), `call(Who, ..)` for a named fixture, a clock frozen at
boot (`harness.clock()`), a data key generated per harness so every composition has the
`SecretCipher`, `Harness::new().section(key, json)` for a section of `config.yaml`, and
`snapshot_descriptors(level, dir)`, one snapshot file per function and level, kernel functions
excluded. For HTTP tests the harness contributes a test `Authenticator` that resolves the header
`X-Keel-Test-Principal` to a fixture principal, a test-only arrangement that coexists with the
authenticators of the composed modules; `harness.socket()` and `TestSocket::at(path)` drive the
control socket with `call`, `call_as` (a frame with `as`), `boot`, `ctl_json` and `ctl_table`;
`harness.stop()` stops the kernel and keeps the database, for a test of what a stop leaves
behind. The two standard tests of the module template
are the helpers `assert_reseller_sees_only_subtree(func_list, &fixture)` and
`assert_user_cannot_edit_foreign_row(func_edit, &fixture)`, where a `Fixture` creates a row in
a given account; a module's permission defaults must list `reseller` for the reseller to hold
the key under the default source ([03-access.md](03-access.md)). `FakeHost` in place of the host
agent and fixture bundles joined by a bridge arrive with the milestones that need them.

`keel-dev` depends on `keel-types` alone, which `hack/check-deps.sh` holds. What exists:
`types --out contracts/schema [--check]` writes the JSON Schema of the wire types, from which
`pnpm generate` derives the TypeScript of `@globalart/keel/meta`; `descriptors dump --url --out
[--as LEVEL=LOGIN:PASSWORD]...` writes the projected contract of a running binary per level to
`contracts/dump/`, `descriptors diff <a> <b>` prints one unified diff per differing file and
`descriptors check --url --dir` diffs the committed directory against the binary; `views share
--dump --frontend` prints the custom-views share per product and for the composition and exits
non-zero above 20 % ([08-frontend.md](08-frontend.md)); `openapi --dump --out [--check]` writes
`contracts/openapi.json` from the admin dump ([10-contracts.md](10-contracts.md)); `i18n extract
--dump --out [--projects] [--project] [--lang]` writes the flat catalogue of the admin dump split
per translation project of `contracts/i18n.json`, `i18n missing --dump [--langs]` derives every
key the descriptors emit by the key table of [08-frontend.md](08-frontend.md) and prints the ones
the committed dictionaries do not translate, and `i18n pull --out` rewrites the dictionaries from
the translation service ([08-frontend.md](08-frontend.md)). What follows: `new-module` and
`new-product` (a bundle lib crate), `migrate create`, `check` and `docs`. `check` lints prefixes,
foreign keys, imports, `.owned()` /
`.unowned()` on every function with a key, additivity of migrations, `ldd` cleanliness of the
binary, that every module crate under `products/` is named by its bundle, and that the facade
depends on no product crate. `docs` generates the catalogue of functions, permissions, events and
settings from the registry.

`keel-conformance` runs against the started `keel` binary with any set of products enabled: boot,
auth, the list contract, errors, idempotency, webhooks and `svc.*`, plus the check that a field
shown by the descriptor is accepted by the server. It is a black-box client on `keel-client`, typed
with `keel-types`, and nothing else of the workspace, takes `--url`, the admin's credentials, the
login of a user in another account and `--cacert`, a PEM whose certificates it pins, prints one line
per check and exits non-zero on any failure. The skeleton — the health routes, boot as a guest, the
auth cycle with `su`, a token scoped to one function, the list contract and the error shapes —
exists from M1 as the mandatory CI job `hack/conformance.sh`. Since M6 the suite leans on no
function of a product, and `hack/conformance.sh target/debug/keel` runs it against `dns` alone,
`panel` alone and both in turn, each on a database, a `data_dir` and an agent of its own,
`KEEL_CONFORMANCE_COMPOSITIONS` naming other sets; `demo` has no composition of its own any more.
For each composition the script admits webhook deliveries to private addresses, sets
`auth.shell_url` to the installation's own address, since pairing sends it, and raises
`auth.rate.guest_per_minute` to 600, since the logins of the groups and the guest calls of pairing
pass thirty a minute; `keel product features` must name a feature of each first-party product the
composition enables and none of the others. The groups:

- The generic row is a webhook subscription, which every composition carries and a user may save:
  a probe is a disabled subscription to a sink address under a mask no event matches, and the
  lists, the errors, the webhooks and the event stream run over probes, the export over
  `keel.principal.list`.
- `features` checks that the listing names no feature the boot leaves out, and the `402` of a
  function withheld by the default edition of a bundle the composition enables, from a table of
  the suite (`dns.zone.dnssec.get` behind `dns.dnssec` for `dns` on `lite`, `note.withheld` behind
  `demo.withheld` for `demo` on `full`); a product of the table that runs on that edition must leave
  the feature out of the boot, and only a lease on another edition or a composition without the
  `license` module takes the row out.
- `fields` is the check that a field shown by the descriptor is accepted, at the level of members,
  since the server evaluates no `show_if`: every field the admin's descriptor carries and the
  user's lacks is refused for the user with `403 forbidden_field` naming exactly those paths, and a
  call of the user carrying every field its descriptor shows, each with a value outside its JSON
  types and the key left out, answers `422 validation` with `fields[]` naming every mistyped field
  and no path outside the shown ones; a member whose schema names no type takes any value.
- `webhooks` checks the signature and the first rung of the retries: a listener answers `500` to
  the first attempt of each event, the delivery reads `failed` at attempt 1 with its next attempt
  about a minute away, `webhook.delivery.retry` sends it again with the same `X-Keel-Event-Id` and a
  signature of a later `t`, and it reads `delivered` at attempt 2.
- `idempotency` finds that a replay answers the first answer with its `once` members masked, that a
  key is its principal's, that a read carrying a key is never replayed and that a malformed key is
  refused.
- `svc` takes every kind from `svc.kinds`, which must agree with the `serviceKinds` of the
  handshake in order, builds the account from the customer's `(origin, externalId)` and the
  parameters from the parameter schema by the lowest value of each required member, the next one
  for `set_params`, so a kind whose schema asks for what the builder cannot make fails its check
  by name rather than the process; `login` answers `409 svc.error.no_principal` until a principal
  of the account's origin is upserted into the account.
- `peers`: a listener of the suite plays the neighbouring installations with Ed25519 instance keys
  of their own. As the initiator it calls `peer.pair` itself, reads the subscription made at pairing
  on its hook and the pull feed through the `Feed` of `keel-client`, rotates its second link with
  `peer.renew` and sees both links revoked by `peer.delete`; as the receiver it answers the
  handshake and the `peer.pair` that `peer.create` sends, then signs events to the installation's
  hook and watches the inbox put the first on the admin's event stream and answer the repeat as a
  duplicate. The group reads the installation's handshake with a challenge, whose proof must verify,
  and without one, and refuses a malformed or repeated challenge. The token the installation hands
  the neighbour answers `403 keel.error.level` and `403 keel.error.token_scope` outside its scope
  and `401` once `peer.delete` removes the link, and a second `peer.create` to an address presenting
  the paired neighbour's instance under another key answers `403 peer.error.instance_key` before any
  token leaves the installation. `peer.rotate` from the installation's side is left out, since the
  listener would have to answer `peer.renew`.

`keel-client` is the Rust client of the wire contract ([10-contracts.md](10-contracts.md)): it
depends on `keel-types` alone among the workspace crates, neither the kernel nor the SDK depends on
it, and `keel-conformance` depends on it and on `keel-types`, three rules `hack/check-deps.sh`
holds. It retries only requests that carry an `Idempotency-Key`, pins a certificate or trusts the
roots of the operating system, and reads the pull feed as a stream. Other languages use the OpenAPI
document generated from the function registry.

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
| `update` | Channel check, update as a task, rollback, the pre-update dump | M4 |
| `backup` | Backup as a task, the list of archives, restore at the next start | M4 |
| `plugins` | Extensions without a rebuild, in any language | M7 |
| `peers` | Neighbouring installations: pairing, peer tokens, on-behalf calls, event delivery, shared identity | M6 |
| `svc` | Services for billing: the kinds products contribute, their catalogue and functions | M6 |
| `stats` | Metrics and reports | M5 |

M8 adds `keel-system::cluster` with the node registry and the `Invoker` that selects a node by a
function attribute ([06-host.md](06-host.md)).

### auth

- Tokens are opaque, prefixed `ks_` (session), `kat_` (API token), `kpt_` (peer token) and
  `kok_` (one-time login key); only their sha256 is stored, so revocation is immediate and a
  backup contains no usable keys. `auth` generates the `kpt_` token of a link, 32 random bytes as
  hex, and its authenticator claims `ks_` and `kat_` alone: `peers` contributes the authenticator
  of `kpt_` ([07-ecosystem.md](07-ecosystem.md)).
- The session cookie is `__Host-keel_session`, one for the installation whatever products are
  enabled; the `__Host-` prefix is why the installation listens on TLS only from its first start
  ([09-operations.md](09-operations.md)).
- `auth.login` is a function of level `guest`, so rate limiting, audit and hooks apply to it
  like to any other function. Passwords are argon2id PHC strings in `auth_password`; an unknown
  login and a wrong password are one answer, `auth.error.credentials`, after the same argon2
  work, since a login without a row is verified against a decoy hash. Login methods are the
  module's own extension point, `LoginMethod` in `auth::api`, `local` being the built-in one.
- The second factor is TOTP, RFC 6238 over HMAC-SHA1 with six digits and a 30-second step:
  `auth.totp.enroll` answers the secret and its `otpauth://` URI once, `auth.totp.confirm {
  code }` answers ten single-use recovery codes once, `auth.login` for an enrolled principal
  answers `{ state: "totp", challenge }` without a cookie and `auth.totp.verify { challenge,
  code }` completes the login as `auth.login` would; the secret is stored sealed through the
  kernel's `SecretCipher`, recovery codes and challenges as hashes; a challenge lives five
  minutes and admits five attempts; a step, once matched, is never accepted again.
- API tokens: `auth.token.create { name, level?, scope?, ipAllow?, expiresAt? }` answers the
  plaintext once; a call under a token runs as the token's principal at the lower of its level
  and the ceiling, with the row's scope, and may not create a token wider than itself; the
  dispatcher's scope guard answers `403` `keel.error.token_scope` outside the masks and
  `keel.error.token_read_only` for a write under `ReadOnly` ([02-functions.md](02-functions.md)).
- Sessions can be listed and revoked, `auth.session.list` and `auth.session.revoke`, each
  principal its own below `reseller`; revoking a session takes its `su` child. The password
  policy refuses a password shorter than `auth.password.min_length` or equal to the login. The
  lockout counts failures per `(login, client address)` pair: `auth.lockout.threshold` failures
  within `auth.lockout.minutes` lock that pair alone, so an attacker locks only its own address
  out of a login and the victim from another address is unaffected; failures for an unknown
  login are counted the same way, so `auth.error.locked` says nothing about whether the login
  exists; a wrong second-factor code counts as a failure of the pair; `auth.unlock { login }`
  clears the pairs of a login. The rate limit gives every guest call from one address
  `auth.rate.guest_per_minute` tokens a minute, `429` beyond. IP rules, `auth.ip_rule.*`, are
  `allow | deny` rows per CIDR applying to callers at or above their level, enforced by a module
  guard that never checks a call without a client address — the socket — which is how an
  operator undoes a rule that locked the browser out.
- Password reset, since M5. An address per principal lives in `auth_principal_address`
  (principal, kind `email`, value, `verified_at`), behind the hidden `AddressSource` seam, and the
  tokens in `auth_mail_token` (the SHA-256 of a `kpr_` reset or `kav_` confirmation token, its
  purpose, principal, the address being confirmed, expiry and use). `auth.address.get`,
  `auth.address.set { principal?, value, current? }` and the guest `auth.address.verify { token }`:
  a principal setting its own address gives its current password, since the address can mail a
  reset link, and gets it unconfirmed with a `kav_` link valid for 24 hours; an admin setting
  another principal's address confirms it at once. Both reuse `auth.password.manage`. The guest
  `auth.password.reset.request { login }` answers `{}` whatever the login and does the lookup,
  the token and the mail in an after-commit hook, so neither the answer nor its timing depends on
  the login; it is limited in memory to 3 requests per login and 10 per client address an hour,
  unknown logins counted too, `429 auth.error.reset_limited` beyond. The link goes only to a
  confirmed address and is valid for `auth.password.reset_minutes` (60). The guest
  `auth.password.reset.confirm { token, password }` applies the password policy, uses up every
  open reset token of the principal and revokes every session it holds or acts in; API tokens
  stay. `auth.password.set` and `auth.address.set` also use up the open reset links. A principal
  at or above `owner` is never mailed a link and `confirm` refuses one, so a mailbox is never
  enough to take over the root principal. The links name the address the shell is served at from
  the admin setting `auth.shell_url`, never from `Host` or `Origin`; while it is empty a request
  answers the same and logs that no link was mailed, and `auth.address.set` refuses with
  `auth.error.shell_url`. `auth` does not require `notify`: it reaches `notify.send_to` through
  the hidden `Caller` by name, and without `notify` the mail is not sent and the failure logged.
- Rate limits, lockouts and second-factor attempts are counted in the process's memory, not in
  a table: a failed `auth.login` is a failed call whose transaction rolls back, so a counter the
  handler wrote would vanish with it, and no background writer has been worth a table yet. The
  counters live in instances the module's `init` creates, are timed by the kernel's `Clock`
  read at each check, and evict entries whose window has passed, so a map is bounded by the
  distinct keys of one window; a restart clears them, while IP rules, revocations and login keys
  are rows and survive it.
- OIDC is a login method for corporate installations and the single sign-on path between
  products ([07-ecosystem.md](07-ecosystem.md)).
- Fine-grained permissions, keys such as `dns.zone.manage` with defaults per level, are stored by
  `auth` through `PermissionSource`: rows of `auth_grant` (principal, account, key, granted),
  written by `auth.grant.set { principal, key, granted }` under `auth.grant.manage`, listed by
  `auth.grant.list` at reseller level within the caller's subtree and removed by
  `auth.grant.delete`; a key nobody registered is refused on the field, a key the caller does not
  hold with `auth.error.grant_not_held`, a target at or above the caller through the `not_below`
  rule, and every write bumps the target's `acl_gen` in the same transaction so its descriptors and
  menu change at once ([03-access.md](03-access.md)). The module's menu nodes are
  `system/sessions`, `system/tokens`, `system/ip-rules` and `system/grants`. `su` goes only down
  the ladder, as a separate session with an `actor`; audit records both sides.
- One-time login keys: `auth.key.issue { principal | externalId, ttl <= 300, redirect }` returns
  a `kok_` key and the relative `url` `/login?key=…`, redeemed by a same-origin `POST
  auth.key.redeem` that answers `{ redirect, principal, issuedBy }`; the key dies on first use.
  `externalId` is accepted only from a caller that arrived through a peer link, and through a link
  the target must be a principal of the peer's origin. `auth.key.handover { ttl, redirect }`, at
  the `user` level, issues the key for the caller itself and answers a call on behalf of a customer
  alone, the link being the key's `issued_by`. For any call through a peer link the `url` is
  absolute under `auth.shell_url`. `auth` reaches `peers` through the hidden `PeerReach` alone,
  since it requires nothing ([07-ecosystem.md](07-ecosystem.md)).
- Credentials of a link: retiring a link's inbound token retires every credential of its principal
  through `retire_credentials` of `auth::api`, and `auth_token`, `auth_login_key` and
  `auth_session` keep in `link_principal_id` the principal of the link a credential was minted
  through, so `peer.rotate` and `peer.delete` revoke those too; minting under a link and retiring it
  are serialised by the transaction lock `auth_link:<principal>`, shared and exclusive, and a
  credential minted with a token the link no longer holds answers `401`
  ([07-ecosystem.md](07-ecosystem.md)).
- M1 scope: argon2id, opaque sessions, `__Host-` cookie with CSRF checks, Bearer, TOTP, tokens
  with scope and IP restrictions, `su`, login keys, IP rules, rate limit, session management,
  password policy, lockout. Since M3 the installation listens on TLS alone
  ([09-operations.md](09-operations.md)), so the `Secure` cookie is accepted from any origin; the
  clients of the repository — the testkit, the scripts of `hack/`, `keel-conformance`, the browser
  suite — speak https with the generated certificate.

### settings

- `SettingDef` contributions are listed by `settings.list` as rows `{ key, module, schema,
  default, level, value, scope }`, cut to the definitions at or below the caller's level with
  the value resolved for the caller's account, read by `settings.get { key }` and written by
  `settings.set { key, value }`, which validates the value against the definition's schema and
  refuses a definition above the caller's level; the shell renders the settings form from those
  rows and submits it through `settings.set`, the shell's own value form taking its control from
  the row's schema ([08-frontend.md](08-frontend.md)). A module reads a setting through
  `call.setting::<T>(key)`, behind the public `SettingsSource` this module provides; a
  composition without the module answers the contributed defaults.
- `settings_pref` holds per-principal preferences, read and written by `settings.pref.get` and
  `settings.pref.set { key, value }` on the caller's own row: `lang` (a language the catalogue
  carries), `tz` (a zone name the `chrono-tz` table knows), `theme` and `columns.<function>`
  (column order and visibility, the shell writing `{ order: [...] }` on a header drag); the
  widget order arrives with the dashboard renderer of M5. The HTTP transport reads them once per
  request through the hidden
  `PreferenceSource`: the language is overlaid on the principal, the zone reaches `Call::tz()`
  and `/boot`; `keel_principal.lang` stays the fallback and what `keel.principal.*` edit.
- Values inherit from admin to reseller: a key resolves to the nearest value up the account
  path from the caller's account to the root, then to the contributed default; an admin writes
  the root scope, a reseller its own account.
- White-label by hostname through `ManifestSource`: `settings_brand` holds one row per host
  (`host`, `name`, `logo`, `hue`, `chroma`, `updated_at`, `updated_by`), edited by the
  admin-level unowned functions `settings.brand.list`, `settings.brand.get`, `settings.brand.set`
  and `settings.brand.delete` behind the menu node `system/brand`, which says white-label by host
  name; the host is stored in lower case under the `domain` check, `hue` and `chroma` carry
  their ranges in the schema and say "from the product" when empty, and the logo is an upload,
  `settings.brand.logo { host, logo }` of kind `upload` taking one `image/svg+xml` file of up to
  64 KiB, refused unless it parses as an SVG document without a script element. The patch of the
  request's `Host`, without its port, goes out in `GET /api/v1/boot` and `/brand/logo.svg`
  ([10-contracts.md](10-contracts.md)). The module's menu nodes are `system/settings` and
  `system/brand`.
- Everything editable from the UI lives here; `config.yaml` holds only what the process needs to
  start ([09-operations.md](09-operations.md)).

### audit

- Subscribes to the in-process audit event emitted after commit or rollback and writes in
  batches from a background worker, so a call's response is not lengthened: `record` pushes onto
  a bounded channel of 10 000 events and returns at once, the worker writes batches of up to 200
  rows or every 250 milliseconds with one multi-row insert into `audit_call`, a full channel
  drops the event and counts it in the log rather than slowing a call, and `stop` drains the
  channel; `api::Flush` awaits the worker for tests and for the support bundle. Functions with
  `mode: read` are not audited by default.
- Masking follows the `secret` attribute in the schema; the same flag drives logs and the
  support bundle. Failed calls are recorded too, with the password masked.
- The dispatcher fills `subject` with the key parameter of a keyed function and `after` with
  the masked result of a committed form function that has a key, `once` fields masked as well,
  so an object edit records what it left behind while the results of key-less forms — the
  plaintext of a token, recovery codes, a login key — never reach the trail; the `before`
  snapshot stays `NULL` at M1, since it costs a read before the handler and nothing reads it
  until the "object history" screen the two snapshots feed.
- For `su`, both the principal and the actor are recorded; so are the actor of a socket frame
  with `as`, the request id, the client address and `via`. The trail has no foreign keys,
  because it outlives the principals and accounts it names.
- `audit.list`, owned by `account_id` under `audit.read` for user and reseller, sorted by `at`
  descending and filterable on function, principal, status, request id and `at` under the
  day-boundary rule ([10-contracts.md](10-contracts.md)), and `audit.get`; the menu node
  `system/audit`. A guest's failed login lands in the root account and is therefore visible to
  admins alone.

### tasks and cron

- A persistent queue with worker classes; progress, log, cancellation and checkpoints. `task_job`
  holds one row per job (function, masked params, the principal it runs as, class, state,
  progress, checkpoint, error, the stored file of a `download` result) and `task_step` one row
  per progress report or checkpoint; the log is `<data_dir>/tasks/<id>.log`. The module provides
  the hidden `TaskRunner`, behind which the dispatcher turns a call into a job, and requires the
  kernel's `Caller`, `AdvisoryLock` and `LiveEvents`. The worker is single per installation on
  the session lock `keeltask`, runs jobs of distinct classes concurrently and jobs of one class in
  creation order, and claims a queued row with a conditional update. Jobs found running when a
  worker takes the lock are requeued with their checkpoint when resumable and marked
  `interrupted` otherwise; `stop` drains, no queued job starts, a running job sees
  `is_cancelled()` at its next checkpoint and what still runs after the grace is `interrupted`,
  so the next start resumes it. Cancellation is cooperative: `task.cancel` flags a running job
  and ends a queued one at once.
- Functions: `task.list`, owned under `task.read`, `task.get` with the prop list, the progress,
  the log tail and the checkpoint, `task.cancel` under `task.manage`, and `task.download`, the
  `download` function that serves a job's stored file to its owner; the menu node `system/tasks`.
- `ui.result = task` opens the progress modal in the frontend; progress travels as the
  `task.progress` SSE event ([08-frontend.md](08-frontend.md)), a `TaskProgress` frame the
  worker publishes on `LiveEvents` for every report and state change.
- A batch above `batch_threshold` becomes a task; so does `keel.export` above its row threshold,
  with a `download` result; `keel.account.delete` runs as a task with a checkpoint
  ([02-functions.md](02-functions.md), [03-access.md](03-access.md)).
- Cron leadership and the single task worker are held through `pg_advisory_lock`, the session
  locks `keelcron` and `keeltask` taken through the hidden `AdvisoryLock`
  ([04-storage.md](04-storage.md)).
- `cron` runs `CronJob` contributions — `CronJob::new(id, schedule, handler)` in `cron::api`, a
  five-field expression, the handler taking a `JobRun` that invokes functions as the owner — and,
  from M7, the `cron` sections of plugin manifests. The leader ticks every second against the
  kernel's `Clock`; an occurrence that arrives while the previous run has not finished is
  recorded `skipped`, and a clock that jumps past several occurrences runs the job once. Every run
  is a `cron_run` row (`job`, `scheduled_at`, `started_at`, `finished_at`, `outcome` among
  `running`, `ok`, `failed`, `skipped`, `interrupted`, `error`). The setting `cron.overrides`, an
  object from job id to schedule at admin level, changes a job's schedule without a build.
  Functions, all admin-level and unowned: `cron.job.list` over the contributions joined with
  their last run, `cron.job.run { id }` starting a job now, `cron.run.list`; the menu node
  `system/cron`. Since M6 the module contributes its own job `cron.idempotency.prune`, hourly at
  minute 25, which calls the kernel's internal `keel.idempotency.prune` as the owner, since the
  kernel cannot depend on the module that runs jobs ([02-functions.md](02-functions.md)).

### notify

- `notify_problem` with idempotent `raise` and `clear` over the open `(key, subject)` pair;
  sources include disk, TLS, license, migrations, the outbox and nodes. Every row and every event
  is written by the dispatcher: `notify.problem.raise`, `notify.problem.clear` and
  `notify.message.create` are internal functions the registry runs through the hidden `Caller`,
  wrapped by `Problems` and `Notifier` of `notify::api`, so the row and its event
  (`notify.problem.raised`, `notify.problem.cleared`, `notify.message.created`) commit together. A
  raised problem notifies the admins, an account-scoped one the account's principals as well,
  resolved through the `at_or_above` and `of_account` lookups of `PrincipalSource`. The poll job
  `notify.problems.poll`, a `CronJob` on every minute, runs the `ProblemSource` contributions
  and raises and clears what they report. Open problems appear on the dashboard through the
  module's `DashboardWidget` over `notify.problem.list`, as menu badges through `badge.problem`,
  and in the `problems` counters of `GET /api/v1/boot` through the hidden `ProblemCounter` the
  module provides.
- Functions: `notify.problem.list` and `notify.problem.get` (unowned, a problem without an
  account visible to admins alone), `notify.message.list`, `notify.message.mark` and
  `notify.message.clear` on the caller's own rows, `notify.channel.list`, `.get`, `.set`,
  `.delete` and `.test`, `notify.delivery.list`; the menu nodes `system/problems` and
  `system/channels`.
- Channels: the kinds `email`, `telegram` and `webhook` are declared; `email` over lettre is the
  one implemented, the other two refused on `kind` until their milestone. A channel row's
  password is sealed through `SecretCipher` and never leaves the row. Delivery goes through the
  outbox: the consumer with the cursor `notify_cursor` reads the `notify.message.created` events
  through `OutboxSubscriber` and creates one `notify_delivery` per enabled channel; the sender
  retries on the ladder 1m, 5m, 30m, 2h, 12h and raises `notify.delivery_failed` for the channel
  after the last rung. `notify.channel.test` sends a test message to the caller on that channel
  alone.
- A delivery to one recipient, since M5: the internal `notify.send_to { principal, template,
  params, unverified? }` resolves the principal's address through `AddressSource`, read at call
  time and not declared, so `notify` boots without `auth`, refuses an unconfirmed address unless
  `unverified` is set and sends over the oldest enabled `email` channel with the channel's own
  recipient replaced by that address. Its rows live in `notify_direct`, since `notify_delivery`
  requires a message and relaxing that column would not be additive; the sender retries them on
  the same ladder, and the params are sealed through `SecretCipher`, marked secret and dropped
  once the mail is sent or given up.
- Templates are minijinja, taken from `MailTemplate` contributions, a template's id being the
  message key it renders; the module ships `notify.problem.raised`, `notify.problem.cleared` and
  `notify.test` in `en` and `ru`, compiled at start.
- In-app notifications reach the shell as the `notification` SSE event, a `NotificationFrame`
  published on `LiveEvents` after commit.

### webhooks

- One module serves both peers and user integrations; it requires `notify` for its dead letter.
  `webhook_subscription` holds the url, the sealed secret, the event masks (the rule of
  `invalidate_on`) and the account a subscription listens to; `webhook_delivery` one row per
  (subscription, event), `pending`, `delivered`, `failed` or `dead`, with the event type copied
  onto the row so the list shows it without the outbox.
- Functions under `webhook.manage`: `webhook.subscription.list`, `.get`, `.set`, `.delete` and
  `.toggle`, `webhook.delivery.list`, `.get` and `.retry`; the menu node `system/webhooks`. The
  secret rule: a secret is shown once, at creation, chosen by the caller or generated; an empty
  secret counts as absent; the stored one is sealed and never answered again.
- Signatures are HMAC-SHA256 ([07-ecosystem.md](07-ecosystem.md)); retries at 1m, 5m, 30m, 2h
  and 12h; the sixth failure marks the delivery `dead` and raises `webhook.dead_letter` for the
  subscription as a problem, cleared by a retry that delivers.
- The installation's own network is guarded by the setting `webhook.private_targets` (admin
  level, `false` by default): while it is off, `webhook.subscription.set` refuses on `url` a
  scheme other than `https`, an IP literal and a host that resolves to a loopback, link-local,
  private, unique-local or unspecified address, and the sender resolves the name again at send
  time, fails the attempt when any answer is private and pins the checked addresses on the
  request, so a rebinding name reaches nothing.
- Delivery reads the outbox at-least-once with its own cursor, `webhook_cursor`, through
  `OutboxSubscriber`, one delivery per enabled subscription whose masks match the event type and
  whose scope admits the event's subject ([02-functions.md](02-functions.md)). An event another
  installation emitted reaches only a subscription whose scope covers every account and that is not
  `local_only`, the flag `webhook.subscription.set` records when the credential saving it is
  narrowed by a scope of functions or a call through a peer, and that a peer's subscription always
  carries ([07-ecosystem.md](07-ecosystem.md)). An event about a service reaches a subscription
  with a principal only when `peer_sees` admits it, after the account scope.
- The subscription of a peer link is made by `peers` at pairing through `subscribe_peer` of
  `webhooks::api`: `peer_id` names the link, `level` carries the level granted to it, whose
  account scope the consumer applies, its scope cache keyed by level, and `floor_seq` the outbox
  `seq` at pairing, below which it delivers nothing; its deliveries carry `X-Keel-Peer`, it obeys
  `webhook.private_targets` like any other, `webhook.subscription.set`, `.toggle` and `.delete`
  refuse it with `409 webhook.error.peer`, and it goes with its deliveries at `peer.delete`. The
  module requires `AccountSource` and takes `PeerReach` at start when `peers` provides it.

### license

- The lease is an Ed25519-signed JSON document, `{ instance_id, issued_at, expires_at,
  grace_days, products: [{ id, edition }] }`, sealed in the envelope `keel-release` signs
  releases with and verified against the license key compiled into the binary
  (`crates/release/keys/license.pub`, a second pair beside the release key). `license.install`
  (admin, and `keel license install <file>` over the socket) verifies it, binds it to the
  installation's `instance_id` and stores the signed text in `keel_kv` under `license_lease`; a
  refused lease is a field error on `lease`: `license.error.signature` for any envelope or
  signature failure, `license.error.lease` for a payload that is not a lease,
  `license.error.instance` for another installation and `license.error.older` for a lease issued
  before the installed one, so an older signed lease never brings back an edition or an expiry
  the vendor replaced. Unknown payload fields are ignored so a later issuer may add some. The
  in-memory lease is reloaded from `keel_kv` after the commit of an install; a stored lease that
  stops verifying, as after a key rotation, is logged and ignored. `license.get` answers the
  state, `instanceId`, the listed products with their editions, the issue and expiry times and
  the end of the grace.
- The states are `none`, `valid`, `grace` and `read_only`: a lease is valid until `expires_at`,
  in grace for `grace_days` after it, raising the problem `license.expiring`, and read-only
  after the grace, raising `license.expired` (critical) instead. The time is the later of the
  wall clock and the largest instant the installation has seen, kept in `keel_kv` under
  `license_clock`, advanced at every start, by the cron job `license.clock` every ten minutes
  (an upsert that only moves forward) and on every reading, so a clock moved backwards never
  revives an expired lease; a clock set forward by mistake keeps its instant, and installing a
  new lease is the remedy. An offline lease cannot tell a clock set back before the first start.
- The features of the installation are those of the editions the lease lists; the features of an
  edition are the ones the bundle declares for it, `/boot.features` is their union and
  `/boot.products[].edition` the licensed edition. A listed edition the bundle does not declare,
  and a product the lease does not list, grant nothing. The guard on `access.feature`: a function
  whose feature the installation lacks is hidden from the menu and answers `402`.
- Read-only is a dispatcher guard beside the feature guard: an entered call of a function whose
  mode is not read answers `keel.error.license` (`402`) with the function and the state, and the
  descriptor projection asks the same guard, so an action it refuses keeps its place with
  `disabled: keel.error.license` and an inline edit is dropped. Reads keep working, and so do the
  functions marked `exempt_from_read_only` on the builder: the steps of a login (`auth.login`,
  `auth.totp.verify`, `auth.key.redeem`), `auth.logout`, `auth.su`, `auth.su.exit` and
  `license.install`. The guard runs for the entered call only and not for internal principals,
  so cron jobs and other housekeeping keep running.
- `keel.product.enable` refuses a product the lease does not list with `license` before it writes
  anything. An enabled product that a later lease no longer lists degrades like an expired lease
  for its modules, whatever the marks of its functions, and raises
  `license.product_unlisted` with the product as the subject; a module turns read-only as
  unlisted when every enabled product that carries it is unlisted, and the rest of the
  installation is unaffected. The offline `keel product enable` on a stopped service has no gate;
  a product enabled that way is degraded as unlisted at the next start.
- Without a lease nothing degrades and every compiled-in product may be enabled at its default
  edition, the first the bundle declares, which keeps the installer and the scripts working
  without the production key; the gate applies once a lease is installed. `license: None`, a
  composition without the module, switches the mechanism off and keeps every feature of every
  edition of the enabled products, which is how the test fixtures and internal builds run.
- The module requires `cron` and `notify` for its clock job and its problem source.
- Leases are issued by hand with `keel-dev license issue` ([09-operations.md](09-operations.md));
  when the license server is written as a product on Keel and where its signing key is kept are
  open questions in [11-decisions.md](11-decisions.md). The edition ceiling for account limits
  is per product and would come from here ([03-access.md](03-access.md)); no edition carries one
  yet.

### update and backup

- The kernel writes `update.request`; the root unit can do exactly one thing, install signed
  version X. Anti-rollback uses the minimum version in the channel manifest. Before an update the
  queue drains and a logical dump of the database is taken.
- An update moves every enabled product at once, because there is one binary and one version.
- A backup is `tar.zst` with a manifest, `BackupSource` contributions from modules and
  `--with-secret-key` under a passphrase. `BackupSource::new(name, write).restore(read)`: both
  hooks receive the source's directory in the archive, a pool `Db`, the data directory and the
  resolved configuration sections (`BackupDir::config::<T>()`). The `write` hook may also rely on
  the state of an attach hook but never on the one of a start hook, because `keel backup` runs
  the attach hooks and the sources without starting any module. `keel restore` runs no attach
  hook, so the `restore` hook gets the configuration sections only, and no module registers a
  restore hook with host needs today. The freeze refuses a name without its module's prefix and
  one claimed by two modules. The kernel
  provides `dyn Backups` as a platform service, so the module reaches the archive writer without
  depending on the kernel. The archive has one target, the local directory
  `<data_dir>/backups/`. The S3 target and the crate `keel-storage` were moved to M5 with
  `hosting-backup`, which M5 built with local archives only; remote targets wait for a later
  milestone, for this module and for `hosting-backup` alike.
- The module carries `backup.create` (a task whose archive `task.download` serves),
  `backup.list` (the screen "Backups", the archives with their manifests) and `backup.restore`
  (a task that checks the archive and writes the restore request the next start consumes).
- `doctor` and the log tail are functions of the kernel module `keel`, `keel.doctor.run` and
  `keel.logs.tail` behind the screen "Health", because the registrar admits a function only
  under its module's prefix. The full procedure, restore and support bundle are in
  [09-operations.md](09-operations.md).

### plugins

- A plugin manifest is registered through the same `Registrar`, so the frontend, the CLI, audit
  and license do not distinguish a module from a plugin.
- Three rights: `observe` (after-only, any events, asynchronously from the outbox), `hook`
  (`before` with veto, not above the plugin's own level, never on `keel.*` or `auth.*`) and
  `patch`. A hook on `auth.*` is rejected at start.
- A manifest that fails a check at start is refused, the kernel starts without the plugin and the
  problem `plugin.refused` names it.
- Storage `plugin.kv.*` with a namespace and a binding to a subject.
- Manifest sections `settings`, `cron`, `problems` and `widgets`.
- A separate uid, timeouts and a circuit breaker; a crashing daemon plugin does not take the
  kernel down. Run modes are `daemon` and `exec`. The process runs as the operating-system
  account `kp_<id>`, which `plugin.enable` creates through the host agent. An open breaker raises
  the problem `plugin.failing` and answers the plugin's functions with `plugin.error.failing`; a
  daemon that ended and is not restarted answers `plugin.error.stopped`; both are
  `unavailable`. Every failure that finds the breaker open, whatever its cause, stops every
  process of the account, and so does a daemon start that failed, since a daemon runs one process
  and an agent may have started it with an answer that never arrived. A kernel stop ends a frame
  that waits for its answer at once and counts as no failure.
- A field added by a plugin's patch is stored through the `keel.patch.stored` event from the
  outbox after commit, at-least-once, and the plugin is idempotent by id. The manifest and the
  frame protocol are in [10-contracts.md](10-contracts.md).

### peers

- Peers link installations on different hosts; products enabled on one installation share the
  kernel, the account tree and the outbox and need no pairing ([07-ecosystem.md](07-ecosystem.md)).
  The module id is `peer`, the feature `peers`; it requires `auth`, `cron`, `notify` and
  `webhook`, contributes the feature `peers` to the handshake and joins the `keel-system` matrix.
- Pairing with a one-time code: `peer.pair_code` on the receiving side, `peer.create` on the
  initiating side, which calls the guest `peer.pair` of the receiver; mutual `kpt_` tokens with
  scopes, each link with a `service`-level principal `peer-<link id>` in the root account; trust on
  first use by the instance's Ed25519 key, proven by a signed challenge at every rotation and daily
  check, so a TLS rotation does not break the link; `peer.rotate` from either side through the guest
  `peer.renew`, and `peer.delete`, both retiring every credential of the link. The tables are
  `peer_product`, `peer_pair_code`, `peer_inbox` and `peer_identity`; the cron job `peer.check`
  checks every link daily and raises `peer.problem.protocol`, `peer.problem.instance_key` and
  `peer.problem.proof`; the menu node is `system/peers`.
- A pull feed `/api/v1/x/peer/feed?after=seq` for instances behind NAT and the hook
  `/api/v1/x/peer/hook` for signed deliveries, both `Endpoint`s; the inbox deduplicates by link and
  event id, bounds a link to 10 000 waiting events and relays them onto the bus through
  `EventRelay`; a pull consumer on the initiating side keeps the cursor of a link it pulls.
- On-behalf calls are allowed only for principals whose `origin` and whose account's `origin` name
  this peer; the module contributes the `OnBehalf` resolution of `X-Keel-On-Behalf-Of`, the hooks
  that bind an origin to its peer on `keel.account.upsert` and `keel.principal.upsert`, and provides
  `PeerReach` and `PeerLinks` to the modules that ask.
- `keel.account.*` and `keel.principal.*` events from an identity peer are applied by a handler
  in this module. The trust model and delivery are in [07-ecosystem.md](07-ecosystem.md).

### svc

- A module, not the kernel: it collects `ServiceKindDef` contributions from product modules through
  the hidden `derive` of the registrar and registers for each kind `svc.<kind>.open`, `suspend`,
  `resume`, `close`, `set_params`, `stat` and `login`, `admin`-level without a permission key,
  `open` and `close` requiring an `Idempotency-Key`, and the catalogue `svc.kinds`. The module id
  and the feature are `svc`; it requires no other module, reads the identity of an account through
  the kernel's hidden `AccountSource` and asks `PeerReach` only when `peers` provides it.
- `serviceKinds` in `GET /api/v1/instance` are supplied through `InstanceInfoSource`, beside the
  feature `svc`.
- `svc_service` keeps the services, unique per kind and external id; the setting
  `svc.retention_days` (1 to 3650, 30 by default) fixes how long a closed service is kept; the
  module hooks `keel.account.suspend` and `resume` to drop the marks of the suspensions it made,
  forgets an account's services through its `AccountLifecycle`, and emits `svc.service.opened`,
  `suspended`, `resumed`, `closed` and `updated`.
- The SDK side lives in `keel_sdk::svc`: `ServiceKind`, `ServiceKindDef`, `ServiceAccount`, the
  wire shapes of the functions, `SUBJECT` and the helpers `peak` and `login` the kinds of the
  first products use. Service semantics (account upsert on `open`, retention and the suspension
  marks on `close`, the reach of a peer) are in [07-ecosystem.md](07-ecosystem.md).

### stats

- `StatCollector` contributions from any module, the kernel's `keel.accounts` and
  `keel.principals` among them. One cron job, `stats.collect`, runs every minute and samples each
  collector whose interval divides the minute since the epoch, at the run's scheduled minute,
  since a module's cron jobs are fixed at registration while the collectors of other modules are
  known only at start; an interval is a whole number of minutes and two collectors never share a
  key, checked at start. A second run of the same minute overwrites its samples, and a failing
  collector fails the run after the others were sampled.
- `stats_sample (key, subject, at, value)` and `stats_agg (key, subject, bucket, resolution, min,
  max, sum, count)`, an installation-wide sample having the empty subject. `stats.aggregate` at
  minute 5 of every hour folds the complete hours into hour buckets and the complete days into day
  buckets, from the last bucket of each key and subject; `stats.prune` at 00:20 aggregates first,
  then deletes samples older than `stats.raw_days` (7) and hour buckets older than
  `stats.hour_days` (90); day buckets are kept.
- `stats.keys` lists the keys with their unit, interval and scope (`installation` or `account`),
  an installation-wide key to a principal whose scope is every account. `stats.series { key,
  subject?, from, to, resolution }` answers `points: [{ at, value, min, max, sum, count }]` for
  `raw`, `hour` or `day` over `[from, to)`, `value` being the mean of the bucket and the bucket in
  progress never synthesised. The subject of a per-account key defaults to the caller's account
  and must lie in its scope; an installation-wide key takes a subject only when its collector is
  per subject. A refused key or subject answers not found (`stats.error.key`,
  `stats.error.subject`), a period that does not end after it starts or more than 10000 points a
  validation error. The series serve the stat widgets of the dashboard and the charts of `report`
  screens ([08-frontend.md](08-frontend.md)); host metrics are sampled by the product modules
  that read them through the executor, as `hosting-services` does.

## The panel bundle

KeelPanel, the product `panel`, arrived with M5 as `products/panel` and its module crates under
`products/panel/crates/`. A module id is chosen so that its functions, tables, permission keys,
limits and i18n keys all carry its prefix, an underscore of the id reading as a dot in function
names; the crate names stay those of the plan. Every function that owns rows is owned by
account, every change to the host runs after commit or in a task through `keel-host`
([06-host.md](06-host.md)), and a row carries `state` (`pending`, `active`, `error`) with the
tool's output when the host refused it.

| Crate | Module | Feature | What it keeps |
|-------|--------|---------|---------------|
| `hosting-users` | `hosting` | `panel.users` | `hosting_user`: a hosting user is the operating-system account `hp_<name>` inside the uid range with its home; the edit form switches its shell between `nologin` and `bash`; the limit `hosting.users`; the file manager's `hosting.file.*` |
| `hosting-web` | `hosting_site` | `panel.web` | `hosting_site`: a site of a hosting user with its domain, aliases, document root inside the home and PHP version, rendered as an nginx server block and a php-fpm pool, and the key of a zone the neighbour refused after commit in `zone_error`; the limit `hosting.site.max`; `DnsProvider`, `Sites` and `SiteCertificates` in its `api`; the configuration editor's `hosting.site.config.get` and `.set`; `hosting.site.open_zone` behind the row action "Open in KeelDNS"; the service kind `hosting.sites` with `hosting_site_service` and the hourly collector `hosting.site.count`, which makes it require `stats` |
| `hosting-ssl` | `hosting_certificate` | `panel.web` | one certificate per site, issued over ACME HTTP-01 or uploaded, renewed thirty days before expiry, the problem `hosting.certificate.expiring` fourteen days before |
| `hosting-db` | `hosting_database` | `panel.databases` | MariaDB databases, users and grants per account, named `<hosting user>_<suffix>`; the limits `hosting.database.max` and `hosting.database.user.max`; the backup source `hosting_database.dumps` |
| `hosting-mail` | `hosting_mail` | `panel.mail` | mail domains and mailboxes of one system user `vmail`, rendered into the postfix maps and the dovecot passwd-file; MX and SPF through `DnsProvider::add_records`; the backup source `hosting_mail.maildirs` |
| `hosting-ftp` | `hosting_ftp` | `panel.ftp` | FTP accounts per site, chrooted under the site's document root through vsftpd and PAM; the limit `hosting.ftp.max` |
| `hosting-backup` | `hosting_backup` | `panel.backup` | per-site archives of the document root, the databases of the site's user and the mailboxes of the site's mail domain, with a schedule per site and retention |
| `hosting-services` | `hosting_service` | none | the services of the agent's policy with their state, memory and start; start, stop, restart and reload; the problem `hosting.service.down` |
| `dns-local`, `dns-peer` | `dns_local`, `dns_peer` | none | the two sides of the bridge to `dns` ([07-ecosystem.md](07-ecosystem.md)); `dns-peer` depends on `keel-client`, on the `api` of `hosting-users` and on the `notify` api, requires `hosting_site`, `hosting`, `task` and `notify`, and contributes the task `dns_peer.zone.create`, the pre-check hook on `hosting.site.create` and the problem source `dns_peer.zone.failed` |

The editions `lite` and `pro` grant every feature of the bundle alike; `pro` has none of its own
yet. The functions of `hosting-services` and the configuration editor are for the operator,
`Level::Admin` without permission keys, since a service action or an nginx directive reaches
beyond one account; the others carry `<entity>.read` and `<entity>.manage` permission keys held
by users and resellers. Suspending an account locks its hosting users, database users and FTP
logins, answers `403` on its sites and marks its mailboxes `nologin` while they keep receiving;
resuming undoes it; deleting it removes, through the `AccountLifecycle` of each module, its FTP
accounts, certificates, sites, databases, mail and backups before its hosting users. The two
custom views, the file manager and the configuration editor, are in
[08-frontend.md](08-frontend.md).

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
    r.stream(logs::log(), logs::tail);
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
        Edition { id: "pro", features: &["dns.zones", "dns.dnssec"] },
    ],
    account_kinds: &["user"],
    i18n: &[("en", include_str!("../i18n/en.json")), ("ru", include_str!("../i18n/ru.json"))],
    assets: &[Asset { id: "product.dns.logo", content_type: "image/svg+xml", bytes: include_bytes!("../brand/logo.svg") }],
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

The panel bundle differs in its bridge, `bridges: &[Bridge { to: "dns", local:
&keel_dns_local::MODULE, remote: &keel_dns_peer::MODULE }]`, which is how `DnsProvider` is served
in-process when `dns` is enabled on the same installation and through a peer otherwise
([07-ecosystem.md](07-ecosystem.md)); the field joined `ProductManifest` with that bundle in M5. The ids of the `dns-records` and `dns-templates`
modules above are illustrative. The composition lists the system modules, which no bundle
names, and every first-party bundle; each bundle names its own module crates, `keel-host`
included where a product manages the host, so `.module()` never repeats what a manifest says.
The embedded frontend is the one shell application built in the repository
([08-frontend.md](08-frontend.md)); since M1 `main.rs` composes `auth`, `settings` and `audit`
and the `demo` bundle behind the `demo` feature, and since M2 the shell through
`embed_frontend!`. The function descriptor this module produces and the envelope of a call are in
[10-contracts.md](10-contracts.md).
