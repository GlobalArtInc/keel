# Modules, the SDK and system modules

This document specifies the unit of composition in Keel: what a module is, how a product assembles
modules into one binary, what happens between process start and the first served request, what a
module registers and contributes, the isolation rules the toolchain enforces, the three planes on
which compatibility is versioned, the tooling around modules, and the catalogue of system modules
shipped in `keel-system`. The dispatcher that runs registered functions is specified in
[02-functions.md](02-functions.md), accounts and scoping in [03-access.md](03-access.md), the
storage rules a module's tables follow in [04-storage.md](04-storage.md), and the layers the crates
fit into in [01-overview.md](01-overview.md).

## Module definition

A module is a Rust crate that exports one static value,
`pub static MODULE: ModuleDef { id, version, sdk, requires, migrations, init }`. `ModuleDef` is a
plain struct with static slices; there is no trait to implement and no macro to invoke.

| Field        | Meaning |
|--------------|---------|
| `id`         | Module identifier; the prefix of every table, function, permission key, setting and i18n key the module owns. |
| `version`    | Crate version, `env!("CARGO_PKG_VERSION")`. |
| `sdk`        | The `keel_sdk::VERSION` the crate was compiled against; compared with the kernel's at boot. |
| `requires`   | Ids of the modules this one depends on. The list fixes migration order, which `api` modules may be imported and which foreign keys are allowed. |
| `migrations` | Optional embedded migration directory (`migrator::embed!`), applied under the module's own ledger `keel_migration_<module>`. |
| `init`       | `fn(&mut Registrar) -> keel::Result<()>`; registration only, no I/O. |

### Product composition

A product is a bin crate: a manifest, an explicit list of modules, a brand and editions. The list
is an explicit builder in `main.rs` (see the example at the end of this document), so the
composition is visible in one place and checked by the compiler. Self-registration through an
`inventory`-style crate is not used: in a static musl build with LTO a registrar crate that nothing
references silently drops out of the binary.

The order of `.module()` calls means nothing. The kernel builds a graph from `requires`, sorts it
topologically and fails at start with a list of cycles, duplicate ids and missing dependencies. A
product depends only on the facade crate `keel` (and on `@globalart/keel` for the frontend), which
is what lets it leave the monorepo later without changes ([01-overview.md](01-overview.md)).

## Lifecycle

Between process start and the first served request the kernel runs these steps, in this order:

1. Validation: `sdk == keel_sdk::VERSION` for every module, uniqueness of `id`, resolvability of
   `requires`; the topological order is fixed here.
2. Migrations: the kernel's own first, then every module in graph order, before any port is
   opened and under a session lock ([04-storage.md](04-storage.md)).
3. `init(&mut Registrar)` for every module in graph order: registration only, no I/O.
4. Freeze of the registries. The kernel checks that every required service has a provider,
   applies patches to other modules' descriptors (a patch cannot weaken `access`; a field added
   by a patch must carry `store`), compiles condition expressions and verifies that every field
   name they mention exists, validates descriptors and checks that i18n keys exist at least for
   `en`.
5. `start(ctx)`: background work is spawned through `ctx.spawn`, then transports open. `stop`
   runs in reverse order.

There is deliberately no dynamic loading after boot: registries are immutable and every check
runs at startup, so a typo in a condition or a missing translation cannot reach a deployed
instance. Installing a plugin ends with `request_restart`.

## What a module registers

Everything a module adds to the product goes through `Registrar` inside `init`:

- Functions, built with `keel::function::<Params, Result>("<module>.<entity>.<verb>")` and
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
- Menu nodes, `r.menu(MenuNode::new("dns/zones", "dns.zone.list").icon("globe").order(10))`.
- i18n dictionaries per language, `r.i18n("en", ...)`; key names follow the convention in
  [08-frontend.md](08-frontend.md).
- Permission keys with their default levels, `r.permission("dns.zone.manage", &[Level::User])`.
- Hooks on functions of other modules. `before` runs outside the transaction with a timeout, may
  only read, enrich `params` or veto, and is the kind available to plugins. `within` runs inside the
  transaction without a timeout and is available only to modules compiled into the binary;
  invariant checks such as quotas and limits belong there and nowhere else. A nested
  `call.invoke` reuses the transaction of the call and does not run plugin hooks; plugins see
  events after commit.
- Patches to other modules' descriptors, within the limits checked at freeze; the `store`
  mechanics of an added field are in [10-contracts.md](10-contracts.md).
- Services, `r.provide::<dyn Trait>(Arc::new(...))`, consumed elsewhere with
  `call.require::<dyn Trait>()`. Which module provides an implementation is a product
  composition choice: `hosting-web` declares `DnsProvider`, and either `dns-local` or `dns-peer`
  provides it ([07-ecosystem.md](07-ecosystem.md)).
- Contributions, `r.contribute(T)`, into extension points that modules themselves declare in
  their `pub mod api` and read back with `ctx.contributions::<T>()`. The kernel knows nothing
  about any of them; it only provides the typed bag.
- `r.invoker` is reserved in the kernel from M0 for the cluster milestone: an `Invoker` selects
  the node a function runs on by a function attribute ([06-host.md](06-host.md)).

### Contribution points

| Type | What is contributed | Consumed by |
|------|---------------------|-------------|
| `SettingDef` | A product setting: key, JSON Schema, default, level; rendered as an automatic form | `settings` |
| `CronJob` | A periodic job: id, schedule, handler | `cron` |
| `DashboardWidget` | A widget for the `dashboard` block of `GET /api/v1/boot` | the shell ([08-frontend.md](08-frontend.md)) |
| `ProblemSource` | A source of problems for the problem registry | `notify` |
| `MailTemplate` | A minijinja template for outgoing mail | `notify` |
| `ConfigTemplate` | A configuration file template with its validate command, overridable in `/etc/keel/<product>/templates/` | `keel-host` ([06-host.md](06-host.md)) |
| `AgentOp` | A typed host-agent operation: argument and result schemas, handler, policy class | `keel-host` ([06-host.md](06-host.md)) |
| `BackupSource` | The module's own data in a backup archive | `backup` |
| `ConfigSection` | The module's own section of `config.yaml` | configuration loading ([09-operations.md](09-operations.md)) |
| `PermissionSource` | Fine-grained permission keys and their defaults | `auth` |
| `ManifestPatch` | A patch of the product manifest, for example white-label branding by hostname | `settings` |
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
cookies. What module code sees is `ModuleDef`, `Registrar` and `Call<P>` with `params()`,
`principal()`, `tx()`, `invoke`, `emit`, `after_commit`, `require::<dyn Trait>()`, `check_limit`
and `extra` (values of patched fields); `Principal`, `Account` and `Level`; `KeelError` with an
i18n key, an optional field and a source; `ListQuery`, `Page` and `ListSource`; `MenuNode`;
`Secret`; `Id`; the `Owned` trait with `scope_by_account`, `resolve_account`, `LimitDef` and
`AccountLifecycle`. The function builder is specified in [02-functions.md](02-functions.md), the
account helpers in [03-access.md](03-access.md), `Secret` and `Id` in
[04-storage.md](04-storage.md).

## keel-system

System modules live in one crate, `keel-system`, with a cargo feature per module. The crate
depends only on `keel-sdk`, which cargo verifies; the kernel imports neither `keel-system` nor
`keel-host`, which CI verifies on the cargo dependency graph. Layer L3 explicitly excludes
operations on the OS: `update` talks to the root unit, not to `keel-host`.

CI runs a matrix over the crate: every module built alone, and the crate built without each
module in turn, plus the test that a product composed of the bare kernel starts and answers on the
control socket. This is what keeps every system module replaceable by product composition, for
example SSO in place of `auth`, or a different `notify` for billing.

### Hidden hooks

Hooks needed only by system modules, `Authenticator`, `TransportRegistry`, `StorageSnapshot` and
`OutboxSubscriber`, live in `keel_sdk::system`, marked `#[doc(hidden)]` and exempt from semver
until 1.0. They are the seams behind which authentication, the transports, backup and the outbox
consumers plug in; a product module has no reason to touch them. The public `keel-sdk` is frozen
at 1.0 (M7); until then only `keel_sdk::system` is exempt from semver.

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
| `keel-conformance` | L7 | Contract suite against a running binary of any product |
| `keel-client` | L4 | Rust client for calling a peer |

`keel-testkit` boots a module with `Harness::new().module(&M).boot()`. Every test gets a
temporary Postgres database created from a template with `CREATE DATABASE`; CI provides the server
through testcontainers. The harness offers `call_as(Level::Reseller, "dns.zone.list", json!({}))`,
`FakeHost` in place of the host agent, a fixed clock and snapshots of descriptors as projected for
each level. The module template ships two standard tests: "a reseller sees only its subtree" and
"a user cannot edit another account's row by id".

`keel-dev` provides `new-module` and `new-product`, `migrate create`, `descriptors dump | check |
diff`, `types` (TypeScript from the schemas), `i18n extract | pull | missing`, `check` and `docs`.
`check` lints prefixes, foreign keys, imports, `.owned()` / `.unowned()` on every function with a
key, additivity of migrations and `ldd` cleanliness of the binary. `docs` generates the catalogue
of functions, permissions, events and settings from the registry.

`keel-conformance` runs against a started binary of any product: boot, auth, the list contract,
errors, idempotency, webhooks and `svc.*`, plus the check that a field shown by the descriptor is
accepted by the server. The skeleton exists from M1 and is a mandatory CI job of every product.

`keel-client` is the Rust client of a peer, typed with `keel-types`; it retries only requests that
carry an `Idempotency-Key`. Other languages use the OpenAPI document generated from the function
registry.

## System modules

A subsystem belongs to the kernel only if the product does not start and does not answer on the
local socket without it, and if it cannot be replaced by product composition without touching its
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
- The session cookie is `__Host-keel_<product>_session`, which is why the panel listens on TLS
  only from its first start ([09-operations.md](09-operations.md)).
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
  ([04-storage.md](04-storage.md)); task logs live under `/var/lib/keel/<product>/tasks/`.
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

- The guard on `access.feature`: a function whose feature is not in the edition is hidden from
  the menu and answers 402.
- The lease is an Ed25519-signed JSON document bound to `instance_id`, with a grace period and
  degradation to read-only (mutating functions answer 402; reads and login keep working).
- Monotonic time in `keel_kv` protects against clock changes.
- `license: None` switches the mechanism off, which is how internal builds run without a key.
  Until M5 the feature guard runs without a lease.
- The edition ceiling for account limits comes from here ([03-access.md](03-access.md)).
- The first releases are expected to run on long offline leases issued by hand; when the license
  server is written as a product on Keel and where its signing key is kept are open questions in
  [11-decisions.md](11-decisions.md).

### update and backup

- The kernel writes `update.request`; the root unit can do exactly one thing, install signed
  version X. Anti-rollback uses the minimum version in the channel manifest. Before an update the
  queue drains and a logical dump of the database is taken.
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

## Example: module and product

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
    r.permission("dns.zone.read", &[Level::User]).permission("dns.zone.manage", &[Level::User]);
    r.within("keel.account.delete", hooks::forbid_delete_with_zones);
    r.contribute(LimitDef::new("dns.zone.max", Unit::Count).default(50).unlimited(true));
    r.contribute(AccountLifecycle { on_delete: Some(lifecycle::drop_zones), ..Default::default() });
    r.contribute(SettingDef::new("dns.default_ttl", schema_for!(u32)).default(3600).level(Level::Admin));
    r.contribute(CronJob::new("dns.serial_check", "0 */6 * * *", jobs::serial_check));
    r.contribute(ConfigTemplate::new("bind/zone.j2", include_str!("../templates/zone.j2")).validate("named-checkzone"));
    r.contribute(BackupSource::new("dns.zonefiles", backup::zonefiles));
    r.provide::<dyn DnsProvider>(Arc::new(BindProvider::new()));
    Ok(())
}

// products/dns/src/main.rs
fn main() -> keel::ExitCode {
    keel::Product::new(&keel_dns::MANIFEST)
        .module(&keel_system::auth::MODULE).module(&keel_system::settings::MODULE).module(&keel_system::audit::MODULE)
        .module(&keel_system::tasks::MODULE).module(&keel_system::cron::MODULE).module(&keel_system::notify::MODULE)
        .module(&keel_system::license::MODULE).module(&keel_system::update::MODULE).module(&keel_system::backup::MODULE)
        .module(&keel_host::MODULE)
        .module(&dns_zones::MODULE).module(&dns_records::MODULE).module(&dns_templates::MODULE)
        .frontend(keel::embed_frontend!("../../frontend/dist/dns"))
        .run()
}
```

The product composes the system modules it needs, the host library and its own module crates
(`dns-zones`, `dns-records`, `dns-templates`); the embedded frontend is either the product's own
build or the prebuilt `@globalart/keel-shell-app` ([08-frontend.md](08-frontend.md)). The
function descriptor this module produces and the envelope of a call are in
[10-contracts.md](10-contracts.md).
