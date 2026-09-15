# Plan

This document orders the work: the milestones M0 to M8 with their scope and exit criteria, the
breakdowns of M0, M1 and M2 into PR-sized steps, the working rules of the repository, and the M0,
M1 and M2 exit checklists. The design itself lives in `design/`; this document only says in which
order it is built and how each step is shown to be done. Nothing here carries a date or a
duration.

## Milestones

Milestones are strictly sequential, and each one ends in a demonstrable state rather than a
percentage. Three ordering rules follow from the risk analysis in
[design/11-decisions.md](design/11-decisions.md):

- the riskiest hypothesis, an unprivileged kernel driving the host through a root agent, is
  tested by a thin spike in M2, before any product depends on it;
- no peers, plugins or cluster work starts before a product has shipped;
- the share of custom views is measured in CI from M2, so the ceiling of metadata-driven
  screens stays visible.

### M0 — Walking skeleton of the kernel

Scope:

- `keel-types`: descriptors, envelope, `ListQuery`/`Page`, `Principal`/`Account`/`Level`,
  `expr`, `check`, golden fixtures.
- `keel-sdk` with the function builder.
- `keel-kernel`: config, boot, module loader, the full dispatcher pipeline with `OwnedGuard`,
  Postgres and sqlmig with a ledger per module, the eight `keel_*` tables,
  `keel.account.*` / `keel.principal.*`, the control socket with peer credentials, HTTP
  `/healthz`, `/readyz`, `/api/v1/boot`, `/api/v1/functions`, `/api/v1/call`, `/api/v1/i18n`.
- `keel-testkit`.
- `bin/keel` with the composition; `products/demo` as a lib crate with the `note` module and the
  `demo` bundle behind the cargo feature `demo`; `keel product list|enable|disable`.
- Seven crates, plus the probe module `keel-sdk-probe` of step 3. No TLS, no metrics, no macros.

The M0 exit checklist at the end of this document records what the built M0 delivers.

Done when:

- `keel ctl note.edit title=x` and `POST /api/v1/call/note.list` return the envelope with the
  `demo` bundle enabled;
- descriptors match the committed snapshot;
- the tests "a user neither sees nor edits another account's notes" and "a reseller sees its
  subtree" pass;
- a nested `invoke` reuses the transaction of the call;
- the binary starts without `config.yaml` against the default database `keel`;
- `keel serve` with no bundle enabled starts and answers `keel.account.list` on the socket;
- `keel product disable demo` leaves the `note_` table in place, flips `note` to `orphaned` and
  makes `note.list` answer `not_found`;
- `fmt`, `clippy -D warnings` and `test` are green.

### M1 — Identity, access and audit

Scope:

- `auth`: argon2id, opaque sessions, `__Host-` cookie with CSRF, Bearer tokens, TOTP, tokens
  with scope and IP restrictions, `su`, login keys, IP rules, rate limiting, session list,
  password policy, lockout.
- `settings`, `audit`.
- Skeleton of `keel-conformance`: boot, lists, errors.
- Postgres through testcontainers in `keel-testkit` when `KEEL_TEST_DATABASE_URL` names no server,
  proved by CI in a step of its own; CI keeps its Postgres service for the checks that need a
  server outliving one process.
- `ConfigSection`.

Done when:

- over HTTP: login, then boot, then `keel.account.list`, then `auth.su`, after which the list is
  narrowed and `actor` is filled, then `su.exit`;
- a token scoped to one function receives `403` on another;
- writing a field above the caller's level yields `forbidden_field`;
- the audit trail contains both sides of `su` and masks the password, including for failed
  calls;
- the conformance skeleton is green against the built binary.

### M2 — Frontend and the host spike

Scope:

- `@globalart/keel` entry points `/meta`, `/client`, `/forms`, `/renderer` (list, form, action,
  wizard, tabs), `/shell`, `/testing`; the one shell application `frontend/apps/shell` with
  `/boot.products`, the brand rule and product headings; `/boot` fixtures with no, one and two
  products; embedding through rust-embed in `bin/keel`; Storybook with fixtures; phoenix-ui
  batch 1; the OwnLate to `PHX_LABELS` bridge; publication of `@globalart/keel` to Nexus.
- In parallel, a thin agent spike in a separate bin crate `bin/spike`: unix socket with
  `SO_PEERCRED`, `host.user.create`, `host.template.apply` with validate and rollback on nginx.
- The server halves of two shell screens, deferred from M1: white-label by hostname through
  `ManifestSource` and its `ManifestPatch` in `settings` (`settings_brand`, `settings.brand.*`),
  served in `/boot.brand` next to the `/brand/logo.svg` route the embedded frontend brings;
  stored permission grants in `auth` through `PermissionSource` (`auth_grant`, `auth.grant.*`),
  overlaid on the principal as `grants` and behind the shell's permissions screen.

Done when:

- the `demo` bundle, enabled on the `keel` binary, is driven entirely from the browser without
  product-specific TypeScript: a list with search, sorting, filters and bulk delete with
  confirmation; a form with pages, `show_if`, `recompute` and a field-level error; a wizard;
- one screen is replaced by a custom view through `provideKeelViews`;
- Storybook renders the shell from `/boot` fixtures with one product and with two, showing the
  product's brand in the first case and the platform brand with a heading per product in the
  second;
- `en` and `ru` switch;
- on a VM the spike, running as an unprivileged uid, creates an OS user and a vhost through the
  agent, and an invalid template is rolled back;
- the binary with the embedded frontend is under 40 MB.

### M3 — Background work and feedback

Scope:

- `tasks`, `cron`, `notify` (in-app, problem registry, email on lettre, templates), `webhooks`.
- SSE in the shell: progress modal, notification centre, filtering by scope, `Last-Event-ID`.
- `ui.result = task`; `keel.export` with the row threshold, and the export button of the list
  renderer; group editing through `ui.multi` with the `{ ids, fields, mask }` request,
  `OwnedGuard` over `ids` element by element, and the multi-row form from the selection bar; a
  batch above `batch_threshold` becomes a task.
- Self-signed TLS and `trusted_proxies`.
- phoenix-ui batch 2.
- The OwnLate projects for the core and `demo`, `keel-dev i18n pull` at build time and
  `keel-dev i18n missing` in CI, deferred from M2.

Done when:

- `note.export` runs as a task; progress and log are visible in the browser and in
  `ctl task.get`;
- cancellation works;
- a task with a checkpoint survives `systemctl restart`;
- a cron job raises a problem, the problem is visible on the dashboard and reaches MailHog as an
  email in the end-to-end test;
- a user receives `data.invalidate` only for their own objects.

### M4 — Operator plane and KeelDNS alpha

Scope:

- The operator plane in full: `install.sh --product <id>` with its clean-host and container
  checks, the directories and units of the single installation with hardening, the unix user
  `keel` and the installation's uid range, `privileged` with the update policy (check, apply,
  rollback, a pre-update dump of the database, anti-rollback), `backup` and `restore` with
  `--with-secret-key`, `doctor`, `support-bundle`; `keel product enable|disable` on a running
  installation through `keel.product.*` and a restart, with the lease check stubbed until M5.
- `keel-host` in full: users, files, services, templates, packages, `AgentOp`, the `host`
  module.
- The feature guard without a lease.
- The bundle `dns`: `dns-zones`, `dns-records`, `dns-templates` on bind, account limits, an
  OwnLate project.
- The release workflow on the tag `keel-vX.Y.Z`: the musl build, `hack/release.py` signing the
  manifest, publication to the `keel-releases` repository of Nexus behind `dl.globalart.hu`
  ([design/09-operations.md](design/09-operations.md), the release channel), the Nexus role and
  deploy account, the route.
- The first dependency rehearsal: `bin/keel` built with the `dns` module crates pulled from a
  git tag instead of the workspace.

Done when:

- on a clean Debian or Ubuntu, `curl | sh -s -- --product dns` installs Keel with `dns` enabled,
  and no process other than `privileged` runs as root;
- `keel product disable dns` leaves the `dns_*` tables orphaned and `enable` brings them back
  without re-running applied migrations;
- a zone is created from the UI, from the CLI and with an API token, and bind serves it;
- a broken template is rolled back without downtime;
- the update 0.1 to 0.2 runs from a button and rolls back without losing the database;
- a backup taken on one VM restores on another with 2FA still working;
- an operation outside `file_roots` is refused by the agent and lands in the audit trail.

### M5 — KeelPanel and the commercial layer

Scope:

- `license`: Ed25519 lease listing products and editions, trial, grace and read-only
  degradation, monotonic time; a product the lease does not list cannot be enabled.
- `stats`.
- Renderers `report`, `dashboard`, `tree`, `master-detail`, `stream`.
- phoenix-ui batch 3: code editor, log view, key-value field, transfer list.
- The bundle `panel`: `hosting-users`, `hosting-web` (nginx, php-fpm), `hosting-mail`,
  `hosting-db`, `hosting-ftp`, `hosting-ssl` (ACME), `hosting-backup`, `hosting-services`;
  `dns-local` and `dns-peer` as the two sides of its bridge to `dns`; the file manager and the
  configuration editor as custom views in `frontend/products/panel`;
  `install.sh --product panel`.

The size cap of the release binary is set at the start of this milestone.

Done when:

- `install.sh --product panel` on a clean host installs Keel with `panel` alone;
  `keel product enable panel` on a host running `dns` yields one login that sees both products,
  with the platform brand and a group per product in the menu;
- a site with a Let's Encrypt certificate is set up by the wizard and is served;
- a site created in `panel` gets its zone through `dns-local` in the same installation, under
  the same account and in the same transaction, and a DNS error reaches the hosting form as a
  key on a field;
- there are exactly two custom views;
- with the license server unreachable the installation works until `expires`, then in grace,
  then read-only, verified on a shifted clock;
- a lease that lists `dns` alone refuses `keel product enable panel` with `license`;
- a function with `feature: dns.dnssec` is hidden from the menu of a lease that licenses `dns`
  as `lite` and answers `402`;
- the share of custom views is at most 10 % of the functions of `panel`;
- `support-bundle` contains no secrets.

### M6 — Ecosystem: peers, events, services

Scope:

- `peers`: handshake, pairing by one-time code, mutual `kpt_` tokens, scopes, on-behalf bound
  to `origin`, TOFU, rotation.
- Peer delivery on top of the `webhooks` module from M3: the subscription created at pairing,
  the pull feed for instances behind NAT, the inbox that deduplicates by event id.
- `Idempotency-Key` end to end.
- The `svc` module, `keel-client`, the full `keel-conformance`.
- `keel.principal.upsert` by `origin` / `external_id` and the `keel.account.*` events.
- `panel` on one host to `dns` on another host through `dns-peer`.

Done when:

- two installations on two hosts are paired by code;
- creating a site in `panel` on one host creates a zone in `dns` on the other on behalf of the
  client, and a DNS error arrives in the hosting form as a key on a field;
- the `dns` installation opens from the `panel` installation by a one-time key, with `actor` in
  the audit trail;
- `svc.dns.zones.open` called twice with the same key produces one service;
- an event arrives signed, and a repeat creates no duplicate;
- conformance is green against an installation with `dns` alone, with `panel` alone and with
  both enabled;
- revoking a token breaks its calls with `401`.

### M7 — Plugins, SDK 1.0, products in their own repositories

Scope:

- `plugins`: protocol 1, `daemon` and `exec` modes, the rights `observe`, `hook` and `patch`,
  the kv store, a separate uid, timeouts.
- A Python SDK and a sample plugin.
- Stabilisation of `keel-sdk` 1.0 and descriptor v1: CHANGELOG, semver policy.
- Product module crates may move to their own repositories and are pulled into the `keel` build
  as dependencies pinned here; the binary is still built in the keel repository; whether their
  frontend libraries move with them is decided then.
- A load test of lists at 100k rows.
- Security review.
- Operator and developer documentation.

Done when:

- a bash plugin adds a field to the zone form, vetoes a deletion with a field-level error and
  shows its own list in the menu, all without a rebuild;
- a plugin hook on `auth.*` is rejected at start;
- a crash of a daemon plugin does not take the kernel down;
- the tag `keel-v1.0.0` exists, `bin/keel` builds with a product module crate pulled from its
  own repository by tag, and the release binary carries `dns` and `panel` at that version;
- a list of 100k zones answers in under 300 ms;
- the security checklist is closed;
- the first external operator installs Keel with `panel` enabled from the documentation without
  the author's help;
- the vocabulary audit finds no borrowed product names or identifiers anywhere in the
  repository.

### M8 — Cluster of nodes

Scope:

- The remote executor: the agent protocol over mTLS gRPC (tonic, protox without protoc).
- The kernel's PKI on rcgen; `agent enroll` with a one-time token; the connection is initiated
  by the agent.
- `keel-system::cluster` with the node registry and the `Invoker` that picks a node by a
  function attribute; drain and status.
- Automatic update of agents.
- The problem "node unavailable".

Done when:

- `dns` manages a secondary server on a remote node with the same module code;
- a node going offline raises a problem and its functions answer `503`;
- a repeated enrol with a used token is rejected;
- a kernel update updates the node agents.

## M0 work breakdown

M0 is split into thirteen steps. Each step is one PR-sized change to `main`, ordered by
dependency; a step is done when its acceptance holds in CI. The steps derive strictly from the
design; where a step names a mechanism the design leaves open, it says so.

Two remarks apply to the whole breakdown.

- Until step 11 exists, tests inside `keel-kernel` use the crate's own test helpers; step 11
  turns those helpers into the public harness `keel-testkit`.
- The design places authentication in the `auth` module, which arrives in M1. M0 therefore has
  two callers: the control socket, where peer credentials yield the owner principal, and HTTP,
  where no authenticator is registered and every request is a guest. How HTTP is exercised as a
  non-guest before M1 is a test-only arrangement described in steps 8 and 11; the design does not
  fix it.

### 1. Repository scaffolding and CI

**Goal.** Turn the empty repository into a workspace that CI builds and tests. The cargo
workspace declares `members = ["crates/*", "products/*"]`; the toolchain is pinned by
`rust-toolchain.toml`. The facade crate `crates/keel` is created empty so that
`cargo-semver-checks` runs on it from the first commit. `.github/workflows/ci.yml` runs
`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo test --workspace` against a Postgres service of version 15 or newer, whose connection
string reaches the tests through the environment. `hack/check-deps.sh` asserts from the cargo
graph that `keel-kernel` depends on neither `keel-system` nor `keel-host`. `contracts/` gets its
directory skeleton for the fixtures of step 2. Step 12 extends the workspace members with
`bin/*`.

**Touches.** `Cargo.toml`, `crates/keel/`, `.github/workflows/ci.yml`, `hack/check-deps.sh`,
`contracts/`.

**Out of scope.** The musl target and the `ldd` check, the release workflow, npm publishing,
the frontend workspace, `keel-dev`, `keel-conformance`, testcontainers (M1 adds them to the
testkit for a run without a named server; the service container stays for the checks that need a
server outliving one process).

**Acceptance.**

- CI runs the three commands on the workspace and is green.
- The workflow waits for the Postgres service with `pg_isready` and fails if it does not come up.
- `hack/check-deps.sh` runs in CI and exits 0.
- `cargo-semver-checks` runs against `crates/keel` in CI.

### 2. keel-types

**Goal.** Fix the wire contract as plain types with no I/O, as
[design/10-contracts.md](design/10-contracts.md) specifies: `PROTOCOL = 1`; the function
descriptor (`"keel": 1`, `name`, `module`, `kind`, `mode`, `access`, `key`, `params` as JSON
Schema, `result`, `ui`); the closed vocabulary as enums — `kind`, `control`, `column.type`,
`action.role`, the `check` presets — and the names of the `x-keel` and `x-ui` attribute spaces;
the `ui` block (columns, actions, pages, `show_if` / `required_if` / `readonly_if`, `recompute`,
`invalidate_on`, `multi`, `empty`, `search`); the `Level` ladder
`guest < service < user < reseller < admin < owner` as an ordered enum; `Principal` with its
optional `actor`, `Account`; the envelope and the error with `fields[]`; the error code list
with its HTTP status mapping; `ListQuery`, `Sort`, `Filter`, `Page<T>`; the event shape
(`instanceId`, `module`, no product); the condition AST (`eq`, `ne`, `in`, `gt`, `lt`, `truthy`,
`empty`, `matches`, `and`, `or`, `not`; variables are fields, `$mode`, `$principal.level`,
`$screen.params.*`), its parser from the mini-language and its interpreter; the `check` preset
evaluator; the ULID identifier type. The types derive `JsonSchema` so that M2 can generate
TypeScript from them. Golden fixtures land in `contracts/fixtures/`: `expr-cases.json`, check
cases, the `dns.zone.list` descriptor from the contracts document, envelope examples for `200`,
`422` and `403`, a `ListQuery` and a `Page`. The implemented step carries `product` in the event
shape and in `contracts/fixtures/event.json`; step 10 renames it to `module`.

**Touches.** `crates/types/`, `contracts/fixtures/`.

**Out of scope.** TypeScript generation and OpenAPI (M2), the request and response shapes of
form dynamics (`options`, `recompute`, group editing, wizard steps, `download` / `upload` /
`stream`), which are added to `keel-types` when M2 implements them, the plugin manifest (M7),
the agent protocol (M2 spike, M4).

**Acceptance.**

- `cargo tree -p keel-types` shows no async runtime, no database driver and no HTTP crate.
- Every file under `contracts/fixtures/` is loaded by a test: each expression case parses and
  evaluates to its expected value; the descriptor fixture deserialises and re-serialises without
  loss; the envelope fixtures deserialise; the error code to status table matches the contracts
  document.
- An unknown value for any closed enum fails to deserialise.
- `Level` compares in ladder order.
- The `ZoneEdit` parameter struct from [design/02-functions.md](design/02-functions.md) compiles
  with its `schemars` attributes and its schema carries `x-keel` and `x-ui` exactly as written.

### 3. keel-sdk

**Goal.** The public surface every module is written against, as
[design/05-modules.md](design/05-modules.md) describes. `ModuleDef { id, version, sdk, requires,
migrations, init }` as a plain struct with static slices; `keel_sdk::VERSION`. `Registrar` with
the registrations M0 consumes: `function`, `menu`, `i18n`, `permission`, `before`, `within`,
`patch`, `contribute`, `provide`; the registrations that need a transport or a module of a later
milestone (`stream`, raw routes, `view`) are added with that milestone. The function builder
`function::<P, R>(name)` with `.kind()`, `.mode()`, `.access(level, permission)`, `.feature()`,
`.owned::<E>()`, `.unowned()`, `.ui()`, `.handler()` and the per-function limits `timeout`,
`max_page_size`, `default_sort`, `batch_threshold`. `ScreenUi` builders (`Form`, `List`,
`Action`) that produce the `ui` block of `keel-types`. `Call<P>` with `principal()`, `params()`,
`tx()`, `emit()`, `invoke()`, `after_commit()`, `require::<dyn T>()` and `extra` for patched
fields. `KeelError` with a constructor per error code, `.field()`, `.source()`, an i18n key and
parameters. `Owned`, `scope_by_account`, the `ScopePolicy` service trait and `resolve_account`
from [design/03-access.md](design/03-access.md). `ListSource` with the sea-orm entity as its
first implementation. The `Secret` type. `keel_sdk::system`, `#[doc(hidden)]`, with
`Authenticator`; the other system hooks arrive with the modules that use them. A prelude.

**Touches.** `crates/sdk/`, `crates/sdk-probe/`, `hack/check-deps.sh`.

**Out of scope.** Proc-macros and a derive for `ui`; `HostExecutor` (it lives in
`keel_host::api`); `LimitDef` and `check_limit` (M4); `AccountLifecycle` (M3); the contribution
types owned by system modules (`SettingDef`, `CronJob`, and the rest of the `pub mod api` list);
plugin-facing types.

**Acceptance.**

- `cargo tree -p keel-sdk` shows no axum and no cookie crate, and the public API exposes no
  sqlx or sea-orm pool type; the crate depends on `keel-types`.
- A test crate that depends only on `keel-sdk` defines a `MODULE` with one function of each
  `mode` and compiles; it is a workspace member, so every CI build proves it, and
  `hack/check-deps.sh` asserts that `keel-sdk` is its only direct dependency.
- The builder for the `dns.zone.edit` example in the functions document compiles, and the
  descriptor it produces is committed as a snapshot under `contracts/fixtures/`.
- A function built with `.owned::<E>()` records the entity's account column in its definition,
  so that step 6 can load ownership before the handler.

### 4. Kernel storage, config and kernel migrations

**Goal.** Give `keel-kernel` the layer everything else stands on, per
[design/04-storage.md](design/04-storage.md) and [design/09-operations.md](design/09-operations.md).
Configuration: `config.yaml` may be absent; the keys are `listen`, `socket`, `data_dir`,
`database`, `tls`, `trusted_proxies` and `log`; the defaults are kernel constants: `socket`
`/run/keel/core.sock`, `data_dir` `/var/lib/keel`, `database`
`postgres:///keel?host=/run/postgresql` for peer authentication under the uid `keel`, with
`${KEEL_DB_PASS}` substituted for a TCP URL, and the configuration file at
`/etc/keel/config.yaml`; `listen` keeps a provisional value while the port is open. The
connection goes through sea-orm 1.1 (sqlx-postgres, runtime-tokio-rustls): a pool of 20, connect
and acquire timeouts of 10 s, `search_path` set to the installation's schema `keel`. The
kernel's migrations are an embedded sqlmig 0.2 catalogue with the ledger `keel_migration`; they
create the eight kernel tables and insert the root account (`kind = system`, `path = /`), and
they run under a session-level `pg_advisory_lock`. On first start the kernel creates `data_dir`,
generates `secret.key` (mode 0600) and `instance.id`, and writes the instance id and the wrapped
key into `keel_kv`. Logging is initialised from the `log` key (tracing, `json` or `text`). The
implemented step carries a product id in `ManifestDefaults`, builds
`/run/keel/<product>/core.sock`, `/var/lib/keel/<product>`, `keel_<product>`,
`${<PRODUCT>_DB_PASS}` and `/etc/keel/<product>/config.yaml`, and names the schema after the
product; step 8 removed the product id and reworked these to the constants above, ahead of the
step 10 that had been planned to do it.

**Touches.** `crates/kernel/` (config, storage, migrations), `crates/kernel/migrations/`.

**Out of scope.** Running module migrations (step 5), the pre-update dump, `schema prune`,
`ConfigSection` (M1), `tls` and `trusted_proxies` beyond parsing (M3), backups (M4).

**Acceptance.**

- On a fresh database the migration creates the eight tables and the ledger; a second run
  changes nothing, and the ledger row count is identical.
- The root account row exists with `kind = system` and `path = /` and is the only account.
- A missing `config.yaml` yields the defaults; `${KEEL_DB_PASS}` is substituted in a TCP URL; an
  unusable `database` value fails start with a message naming the key.
- Two processes migrating the same database at once serialise on the advisory lock and leave one
  ledger.
- `secret.key` is created with mode 0600 and reused on the second start; the instance id is
  stable across restarts.

### 5. Module loader

**Goal.** Implement the lifecycle of [design/05-modules.md](design/05-modules.md). The
composition is an explicit builder; the order of `.module()` calls means nothing. The loader
checks `sdk == keel_sdk::VERSION`, the uniqueness of ids and the resolvability of `requires`,
builds the graph, sorts it topologically and fails at start listing every cycle, duplicate and
missing dependency. Migrations run kernel first, then modules in graph order, each module with
its own `keel_migration_<module>` ledger; `keel_module` records each module as `active`, and a
module missing from the composition becomes `orphaned` while its tables stay. `init(&mut
Registrar)` registers only. Freezing the registries checks required services, applies patches
to foreign descriptors (a patch cannot weaken `access`; an added field must carry `store`),
compiles conditions and checks the field names they use, validates descriptors and checks
that every i18n key exists at least for `en`. `start(ctx)` runs in graph order with
`ctx.spawn` for background work; `stop` runs in reverse. The typed contribution bag
`r.contribute(T)` / `ctx.contributions::<T>()`, `provide::<dyn Trait>` / `require`, and the
reserved point `r.invoker` are part of this step. Step 10 narrows the composition to the enabled
set; the acceptance here holds for the composition as a whole.

**Touches.** `crates/kernel/` (loader, registry, freeze, contributions, services).

**Out of scope.** Any loading after boot and `request_restart` (plugins, M7); transports (steps
8 and 9); `ConfigSection` (M1); the icon check against the phoenix icon set, which needs the
list generated from phoenix-ui and arrived with M2 step 11c as `contracts/icons.json`.

**Acceptance.**

- A composition with the cycle A → B → A fails at start with a message naming the cycle; a
  missing `requires` entry, a duplicate id and an `sdk` mismatch fail the same way, each named.
- Migrations run in graph order, observable in the ledgers; every module has a
  `keel_migration_<module>` ledger and a `keel_module` row with state `active`.
- Removing a module from the composition leaves its tables in place and flips its `keel_module`
  row to `orphaned` on the next start.
- `Registrar` exposes no database or network handle, so `init` cannot do I/O.
- A condition that names an unknown field fails start with the function and the field
  named; a patch that lowers `access` is rejected; a patch that adds a field without `store` is
  rejected; a function whose title key has no `en` entry fails start.
- `require::<dyn T>` of a service nobody provides fails at freeze, not at the first call.
- Modules start in graph order and stop in reverse. That tasks spawned through `ctx.spawn` are
  cancelled on stop is a plan-level expectation; the design fixes only the reverse order.

### 6. Dispatcher pipeline

**Goal.** Implement the twelve stages of [design/02-functions.md](design/02-functions.md) in the
fixed order: resolve, guards (level, permission with its per-level defaults, module guards,
`OwnedGuard` for functions with a `key`), strip and validate (schema plus `check` presets),
idempotency through `keel_idempotency`, `before` hooks outside the transaction with a timeout,
the transaction opened by the kernel and held in `Call`, `within` hooks, the handler with nested
`invoke` on the same transaction, `call.emit` into `keel_outbox`, commit or rollback, the
in-process audit event after either, and `after_commit`. `mode: read` takes one pooled
connection and skips idempotency, outbox and audit by default. The generic list execution over
`ListSource` applies `scope_by_account`, the whitelist of sortable and filterable fields from
`ui.columns`, `max_page_size`, `default_sort` and totals.

**Touches.** `crates/kernel/` (dispatcher, guards, strip, validation, idempotency, outbox,
audit event, list execution).

**Out of scope.** The `audit` module (M1) — only the in-process event exists; outbox delivery
(M3, M6); the token scope guard (M1); the feature guard (M4); `check_limit` (M4); the batch
threshold turning a call into a task, `keel.export` and rate limiting (M3).

**Acceptance.**

- Guards run in the documented order and each failure is observable by its error code.
- `OwnedGuard`: a user calling a keyed function with the id of a row in another account receives
  `not_found`; a batch that contains one foreign id receives `not_found`.
- Strip: a field with `x-keel.level` above the caller's level is absent from the result; the same
  field on write yields `403 forbidden_field` with the field path in `fields[]`.
- Validation: `422` with `fields[].path` and `check.<preset>` codes, driven by the fixture cases
  of step 2.
- Idempotency: the same key with the same body is answered from `keel_idempotency` and marked as
  a replay; the same key with a different body yields `409 idempotency_mismatch`; a read call
  ignores the key.
- A `before` hook has no transaction handle, can enrich parameters and can veto; a veto returns
  the hook's key and field in the error; the hook timeout ends the hook.
- A `within` hook runs inside the transaction and its failure rolls back the handler's writes.
- A handler that invokes another write function through `call.invoke` and then fails leaves no
  rows from either function.
- An outbox row exists exactly when the call committed.
- The audit event is raised after commit and after rollback and carries the outcome.
- `after_commit` runs only after a commit.
- A read function's `Call` has no transaction.
- Sorting by a column not marked `sortable` yields `422`; `max_page_size` caps `pageSize`;
  `default_sort` applies when `sort` is absent; a column with `total: sum` fills `totals`.

### 7. keel.account.* and keel.principal.* functions

**Goal.** Register the functions of the `keel` module through the same `Registrar` and with the
same descriptors as any other module, per [design/03-access.md](design/03-access.md):
`keel.account.list`, `.get`, `.create`, `.update`, `.options` and `keel.principal.list`,
`.get`, `.create`, `.update`, on sea-orm entities for `keel_account` and `keel_principal`.
Scoping follows `scope_by_account`: a user sees their own account, a reseller its subtree by
`path`, admin and owner everything; `resolve_account` applies to the `account` parameter.
`acl_gen` is incremented in the transaction that changes a principal's role or account, and for
the subtree of a reseller by `path`. The entities carry the `origin` and `external_id` columns
from the kernel migration of step 4, so that the M6 functions land additively.

**Touches.** `crates/kernel/` (the `keel` module: entities, functions).

**Out of scope.** `keel.account.upsert` and `keel.principal.upsert` by `(origin, external_id)`
and the `keel.account.*` / `keel.principal.*` events through the outbox (M6, where the design
places them); `keel.account.delete` as an action that runs as a task walking
`AccountLifecycle`, and `suspend` / `resume` (M3, when `tasks` exists); `keel.setup.admin`
(M4, with `install.sh` and the one-time key); N:M membership (`keel_membership`); limits (M4).

**Acceptance.**

- The descriptors of `keel.account.*` and `keel.principal.*` match committed snapshots: plain
  JSON files under `contracts/fixtures/descriptors/keel.<entity>.<verb>.json`, one per function,
  rewritten with `KEEL_UPDATE_SNAPSHOTS=1` after review and listed in `contracts/README.md`.
- `keel.account.list` as a reseller returns only its subtree; as a user, only the caller's
  account; as owner, every account including the root. The permission defaults that make this
  hold are `keel.account.read`, `keel.principal.read` and `keel.principal.manage` for
  `[user, reseller]` and `keel.account.manage` for `[reseller]`.
- Changing a principal's level, role or account increments its `acl_gen`; changing a reseller
  account, its kind or its parent included, increments `acl_gen` of every principal in its
  subtree.
- `resolve_account` on the `account` parameter: a reseller may name any account in its subtree
  and is refused outside it; a user may name only its own account. The refusal is
  `422 validation` with `keel.error.account_not_found` on the field. The account forms name
  the tenant `parent`; the rows on the wire are camelCase (`parentId`, `accountId`), the
  columns stay snake_case ([design/10-contracts.md](design/10-contracts.md)).
- A principal's level is never `guest`, never above the caller's, and fits the account's kind:
  `admin` and `owner` in the root account only, `reseller` in a `reseller` account only.

### 8. HTTP transport

**Goal.** Serve the kernel over plain HTTP with axum, per
[design/10-contracts.md](design/10-contracts.md): `GET /healthz`; `GET /readyz` (database
reachable, migrations applied, not draining); `GET /api/v1/functions` and
`GET /api/v1/functions/{name}`, projected for the principal, with
`ETag = hash(build, modules, products, lang, principal_id, acl_gen)` and `private, no-store`;
`POST /api/v1/call/{func}`, with `GET` allowed for `kind: list | get`; `GET /api/v1/i18n` for
the catalogue in the principal's language; `GET /api/v1/boot` is delivered by step 10 with the
payload it serves. Every response carries `X-Keel-Request-Id`, calls carry
`X-Keel-Meta-Version`, replays carry `X-Keel-Idempotent-Replay`. Error codes map to HTTP
statuses as the contracts document lists; the body limit is the kernel constant
`http::BODY_LIMIT`, 1 MiB, carried by `HttpOptions.body_limit`, not a configuration key. The
principal is resolved through `keel_sdk::system::Authenticator`; with no authenticator
registered every request is a guest. Requests that carry the session cookie are checked by
`Sec-Fetch-Site` / `Origin`; Bearer requests and requests without a credential are not. The
`GET` parameter grammar, the `422` answers for a wrong method, an oversized or non-JSON body,
the `{ lang, messages }` shape of the catalogue, the `m-<16 hex>` entity tag and the
`{ "status": … }` bodies of `/healthz` and `/readyz` are recorded in
[design/10-contracts.md](design/10-contracts.md).

**Touches.** `crates/kernel/` (HTTP transport, routes, status mapping, headers).

**Out of scope.** TLS and `trusted_proxies` (M3); SSE (M3); raw routes `/api/v1/x/<module>/…`;
metrics; the embedded frontend (M2); cookies and rate limiting (M1; the CSRF check itself is
in place and waits for the session cookie `auth` will set); `GET /api/v1/instance` (M6).

**Acceptance.**

- `/healthz` answers `200` as soon as the process listens; `/readyz` answers `503` before
  migrations finish and `200` after.
- `/api/v1/functions` as a guest lists only guest-level functions.
- An unknown function name yields the `404 not_found` envelope.
- `POST /api/v1/call/keel.account.list` returns the envelope: as a guest, the
  `401 unauthenticated` error envelope with `ok: false`; with the test authenticator of step 11
  mapping the credential token to a user, `200` with a `Page`. The same check against
  `note.list` is part of step 12.
- Every error code of the contract maps to the documented status in a table-driven test.
- Every response carries `X-Keel-Request-Id`; the `ETag` of a descriptor changes after the
  principal's `acl_gen` is bumped.

### 9. Control socket and ctl

**Goal.** Serve the same call protocol on the unix socket named by `socket` (default
`/run/keel/core.sock`). The kernel reads `SO_PEERCRED`; the design fixes only that the local
socket is the owner level and that `ctl` is identified by peer credentials. As a plan-level
arrangement, M0 accepts only the kernel's own uid, which becomes the owner principal of the root
account; whether root or other uids are admitted is not fixed by the design and stays open. The
`ctl` subcommand is a client of that socket: `keel ctl <func> key=value … --out json|table`,
returning the same envelope as HTTP; `--out table` renders `ui.columns` for `kind: list`,
translating keys through the catalogue of step 10. The `serve` subcommand binds the HTTP
listener, boots the kernel (`Kernel::boot`), attaches the dispatcher to the listener and opens
the socket, so `/healthz` answers during the migrations; `migrate` runs the kernel's
migrations, then those of the loaded modules, prints the applied names per module and exits;
`product list|enable|disable` reads and writes `keel_kv.products` under the `keelinst` lock
directly against the database while the service is stopped, the same arrangement as `migrate`;
`enable` and `disable` refuse to run while the socket answers (the running-installation path
through `keel.product.*` and a restart arrives in M4). Running `migrate` only while the service is
stopped is a plan-level arrangement for M0; the migrations serialise on the advisory lock of
step 4 either way. This step also adds the test that a composition of the bare kernel starts and
answers on the socket, which is the state of a fresh installation before its first
`keel product enable`. The socket line protocol, the parameter rule of `ctl`, its renderings,
`--lang`, `--socket`, the global `--config` / `KEEL_CONFIG` and the exit codes are recorded in
[design/10-contracts.md](design/10-contracts.md) and
[design/09-operations.md](design/09-operations.md).

**Touches.** `crates/kernel/` (socket transport, peer credentials, the `ctl` client and
renderer), `crates/keel/` (the facade `Runtime` with the subcommands `serve`, `ctl`, `migrate`,
`product`), `bin/keel/` (the binary, package `keel-bin`).

**Out of scope.** `--as <login>`, which is delegation and belongs to `auth.su` in M1; the full
CLI parameter grammar (repeated keys, `a.b=v`, `@file.json` versus JSON only), an open question
in [design/11-decisions.md](design/11-decisions.md) — M0 implements the `key=value` form the
exit criteria use and leaves the rest open; the subcommands `privileged`, `backup`, `restore`,
`doctor` and `support-bundle` (M4).

**Acceptance.**

- `ctl keel.account.create kind=user` returns the envelope with `ok: true` and the new id
  (accounts have no name); the `note.edit` form of the exit criteria is checked in step 12.
- A connection from a uid other than the kernel's is refused (unit test on the credential check).
- For the same call the envelope over the socket and over HTTP differ only in `meta`.
- `--out table` for `keel.account.list` renders the columns declared in `ui.columns`.
- A composition of the bare kernel with no bundle enabled starts and answers `keel.account.list`
  on the socket: the test
  `a_runtime_of_the_bare_kernel_with_no_products_starts_on_a_temporary_socket_and_answers_keel_account_list`
  in `crates/keel/tests/runtime.rs`.
- `keel product enable x` for an id the composition does not carry fails naming the id;
  `keel product list` prints every bundle the binary carries as enabled or disabled, and an
  enabled id the binary does not carry as not carried; `serve` refuses to start in that state.

### 10. i18n catalogue, product bundles, menu and /boot

**Goal.** The `ProductManifest` type in `keel-sdk`: the bundle of a product, with `id`, `name`
(the brand name key), `version` (the workspace version, `env!("CARGO_PKG_VERSION")`), `api`
(the descriptor version the bundle is written against, checked at composition), `brand`
(`logo`, `hue`, `chroma`), `modules` (the `ModuleDef`s of its modules, not their ids), `menu`
(groups with `id`, `label`, `icon`, `order`), `levels` (`LevelLabels`: labels and disabled
levels), `editions` (id and feature set; the first is the default edition), `account_kinds`
(identifiers, never `system`) and `i18n` (`(lang, json)` pairs), as
[design/05-modules.md](design/05-modules.md) lists them; `bridges` joins the type with the
first bridge in M5, and any further field (languages, update channel) is proposed at M0 and not
fixed by the design. There is no `listen`, `socket` or `data_dir` in the bundle: the defaults
are kernel constants, and `version` only repeats the binary's for `keel product list`. The
default listen port stays open, per [design/11-decisions.md](design/11-decisions.md); M0 keeps
the provisional `127.0.0.1:1500`. Step 8 had already reworked `ManifestDefaults` into the
constants of step 4 and the `Config` type, so this step only confirms them: `ManifestDefaults`
loses its product id and the paths, database, password variable, configuration path and schema
are the constants of step 4; the event shape's `product` becomes `module` with its fixture;
`HttpOptions` carries the body limit alone, neither `build` nor `products`. The runtime reads
`keel_kv.products` after the kernel migrations and before validation (no row, no bundle
enabled) and passes the ids to the loader with `Composition::enabled`, next to every compiled-in
bundle through `Composition::product` and the platform version through `Composition::version`;
a composition without `.enabled()` enables every bundle it carries, which is what the testkit
and CI use. The loader fixes the loaded set as the platform modules (those no bundle names) plus
the modules of the enabled bundles plus one side of every bridge of an enabled bundle, attributes
a module named by two enabled bundles to the first in composition order, composes the kernel's
`keel` module first without any bundle listing it, and runs validation, migrations, `init`,
freeze and `start` over that set only. The i18n catalogue merges the `r.i18n(lang, json)`
dictionaries of the loaded modules and the dictionaries of the enabled bundles, applies the key
convention of [design/08-frontend.md](design/08-frontend.md) (`<module>.<entity>.<field>`,
`.<enum>`, `.action.<id>`, `.page.<id>`, `.column.<key>`, the column values, `.hint` and
`.placeholder` only where `x-ui` names them, `<patching module>.<entity>.<field>` for patched
fields, `menu.<group>`, `product.<id>.*`, shared `keel.*` with inheritance) and the single
placeholder format `{name}`, refusing `{{`, `%s`, `{0}` and `{ name }`; freeze fails on a key
missing for `en` and on a bundle dictionary that is not a JSON object. Menu nodes registered
with `r.menu` are placed under the groups of the enabled bundles and the platform group `system`
(ordered by product, then `order`, the platform group last) and projected by level and
permission; a node whose group no enabled bundle declares, a node without a label whose function
has no screen title, and a group id declared twice fail freeze. `GET /api/v1/boot` returns
`keel { version, api }`, `products` as `{ id, name, edition, brand }` with `edition` the
bundle's default edition while no lease exists, `brand` (the single enabled product's brand,
otherwise the platform brand `keel.brand`, hue 220, chroma 0.08, no logo), `principal` (`null`
for a guest; `tz: null` and `totp: false` as placeholders until M1), `features` (the union of
the default editions), `menu` with `product` and `icon` on every group and
`{ path, label, func, kind, icon?, badge? }` items, `views`, `meta` (also the response's `ETag`,
with `private, no-store` and `304`), and the fields fed by modules absent from the composition
(`license`, `dashboard`, `problems`) as null or empty; the socket answers the same payload to
the frame `{ "boot": true }`. The `keel` module registers `keel.product.list`,
`keel.product.enable` and `keel.product.disable` (`admin`, `.unowned()`): the list answers
`{ id, name, version, state, enabledAt, enabledBy }` for every compiled-in bundle, disabled ones
included; enable and disable take `{ id }`, refuse an id the binary does not carry, and answer
`{ id, state, changed, restart }`, `restart` being their request for the restart M4 brings; the
`product` subcommand of step 9 keeps writing the database directly with the service stopped,
and the menu node `system/products` opens the list. With `license: None` the license mechanism
is off and every compiled-in bundle may be enabled. Left out at M0 and recorded for later: the
bridges and the two bridged fixture bundles of the testkit (M5), the lease (M5), the restart
(M4), the attribution of a module's `keel_module` row to its bundle, and the check of an
account's `kind` against the union of the enabled bundles' `account_kinds`, of which only the
reservation of `system` exists.

**Touches.** `crates/types/` (event shape, fixture, the `/boot` types), `crates/sdk/`
(`ProductManifest`, `Brand`, `MenuGroup`, `Edition`, `LevelLabels`), `crates/kernel/` (config
constants, schema name, enabled set in the loader, i18n, menu, boot, meta-version,
`keel.product.*`), `crates/keel/` (reading the enabled set), `contracts/fixtures/`.

**Out of scope.** `ManifestPatch` and white-label by hostname (`settings`, M2); dashboard
widgets, problems and license state (M3, M5); custom views in `views` (M2); OwnLate export at
build time and `keel-dev i18n missing` in CI (deferred from M2 to M3); the icon check against
`contracts/icons.json` (M2 step 11c); the lease check
in `keel.product.enable` (M5); the restart after enable or disable, `--switch-remote` and the
kernel learning the carried set from a running installation (M4); `bridges` (M5).

**Acceptance.**

- `/api/v1/boot` over the socket as owner with the `demo` bundle enabled returns `keel`,
  `products` with `demo`, the demo brand as `brand`, the owner principal with `account` set to
  the root account, the menu with the platform group and the demo group carrying
  `product: "demo"`, and a `meta` value; with no bundle enabled `products` is empty, `brand` is
  the platform brand and the menu has the platform group only; as a guest no entry above the
  guest level is returned.
- With `demo` disabled, `note.*` is absent from `/api/v1/functions`, no `note_` table and no
  `keel_migration_note` ledger exist and `keel_module` has no `note` row; after
  `keel product enable demo` and a start they exist; after `keel product disable demo` and a
  start the row is `orphaned` and the tables stay; enabling again returns it to `active` without
  re-running applied migrations.
- An enabled id no bundle carries fails start naming it and the known ids; a module whose
  `requires` names a module outside the loaded set fails start naming the module and the
  bundle; a bundle whose `api` is not the descriptor version fails start naming the product; a
  menu node whose group no enabled bundle declares fails freeze; a group id declared by two
  enabled bundles fails freeze.
- A missing `config.yaml` yields `/run/keel/core.sock`, `/var/lib/keel` and
  `postgres:///keel?host=/run/postgresql`; the schema is `keel`; `${KEEL_DB_PASS}` is the only
  substitution.
- A function registered without an `en` title key fails start with the key named.
- `/api/v1/i18n` returns the merged catalogue of the loaded modules and enabled bundles for the
  requested language and contains no key of a disabled bundle; a key missing in `ru` falls back
  to `en`.
- `meta` changes when the enabled set changes and when the principal's `acl_gen` changes.
- The bridge acceptance — with two fixture bundles joined by a bridge, the `local` module loads
  when the bridged product is enabled and the `remote` one otherwise, and a module named by both
  has one `keel_module` row — moves to M5 with `bridges`.

### 11. keel-testkit

**Goal.** The public test harness of [design/05-modules.md](design/05-modules.md):
`Harness::new().module(&M).product(&MANIFEST).boot()`; a temporary Postgres database per test
(`keel_testkit::TestDatabase`), created with `CREATE DATABASE` from a template and dropped
afterwards, with the server taken from `KEEL_TEST_DATABASE_URL`; `call_as(Level, "func",
json!{})` in process, which picks the fixture of that level (`A1` for `user`), and
`call(Who, ..)` for a named one; the fixture principals `Who::{Guest, Service, Owner, Admin,
Reseller, A1, A2, Unrelated}` over a fixture account tree (root, a reseller with a subtree, a
user, an unrelated account); a clock frozen at boot; descriptor snapshots per level, one file
per function and level with the kernel's functions excluded; the two standard tests as reusable
helpers, `assert_reseller_sees_only_subtree` and `assert_user_cannot_edit_foreign_row`, driven
by a `Fixture` that creates a row in an account. For HTTP tests the harness registers a test
`Authenticator` that maps the credential token, sent as the header `X-Keel-Test-Principal`, to
a fixture principal through a testkit middleware; this is a test-only arrangement, not part of
the design's authentication model. `harness.socket()` and `TestSocket::at(path)` drive a
control socket with `call`, `boot`, `ctl_json` and `ctl_table`. The harness links `keel-kernel`
to boot the module in process, which is why it is the one L2 crate with a kernel dependency.
`.product(&MANIFEST)` boots a bundle with the modules it names; the two fixture bundles joined
by a bridge follow `bridges` in M5.

**Touches.** `crates/testkit/`, `crates/sdk-probe/tests/harness.rs` (the module template's
test), `hack/check-test-databases.sh`, `.github/workflows/ci.yml`.

**Out of scope.** `FakeHost` (M2 spike, M4); testcontainers (M1); Playwright and the frontend
`/testing` entry point (M2); bridged fixture bundles (M5).

**Acceptance.**

- A module test that depends only on `keel-testkit` and `keel-sdk` boots a harness with one
  module against Postgres in CI: `crates/sdk-probe/tests/harness.rs`.
- Every test gets its own database, and no database remains after the test run:
  `hack/check-test-databases.sh` runs in CI after the tests; the kernel's and the facade's own
  test helpers moved onto `keel_testkit::TestDatabase`.
- A changed descriptor makes the snapshot test fail with a diff.
- The two standard helpers pass against the `keel` module's own functions before the demo exists,
  and the testkit's own tests cover its fixtures, clock and socket.

### 12. The `keel` binary and the demo bundle

**Goal.** The facade crate `keel` gains `Runtime::new()`, `.module()`, `.product()`, `.run()`
returning `keel::ExitCode`, and re-exports of `keel-sdk` and `keel-types`; it stays a library,
so `cargo-semver-checks` keeps running on it and no product crate enters its dependency graph.
The builder is named `Runtime` because `keel-kernel` already owns a loader type named
`Composition`. The binary lives in `bin/keel` (package `keel-bin`, binary `keel`), a workspace
member under `bin/*`; its `main.rs` is the explicit composition of the kernel, every system
module that exists at this milestone and every bundle of the workspace, each bundle bringing
its own modules, so at M0 it reads `keel::Runtime::new().product(&keel_demo::MANIFEST).run()`.
The `demo` bundle and its `note` module are composed behind the cargo feature `demo` of
`bin/keel`, on by default so that the workspace, CI and `hack/prove-binary.sh` carry it, and
off in a release build through `--no-default-features`. `products/demo` becomes a lib crate
exporting `MANIFEST` and the `note` module: a dogfood module without host access, one table
`note_item` with an account column, the functions `note.list` (`list`, read), `note.get`
(`get`, read), `note.edit` (`form`, write, `.owned`) and `note.delete` (`action`, write,
`.owned`), the permissions `note.read` and `note.manage` defaulting to `[user, reseller]`, a
menu node `demo/notes` under the bundle's group, dictionaries for `en` and `ru`, and one
`before` and one `within` hook so that both hook kinds are exercised by a real module.
`note.edit` invokes `keel.account.get` through `call.invoke` to check the account it writes
into, and `note.get` to read back the saved row, which exercises nested invocation. Its
descriptor snapshots live next to its tests under `products/demo/tests/snapshots/`, one file per
function and level; a changed descriptor fails the test with a diff and leaves the new snapshot
beside the old one to review and accept. No lease ever lists `demo`; it is enabled only by
`keel product enable demo` in development and CI.

**Touches.** `Cargo.toml` (members), `crates/keel/`, `bin/keel/`, `products/demo/`,
`hack/check-deps.sh`.

**Out of scope.** `embed_frontend!` and any frontend (M2); `note.export` (M3); `bin/spike`
(M2); the `dns` and `panel` bundles.

**Acceptance.**

- `products/demo` depends only on the facade `keel`; `bin/keel` is the only crate that depends
  on product crates; `hack/check-deps.sh` asserts both.
- `keel product enable demo` against a fresh database, then `keel serve` without `config.yaml`,
  starts against the default database and answers on the socket and on HTTP.
- `keel ctl note.edit title=x` and `POST /api/v1/call/note.list` return the envelope.
- With `demo` disabled `note.*` are unknown and no `note_` table is created; a build with
  `--no-default-features` carries no `note` module, and CI tests `keel-bin` that way.
- The `note.*` descriptors match the committed snapshot.
- The standard tests pass against `note`: a user neither sees nor edits another account's notes;
  a reseller sees the notes of its subtree.
- The test "a nested `invoke` reuses the transaction of the call" passes on `note.edit`.
- The i18n check passes for `en` and `ru`, and `note.list --out table` shows translated headers.

### 13. hack/prove-binary.sh

**Goal.** A script CI runs against the built `keel` binary: create a fresh database and a
temporary `data_dir`, run `keel product enable demo`, assert `keel product list` shows `demo`
enabled, start `serve`, wait for `/readyz`, call `ctl keel.account.create kind=user`,
`/healthz`, `/api/v1/boot` asserting `products` contains `demo` and `brand` is the demo brand,
`ctl note.edit title=x` and `ctl note.list --out json`, then `POST /api/v1/call/note.list` over
HTTP, where no authenticator is registered, asserting the `401 unauthenticated` error envelope
with `ok: false`; stop the process and exit non-zero on any failure. A second run against the
same database after `keel product disable demo` asserts that `serve` starts and
`ctl keel.account.list` answers, and that `note.list` answers `not_found`, which is how the
exit-checklist items "`keel serve` with no bundle enabled starts and answers on the socket" and
"`keel product disable demo` leaves the `note_` table and flips `note` to `orphaned`" are
demonstrated. CI runs it after the build and the binary's tests, before the leftover-database
check.

**Touches.** `hack/prove-binary.sh`, `.github/workflows/ci.yml`.

**Out of scope.** The musl target and the `ldd` check, `hack/vm-test.sh`, `hack/release.py`,
`keel-conformance`.

**Acceptance.**

- The CI job runs the script and is green.
- The script fails when any of its checks fails, and it stops the process and removes its
  temporary `data_dir` on both success and failure.

## M1 work breakdown

M1 is split into eighteen PR-sized steps in nine groups, ordered by dependency; a step is one change
to `main` and is done when its acceptance holds in CI. The letters of a group (`1a`, `1b`) name the
steps of one delivery that land separately, and a step that may land before its place in the order
says so; this breakdown and the later ones name their steps that way, and the M1 exit checklist and
`hack/check-docs.sh` cite a step by that name. The steps derive strictly from the design; where a
step names a mechanism the design leaves open, it says so, and the commit message of the step
records the choice. Where a step diverges from a sentence of the design, it amends that sentence in
the same change, as the repository rule requires, and names the file in its Touches; steps 1a and 2
did so, steps 3a to 8b recorded the sentences they owed in their commit messages and left them to
step 9, which applied them together with the exit checklist, `hack/check-docs.sh` and the final
consistency pass.

Five remarks apply to the whole breakdown.

- The three system modules of M1 live in one crate, `crates/system` (package `keel-system`), with a
  cargo feature per module (`auth`, `settings`, `audit`), each exporting `pub static MODULE:
  ModuleDef` from `keel_system::<module>`. The crate depends on `keel-sdk` alone (`keel-types`
  through it) and never on `keel-kernel`; step 1b teaches `hack/check-deps.sh` that rule next to the
  existing one that the kernel depends on neither `keel-system` nor `keel-host`. Its tests are
  integration tests under `crates/system/tests/`, booted through `keel-testkit` as a dev-dependency;
  the testkit itself does not depend on `keel-system`, and a test composes the system modules it
  needs with `.module(&keel_system::auth::MODULE)`. The binary composes every system module that
  exists.
- Anything a system module needs from the kernel that the public SDK does not expose goes into the
  hidden, semver-exempt `keel_sdk::system`, and the kernel implements it. Each step names the
  additions it makes there; the kernel keeps importing none of the modules.
- The installation listens on plain HTTP until TLS arrives in M3, while the session cookie is
  `__Host-keel_session` and carries `Secure` from its first version, as the design fixes. The
  clients of M1 — the testkit, `hack/prove-binary.sh`, `keel-conformance` — keep and return the
  cookie themselves; a browser is not a client before M2.
- There is no `trusted_proxies` before M3, so the client address of a request is the peer address of
  its TCP connection, which the HTTP transport starts recording in step 1a.
- Rate limits, lockouts and second-factor attempts are counted in the process's memory, not in a
  table: a failed `auth.login` is a failed call whose transaction rolls back, so a counter the
  handler wrote would vanish with it, and a background writer arrives with `tasks` in M3. The
  counters live in the guard and counter instances a module's `init` creates; `init` receives the
  `Registrar`, not a `Context`, so an instance holds no clock of its own and reads the kernel's
  `Clock` at each check through `call.require::<dyn Clock>()`, the handlers through the same call,
  never a process-wide static or `std::time::Instant`, so two harnesses in one test process do not
  share them and the frozen clock drives the tests. Every map evicts entries whose window has passed
  — a bucket idle for a minute, a lockout pair past `auth.lockout.minutes`, a challenge past its
  five minutes — on each insert, so a map is bounded by the distinct keys of one window. A restart
  therefore clears them; IP rules, revocations and login keys are rows and survive it. The design
  fixes the behaviour, not the storage; this is the plan's choice, and step 5a writes it into the
  `auth` section of [design/05-modules.md](design/05-modules.md).

### 1a. Kernel seams: the client, the authenticator chain and the session change

**Goal.** The seams of the kernel, the hidden SDK and the testkit that the `auth` module of steps 1b
and 1c stands on, per the `Authenticator` seam of [design/03-access.md](design/03-access.md) and the
HTTP rules of [design/10-contracts.md](design/10-contracts.md), as one refactor that keeps every
existing test green. `keel_sdk::system::Client { ip, user_agent, session }` is carried by
`CallOptions` from the transport, which starts serving with connect info to learn the peer address,
and is read from the call through the hidden `keel_sdk::system::CallExt::client()`; a socket call
carries no `Client`. `Authenticator::authenticate` takes the `Client` next to the credential and
answers `keel_sdk::system::Authenticated { principal, session: Option<String> }`, where `session` is
the `id_hash` of the session row for a cookie credential and `None` for a token; the transport
copies it into `Client.session`, which is how `auth.logout`, `auth.su.exit` and the `current` column
of `auth.session.list` find the row of the request. Authenticators are registered through the
contribution bag rather than as a single service — `r.contribute(Arc<dyn Authenticator>)` — so that
several modules answer for their own token prefixes: the transport asks them in module order;
`Ok(None)` means "not my credential" and the next one is asked, `Ok(Some)` ends the chain, and `Err`
ends it with that error; a credential none recognises answers `401 unauthenticated`. This keeps the
testkit's header authenticator next to the real one and is the shape `peers` needs for `kpt_` in M6;
the design names the trait and leaves its multiplicity open, and this step writes the chain,
`Client`, `Authenticated` and `SessionChange` into the hidden-hooks paragraph of
[design/05-modules.md](design/05-modules.md). A handler tells the transport what to do with the
cookie through `keel_sdk::system::SessionChange { Set(Secret) | Clear }`, recorded on the call and
carried out by the dispatcher's `Reply`; the dispatcher copies a recorded `SessionChange` into
`Reply` only for a committed call, so a call that fails at any stage after the handler recorded it
answers without `Set-Cookie`. The transport turns `Set` into `Set-Cookie: __Host-keel_session=ks_…;
Path=/; Secure; HttpOnly; SameSite=Lax` without `Max-Age`, since the row decides validity, and
`Clear` into the same cookie with `Max-Age=0`. The transport applies the cross-site rule of
`crates/kernel/src/http/auth.rs` to every API request that is not a `Bearer` request, not only to
requests carrying the cookie, so a guest form that sets a cookie is refused from a foreign `Origin`
or with `Sec-Fetch-Site: cross-site` exactly like a cookie call; a request that carries neither
header is admitted as before. The design checks cookie-bearing requests alone, which leaves the
login form open to a cross-site post that plants a session; this step amends the rule in
[design/10-contracts.md](design/10-contracts.md) and the matching sentence of
[design/08-frontend.md](design/08-frontend.md) and states that reason there. The testkit keeps its
`X-Keel-Test-Principal` middleware and moves its fixture authenticator from `r.provide` to
`r.contribute`; `TestHttp::request` already takes arbitrary headers, which is how a test carries a
cookie.

**Touches.** `crates/sdk/src/system.rs` (`Client`, `Authenticated`, `CallExt`, `SessionChange`, the
authenticator signature and registration), `crates/sdk/src/call.rs` (the client slot read by
`CallExt::client()` and the `SessionChange` slot), `crates/kernel/src/dispatcher/` (`CallOptions`,
`Reply`), `crates/kernel/src/http/` (connect info, the authenticator chain, the cross-site rule, the
cookie), `crates/kernel/tests/kernel/http.rs`, `crates/testkit/src/auth.rs` and `http.rs` (contribution
instead of service, connect info), `docs/design/10-contracts.md` and `docs/design/08-frontend.md`
(the cross-site rule), `docs/design/05-modules.md` (the hidden hooks).

**Out of scope.** Every table, function and dictionary of `auth` (steps 1b and 1c);
`trusted_proxies` (M3); `kpt_` (M6); `PermissionSource` and stored grants, which the M2 scope owns
next to the shell's permissions screen, since the design makes the registered defaults the answer
while no source is composed and M1 composes none.

**Acceptance.**

- In `crates/kernel/tests/kernel/http.rs`, a stub authenticator contributed by a test module answers a
  `Session` credential with a principal and `session: Some(…)`; a stub function answering
  `SessionChange::Set` makes the reply carry `Set-Cookie: __Host-keel_session=…; Path=/; Secure;
  HttpOnly; SameSite=Lax` without `Max-Age`, and `Clear` the same cookie with `Max-Age=0`; a stub
  function that records `SessionChange::Set` and then returns an error answers `4xx` without
  `Set-Cookie`; the handler reads `client().ip` as `127.0.0.1`, `client().user_agent` as the header
  sent and `client().session` as the stub's value, and over the socket `client()` is `None`.
- Two stub authenticators contributed by two test modules: the first answering `Ok(None)` falls
  through to the second, the first answering `Ok(Some)` ends the chain, the first answering
  `Err(unauthenticated)` answers `401` without asking the second; a credential neither recognises
  answers `401`.
- `POST /api/v1/call/keel.account.list` without a credential and with `Origin: https://evil.example`
  answers `403 forbidden` `keel.error.cross_site`, and so does the same call with `Sec-Fetch-Site:
  cross-site`; a `GET` to `keel.account.list` without any credential and without an `Origin` reaches
  the dispatcher and answers `401 unauthenticated` `keel.error.unauthenticated` from the level
  guard, not `403` `keel.error.cross_site`; a `Bearer` request with a foreign `Origin` is not
  refused.
- `cargo test --workspace` stays green with the testkit's authenticator as a contribution, and
  `hack/prove-binary.sh` is unchanged.

### 1b. auth: the crate and passwords

**Goal.** Create `keel-system` with the `auth` module (`id: "auth"`, no `requires`, migrations under
`keel_migration_auth`) and its first table, per [design/03-access.md](design/03-access.md) and the
`auth` section of [design/05-modules.md](design/05-modules.md). The table `auth_password`
(`principal_id` referencing `keel_principal`, `hash`, `updated_at`) holds an argon2id PHC string
computed with the `argon2` crate's default parameters and verified on a blocking thread. One
function, registered through the same `Registrar` as any module, with `en` and `ru` dictionaries and
descriptor snapshots: `auth.password.set { principal?, current?, password }` (`user`, permission
`auth.password.manage` defaulting to `[user, reseller]`), where a call without `principal` changes
the caller's own password and requires `current`, answering `422 validation` on `current` when it is
absent and `401 unauthenticated` `auth.error.credentials` when it does not verify — a caller without
an `auth_password` row is answered the same after the decoy verify — and a reseller or admin sets
one with `principal` and no `current` for a principal inside its scope, resolved through
`keel.principal.get` by `call.invoke` so that a foreign principal answers `not_found`, and only for
a target below the caller's level, else `403 forbidden` `auth.error.not_below` — the rule `auth.su`,
`auth.key.issue` and `auth.totp.disable` share; the `principal` field sits at level `reseller`. The
password check is one function of the module, `verify(login, password, db)` answering the principal
id, that steps 1c and 3b call: when the login has no `auth_password` row it verifies the supplied
password against a decoy, an argon2id hash of a random string generated at module start, so an
unknown login and a wrong password cost the same argon2 work and are one answer,
`auth.error.credentials`. The `not_below` targets every later check of M1 reuses are fixed here: the
reseller for a second reseller-level principal of its account (an admin cannot be placed in a
reseller account, `keel.principal.error.level_account`, so the reseller's `above` case is the admin
itself, which is outside its scope and answers `not_found`), the admin for a second admin of the
root account.

**Touches.** `Cargo.toml` (members, the pinned `argon2`), `crates/system/` (the crate, the `auth`
module, its migration, dictionaries, tests and snapshots), `hack/check-deps.sh`.

**Out of scope.** Sessions, the login and the authenticator (step 1c); API tokens (step 2); the
second factor (step 3b); `su` and login keys (steps 4a and 4c); lockout, rate limiting, IP rules,
the password policy and the session list (steps 5a and 5b); `PermissionSource` and stored grants
(M2, as step 1a says); password reset through `notify` (M3); `keel.setup.admin` (M4).

**Acceptance.**

- `cargo build -p keel-system --no-default-features --features auth` and `cargo build -p keel-system
  --no-default-features` both build; `hack/check-deps.sh` asserts that `keel-sdk` is the only direct
  workspace dependency of `keel-system`, in the form of the `keel-testkit` check, which filters the
  direct dependencies to the workspace's `keel-*` crates, and that `keel-kernel` still depends on
  neither `keel-system` nor `keel-host`.
- On a fresh database the ledger `keel_migration_auth` and the table exist; after
  `auth.password.set` the stored hash starts with `$argon2id$`.
- In `crates/system/tests/system/auth_passwords.rs`, driven through `X-Keel-Test-Principal` and the socket:
  A1 sets its own password with `current`; A1 without `current` answers `422` on `current`; with a
  wrong `current` `401` `auth.error.credentials`; the admin with `principal` and no `current` sets
  A1's password; `auth.password.set` with `principal` is refused for a target at or above the
  caller's level with `403` `auth.error.not_below`: the reseller for a second reseller-level
  principal of its account, the admin for a second admin of the root account — the two targets
  above; the reseller is answered `not_found` for `Unrelated` and for the admin; the admin sets the
  reseller's password.
- A unit test of the module's password check shows that a wrong password and an unknown login
  answer the same `auth.error.credentials` and that an absent `auth_password` row is verified
  against the decoy hash, so both branches run the same `argon2` verify.
- The descriptors of `auth.password.set` match snapshots under `crates/system/tests/snapshots/`,
  one file per function and level, and `password` and `current` are marked `secret`.

### 1c. auth: sessions, the login and the authenticator

**Goal.** The first credential behind the seams of step 1a, per
[design/03-access.md](design/03-access.md) and the `auth` section of
[design/05-modules.md](design/05-modules.md). The table `auth_session` (`id_hash`, `principal_id`,
`account_id`, `actor_principal_id`, `kind` ∈ `session | su`, `ip`, `user_agent`, `created_at`,
`idle_until`, `absolute_until`, `revoked_at`, `reason`). A session token is `ks_` followed by 32
random bytes as hex; the row keeps its sha256 as `id_hash`, so the table holds no usable token and
revocation is a row change. Two functions with `en` and `ru` dictionaries and descriptor snapshots:
`auth.login { login, password, method? }`, a `guest`-level `form` in `mode: write` that runs through
the whole dispatcher pipeline, verifies the password through the check of step 1b, inserts the
session row with the client's address and user agent and answers `{ state: "authenticated",
principal }` with `SessionChange::Set`; a cookie present on the login request is never reused:
`auth.login` always generates a fresh `ks_` token and answers `Set` with it, and a valid session the
request arrived with is revoked with `reason: replaced`. `auth.logout` revokes the session
`client().session` names with `reason: logout` and answers `Clear`. An unknown login and a wrong
password are one answer, `401 unauthenticated` with `auth.error.credentials`, in the same time, as
the check of step 1b guarantees. The idle and absolute lifetimes are the module constants 30 minutes
and 12 hours until step 5b makes them settings; every authenticated request slides `idle_until`
from the kernel's `Clock`. Login methods are an extension point in the module's own `pub mod api`: a
`LoginMethod` trait (`id`, `verify(login, secret, db)` answering the principal id) collected from
the contribution bag, with the built-in `local` method over `auth_password`; `method` names one and
defaults to `local`; OIDC is a later method with its own redirect flow. The module's authenticator
resolves a `Session` credential carrying `ks_` to its row and principal and answers `session` as the
row's `id_hash`; it answers `None` for a `Bearer` value without the `kat_` prefix and, until step 2
fills the `kat_` branch, `Err(unauthenticated)` for one with it. The login functions — `auth.login`,
and `auth.totp.verify` and `auth.key.redeem` of steps 3b and 4c — require `via: http`, the only
transport that carries a cookie, and answer `422 validation` with `auth.error.http_only` on the
socket, where the owner needs no session; the session functions — `auth.logout`, and `auth.su` and
`auth.su.exit` of step 4a — require a session credential, `client().session` being `Some`, and
answer `422 validation` with `auth.error.session_only` over the socket and under a Bearer token.

**Touches.** `crates/system/` (auth: the migration, `login`, `logout`, `LoginMethod`, the
authenticator, dictionaries, tests and snapshots), `bin/keel/` (the composition),
`hack/prove-binary.sh`.

**Out of scope.** API tokens (step 2), the second factor (step 3b), `su` and login keys (steps 4a
and 4c), lockout, rate limiting, IP rules, the password policy and the session list (steps 5a and
5b), settings-driven lifetimes (steps 5b and 6a), OIDC (no milestone fixes it), TLS (M3).

**Acceptance.**

- On a fresh database `auth_session` exists, and no column of it ever holds a value starting with
  `ks_`.
- Over HTTP, in `crates/system/tests/system/auth_sessions.rs`: after an admin sets A1's password, `POST
  /api/v1/call/auth.login` as a guest answers `200` with `data.principal` naming A1 and a
  `Set-Cookie` header carrying exactly the attributes of step 1a; `GET /api/v1/boot` with that
  cookie returns A1 as the principal; `keel.account.list` with it returns A1's account only; the
  same call with `Origin: https://evil.example` answers `403 forbidden` `keel.error.cross_site`;
  `keel.principal.update` of A1 by A1 with an `account` answers `403 forbidden_field` with
  `fields[].path == "account"`, the M1 exit item on writing above one's level, now over a real
  session; `auth.logout` answers with the clearing cookie and sets the row's `revoked_at`, after
  which the cookie answers `401`.
- `POST /api/v1/call/auth.login` with `Origin: https://evil.example` answers `403 forbidden`
  `keel.error.cross_site` and sets no cookie; logging in while sending the cookie of an earlier
  session answers a different `ks_`, and the earlier row gains `revoked_at` with `reason: replaced`.
- A wrong password and an unknown login answer the same `401 unauthenticated`
  `auth.error.credentials`; the audit event the dispatcher raises for that failed call carries
  `params.password == "***"`, observed through a test `AuditSink` until step 7 stores it.
- With `harness.clock()` moved 31 minutes past the last request the cookie answers `401`; with a
  request every ten minutes for twelve hours the next one answers `401`; the session row records
  `ip` and `user_agent`.
- A `ks_` token sent as `Bearer` answers `401`; so does any `kat_` value until step 2.
- A test that composes `auth` still drives HTTP through `X-Keel-Test-Principal`, so two
  authenticators coexist; `auth.login` over the socket answers `422` `auth.error.http_only` and
  `auth.logout` over the socket `422` `auth.error.session_only`.
- The descriptors of `auth.login` and `auth.logout` match their snapshots, and `password` is marked
  `secret` in the login form.
- `bin/keel` composes `keel_system::auth::MODULE`; `hack/prove-binary.sh` creates a principal and
  sets its password over `ctl`, logs in over HTTP and calls `note.list` with the cookie, asserting
  the `200` page next to the existing `401` without a credential.

### 2. auth: API tokens and the scope guard

**Goal.** The `Bearer` branch of the authenticator and the guard the design places between
permission and module guards ([design/02-functions.md](design/02-functions.md),
[design/03-access.md](design/03-access.md)). The table `auth_token` carries `id`, `token_hash`,
`principal_id`, `account_id`, `name`, `level` (the ceiling), `scope` (the serialised
`keel_types::Scope`: `Full`, `Functions` with masks, or `ReadOnly`), `ip_allow` (a list of CIDRs),
`expires_at`, `last_used_at`, `created_at` and `revoked_at`. A token is `kat_` and 32 random bytes
as hex, stored as its sha256. Functions: `auth.token.list` (`list`, permission `auth.token.read` for
`[user, reseller]`, over a `ListSource` of the module that applies `call.account_scope()` to
`token::Entity` and, for a caller below `reseller`, `principal_id = caller`, since a token is the
credential of the principal that created it and an account may hold several principals; columns
`name`, `level`, `scope`, `ipAllow`, `expiresAt`, `lastUsedAt`, `revokedAt`); `auth.token.create {
name, level?, scope?, ipAllow?, expiresAt? }` (`form`, `auth.token.manage` for `[user, reseller]`),
answering `{ id, token }` with the plaintext shown this once; `auth.token.revoke { id }` (`action`,
owned by `account_id`, answering `not_found` for a row of another principal of the same account
when the caller is below `reseller`). The ceiling defaults to the creator's level and may not exceed
it; a call made with a token runs as the token's principal at the lower of its level and the
ceiling, with `scope` from the row, so `meta.principal.level` shows the effective level. A call made
with a token may not create a token wider than itself: the new `scope` must be contained in the
caller's — `Full` only from `Full`, every mask of a new `Functions` scope matched by a mask of the
caller's (an exact name by the equal name or by a prefix that covers it, a prefix `x.*` only by an
equal or shorter prefix), `ReadOnly` never creates — else `403 forbidden` `auth.error.token_scope`.
`ip_allow` entries pass the `cidr` check preset and are matched with `ipnet`. The authenticator's
`Bearer` branch requires the `kat_` prefix, looks the hash up, refuses a revoked or expired row or a
client address outside `ip_allow` with `Err(unauthenticated)`, answers `session: None` and stamps
`last_used_at`. The scope guard is `guards::scope` in the dispatcher, after level and permission and
before module guards, with the feature guard's slot left for M4: a `Functions` scope whose masks —
exact names or a prefix ending in `.*`, as `svc.*` in the design — match none answers `403
forbidden` with `keel.error.token_scope` and the function in `params`; `ReadOnly` refuses `mode:
write` with `keel.error.token_read_only`. The guard runs for the entered call only; a nested
`invoke` belongs to a call the guard already admitted, the same rule as module guards. This is a
plan-level reading of "runs under the outer call's principal and scope"; the design does not spell
it out.

**Touches.** `Cargo.toml` (the pinned `ipnet`), `crates/system/` (auth: migration, entity,
functions, the `Bearer` branch), `crates/kernel/src/dispatcher/guards.rs` and `mod.rs` (the scope
guard), `products/demo/tests/` (the nested-invoke check, with `keel-system` as a dev-dependency);
`design/02-functions.md`, `design/08-frontend.md` and `design/11-decisions.md` gain the `x-keel`
attribute `once` for the plaintext of a token, answered to the call that produced it and masked in
every stored copy.

**Out of scope.** Peer tokens `kpt_`, on-behalf calls and `X-Keel-On-Behalf-Of` (M6); the feature
guard (M4); stored grants (M2); the token screens of the shell (M2); a service principal created for
a token — a token acts as the principal that created it.

**Acceptance.**

- A1 creates a token scoped to `["keel.account.list"]`; `keel.account.list` with `Authorization:
  Bearer kat_…` answers `200`, `keel.principal.list` answers `403 forbidden`
  `keel.error.token_scope` — the M1 exit item — and a read-only token answers
  `keel.principal.update` of A1 with `403` `keel.error.token_read_only`; a token with the ceiling
  `service` and the same scope answers `keel.principal.list`, a `user`-level function outside its
  masks, with `403` `keel.error.level` rather than `keel.error.token_scope`, which shows that the
  level guard runs before the scope guard.
- A token scoped to `["auth.token.create"]` is refused a `Full` token and one scoped to `["keel.*"]`
  with `403` `auth.error.token_scope`, and creates one scoped to `["auth.token.create"]`; a token
  scoped to `["keel.*"]` creates one scoped to `["keel.account.list"]` and is refused `["auth.*"]`;
  `auth.logout` under a `kat_` token answers `422` `auth.error.session_only`.
- A token scoped to `note.edit` alone edits a note whose handler invokes `keel.account.get` and
  `note.get`, so the scope guard admits nested invokes: `products/demo/tests/demo/tokens.rs`.
- `ip_allow: ["10.0.0.0/8"]` answers a request from `127.0.0.1` with `401`; `["127.0.0.0/8"]` with
  `200`; the plaintext appears once, in the result of `auth.token.create`, and nowhere in the table;
  a revoked token answers `401` at its next call; an expired one, by the frozen clock, likewise;
  `last_used_at` is set after a call.
- A `Bearer` request with a foreign `Origin` is not refused as cross-site.
- A second `user` principal the test creates in A1's account sees none of A1's tokens and is
  answered `not_found` for their ids; `assert_reseller_sees_only_subtree` holds for
  `auth.token.list` and `assert_user_cannot_edit_foreign_row` for `auth.token.revoke`; the
  descriptors match their snapshots.

### 3a. Kernel: the data key as a service

**Goal.** The kernel side of the second factor, which touches no module and may land before step
1b. A secret a module stores is sealed and opened through `keel_sdk::system::SecretCipher`, a hidden
service the kernel provides over the data key it already opens from `keel_kv.secret_key`, so a table
holds nothing the key does not unlock; promoting that sealing into the `Secret` type's storage is
left to the first product that stores one (M4). `Composition::key(Key)` hands the data key to the
loader, which provides `SecretCipher` as a kernel service; `crates/keel/src/runtime.rs` passes
`instance.key()` after `Instance::establish`, and `Harness::try_boot` generates a key with
`Key::generate()` so a test composition has the cipher. `/api/v1/boot` fills `principal.totp` from
`keel_sdk::system::SecondFactorSource`, a hidden service a module may provide, and answers `false`
while none does; the socket's boot frame keeps `false` for the owner.

**Touches.** `crates/sdk/src/system.rs` (`SecretCipher`, `SecondFactorSource`),
`crates/kernel/src/secret.rs` and `boot.rs` (the cipher service, `principal.totp`),
`crates/kernel/src/instance.rs` and `crates/kernel/src/loader/composition.rs` (the key into the
composition), `crates/keel/src/runtime.rs`, `crates/testkit/src/harness.rs` (a generated key).

**Out of scope.** Everything of the `auth` module (step 3b); sealing `Secret` columns (M4).

**Acceptance.**

- In `crates/kernel/tests/`, a handler of a test module seals a value through `SecretCipher` and
  opens it again; the sealed bytes are not the value, and a composition booted with another key
  does not open them; `crates/keel/tests/runtime.rs` shows the `Runtime` composition provides the
  cipher.
- `/api/v1/boot` shows `principal.totp: false` for A1 while no `SecondFactorSource` is provided and
  `true` when a test module provides one answering `true`; the socket's boot frame keeps `false`.

### 3b. auth: second factor

**Goal.** TOTP as the second step of a login, per the `auth` section of
[design/05-modules.md](design/05-modules.md). Tables: `auth_totp` (`principal_id`, `secret` sealed
through the `SecretCipher` of step 3a, `confirmed_at`, `last_step`, `created_at`);
`auth_recovery_code` (`id`, `principal_id`, `code_hash`, `used_at`); and `auth_login_challenge`
(`challenge_hash`, `principal_id`, `ip`, `expires_at`, `used_at`, `created_at`), the pending state
between the password and the code. Functions, all `user` under `auth.totp.manage` for `[user,
reseller]` unless named otherwise: `auth.totp.enroll` writes an unconfirmed row and answers `{
secret, uri }`, the base32 secret and its `otpauth://` URI with the issuer `keel` and the login as
label, and for a principal with a confirmed factor answers `409 conflict` `auth.error.totp_enrolled`
— disable first; `auth.totp.confirm { code }` confirms it and answers `{ recoveryCodes }`, ten
single-use codes of 10 characters from the base32 alphabet (50 bits) shown this once, stored as
sha256 and compared by hash lookup; `auth.totp.disable { principal?, code? }` disables it for
oneself with a valid code, or for a principal inside the caller's scope and below the caller's level
without one when the caller is an admin (the `principal` field is at level `admin`; a target at or
above the caller answers `403` `auth.error.not_below`); `auth.totp.verify { challenge, code }` is a
`guest`-level `form`, HTTP only, that completes a login. `auth.login` for a principal with a
confirmed factor creates a challenge (32 random bytes as hex, stored as its sha256, expiring after
five minutes), sets no cookie and answers `{ state: "totp", challenge }`; `auth.totp.verify` accepts
the code of the current step or its neighbours, where the current step is computed from the
kernel's `Clock`, refuses any step at or below `last_step`, which is set to the step that matched,
accepts an unused recovery code in its place, which does not move `last_step`, marks the challenge
used, opens the session exactly as step 1c does — a fresh `ks_`, the session the request arrived
with revoked as `replaced` — and answers `{ state: "authenticated", principal }`. `auth.totp.verify`
advances the step and consumes the challenge with one conditional statement each — `UPDATE
auth_totp SET last_step = $step WHERE principal_id = $1 AND (last_step IS NULL OR last_step <
$step)` and `UPDATE auth_login_challenge SET used_at = $now WHERE challenge_hash = $1 AND used_at IS
NULL AND expires_at > $now` — and a recovery code with `UPDATE auth_recovery_code SET used_at = $now
WHERE code_hash = $1 AND used_at IS NULL`; zero rows from any of them answers `401`
`auth.error.code` or `auth.error.challenge`; `auth.totp.confirm` and `auth.totp.disable { code }`
set `last_step` to the step that matched. A challenge admits five attempts, counted in an in-memory
map the module's `init` creates, keyed by the challenge hash and timed by the kernel's `Clock` the
handler reads through `call.require::<dyn Clock>()`, evicted as the whole-breakdown remark says. The
codes are RFC 6238 over HMAC-SHA1 with six digits and a 30-second step, implemented on `hmac` and
`sha1` with an RFC link next to the code, compared in constant time with `subtle`. The module
provides the `SecondFactorSource` of step 3a, so `/api/v1/boot` fills `principal.totp` from
`auth_totp`.

**Touches.** `Cargo.toml` (the pinned `hmac`, `sha1`, `base32`, `subtle`), `crates/system/` (auth:
migration, TOTP, recovery codes, the challenge, the login change, the `SecondFactorSource`).

**Out of scope.** Enforcing a second factor per level (a policy setting for a later milestone),
WebAuthn, remembering a browser, the enrolment screens of the shell (M2), a second factor on
`auth.key.redeem` (the key is issued by an already authenticated party).

**Acceptance.**

- The RFC 6238 test vectors pass against the code generator.
- A1 enrols and confirms with a code the test computes from the returned secret and
  `harness.clock().now()`; `auth.login` then answers `state: "totp"` with a challenge and no
  `Set-Cookie`; `auth.totp.verify` with the right code sets the cookie and creates the session; a
  wrong code answers `401` `auth.error.code`; the sixth wrong code kills the challenge, and an
  expired one, by the frozen clock, answers `401` `auth.error.challenge`; with the clock frozen
  inside step N, the code of step N is accepted first and the code of step N−1, still inside the
  window, is refused afterwards, both computed from `harness.clock().now()`; the code that confirmed
  the factor is refused by a login inside the same step; a recovery code logs in once and is
  refused the second time; `/api/v1/boot` shows `principal.totp: true`; after `auth.totp.disable`
  the login is one step again, and an admin disables A1's factor without a code while a reseller
  cannot; the admin is refused a second admin, a `not_below` target of step 1b, with
  `auth.error.not_below`.
- Two verifies of one challenge with the right code fired concurrently answer one `200` and one
  `401`; two concurrent redeems of one recovery code likewise.
- A second `auth.totp.enroll` after the confirmation answers `409 conflict`
  `auth.error.totp_enrolled`; every recovery code is 10 characters of the base32 alphabet and the
  table holds its sha256.
- `POST /api/v1/call/auth.totp.verify` with `Origin: https://evil.example` answers `403 forbidden`
  `keel.error.cross_site` and sets no cookie; a verify sent with the cookie of an earlier session
  answers a different `ks_` and revokes that session as `replaced`.
- The `secret` column is not the base32 secret and opens with the kernel's data key; the tables hold
  hashes of the recovery codes and of the challenge, never the values.
- `code`, `secret` and `challenge` are `secret` fields, masked in the audit event of a failed
  verify; the `auth.totp.*` descriptors match their snapshots.

### 4a. auth: delegation with su and su.exit

**Goal.** The first of the two ways one principal acts as another that the design fixes for M1
([design/03-access.md](design/03-access.md), the delegation table). `auth.su { principal }`
(`reseller`, permission `auth.su` for `[reseller]`) resolves the target through `keel.principal.get`
by `call.invoke`, so a principal outside the caller's scope answers `not_found`, refuses a target at
or above the caller's level with `403 forbidden` `auth.error.not_below`, refuses a caller already
delegated with `auth.error.su_nested`, and opens a second `auth_session` row of `kind: su` with
`actor_principal_id` the caller and `parent_id_hash` the caller's session. The child row is created
with the parent's `idle_until` and `absolute_until`, and every request the authenticator resolves
through `su_id_hash` slides the child's `idle_until` together with the parent's, so a delegated
session expires only when the parent does or when `su.exit` or a revoke ends it. The cookie does not
change: the parent row gains `su_id_hash` naming its active `su` child — `parent_id_hash` and
`su_id_hash` are added by one additive migration of this step — and the authenticator, having
resolved the cookie to the parent row, follows `su_id_hash` to the child and answers the child's
principal with `actor` filled, `session` staying the parent's `id_hash`; a revoked child is ignored,
and the parent answers as itself again. The principal an `su` session resolves to carries `actor:
Some(caller with via: Su)`, so every call in it is attributed the same way and
`meta.principal.actor` and `/boot.principal.actor` are filled by the code M0 already has.
`auth.su.exit` revokes the child with `reason: su_exit` and clears `su_id_hash`; a revoked or
expired parent already answers `401` at the transport, so no cookie is cleared; outside an `su`
session it answers `422` `auth.error.not_delegated`. Revoking the parent (step 5b) revokes the child
through `parent_id_hash`. Both functions are session functions under the rule of step 1c. The
delegation paragraph of [design/03-access.md](design/03-access.md) gains the two columns and the
cookie that does not change.

**Touches.** `crates/system/` (auth: `su`, `su.exit`, the migration adding `parent_id_hash` and
`su_id_hash`, the authenticator following `su_id_hash`), `docs/design/03-access.md` (the delegation
paragraph).

**Out of scope.** On-behalf calls from a peer (M6); the socket side of the delegation table (step
4b); login keys (step 4c); the `su` banner of the shell (M2).

**Acceptance.**

- `login_boot_list_su_narrows_the_list_and_fills_the_actor_then_su_exit_returns` in
  `crates/system/tests/system/auth_su.rs`, the M1 exit sequence over HTTP: login as the admin, `boot`,
  `keel.account.list` listing every account, `auth.su` into A1 with no `Set-Cookie` in the answer,
  `keel.account.list` listing A1's account only with `meta.principal.actor` the admin and `via:
  "su"`, `boot` with `principal.actor` filled, `auth.su.exit`, after which the actor is `null` and
  the list is wide again.
- The reseller enters A1 and is answered `not_found` for `Unrelated`; the `not_below` targets of
  step 1b are refused with `403` `auth.error.not_below`, and the admin a nested `su` with
  `auth.error.su_nested`; `su.exit` outside an `su` session answers `422`; when the parent was
  revoked, the next call, `su.exit` included, answers `401`; `auth.su` under a `kat_` token answers
  `422` `auth.error.session_only`.
- Inside the `su` session, with a request every ten minutes for an hour, the actor stays filled; 31
  idle minutes answer `401` for the parent rather than a call as the admin.
- The `su` session is a distinct `auth_session` row of `kind: su` with `actor_principal_id` and
  `parent_id_hash`, and the parent row names it in `su_id_hash` until `su.exit` clears it; the
  descriptor `ETag` under `su` differs from the actor's.

### 4b. Socket delegation: the `as` frame and `keel ctl --as`

**Goal.** The socket side of the delegation table, kernel work that M0 step 9 deferred here and that
depends on no module, so it may land before step 4a. The call frame gains `as`, `keel ctl <func> …
--as <login>` sends it, and the kernel runs the frame as the named principal with the owner as
`actor` and `via: socket`; an unknown login answers `not_found`.

**Touches.** `crates/kernel/src/socket/` and `ctl.rs` (`as`, `--as`), `crates/keel/src/cli.rs` (the
flag), `crates/testkit/src/socket.rs` (the frame with `as`), `hack/prove-binary.sh`.

**Out of scope.** `as` over HTTP, which is `su`; a peer's on-behalf header (M6).

**Acceptance.**

- `keel ctl keel.account.list --as a1` returns A1's account only with `meta.principal.actor` login
  `owner` and `via: "socket"`; `--as nobody` answers `not_found`; `harness.socket()` covers the
  frame with `as`, and `hack/prove-binary.sh` runs the command.

### 4c. auth: one-time login keys

**Goal.** The one-click login of [design/07-ecosystem.md](design/07-ecosystem.md) with the
same-origin rule of [design/10-contracts.md](design/10-contracts.md). Login keys: `auth_login_key`
(`key_hash`, `principal_id`, `issued_by`, `redirect`, `expires_at`, `used_at`, `created_at`);
`auth.key.issue { principal, ttl, redirect }` (`reseller`, permission `auth.key.issue` for
`[reseller]`), with `ttl` at most 300 by schema, the target below the caller and inside its scope as
for `su`, and `redirect` a path starting with a single `/` — not `//`, no scheme, no `\` — else
`422` `auth.error.redirect`, answers `{ key: "kok_…", url: "/login?key=kok_…" }`. The design writes
the call with `external_id` and an absolute `url`; this step amends the `auth` section of
[design/05-modules.md](design/05-modules.md) and the example of
[design/07-ecosystem.md](design/07-ecosystem.md) to the `{ principal, ttl, redirect }` form and the
relative `url`, naming the `peers` delivery of M6 as what adds `externalId`, resolved against
`(origin, external_id)` for the peer that carries an `origin`, and a public-URL setting no M1 item
defines as what makes the `url` absolute. `auth.key.redeem { key }` is a `guest`-level `form`, HTTP
only, under the cross-site rule of step 1a: it marks the key used with one conditional statement,
`UPDATE auth_login_key SET used_at = $now WHERE key_hash = $1 AND used_at IS NULL AND expires_at >
$now RETURNING …`, answers `401` `auth.error.key` when no row comes back, otherwise opens a
`session` row as step 1c does — a fresh `ks_`, the session the request arrived with revoked as
`replaced` — sets the cookie and answers `{ redirect, principal, issuedBy }`, so that the audit
record of the redeem names who issued the key; `key` is a `secret` field. The page at `/login?key=…`
belongs to the shell (M2), which performs the same-origin `POST auth.key.redeem` itself, as
[design/08-frontend.md](design/08-frontend.md) says; the clients of M1 post `auth.key.redeem`
directly.

**Touches.** `crates/system/` (auth: the migration, `key.issue`, `key.redeem`),
`docs/design/05-modules.md` and `docs/design/07-ecosystem.md` (the `auth.key.issue` shape and the
relative `url`).

**Out of scope.** The `/login` page (M2, the shell); `keel.setup.admin` and the key printed at first
start (M4); a public-URL setting; `externalId` and on-behalf issue from a peer (M6).

**Acceptance.**

- `auth.key.issue` for A1 with `ttl: 60` answers a `kok_` key and `url: "/login?key=…"`; `POST
  auth.key.redeem` sets the cookie and answers `redirect`, `principal` and `issuedBy`; the second
  redeem, an expired key and a made-up key answer `401` `auth.error.key`; two redeems of one key
  fired concurrently answer one `200` and one `401`; `ttl: 301` answers `422` on `ttl`; `redirect:
  "https://evil.example"` and `redirect: "//evil.example"` answer `422` `auth.error.redirect`; a
  target outside the scope answers `not_found` and the `not_below` targets of step 1b `403`
  `auth.error.not_below`; the table holds the sha256 only.
- `POST /api/v1/call/auth.key.redeem` with `Origin: https://evil.example` answers `403 forbidden`
  `keel.error.cross_site` and leaves the key unused; a redeem sent with the cookie of an earlier
  session answers a different `ks_` and revokes that session as `replaced`; the audit event of a
  redeem carries `params.key == "***"`.

### 5a. auth: IP rules, rate limiting, lockout and unlock

**Goal.** The guards of the M1 `auth` scope, on the module-guard extension point M0 built
([design/02-functions.md](design/02-functions.md), module guards;
[design/05-modules.md](design/05-modules.md), `auth`). IP rules: `auth_ip_rule` (`id`, `action` ∈
`allow | deny`, `cidr`, `level`, `comment`, `created_at`), managed by `auth.ip_rule.list`, `.create`
and `.delete` (`admin`, `.unowned()`), and enforced by the `IpRules` guard registered with
`r.guard`: a rule applies to callers at or above its `level`; a client address matching a `deny`
rule answers `403 forbidden` `auth.error.ip_denied`, and when any `allow` rule applies the address
must match one of them; a call without a client address, the socket, is not checked, which is how an
operator undoes a rule that locked the browser out. The guard keeps the rules in memory, loaded on
first use and refreshed from `after_commit` of the three functions. Rate limiting: the `RateLimit`
guard, applied to every call whose principal is the guest (`call.principal().level ==
Level::Guest`), which is every call to a guest-level function that matters and needs no change to
the `Guard` trait, which hands a guard the function name and the call but not the descriptor; a
token bucket per client address refilled to `auth.rate.guest_per_minute` every minute, answering
`429 rate_limited` `auth.error.rate_limited`. Lockout: `auth.lockout.threshold` failures for one
`(login, client address)` pair within `auth.lockout.minutes` lock that pair for as long, so an
attacker locks only its own address out of a login and the victim from another address is
unaffected; spraying from one address is what the rate limit answers. Failures for an unknown login
are counted the same way, so `auth.error.locked` says nothing about whether the login exists.
`auth.login` and `auth.totp.verify` answer a locked pair with `403 forbidden` `auth.error.locked`
before touching the password; for `auth.totp.verify` the pair is the login of the challenge's
principal and the client address, checked before the code is compared, and a wrong code counts as a
failure of that pair, so the password does not buy an attacker more than `auth.lockout.threshold`
codes per address and window. `auth.unlock { login }` (`admin`, `action`, `.unowned()`) clears every
pair of that login, which is what `keel ctl auth.unlock login=<login>` does. The buckets and lockout
counters live in the guard instances `init` creates and are timed by the kernel's `Clock` the guard
reads at each check through `call.require::<dyn Clock>()` and the handlers through the same call —
a module's `init` receives the `Registrar`, not a `Context`, so the instance holds no clock of its
own — never in a process-wide static or on `std::time::Instant`, so two harnesses in one test
process do not share them and the frozen clock drives the tests; each map evicts as the
whole-breakdown remark says, and the module's `pub mod api` exposes the guard with a `len()` that
tests read. The numbers are module constants — threshold 5, 15 minutes, 30 guest calls per minute —
until step 5b contributes their definitions. The `auth` section of
[design/05-modules.md](design/05-modules.md) gains the pair rule of the lockout and the in-memory
counters of the whole-breakdown remark.

**Touches.** `crates/system/` (auth: migration, guards, `ip_rule.*`, `unlock`),
`hack/prove-binary.sh` (`auth.unlock` and an IP rule over `ctl`), `docs/design/05-modules.md` (the
lockout pair and the counters).

**Out of scope.** The setting definitions (step 5b) and their resolution (step 6a); request limits
beyond guest calls and `sse_max_clients` (M3); geo rules; persistence of counters (M3).

**Acceptance.**

- Address-dependent checks run through the dispatcher with `CallOptions.client` naming the address,
  since the testkit's HTTP client always connects from `127.0.0.1`: the thirty-first `auth.login`
  from one address within a minute answers `429 rate_limited` while another address is unaffected,
  an authenticated cookie call to `keel.account.list` from the rate-limited address is not counted,
  and after the frozen clock moves a minute the address is admitted again; two harnesses in one
  test process keep separate buckets.
- Five wrong passwords for A1 from `10.1.1.1` (through `CallOptions.client`) lock that address: the
  right password from it answers `403` `auth.error.locked` while the right password from `127.0.0.1`
  answers `200`; five wrong passwords for a login that does not exist lock it the same way; five
  wrong codes for A1's challenge from `10.1.1.1` lock the pair: the right code from it answers `403`
  `auth.error.locked`, a new `auth.login` from it answers the same, and from `127.0.0.1` the code is
  accepted; after the frozen clock moves fifteen minutes the pair is admitted; `auth.unlock` as the
  admin admits the next login at once; `hack/prove-binary.sh` runs `ctl auth.unlock login=…` and
  expects the envelope.
- After the frozen clock moves past the window the guard's maps hold no entry for the addresses and
  pairs the test used, observed through a `#[cfg(test)]`-free `len()` on the guard exposed in `pub
  mod api`.
- A `deny` rule for `127.0.0.1/32` at level `guest` refuses `auth.login` and an authenticated call
  from that address with `403` `auth.error.ip_denied`; an `allow` rule for `10.0.0.0/8` at level
  `admin` refuses the admin from `127.0.0.1` while A1 passes; the rules take effect without a
  restart; `keel.account.list` over the socket passes whatever the rules say.
- The rules, being rows, survive a restart: `hack/prove-binary.sh` creates, at the end of its first
  run, a `deny` rule for `127.0.0.1/32` at level `user` over `ctl`; in its second run the login over
  HTTP answers `200`, the guest being below the rule's level, `keel.account.list` with the cookie
  answers `403` `auth.error.ip_denied`, and after `ctl auth.ip_rule.delete` over the socket, which
  the guard does not check, the same call answers `200`.
- The `auth.ip_rule.*` and `auth.unlock` descriptors match their snapshots.

### 5b. auth: SettingDef, password policy, session list and revoke

**Goal.** The remaining `auth` items of the M1 scope. Password policy: `auth.password.set`, and
every other place a password is stored, refuses a password shorter than `auth.password.min_length`
with `422 validation` on `password` keyed `auth.error.password_short`, and one equal to the login
with `auth.error.password_login`. The numbers of this step and of step 5a are `SettingDef`
contributions — `auth.password.min_length` 12, `auth.session.idle_minutes` 30,
`auth.session.absolute_hours` 12, `auth.lockout.threshold` 5, `auth.lockout.minutes` 15,
`auth.rate.guest_per_minute` 30, all at level `admin` — and `keel_sdk::SettingDef` (`new(key,
schema)`, `.default()`, `.level()`) joins the public SDK here as `crates/sdk/src/settings.rs`,
re-exported from `lib.rs`, the first of the contribution definitions the design names (`LimitDef`
follows in M4), because a product module contributes settings without naming `settings` in
`requires`, as the `dns` example of the design does; the sentence of the contribution table in
[design/05-modules.md](design/05-modules.md) that places every other point in a module's `pub mod
api` is amended to name `SettingDef` in the public SDK; the values stay the module's own constants
until step 6a resolves them. Sessions: `auth.session.list` (`list`, permission `auth.session.read`
for `[user, reseller]`, over a `ListSource` of the module shaped like the one of `auth.token.list`
in step 2 — the account scope narrowed to `principal_id = caller` for a caller below `reseller`;
rows `{ id, kind, ip, userAgent, createdAt, idleUntil, absoluteUntil, actor, current, revokedAt }`,
where `id` is the row's `id_hash` and `current` marks the row `client().session` names) and
`auth.session.revoke { id }` (`action`, owned by `account_id` and answering `not_found` for a row of
another principal of the same account when the caller is below `reseller`, `auth.session.manage` for
`[user, reseller]`); revoking a session, by `auth.session.revoke` or `auth.logout`, revokes its `su`
children through `parent_id_hash`, revoking an `su` child alone ends the delegation, since the
parent's `su_id_hash` then names a revoked row, and revoking the current one clears the cookie like
a logout.

**Touches.** `crates/system/` (auth: the settings contributions, the policy, `session.*`),
`crates/sdk/src/settings.rs` and `lib.rs` (`SettingDef`), `docs/design/05-modules.md` (the
contribution table's sentence).

**Out of scope.** Resolving the settings through the `settings` module (step 6a); a device list with
names.

**Acceptance.**

- `auth.password.set` with eight characters answers `422` on `password` `auth.error.password_short`,
  and with the login as the password `auth.error.password_login`.
- A1 lists its own sessions only, a second `user` principal the test creates in A1's account sees
  none of A1's sessions and is answered `not_found` for their ids, the reseller lists those of its
  subtree, the admin every session, and the row of the request carries `current: true`; A1 revokes
  its other session and that cookie answers `401`; a foreign session answers `not_found`
  (`assert_user_cannot_edit_foreign_row` on `auth.session.revoke`,
  `assert_reseller_sees_only_subtree` on `auth.session.list`); revoking a parent session revokes
  its `su` child, and revoking the child alone makes the next call answer as the parent with
  `actor: null`.
- `ctx.contributions::<SettingDef>()` in a test module yields the six definitions with the defaults
  above; the `auth.*` descriptors match their snapshots.

### 6a. settings: values and inheritance

**Goal.** The `settings` module (`id: "settings"`, no `requires`, ledger `keel_migration_settings`)
of [design/05-modules.md](design/05-modules.md). The table `settings_value` (`scope_id`, `key`,
`value`, `updated_at`, `updated_by`, keyed by `(scope_id, key)`, where `scope_id` is an account id).
Functions: `settings.list` (`list`, `reseller`, permission `settings.read` for `[reseller]`), whose
rows `{ key, module, schema, default, level, value, scope }` come from the `SettingDef`
contributions of the loaded set, cut to the definitions at or below the caller's level, with `value`
resolved for the caller's scope; `settings.get { key }` (`get`) and `settings.set { key, value }`
(`form`, `.unowned()`, `settings.manage` for `[reseller]`), which validates `value` against the
definition's schema with `jsonschema`, refuses a definition above the caller's level with `403
forbidden` `settings.error.level`, an unknown key with `not_found`, and writes the root scope for an
admin and the caller's account for a reseller. The automatic form the design describes for the
`SettingDef` contributions is what the shell renders in M2 from the rows of `settings.list`, each
carrying its schema, and submits through `settings.set`; this step amends that sentence of the
`settings` section in [design/05-modules.md](design/05-modules.md) to say so. Values inherit from
admin to reseller: a key resolves to the nearest value up the account path from the caller's account
to the root, then to the contributed default. Resolution is the public SDK trait
`keel_sdk::SettingsSource` (`resolve(key, account, db)`), provided by this module, behind
`Call::setting::<T>(key)`, which falls back to the contributed default and to a default-only source
the kernel provides when no module does, so a composition without `settings` keeps working; the
`auth` module switches its constants to `call.setting`. The menu node `system/settings` opens
`settings.list`.

**Touches.** `crates/system/` (the `settings` module, tests, snapshots; `auth` reading its
settings), `crates/sdk/src/` (`SettingsSource`, `Call::setting`), `crates/kernel/src/loader/` (the
default source), `bin/keel/` (the composition), `docs/design/05-modules.md` (the `settings`
section: the automatic form, `SettingsSource` in the public SDK).

**Out of scope.** `settings_pref`, preferences and the caller's time zone (step 6c);
`ManifestPatch` and white-label by hostname, which M0 step 10 pointed at M1 and which the M2 scope
now owns next to the shell's brand rule and the `/brand/logo.svg` route a white-label logo needs;
`ConfigSection` (step 6b); the settings screens themselves (M2); ACME and TLS settings (M3);
`LimitDef` values (M4).

**Acceptance.**

- `settings.list` as the admin lists the six `auth.*` definitions with their defaults; `settings.set
  auth.password.min_length = 16` makes `auth.password.set` refuse twelve characters; the reseller is
  answered `403` `settings.error.level` for an admin-level key, a value failing the schema `422` on
  `value`, an unknown key `not_found`.
- Inheritance: with a reseller-level definition from a test module, the admin sets a root value and
  the reseller its own; A1, under the reseller, resolves the reseller's value through
  `call.setting`, `Unrelated` the root's, and a fresh composition without `settings` resolves the
  contributed default.
- The `settings.list`, `settings.get` and `settings.set` descriptors match their snapshots.

### 6b. ConfigSection

**Goal.** The `ConfigSection` contribution of [design/09-operations.md](design/09-operations.md),
kernel and SDK work that touches nothing the `settings` module needs, so it may land any time after
step 1a. `ConfigSection` is a public SDK contribution, `ConfigSection::new::<T>("host")` with `T:
DeserializeOwned + JsonSchema + Default`: at freeze the kernel matches the raw top-level sections
its `Config` already keeps against the contributed sections, validates each against the schema,
fails the start naming the file, the section and the path on a violation, substitutes `T::default()`
for an absent section, and logs a warning for a top-level key no loaded module claims, so that the
section of a disabled product stays valid; the module reads its section as `ctx.config::<T>()` in
`start` and `call.config::<T>()` in handlers. `Composition::sections` hands the raw sections to the
loader, and `Harness::new().section(key, json)` feeds them in tests; `keel-sdk-probe` contributes a
section as the module template. The contribution table's sentence in
[design/05-modules.md](design/05-modules.md), amended by step 5b for `SettingDef`, names
`ConfigSection` in the public SDK as well.

**Touches.** `crates/sdk/src/` (`ConfigSection`, `Context::config`, `Call::config`),
`crates/kernel/src/loader/` (sections at freeze), `crates/kernel/src/config.rs`,
`crates/keel/src/runtime.rs` (the sections into the composition), `crates/testkit/src/harness.rs`
(`section`), `crates/sdk-probe/` (the sample section), `docs/design/05-modules.md` (the
contribution table's sentence).

**Out of scope.** The `host` and `plugins` sections (M4, M7); a section of a system module (none of
M1 needs one).

**Acceptance.**

- `keel-sdk-probe` contributes a `ConfigSection`; a harness with a valid section starts and
  `ctx.config::<T>()` carries it; a section of the wrong shape fails the boot naming `probe` and the
  path; an absent section yields the defaults; an unclaimed top-level key is logged and the boot
  succeeds.

### 6c. settings: preferences and the caller's time zone

**Goal.** The per-principal half of the `settings` section of
[design/05-modules.md](design/05-modules.md). The table `settings_pref` (`principal_id`, `key`,
`value`, `updated_at`, keyed by `(principal_id, key)`). Functions: `settings.pref.get` and
`settings.pref.set { key, value }` (`user`) for the caller's own `lang` (a language the catalogue
carries), `tz` (a zone name the `chrono-tz` table knows, else `422` on `value`), `theme` and
`columns.<func>`. `/boot` takes `tz` and a preferred `lang` from
`keel_sdk::system::PreferenceSource`, a hidden service this module provides; the HTTP transport
resolves both once per request, overlays that `lang` on the authenticated principal, which is where
[design/08-frontend.md](design/08-frontend.md) places the user's language, and hands `tz` to
`CallOptions`, read as `Call::tz()` (`Tz` of `chrono-tz`, UTC when no preference is set, as on the
socket); the `keel_principal.lang` column stays the fallback and what `keel.principal.*` edit. The
day-boundary rule of [design/10-contracts.md](design/10-contracts.md) follows from that: in the
`Owned` list source of `keel_sdk::list`, a filter on a `Datetime` column whose value is a bare
calendar date `YYYY-MM-DD` resolves to the day's boundaries in the caller's `tz` — `gte` and `lt` at
the day's start, `gt` and `lte` at the next day's start, `eq` the day as a range — while a full RFC
3339 value is compared as it is; a module with its own `ListSource` reads `call.tz()` for the same,
and the day-boundary sentence of the design gains the source and `Call::tz` by name.

**Touches.** `Cargo.toml` (the pinned `chrono-tz`), `crates/system/` (settings: the migration,
`pref.*`, `PreferenceSource`, tests, snapshots), `crates/sdk/src/` (`Call::tz`, the day-boundary
rule in `list.rs`, `PreferenceSource` under `system`), `crates/kernel/src/dispatcher/` (`tz` on
`CallOptions`), `crates/kernel/src/http/` and `boot.rs` (preferences), `products/demo/tests/` (the
day-boundary check), `docs/design/10-contracts.md` (the day-boundary sentence).

**Out of scope.** The `widgets` preference, the widget order of the design, which arrives with the
`DashboardWidget` contributions and the `dashboard` block of `/boot` (M3, as M0 step 10 says); the
preference screens of the shell (M2).

**Acceptance.**

- A1 sets `tz: Asia/Tokyo` and `lang: ru`; `/api/v1/boot` with A1's cookie shows both and
  `/api/v1/i18n` without `lang` serves `ru`; A2 cannot read A1's preferences; a zone name the table
  does not know answers `422` on `value`, and a key outside `lang`, `tz`, `theme` and
  `columns.<func>` answers `422` on `key`.
- In `products/demo/tests/`, with the clock frozen at `2026-09-13T20:00:00Z` and a note edited then,
  `note.list` with the filter `updated_at gte 2026-09-14` lists it for A1 in `Asia/Tokyo` and not
  for A2 without a preference, and `updated_at lte 2026-09-13` the other way round; `audit.list`
  repeats the check on `at` in step 7.
- The `settings.pref.*` descriptors match their snapshots.

### 7. audit

**Goal.** The `audit` module (`id: "audit"`, no `requires`, ledger `keel_migration_audit`) on the
in-process event of [design/02-functions.md](design/02-functions.md), per the `audit` section of
[design/05-modules.md](design/05-modules.md). The table `audit_call` carries `id`, `at`,
`request_id`, `function`, `module`, `principal_id` (`NULL` for a guest), `principal_login`,
`account_id`, `actor_principal_id`, `actor_login`, `via`, `ip`, `params_redacted`, `subject`,
`after`, `status` ∈ `committed | failed`, `error_code`, `error_key` and `elapsed_ms`, indexed by
`at`, by `(account_id, at)` and by `request_id`. The module provides the
`keel_sdk::system::AuditSink` the dispatcher already looks for; `record` pushes onto a bounded
channel of 10 000 events and returns at once, a worker spawned in `start` writes batches of up to
200 rows or every 250 milliseconds with one multi-row insert, a full channel drops the event and
counts it in the log rather than slowing a call, and `stop` drains the channel. For that the worker
needs a connection outside any call: `keel_sdk::system::ContextExt::db()` gives one, hidden, the
first consumer of the database handle the design says a module gets with the milestone that needs
it. `AuditEvent` grows by `client` (the `Client` of step 1a), `subject` and `result`: the dispatcher
sets `subject` to the value of the key parameter of a keyed function and `result` to the masked
result of a committed `form` function that has a key, so an object edit records what it left behind,
while the results of key-less forms — the plaintext of `auth.token.create`, the codes of
`auth.totp.confirm`, the key of `auth.key.issue` — never reach the trail; the `before` snapshot
stays `NULL` at M1, because it costs a read before the handler and nothing reads it until the
object-history screen, and the before/after sentences of the `audit` section in
[design/05-modules.md](design/05-modules.md) and of [design/02-functions.md](design/02-functions.md)
are amended to say that M1 records `after` alone. Masking is the dispatcher's, driven by the
`secret` attribute, applied to failed calls as well; `principal` and `actor` are the two sides of an
`su` session and the `as` frame. Functions: `audit.list` (`list`, `.owned::<call::Entity>()` by
`account_id`, permission `audit.read` for `[user, reseller]`, default sort `at` descending, filters
on `function`, `principalId`, `status`, `requestId` and `at`, whose `gte` and `lte` values are the
`from` and `to` of the screen and take a bare date under the day-boundary rule of step 6c) and
`audit.get { id }` (`get`, owned); the menu node `system/audit` opens the list. A guest's failed
login lands in the root account and is therefore visible to admins only. The module's `pub mod api`
exposes `Flush`, a service that awaits the worker, for tests and for the support bundle of M4.

**Touches.** `crates/system/` (the `audit` module, tests, snapshots), `crates/sdk/src/system.rs`
(`AuditEvent` members, `ContextExt`), `crates/sdk/src/context.rs` (the handle behind
`ContextExt::db()`), `crates/kernel/src/dispatcher/mod.rs` (subject and result),
`crates/kernel/src/loader/lifecycle.rs` (passing the handle at `Context::new`), `bin/keel/` (the
composition), `docs/design/05-modules.md` and `docs/design/02-functions.md` (the `after`-only
snapshot).

**Out of scope.** Auditing read functions on request (the opt-in arrives with the first function
that needs it), the `before` snapshot and the object-history screen (M2 and later), retention and
export of the trail, audit of raw routes (with the first raw route), the tail of the audit log in
the support bundle (M4).

**Acceptance.**

- After the `su` sequence of step 4a, `audit_call` holds the calls made inside the `su` session with
  `principal_login` A1 and `actor_login` the admin, and the `auth.su` and `auth.su.exit` rows
  themselves — both sides of `su`, the M1 exit item.
- A failed `auth.login` leaves a row with `status: failed`, `error_key` `auth.error.credentials` and
  `params_redacted.password == "***"`; a committed login masks it the same way; a committed
  `auth.password.set` records `params_redacted.current == "***"`; the trail contains no value
  starting with `ks_`, `kat_` or `kok_` after the token, key and factor flows of steps 2 to 4c ran.
- A read call leaves no row; an idempotent replay leaves no second row; `request_id`, `ip` and `via`
  are filled, `via` reading `http` for a cookie call and `socket` for a frame, and a frame with `as`
  records the owner as actor.
- `note.edit { id }` records `subject` as the id and `after` as the masked result;
  `auth.token.create` records neither.
- `Flush` returns after the batch is written; a test that records 5 000 events sees 5 000 rows after
  it, and `finish()` on a harness with pending events leaves none unwritten.
- `audit.list` filters by function, principal, status, request id and time, where `at gte
  2026-09-14` as A1 in `Asia/Tokyo` includes a row written at `2026-09-13T20:00:00Z` by the frozen
  clock and excludes it for A2 in UTC; the reseller sees the rows of its subtree, A1 those of its
  account, the admin the guest's failed logins; `assert_reseller_sees_only_subtree` holds for
  `audit.list`; the descriptors match their snapshots.

### 8a. Postgres through testcontainers and the keel-system matrix

**Goal.** The CI part of the M1 scope. The milestone text says testcontainers replace the service
container, and this step decides the split. CI keeps the Postgres service, because
`hack/prove-binary.sh`, the conformance run of step 8b and `hack/check-test-databases.sh` all need a
server that outlives one test process, and the leftover check is meaningless against a container
that dies with the process. testcontainers-rs joins `keel-testkit` for the other case: when
`KEEL_TEST_DATABASE_URL` is unset, `TestDatabase` starts one pinned `postgres:16` container through
`testcontainers-modules`, reused across test processes by the reuse directive under one container
name, so a workspace run starts it once and a developer needs no local server; the URL it used is
printed once. CI proves that path by running `cargo test -p keel-testkit` without the variable in a
separate step, and adds the `keel-system` matrix the design asks for: the crate checked with each
module alone and without each in turn. The M1 scope line "Postgres through testcontainers in CI",
the note of M0 step 1 that says M1 replaces the service container and the sentence of
[design/05-modules.md](design/05-modules.md) that says testcontainers replace the service in M1 are
amended by this step to what the split means: the testkit starts its own Postgres when no server is
named, CI proves that path in a step of its own, and the service stays for the checks that need a
server outliving one process.

**Touches.** `.github/workflows/ci.yml`, `Cargo.toml` (the pinned `testcontainers`,
`testcontainers-modules`), `crates/testkit/src/database.rs`, `docs/plan.md` (the M1 scope line, the
note of M0 step 1), `docs/design/05-modules.md` (the testkit paragraph).

**Out of scope.** The conformance suite (step 8b); the musl target and the `ldd` check (M4).

**Acceptance.**

- `cargo test -p keel-testkit` with `KEEL_TEST_DATABASE_URL` unset starts a container and is green
  in CI; with the variable set no container is started; `hack/check-test-databases.sh` still finds
  nothing left on the service.
- The `keel-system` matrix is green: `--features auth`, `--features settings`, `--features audit`,
  and the crate without each in turn.

### 8b. keel-conformance skeleton and hack/conformance.sh

**Goal.** `crates/conformance` (package `keel-conformance`, a binary), a black-box client of the
started binary typed with `keel-types` and nothing else of the workspace, on `reqwest` with rustls;
it keeps the session cookie itself, because a standards-following cookie store does not return a
`Secure` cookie over the plain HTTP the binary listens on until M3. It takes `--url`, the
credentials of an admin, and the login of a user in another account, prints one line per check and
exits non-zero on any failure. The skeleton checks: `/healthz` and `/readyz` bodies; `/api/v1/boot`
as a guest (`principal: null`, the platform group only, `meta` equal to the `ETag`, `304` on
`If-None-Match`); the auth cycle `auth.login` → `boot` → `keel.account.list` → `auth.su` into the
user, after which the list is narrowed and `meta.principal.actor.via` is `su` → `auth.su.exit` →
`auth.logout` and `401` after it; a token from `auth.token.create` scoped to one function answering
`200` on it and `403 forbidden` on another; the list contract on `keel.account.list` and `note.list`
(`pageSize` capped, `sort` on a column not marked sortable `422`, `filters`, `search`, `total`); the
error envelope shapes (`404` for an unknown function, `401` for a guest on a user function, `403`
for the user on an admin function, `422` with `fields[]` for a missing required field, `403
forbidden_field` with `fields[].path == "account"` for the user sending `account` to `note.edit`,
`403` `keel.error.cross_site` for a cookie request with a foreign `Origin`, every envelope carrying
`requestId` and every response `X-Keel-Request-Id`); and `X-Keel-Meta-Version` on calls.
`/api/v1/instance` is M6 and is skipped; idempotency, webhooks and `svc.*` join the suite with their
milestones. `hack/conformance.sh <binary>` prepares a fresh database and `data_dir` as
`prove-binary.sh` does, enables `demo`, creates the admin, a second account with a user and their
passwords over `ctl`, starts `serve`, runs the suite and stops the process. CI runs it after the
binary proof, and the README's command list gains it in CI's order.

**Touches.** `.github/workflows/ci.yml`, `Cargo.toml` (members; the pinned `reqwest`),
`crates/conformance/`, `hack/conformance.sh`, `hack/check-deps.sh` (the conformance rule),
`README.md` (the command list in CI's order).

**Out of scope.** Running the suite with each first-party bundle alone and with all of them (there
is one bundle before M4); the field-shown-iff-accepted check, which needs descriptors rendered by
the frontend (M2); idempotency, webhooks and `svc.*` (M3, M6); `GET /api/v1/instance` (M6).

**Acceptance.**

- `hack/conformance.sh target/debug/keel` runs in CI after `hack/prove-binary.sh` and is green; the
  binary prints one line per check; a deliberately broken expectation, in the suite's own unit test
  against a canned server, makes it exit non-zero.
- The suite covers the three M1 exit items observable over HTTP — the `su` sequence, the scoped
  token's `403` and `forbidden_field` — and its green run is the fourth.
- `hack/check-deps.sh` asserts that `keel-conformance` depends on `keel-types` and on no other
  workspace crate.

### 9. M1 closure

**Goal.** Close the milestone the way the M0 checklist did. Run every M1 "Done when" item by hand or
through the conformance suite and record for each the test name or the command that shows it; add
the section "M1 exit checklist" after the M0 checklist, in the same form; and make the final
consistency pass over `docs/design/` and this plan against the built M1. Steps 1a and 2 amended
the sentences they diverged from in their own change; steps 3a to 8b left theirs to this step —
the delegation paragraph of the access design, the `as` frame and the day-boundary sentence of
the contracts, the `auth`, `settings`, `audit`, hidden-hooks, contribution-table and testkit
paragraphs of the module design, the one-click login example, the `after`-only snapshot, the
testing paragraph of the storage design and the `hack/` row of the overview — next to the plan's
own notes: the opening paragraph of this plan, which named only the M0 breakdown, the M1 scope
line and the note of M0 step 1 on testcontainers, the `ManifestPatch` note of M0 step 10, which
now points at M2, and any one-line mismatch the pass reveals. `README.md` follows the code and
is edited by step 8b, whose command list this step re-checks and extends by `hack/check-docs.sh`;
its M1 row is set to done. The check is
`hack/check-docs.sh`, which carries the list of M1 names — the authenticator chain, `Client`,
`Authenticated`, `SessionChange`, `SecretCipher`, `SecondFactorSource`, `parent_id_hash`,
`su_id_hash`, `SettingDef`, `SettingsSource`, `PreferenceSource`, `Call::tz`, `ConfigSection`,
`Flush` — and asserts that each is found under both `docs/design/` and `crates/`, that every test
name the M1 exit checklist cites is defined under `crates/` or `products/` and every command it
cites exists under `hack/` or in `.github/workflows/ci.yml`, and that every command of the README's
check list is a `run:` command of `ci.yml` and is named in the "How we work" section; CI runs it
after `hack/check-deps.sh`.

**Touches.** `docs/plan.md`, `docs/design/*.md` (the consistency pass), `README.md` (the M1 row,
`hack/check-docs.sh` in the command list), `hack/check-docs.sh`, `.github/workflows/ci.yml`.

**Out of scope.** Any code change beyond what a document alignment reveals as a one-line mismatch;
anything of M2.

**Acceptance.**

- Every "Done when" item of M1 is ticked in the new checklist with the name of the test or the
  command that demonstrates it, and every item is green on `main`.
- `hack/check-docs.sh` is green in CI: every M1 name it carries is found under both `docs/design/`
  and `crates/`, every test name the checklist cites is defined, every command it cites exists, and
  the README's check list, the "How we work" section and `ci.yml` agree, `hack/conformance.sh`
  included.

## M2 work breakdown

M2 is split into twenty-five PR-sized steps in twelve groups, ordered by dependency; a step is one
change to `main` and is done when its acceptance holds in CI, and the letters of a group (`7a`,
`7b`) name the steps of one delivery that land separately, as the M1 breakdown named them. The steps
derive strictly from the design; where a step names a mechanism the design leaves open, it says so,
and the commit message of the step records the choice. Unlike the M1 breakdown, steps 1 to 11e touch
nothing under `docs/`: where a step diverges from a sentence of the design or fills a gap the design
leaves, its commit message names the sentence and the choice, and step 12 applies every owed
amendment together with the exit checklist, `hack/check-docs.sh` and the consistency pass, so the
design changes once, against the built M2.

Seven remarks apply to the whole breakdown.

- The frontend is the pnpm workspace `frontend/` of [design/08-frontend.md](design/08-frontend.md):
  the one Angular library `frontend/libs/keel` (`@globalart/keel`, built by ng-packagr) with the
  secondary entry points `meta`, `client`, `forms`, `renderer`, `shell` and `testing`, the
  application `frontend/apps/shell` built to `frontend/dist/shell`, and one lazily loaded view
  library per product with custom views under `frontend/products/<id>`, of which M2 has `demo`
  alone. pnpm 11 and node 24 are pinned in `engines` and `packageManager` and used by CI through
  `pnpm/action-setup` and `actions/setup-node`; Angular is 19.1, the version the sibling monorepo
  installs and inside the `^19.0.0` range `@globalart/phoenix-ui` declares for its peers, and
  `@globalart/keel` declares `^19.1` for its own; unit tests run under Jest through
  `jest-preset-angular` as in the sibling monorepo, ESLint and prettier 3 lint and format, Storybook
  10 renders the renderer and the shell from fixtures, and Playwright drives the built binary. The
  `paths` of `frontend/tsconfig.base.json` resolve `@globalart/keel/<entry>` to the library sources,
  so the shell, the product libraries, Jest and Storybook build from source and the ng-packagr build
  exists for publication alone.
- `@globalart/phoenix-ui` comes from the private Nexus registry
  `https://nexus.globalart.hu/repository/npm-hosted/`, which admits no anonymous read:
  `frontend/.npmrc` binds the scope `@globalart` to it and reads the token from
  `${NEXUS_NPM_TOKEN}`, which CI takes from the repository secret of that name and a developer keeps
  in the environment or in `~/.npmrc`, never in the repository. The version is pinned exactly,
  `0.1.0` until step 5 lands and the batch's release after it, `0.2.2` once its two patch releases
  landed. The boundary of the design holds throughout:
  phoenix-ui receives atomic components and the Keel runtime carries no workaround for a gap of the
  design system, so the four gaps of batch 1 close in step 5, which lives in the globalart monorepo;
  step 6a is its first consumer and starts with the pin.
- The frontend is a browser client of the M1 kernel, which listens on plain HTTP until TLS arrives
  in M3, while the session cookie is `__Host-keel_session` with `Secure`. A browser accepts such a
  cookie on plain HTTP from `localhost` and `127.0.0.1` alone, which is where Playwright,
  `hack/e2e.sh` and the demo through an SSH port forward run; any other plain-HTTP origin gets no
  session before M3, and the plan says so rather than weakening the cookie. The client sends no CSRF
  token of its own: the kernel checks `Origin` and `Sec-Fetch-Site`, which the browser sets on every
  request, so the client only sends its cookie same-origin and never adds a header for it.
- Steps 3 to 8d test against the golden fixtures of `contracts/fixtures/`, above all
  `descriptor-dns-zone-list.json`, `descriptor-dns-zone-edit.json`, the envelopes,
  `list-query.json`, `page.json` and `boot.json`, which already carry every mechanism the renderers
  must handle, handed to the Angular specs through the `KeelMockClient` step 4 brings; a fixture set
  taken from the running binary, projected per level, arrives with step 11a as `contracts/dump/` and
  feeds Storybook, the mock client's dump loader and the custom-views metric.
- The M2 exit criteria need more of the `demo` bundle than M0 gave it: a list with filters, a form
  with `recompute` and a field-level error, a wizard and a screen a custom view replaces. Step 7b
  adds them to `products/demo` as plan-level shapes, named there; the design fixes the mechanisms,
  not the demo's fields.
- The M2 scope carries three server halves next to the frontend: the view registry `r.view` behind
  `/boot.views` (step 7a), stored permission grants through `PermissionSource` behind the shell's
  permissions screen (step 8a), and white-label by hostname through `ManifestPatch` next to the
  `/brand/logo.svg` route the embedded frontend brings (step 9b). Each is the server step of the
  group whose screen stands on it, in `crates/kernel`, `crates/sdk` and `crates/system`, tested the
  way M1 tested its modules, so a Rust change and a frontend change are reviewed and reverted
  apart; nothing of them touches `docs/` before step 12.
- Every screen of a system module that exists as a descriptor is rendered generically by the shell.
  The shell adds a screen of its own only where a descriptor cannot say it: the settings value form,
  whose control comes from the schema the setting's row carries, and the security page with the
  second-factor enrolment. Those are custom views of the shell, listed in
  `frontend/apps/shell/views.json`, and count in the custom-views share of the composition that step
  11a puts into CI.

### 1. Frontend workspace: pnpm, Angular 19, phoenix-ui from Nexus, lint/test/build in CI

**Goal.** Create `frontend/` as the pnpm workspace of [design/08-frontend.md](design/08-frontend.md)
and [design/01-overview.md](design/01-overview.md), empty of runtime code but complete as tooling,
so that every later step lands as source under a green `pnpm lint && pnpm test && pnpm build`.
`frontend/package.json` is private, carries `packageManager: pnpm@11.1.1` and
`engines { node: "24", pnpm: "11" }` with `engine-strict=true` in `.npmrc`, and the scripts `lint`
(ESLint 9 with angular-eslint over the workspace), `test` (Jest with one project per package),
`build` (`ng build keel` then `ng build shell`) and `format` (prettier 3 `--check`;
`.prettierignore` at the root only, since prettier 3 is the version that formats `@if` and `@for`).
`pnpm-workspace.yaml` lists `libs/*`, `apps/*` and `products/*`; `frontend/.npmrc` carries
`@globalart:registry=https://nexus.globalart.hu/repository/npm-hosted/` and
`//nexus.globalart.hu/repository/npm-hosted/:_authToken=${NEXUS_NPM_TOKEN}`; `pnpm-lock.yaml` is
committed and CI installs with `--frozen-lockfile`. `angular.json` declares the project `keel`, the
library `frontend/libs/keel` with `ng-package.json` and a secondary entry point per directory
`meta`, `client`, `forms`, `renderer`, `shell`, `testing`, each with its own `ng-package.json` and
`src/public-api.ts` exporting a version constant and nothing else yet, and the project `shell`, the
application `frontend/apps/shell` on the `application` builder with `outputPath: { base:
"dist/shell", browser: "" }`, so `index.html` lands in `frontend/dist/shell/` and not under
`browser/`, output hashing on, the phoenix styles and the IBM Plex fonts in `styles`, and the
budgets of the sibling monorepo, `anyComponentStyle` 8 kB warning and 16 kB error; the application
renders a placeholder on `phx-fullscreen-layout` with the platform name that step 8b replaces.
`frontend/libs/keel/package.json` names `@globalart/keel` at the workspace version `0.1.0` with
`peerDependencies` on `@angular/*` `^19.1`, `@angular/cdk`, `lucide-angular`, `rxjs` and
`@globalart/phoenix-ui` pinned to `0.1.0`; the library's version is the binary's until SDK 1.0 in
M7, from which the rule of [design/08-frontend.md](design/08-frontend.md) that its major equals the
descriptor version applies — the plan's choice, which step 12 records. `frontend/tsconfig.base.json`
is strict and maps `@globalart/keel/*` to `libs/keel/*/src/public-api.ts`.
`.github/workflows/ci.yml` gains the job `frontend` with `working-directory: frontend`: checkout,
`pnpm/action-setup@v6` (the version from `packageManager`; the `v4` tag predates pnpm 11),
`actions/setup-node@v4` with node 24,
`cache: pnpm` and `cache-dependency-path: frontend/pnpm-lock.yaml`, `pnpm install --frozen-lockfile`
with `NEXUS_NPM_TOKEN: ${{ secrets.NEXUS_NPM_TOKEN }}` in its environment, then `pnpm lint`,
`pnpm test` and `pnpm build`. The repository is private and pushes go to `main`, so the secret is
always present; a pull request from a fork would fail at the install step, which is the intended
answer. `.gitignore` gains `frontend/node_modules`, `frontend/dist` and `frontend/.angular`.

**Touches.** `frontend/` (`package.json`, `pnpm-workspace.yaml`, `pnpm-lock.yaml`, `.npmrc`,
`angular.json`, `tsconfig.base.json`, `jest.config.ts`, `jest.preset.js`, `eslint.config.mjs`,
`.prettierrc`, `.prettierignore`, `libs/keel/` with its six entry points, `apps/shell/`),
`.github/workflows/ci.yml` (the job `frontend`), `.gitignore`.

**Out of scope.** Any runtime code of the entry points (steps 3 to 8d), the generated types (step
2), Storybook and Playwright (steps 11d and 11e), the publish workflow of `@globalart/keel` and the
embedding (step 9a), the README's command list and the "How we work" section (step 12).

**Acceptance.**

- The job `frontend` of `.github/workflows/ci.yml` is green: `pnpm install --frozen-lockfile`
  resolves `@globalart/phoenix-ui@0.1.0` from Nexus with the token, and `pnpm lint`, `pnpm test` and
  `pnpm build` pass; `pnpm-lock.yaml` records `@globalart/phoenix-ui@0.1.0` by its integrity alone,
  the registry being `frontend/.npmrc`'s at install time, which is why the unset-token run below
  must fail with `401` from Nexus and not resolve elsewhere.
- With `NEXUS_NPM_TOKEN` unset, `pnpm install` fails naming the Nexus registry and `401`, which
  shows that the scope resolves nowhere else; with node 22 or pnpm 10 it is refused by
  `engine-strict`.
- `pnpm build` writes `frontend/dist/shell/index.html`, which names a hashed `main-*.js`, and
  `frontend/dist/keel/package.json`, whose `exports` name the six entry points.
- `pnpm test` runs one spec per entry point and one for the shell under `jest-preset-angular`, and
  the spec of the shell renders the placeholder through `@globalart/keel/shell` resolved from
  source; `pnpm format` is green.

### 2. keel-dev types: TypeScript types from the JSON Schema of keel-types, shared fixtures

**Goal.** The developer CLI `keel-dev` of [design/05-modules.md](design/05-modules.md) in its first
subcommand, and the rule of [design/10-contracts.md](design/10-contracts.md) that the TypeScript
types of `@globalart/keel/meta` are generated from the JSON Schemas of `keel-types`, never written
by hand. `crates/dev` (package `keel-dev`, binary `keel-dev`, on `clap`) depends on `keel-types`
alone, the rule `hack/check-deps.sh` already holds for `keel-conformance`.
`keel-dev types --out contracts/schema` writes `contracts/schema/keel.json`, one draft 2020-12
document whose `$defs` hold every root type named below and everything they reach, produced by one
`SchemaGenerator` — each root added through `subschema_for`, the definitions taken once — and beside
it one thin file per root type, `contracts/schema/<Type>.json` =
`{ "$ref": "keel.json#/$defs/<Type>" }`, which is what Ajv compiles; the root types are
`Descriptor`, `ScreenUi`, `KeelAttrs`, `UiAttrs`, `Envelope`, `CallError`, `ListQuery`, `Page`,
`OptionsQuery`, `Choices`, `Expr`, `Context`, `Boot`, `Principal`, `Account`, `Event` and the closed
enums `Kind`, `Mode`, `Control`, `ColumnType`, `ActionRole`, `Check`, `ErrorCode`, `Level`, `Tone`,
`Total`, `ScreenResult`. `Envelope` and `Page` are generic in Rust and are added as
`Envelope<serde_json::Value>` and `Page<serde_json::Value>`, the `$defs` names forced to `Envelope`
and `Page` through `SchemaSettings` transforms, so their `data` is `unknown` in TypeScript; `/meta`
exports the two generic aliases `Envelope<T = unknown>` and `Page<T = unknown>` in `generic.ts`,
narrowing `data` over the generated shapes — the one hand-written file of the entry point, which
`pnpm generate` never touches. `--check` rewrites nothing and exits non-zero when a file would
change, and CI runs it. The TypeScript side is `pnpm generate` in `frontend/`:
`json-schema-to-typescript` over `contracts/schema/keel.json` alone into
`frontend/libs/keel/meta/src/generated/keel.ts`, every `$defs` entry becoming one exported type
declared once, and an `index.ts` that re-exports it, with `additionalProperties: false` handed to
the generator so no object gains an index signature and `frontend/tools/generate.mjs` rewriting,
before it compiles, every `prefixItems` into the array form of `items` (an `items: false` beside it
becoming `additionalItems: false`), since `json-schema-to-typescript` 16 reads the draft-07 tuple
form alone while the schema stays draft 2020-12 for Ajv, and prettier over the output; the closed
enums arrive as string-literal unions. The generated files are committed, and the job `frontend`
regenerates them and fails on a difference. The same fixtures Rust loads are loaded by Jest:
`frontend/libs/keel/meta/src/fixtures.spec.ts` reads `contracts/fixtures/` through the repository
path, mirrors the `KNOWN` list of `crates/types/tests/types/fixtures.rs` so that a fixture nobody tests
fails on both sides, and validates every fixture against its per-type schema with `Ajv2020` from
`ajv/dist/2020` and `ajv-formats`, `keel.json` registered under its `$id` so the `$ref` resolves,
since `keel-dev types` writes draft 2020-12, the draft `jsonschema` validates on the kernel side:
the two descriptors against `Descriptor`, the three envelopes against `Envelope`, `list-query.json`,
`page.json`, `boot.json` and `event.json` against their types, the `ast` of every valid case of
`expr-cases.json` against `Expr`, the values of `error-codes.json` and of the `check` member of
`check-cases.json` against the enum schemas.

**Touches.** `Cargo.toml` (the member is covered by `crates/*`), `crates/dev/` (`Cargo.toml`,
`src/main.rs`, `src/types.rs` and its unit test), `contracts/schema/` (`keel.json` and the per-type
`$ref` files), `frontend/tools/generate.mjs`, `frontend/package.json` (`generate`,
`json-schema-to-typescript`, `ajv`, `ajv-formats`), `frontend/libs/keel/meta/src/generated/`,
`frontend/libs/keel/meta/src/generic.ts`, `frontend/libs/keel/meta/src/fixtures.spec.ts`,
`.github/workflows/ci.yml` (`cargo run -p keel-dev -- types --out contracts/schema --check` in the
job `check`; `pnpm generate` and `git diff --exit-code` in the job `frontend`), `hack/check-deps.sh`
(the `keel-dev` rule).

**Out of scope.** `keel-dev descriptors` and `views` (step 11a) and `openapi` (step 11b); `check`,
`i18n`, `new-module`, `new-product`, `migrate create` and `docs` (the milestones that need them);
the condition interpreter and the validators (step 3); any hand-written type in `/meta` beyond
`generic.ts`.

**Acceptance.**

- `cargo run -p keel-dev -- types --out contracts/schema --check` is green in CI, and the unit test
  of the subcommand writes the committed `keel.json` with one property of one type removed into a
  temporary `--out` directory beside the untouched per-type files and asserts that
  `types --out <tmp> --check` exits non-zero naming `keel.json`, while the unchanged directory exits
  zero.
- `contracts/schema/keel.json` holds under `$defs` `Kind` enumerating the ten kinds, `Check` the
  eight presets, `Expr` referring to itself, `Descriptor` admitting `params` and `result` as an
  object or a boolean, and `Envelope` and `Page` under those names with `data` unconstrained;
  `Expr.json`, `Boot.json` and the other per-type files are one `$ref` each into it.
- `pnpm generate` leaves `frontend/libs/keel/meta/src/generated/` unchanged in CI; `Kind`,
  `Control`, `ColumnType`, `ActionRole`, `Check`, `ErrorCode` and `Level` are string-literal unions,
  no generated file carries `[k: string]: unknown`, the generated `Expr` types `eq` as
  `[string, unknown]` and `and` as `Expr[]`, and `Envelope<Page<Account>>` from `generic.ts` types
  `data.data[0].id` as a string.
- `fixtures.spec.ts`: "every fixture file has a test" fails when a file is added to
  `contracts/fixtures/` without a case, and every fixture validates against its schema; a descriptor
  fixture with `"kind": "table"` is refused by `Ajv2020`.
- `hack/check-deps.sh` asserts that `keel-dev` depends on `keel-types` and on no other workspace
  crate.

### 3. @globalart/keel/meta: condition interpreter, validation subset, check presets

**Goal.** The Angular-free half of the runtime, per the `/meta` section of
[design/08-frontend.md](design/08-frontend.md) and the conditions of
[design/02-functions.md](design/02-functions.md), exercised under Jest against the fixtures Rust
runs. `evaluate(expr, context)` mirrors `crates/types/src/expr/eval.rs` operator by operator — `eq`,
`ne`, `gt`, `lt`, `in`, `matches`, `truthy`, `empty`, `and`, `or`, `not` — with the same value
model, the same comparison of numbers and strings, the same regular-expression failure as an error
rather than `false`, and the same variable resolution: a dotted field path into `fields`, `$mode`,
`$principal.level` and `$screen.params.<name>`, `null` for anything unresolved; the client never
parses the source form, which the server compiles at start. The validators are the subset the design
names, applied to a parameter schema with its `$defs` resolved: `type`, `required`, `minimum`,
`maximum`, `minLength`, `maxLength`, `pattern`, `enum`, `format` (`date`, `date-time`, `email`) and
the `x-keel.check` presets `domain`, `ip`, `cidr`, `email`, `int`, `name`, `port`, `path`, ported
from `crates/types/src/check.rs` rule by rule, IP parsing included, since a regular expression would
disagree with Rust's parser on the cases the fixtures carry. A violation is a `FieldError` in the
shape of the envelope — `path` as a JSON path with array indices (`records[1].ip`), `code`
`schema.<keyword>` or `check.<preset>`, `key` `keel.schema.<keyword>` or `keel.check.<preset>`,
`params` with the offending `value` — so the form of steps 6a and 6b shows a client-side finding and
a server-side one through one code path; a preset applies to a string field and to every string item
of an array at any depth, as the kernel does. The key convention of the design is code here too,
since every later entry point needs it and none of it is Angular: `derivedKeys(descriptor)` yields
the field, enum, action, page, column and column-value keys of a descriptor exactly as the table in
[design/08-frontend.md](design/08-frontend.md) derives them, `lookup(catalogue, key)` walks the
nested catalogue and falls back from `<module>.<entity>.<tail>` to the shared `keel.<tail>`, and
`format(message, params)` substitutes the single placeholder form `{name}`.

**Touches.** `frontend/libs/keel/meta/src/` (`expr.ts`, `validate.ts`, `check.ts`, `i18n.ts`,
`public-api.ts`, their specs).

**Out of scope.** Angular validators and the mapping onto controls (step 6a), the catalogue loading
(step 4), the compile-time parser of conditions (server only), the conformance check that a field is
shown if and only if the server accepts it (`keel-conformance`, with the first descriptor rendered
by the frontend in step 11a's dump).

**Acceptance.**

- `expr.spec.ts` runs every case of `contracts/fixtures/expr-cases.json`: for a valid case
  `evaluate(case.ast, case.context)` equals `case.expect`, and the case count asserted is the one
  Rust asserts; a `matches` with an invalid pattern throws.
- `check.spec.ts` runs every case of `contracts/fixtures/check-cases.json` and asserts that the
  eight presets are all covered, the mirror of `check_cases_match_their_expected_verdict`.
- `validate.spec.ts`: against the `params` of `descriptor-dns-zone-edit.json`, `{ name: "a.org",
  kind: "slave", master_ips: ["10.0.0.1", "10.0.0"], ttl: 300 }` yields one error,
  `path: master_ips[1]`, `code: check.ip`, `key: keel.check.ip`, `params.value: "10.0.0"`, the entry
  of `envelope-422.json`; a missing `name` yields `schema.required` on `name`; a `ttl` below the
  schema's minimum yields `schema.minimum` with the bound in `params`.
- `i18n.spec.ts`: the keys derived from `descriptor-dns-zone-list.json` include
  `dns.zone.column.name`, `dns.zone.status.active`, `dns.zone.action.suspend` and
  `dns.zone.column.records`; `lookup` answers `keel.column.createdAt` for a missing
  `dns.zone.column.createdAt`; `format("The note {title} is locked", { title: "x" })` substitutes
  and leaves `100%` alone.

### 4. @globalart/keel/client: call, list adapter, SSE stub, session and meta-version handling

**Goal.** The HTTP client of the `/client` section of [design/08-frontend.md](design/08-frontend.md)
over the routes of [design/10-contracts.md](design/10-contracts.md), as Angular services on
`HttpClient` with `withFetch()`, provided by `provideKeelClient()`. `KeelClient` is the abstract
class the service of this step implements and every later entry point injects, so a test provides a
double in its place. `KeelClient.call<T>(func, params, options)` posts `POST /api/v1/call/{func}`
with a JSON body and the cookie sent same-origin, and answers the envelope typed from `/meta`,
`Envelope<T>` of `generic.ts`; `options` carry an `idempotencyKey` (sent as `Idempotency-Key`), a
`requestId` (sent as `X-Keel-Request-Id`) and, for tools and tests, a `bearer` token, which is the
only case the client sets `Authorization`. Every response feeds three signals: `lastRequestId` from
`X-Keel-Request-Id`, kept for error display; `replayed` from `X-Keel-Idempotent-Replay`; and
`metaVersion` from `X-Keel-Meta-Version`, compared with the version `/boot` answered, so a differing
value emits `metaChanged`, on which `KeelDescriptors` drops its cache and the shell refetches
`/boot`. A `401` with `keel.error.unauthenticated`, the transport's answer when no credential is
valid, emits `unauthenticated` once per burst, which the shell of step 8b turns into the login
route; every other error envelope is returned as it is, a function's own `401` such as
`auth.error.credentials` and `403` `keel.error.cross_site` included, since the browser and the
kernel own that check. Retries are the design's rule: a call
that carries an idempotency key is retried once on a network failure without a response, a call
without one never. `KeelBoot.load()` fetches `GET /api/v1/boot` and keeps the payload as signals —
`principal`, `products`, `brand`, `menu`, `views`, `features`, `meta` — sending `If-None-Match` only
for a refetch inside one page life; `KeelDescriptors.get(func)` fetches
`GET /api/v1/functions/{func}` and `list()` the index, cached in memory under the meta version;
`KeelI18n.load(lang)` fetches `GET /api/v1/i18n?lang=` and exposes `t(key, params)` through the
helpers of `/meta`, the catalogue's `ETag` kept for the same refetch rule. `KeelAuth` wraps the
login functions of `crates/system/src/auth` by their real shapes: `login({ login, password })`
answers `{ state: "authenticated", principal }` or `{ state: "totp", challenge }`,
`verifyTotp({ challenge, code })`, `redeemKey({ key })` answering
`{ redirect, principal, issuedBy }`, `logout()`, `su({ principal })` and `suExit()`, each followed
by a boot reload. The list adapter is `listFetch(func, scope)` answering a `PhxDataTableFetch<Row>`:
it maps `PhxDataTableQuery { page, pageSize, search, sort, filters }` onto `ListQuery { page,
pageSize, search, sort: { key, dir }, filters: [{ key, value }], scope }`, drops the table's
`offset` and `limit`, which are derived, calls the function and maps `Page { data, total, totals }`
onto `{ data, total }`, keeping `totals` on a companion signal for the footer; it posts, since `GET`
is permitted for lists and not required. `KeelDates` converts the wire's RFC 3339 through the
principal's `tz` from `/boot` with `Intl.DateTimeFormat` and produces the bare `YYYY-MM-DD` of a
date filter in that zone, the client side of the day-boundary rule of M1. In-flight calls are
counted into `PhxGlobalProgressService`. The event stream is a stub, since the kernel serves none
before M3: `KeelEvents.subscribe()` opens an `EventSource` on the path the token `KEEL_EVENTS_PATH`
names, `/api/v1/events` as the plan's provisional value for M3 to confirm, with credentials,
dispatches frames by type into `taskProgress$`, `notification$`, `dataInvalidate$` and
`metaInvalidate$`, reconnects with a backoff doubling from one second to thirty and relies on the
browser to resend `Last-Event-ID`; against the M1 kernel the route answers `404`, the stub stays
quiet at backoff pace and reports `closed`, and the list renderer of step 7d is written against
these streams now so that M3 changes the kernel alone. `@globalart/keel/testing` gains its first
member here: `KeelMockClient`, the `KeelClient` over handed fixtures — a `boot` payload,
descriptors, a catalogue and canned answers per function, an envelope or a sequence of them — that
records every call with its function, params and options for assertions, which steps 7c to 8d test
against; step 11d adds the dump-directory loader and the Storybook and Playwright helpers.

**Touches.** `frontend/libs/keel/client/src/` (`client.ts`, `boot.ts`, `descriptors.ts`, `i18n.ts`,
`auth.ts`, `list.ts`, `dates.ts`, `events.ts`, `progress.ts`, `provide.ts`, specs on
`HttpTestingController` and a fake `EventSource`), `frontend/libs/keel/testing/src/mock-client.ts`
and its spec. Kernel routes consumed: `GET /api/v1/boot`, `GET /api/v1/functions`,
`GET /api/v1/functions/{func}`, `POST /api/v1/call/{func}`, `GET /api/v1/i18n`; the provisional
`/api/v1/events`.

**Out of scope.** `keel.export` and the export button (the kernel gains the function with the row
threshold in M3, where the scope line names them, and step 12 records the deferral against the
`/client` row of the design); a real event stream, the progress modal and the notification centre
(M3); the login route and what the shell does on `unauthenticated` (step 8b); a browser session on a
`Bearer` token; the dump loader of the mock client and the principal fixtures (step 11d).

**Acceptance.**

- `client.spec.ts`: "call posts the body and keeps the request id and the meta version" — the
  request is a `POST` with a JSON body and no `Authorization`, the response headers set
  `lastRequestId` and `metaVersion`, and a version other than the booted one emits `metaChanged`
  once; "a bearer option sets the header and nothing else does"; "a 401 emits unauthenticated once
  and a 403 cross_site is returned as the envelope".
- "a write with an idempotency key is retried once on a network failure and a call without one
  never", with `X-Keel-Idempotent-Replay` on the retry's answer setting `replayed`.
- `list.spec.ts`: "the adapter maps a table query onto the list query and the page back" — a table
  query for page 2, 25 rows, search `www`, sort `name` ascending, the filter `type` in `[A, AAAA]`
  and the scope `zone_id` produces the request of `contracts/fixtures/list-query.json` less its `op`
  filter, and the answer of `page.json` comes back as `{ data, total }` with `totals` on the signal.
- `descriptors.spec.ts`: "descriptors are cached under the meta version and dropped when it changes"
  — two `get` calls make one request, and one more after `metaChanged`.
- `events.spec.ts` with a fake `EventSource`: "the stub reconnects with a doubling backoff and
  dispatches frames by type" and "a 404 leaves the stub closed and quiet".
- `dates.spec.ts`: `2026-09-13T20:00:00Z` formats as the 14th in `Asia/Tokyo` and as the 13th in
  UTC, and `dayFilter` answers `2026-09-14` and `2026-09-13` respectively.
- `auth.spec.ts`: the six functions post to their names with the shapes above, `redeemKey` posts
  `{ key }` and answers `redirect`, and each is followed by one `GET /api/v1/boot`.
- `mock-client.spec.ts`: a mock over `boot.json` and the two descriptor fixtures answers `boot`,
  `get("dns.zone.list")` and a canned `envelope-422.json` for `dns.zone.edit`, records the call
  with its params and idempotency key, and throws on a function with no canned answer.

### 5. phoenix-ui batch 1 in the globalart monorepo and its Nexus release

**Goal.** The four additions the design binds to M2 in the boundary section of
[design/08-frontend.md](design/08-frontend.md), made in `platform/frontend/libs/phoenix-ui` of the
globalart monorepo under its conventions — standalone, `OnPush`, signal inputs, inline styles on
`--phx-*` tokens, no class name that is a Tailwind utility, a story with a play function next to
each component, labels through `PHX_LABELS` — and released as `@globalart/phoenix-ui@0.2.0`. This
step lives in another repository, and its Keel-side change is the pin, which step 6a makes as its
first commit. `phx-selection-bar` (`src/lib/table/selection-bar.component.ts`): inputs `count`,
`label`, `actions` as `{ id, label, icon?, appearance? }[]`, outputs `action` and `clear`, hidden
while `count` is zero, placed above the table by the consumer. The remote mode of `phx-select`: an
input `remote`, in which the component filters nothing locally, emits `searchChange` with the
debounced query, shows `loading` and offers `more` through a `loadMore` output at the end of the
list, and keeps the label of a selected value that is not among the current options.
`phx-tags-field` (`src/lib/field/tags-field.component.ts`): a `ControlValueAccessor` over
`string[]`, a tag added on Enter, comma or blur and removed by its chip or backspace, with
`placeholder`, `max`, `invalid` and `disabled`. `phx-number-field`
(`src/lib/field/number-field.component.ts`): a `ControlValueAccessor` over `number | null` with
stepper buttons, `min`, `max`, `step`, a `unit` suffix and an `unlimited` mode whose toggle sets
`null` and shows the label `unlimited` from `PHX_LABELS`. The new labels join `PHX_DEFAULT_LABELS`
and the `phx.*` keys of the apps' translation; the public API exports the four;
`libs/phoenix-ui/package.json` is bumped to `0.2.0`, and the tag `phoenix-ui-v0.2.0` runs
`.github/workflows/phoenix-ui-publish.yml`, which builds with ng-packagr, stamps the version and
publishes to `npm-hosted`, where a version is published once.

**Touches.** In the globalart monorepo:
`platform/frontend/libs/phoenix-ui/src/lib/table/selection-bar.component.ts` and its story,
`src/lib/select/select.component.ts` and `select.stories.ts`,
`src/lib/field/tags-field.component.ts`, `number-field.component.ts` and their stories,
`src/lib/labels/labels.ts`, `src/public-api.ts`, `libs/phoenix-ui/package.json`, `README.md` of the
library; the tag `phoenix-ui-v0.2.0`. In Keel: nothing; the pin is the first commit of step 6a.

**Out of scope.** Batch 2 (`phx-datetime-field`, `select` and `number` in `PhxCellEditService`,
`columnOrder`; M3) and batch 3 (M5); any Keel logic in phoenix-ui — the link between selection and
bulk actions, remote options from a descriptor and validation stay in `@globalart/keel`.

**Acceptance.**

- In the globalart CI (`phoenix-ui-tests.yml`): `pnpm --filter @globalart/phoenix-ui typecheck`,
  `lint` (the Tailwind collision check included), `format`, `build-storybook` and `test-storybook`
  are green with the four new stories: `SelectionBar/Default` plays an action and clear,
  `Select/Remote` types a query and receives `searchChange`, shows loading and loads more,
  `TagsField/Default` adds by Enter and comma and removes by backspace, `NumberField/Unlimited`
  steps, shows the unit and toggles unlimited to `null`.
- `pnpm view @globalart/phoenix-ui@0.2.0 --registry
  https://nexus.globalart.hu/repository/npm-hosted/` with a token answers the version, and the
  published `package.json` carries no `main` pointing into `src/`.

### 6a. @globalart/keel/forms: the pin, model, control mapping, pages, conditions and validators

**Goal.** The static half of the `/forms` entry point of
[design/08-frontend.md](design/08-frontend.md): a descriptor's parameter schema with its `x-keel`
and `x-ui` attributes and its `ui` block become an Angular reactive form on phoenix fields, with
pages, conditions and validators handled by metadata alone; the dynamic half — remote options,
recompute, server errors, submit and the `once` panel — is step 6b. The step opens with the pin:
`frontend/libs/keel/package.json` and `frontend/package.json` name `@globalart/phoenix-ui` `0.2.2`
— the batch of step 5 with the two patch releases that landed before this step, neither changing a
component surface — and `pnpm-lock.yaml` follows, the one Keel-side change of step 5, as its first
commit.
`buildForm(descriptor, { mode, params, principal, values })` walks the properties of `params` —
`$defs` resolved, the key parameter hidden, the order from `x-ui.order` then the schema — into a
`FormGroup` and a model of pages and fields, each field carrying its label key, hint and placeholder
keys when `x-ui` names them, its schema, `KeelAttrs`, `UiAttrs` and the validators of step 3. The
mapping from the closed `control` enum onto phoenix components is fixed here and is the whole table:
`input` to `phx-input`; `password` to `phx-password`, with its generate control when `x-ui.generate`
is set; `select` to `phx-select`, local options from `enum` and the remote mode of step 5, which
step 6b feeds, when `x-keel.options` names a function; `toggle` to `phx-toggle`; `switcher` to
`phx-switcher`; `slider` to `phx-slider`, with `phx-number-field` beside it for the typed value and
the `unlimited` state; `date` to `phx-date-field`; `time` to `phx-time-field`; `file` to
`phx-file-field`; `list` to `phx-list-field`; `code` to `phx-code-field`; `tags` to
`phx-tags-field`. A field without a control is mapped by its schema: a boolean to `phx-toggle`, an
`enum` to `phx-select`, an integer or number to `phx-number-field`, a string with `format: date` to
`phx-date-field`, an array of strings to `phx-tags-field`, anything else to `phx-input`; a field
marked `secret` is `phx-password` whatever it says. The form sits on `phx-form-sheet`; a single page
is one `phx-form-section`, several pages are a `phx-tab-group` with a `phx-tab` per visible page,
which is the `tabs` layout the renderer table of the design names, and fields sit on `phx-form-row`
inside `phx-field` with `phx-field-hint` and `phx-field-error`. Conditions are evaluated on every
value change through `/meta` with `fields` the current values, `$mode` `edit` when the key parameter
is present and `create` otherwise, `$principal.level` and `$screen.params`: `show_if` on a page or a
field hides it and leaves it out of the body, `required_if` adds the required validator,
`readonly_if` disables the control and keeps its value in the body. The validators of step 3 are
wrapped as Angular validators whose finding is the `FieldError` itself, so `phx-field-error` shows
`t(key, params)` for a client-side finding and a server-side one alike. `provideKeelWidgets`
registers product widgets under a name and is handed through to the mapping, but how a field names a
widget is not fixed by the design and is left to the first product widget.

**Touches.** `frontend/libs/keel/package.json`, `frontend/package.json` and
`frontend/pnpm-lock.yaml` (the pin), `frontend/libs/keel/forms/src/` (`model.ts`, `mapping.ts`,
`conditions.ts`, `validators.ts`, `widgets.ts`, the components `keel-form`, `keel-page`,
`keel-field`, their specs). phoenix-ui: `phx-form-sheet`, `phx-form-section`, `phx-form-row`,
`phx-form-buttons`, `phx-field`, `phx-field-hint`, `phx-field-error`, `phx-input`, `phx-password`,
`phx-select`, `phx-toggle`, `phx-switcher`, `phx-slider`, `phx-number-field`, `phx-date-field`,
`phx-time-field`, `phx-file-field`, `phx-list-field`, `phx-code-field`, `phx-tags-field`,
`phx-tab-group`, `phx-tab`. Depends on the `0.2.0` of step 5 for `phx-tags-field` and
`phx-number-field`.

**Out of scope.** Remote options, recompute, server errors, submit and the `once` panel (step 6b);
group editing through `ui.multi` and the `{ ids, fields, mask }` request, whose kernel side lands
with `batch_threshold` in M3, where the scope line names it; the wizard container and navigation
(step 7e); the object screen (step 7c); a widget of a product.

**Acceptance.**

- `pnpm install --frozen-lockfile` resolves `@globalart/phoenix-ui@0.2.0` from Nexus,
  `frontend/libs/keel/package.json` names it in `peerDependencies`, and `pnpm build` is green.
- `mapping.spec.ts` against `descriptor-dns-zone-edit.json`: `name` is `phx-input` with the `domain`
  preset, `master_ips` is `phx-tags-field` with the `ip` preset on every item, `tsig_key` is
  `phx-password` with generate, `account` is `phx-select` in remote mode over
  `keel.account.options`, `ttl` is `phx-number-field`, `kind` is `phx-select` over the enum; the
  twelve controls of the enum map onto the twelve components named above, and a boolean without a
  control maps onto `phx-toggle`.
- `conditions.spec.ts`: the page `transfer` is shown only while `kind` is `slave`, `master_ips` is
  required then and not otherwise, `name` is read-only in `edit` mode and editable in `create`, and
  the model of a `create` with `kind: master` marks `master_ips` and `tsig_key` hidden.
- `validators.spec.ts`: `master_ips` set to `["10.0.0.1", "10.0.0"]` marks the control invalid with
  the `FieldError` of `envelope-422.json` — `path: master_ips[1]`, `code: check.ip` — and
  `phx-field-error` shows `t("keel.check.ip", params)`; a `ttl` below the schema's minimum shows
  `keel.schema.minimum` with the bound.

### 6b. @globalart/keel/forms: remote options, recompute, server errors, submit and the once panel

**Goal.** The dynamic half of the `/forms` entry point of
[design/08-frontend.md](design/08-frontend.md) over the model of step 6a, the form dynamics of
[design/10-contracts.md](design/10-contracts.md). Remote options call the named `kind: options`
function with `{ search, scope, limit: 20, cursor }`, `scope` from the screen parameters, and feed
`Choices` into the remote mode of `phx-select` of step 5 as its options, `loading` and `more`, a
`loadMore` calling again with the `cursor`. A field whose `ui` names `recompute` calls that
function, debounced, with `{ values, changed }` and applies the answer: `values` patched over the
form without re-triggering recompute, `options` replacing the choices of the named fields, `errors`
mapped like a `422`. `applyFieldErrors(form, fields)` maps the `fields[]` of a `422` and of a
`403 forbidden_field` onto controls by `path`, array indices included, and shows what maps nowhere
on a `phx-banner` above the form; the message is `t(key, params)`. `submit()` builds the body —
hidden fields left out, cleared optional fields as `null`, which the kernel reads as absent — and
calls the function with a fresh idempotency key; the values of `ui.read` are loaded for an edit
through the `get` function with the key parameter. A result field marked `once` in the result schema
is shown by the form the one time it arrives, in a panel of `phx-clipboard` rows, since no replay
answers it again.

**Touches.** `frontend/libs/keel/forms/src/` (`options.ts`, `recompute.ts`, `errors.ts`,
`submit.ts`, `load.ts`, the component `keel-once`, their specs; `keel-form` gains the submit and the
banner). phoenix-ui: `phx-select` in the remote mode of step 5, `phx-banner`, `phx-clipboard`.

**Out of scope.** Group editing through `ui.multi` (M3, as step 6a says); the wizard's per-step
validation and navigation (step 7e); `ui.result` `task` and `download` (M3).

**Acceptance.**

- `options.spec.ts` against `descriptor-dns-zone-edit.json` with `KeelMockClient`: typing `acme`
  into `account` calls `keel.account.options` with `{ search: "acme", scope, limit: 20 }` once after
  the debounce, the answered `Choices` fill the options and clear `loading`, and `more` offers
  `loadMore`, which calls again with a `cursor`.
- `recompute.spec.ts`: a change of `ttl` calls `dns.zone.recompute` with
  `{ values, changed: "ttl" }` once after the debounce, the answered `values` patch is applied
  without a second call, and answered `errors` land on their fields.
- `errors.spec.ts`: `envelope-422.json` lands `check.ip` on the control `master_ips[1]` and
  `dns.zone.error.exists` on `name` with the translated messages, and `envelope-403.json` lands
  `forbidden_field` on `account`; an unknown path shows on the banner.
- `submit.spec.ts`: a submit posts one `Idempotency-Key`, a `422` answer maps and leaves the key for
  the retry, a `create` with `kind: master` posts a body with no `master_ips` and no `tsig_key`, an
  edit loads the values of `ui.read` through `dns.zone.get` with the key parameter, and a result
  with a field marked `once` renders one `phx-clipboard` row with its value.

### 7a. View registry on the server

**Goal.** The server side of the custom-views registry of
[design/08-frontend.md](design/08-frontend.md), the first Rust half of the renderer group; it needs
step 2 alone, for the regenerated types, and may land as soon as step 2 has. The registry is
`Registrar::view(name, ViewSpec { access, menu, needs })` in `keel-sdk`: the freeze checks the
module prefix of the name, that the permission is registered and the menu group known, `/boot.views`
and the socket's boot frame list the views whose level and permission the principal meets (`needs`
waits for the feature guard of M4), and the menu node of a view yields an item without a `kind`,
which `BootMenuItem` makes optional — the plan's choice, since the design fixes the registry and not
the item — with `contracts/schema/keel.json` and the generated types following.

**Touches.** `crates/sdk/src/registrar.rs` and `ui.rs` (`view`, `ViewSpec`),
`crates/kernel/src/loader/freeze.rs`, `boot.rs`, `menu.rs` and `socket/` (the views in the boot
payload), `crates/types/src/boot.rs` (`kind` optional on an item) with `contracts/schema/keel.json`
and the generated types, `crates/kernel/tests/kernel/views.rs`.

**Out of scope.** The demo's M2 shapes (step 7b); the client half of the registry and the renderers
(steps 7c to 7e); the feature guard on `needs` (M4); a view registered with `r.view` by a
first-party product (none before `panel`).

**Acceptance.**

- `crates/kernel/tests/kernel/views.rs`:
  `a_registered_view_is_listed_in_boot_for_the_levels_its_access_admits_and_menus_without_a_kind`,
  and `a_view_with_a_foreign_prefix_an_unknown_permission_or_an_unknown_group_fails_freeze`;
  `contracts/fixtures/boot.json` still round-trips, and `pnpm generate` leaves the regenerated
  `BootMenuItem` with `kind` optional.

### 7b. The demo's M2 shapes

**Goal.** The additions to `products/demo` the exit criteria need, the second Rust half of the
renderer group; it needs no step of this breakdown and may land beside step 7a. The demo gains its
M2 shapes in `products/demo`, all additive: the migration adds `kind` (`memo` or `todo`, default
`memo`), `tags` (a text array, default empty) and `due` (a date, nullable) to `note_item`;
`NoteEdit` gains `kind`, `tags` with the `tags` control and the `name` preset on every item, and
`due` with the `date` control; `note.edit` keeps its pages `main` and `owner`, puts `kind`, `due`
and `tags` on `main`, shows and requires `due` while `kind == 'todo'`, names `note.recompute` on
`kind`, and refuses a second note with the same title in the account with
`conflict("note.error.exists").field("title")`, the field-level error of the exit criteria;
`note.recompute` takes `{ values, changed }` and answers `{ values: { due } }` with a date one week
ahead in the caller's zone when `kind` turned `todo` and `due` is empty, and an empty patch
otherwise; `note.list` gains the columns `kind` (`status` with a tone map and the static filter
`memo`, `todo`), `due` (`date`), `tags` (`text`) and `accountId` (`text`, level `reseller`,
`filter.remote: keel.account.options`), and its `delete` confirm gains `input: count`; `note.import`
is the `kind: wizard`, `mode: write`, `.unowned()` function with the pages `source` (`text` on the
`code` control, validated by `note.import.check`) and `options` (`kind`, `tags`, `account` at level
`reseller`), `ui.result` `redirect`, creating one note per non-empty line of `text` inside the
caller's resolved account and refusing the batch that would pass `MAX_PER_ACCOUNT` with
`note.error.limit`; `note.import.check` answers `422` on `text` with `note.error.line_too_long` and
the line number for a line above 200 characters and `note.error.empty` for no line; `note.tag.list`
is a `kind: list` over a `ListSource` of the module counting `unnest(tags)` per tag within the
caller's account scope, columns `tag` (`text`, primary, sortable) and `count` (`number`, sortable,
`total: sum`), search on `tag`, behind the menu node `demo/tags`. `note.recompute` and
`note.import.check` are registered as `kind: action` in `mode: read` and `.unowned()`, the plan's
choice for the two helper shapes the design fixes without a kind.

**Touches.** `products/demo/` (the migration, `note/mod.rs`, `note/functions.rs`, `note/tags.rs`,
`note/import.rs`, the dictionaries, `tests/` and their snapshots), `hack/prove-binary.sh`
(`note.import` over `ctl` and `note.tag.list --out table`).

**Out of scope.** The view registry (step 7a) and a view of the demo registered through `r.view`,
since the demo's custom view replaces `note.tag.list` from `frontend/products/demo` (step 7e);
`ui.result` `task` and `download` (M3); `note.export` (M3).

**Acceptance.**

- `products/demo/tests/`:
  `note_edit_refuses_a_second_note_with_the_same_title_as_a_field_error_on_title`,
  `note_recompute_fills_due_a_week_ahead_when_kind_turns_todo_and_patches_nothing_otherwise`,
  `note_import_checks_the_source_page_and_creates_one_note_per_line_inside_the_limit`,
  `note_tag_list_counts_tags_within_the_callers_scope_with_a_sum_total`; the two standard helpers
  hold for `note.list`, `note.edit` and `note.tag.list`; every `note.*` descriptor matches its
  snapshot; `hack/prove-binary.sh` imports two lines over `ctl` and renders
  `note.tag.list --out table` with translated headers.

### 7c. Renderer: KeelScreen, the views registry, the object renderer and the action runner

**Goal.** The frame of the `/renderer` section of [design/08-frontend.md](design/08-frontend.md) and
the client side of the registry of custom views. `KeelScreen` is the component behind the shell's
route `/f/:func`: it loads the descriptor, asks the registry of views for the function and falls
back to the renderer of its `kind` — `list` (step 7d), `form` on the form of steps 6a and 6b,
`action`, `wizard` (step 7e), and `get` rendered as a read-only object on `phx-prop-list` from the
result schema; `options`, `stream`, `download`, `upload` and `report` show `phx-not-found` until
their milestones. The object renderer is the plan's addition to the M2 renderer table of the design,
since `audit.list` links its rows to `audit.get` and the `ui` row of
[design/02-functions.md](design/02-functions.md) gives a `get` no screen of its own; the commit
message names both sentences and step 12 amends them. The shared action runner is what a row action,
a toolbar action, the selection bar and the action route call: `confirm: true` asks through
`PhxConfirmService`, a `{ destructive, items, input: count }` confirm lists the `items` column of
the affected rows and requires their count typed, the call carries `{ ids }` for a `delete` or
`group` action, the selection's ids or the one row's, and the action's `params` with `$id`
substituted for a `button`, a success hands back to the caller, which reloads its table, and a
failure shows a `PhxToastService` toast with the message and the request id. The action renderer is
the same runner reached by route, for an action opened from a menu or a link. On the client the
registry is `provideKeelViews([{ func, component }])` behind the multi-provider `KEEL_VIEWS`;
`KeelScreen` creates the registered component with `KeelViewContext { descriptor, params, client,
principal, tabs }` under the token `KEEL_VIEW_CONTEXT`, `descriptor` filled for a replaced function
screen and absent for a view of `/boot.views`; a name of `/boot.views` no product library registered
shows `phx-not-found` with a hint naming the product.

**Touches.** `frontend/libs/keel/renderer/src/` (`screen.ts`, `views.ts`, `object/`, `action/`,
specs). phoenix-ui: `phx-prop-list`, `phx-confirm` through `PhxConfirmService`, `PhxToastService`,
`phx-not-found`.

**Out of scope.** The list renderer and its columns (step 7d); the wizard renderer and the demo's
view library (step 7e); the `report`, `dashboard`, `tree`, `master-detail` and `stream` renderers
(M5); `ui.result` `task` and `download` (M3); the shell's route, layout and navigation (steps 8b and
8c).

**Acceptance.**

- `object.spec.ts`: a `get` descriptor renders its result as a property list; `views.spec.ts`: "a
  component registered through provideKeelViews replaces the generic screen and receives the context
  with the descriptor" and "a view of boot.views without a component shows phx-not-found naming the
  product".
- `action.spec.ts` with `KeelMockClient`: "a destructive confirm lists the items and requires the
  count" — the runner handed the `delete` action of `descriptor-dns-zone-list.json` and two rows
  lists their names, refuses the count `3`, accepts `2` and posts `{ ids }`; "confirm: true asks
  once and a refusal posts nothing"; "a failure shows a toast with the message and the request id";
  and the route `/f/<func>` of a `kind: action` descriptor runs the same runner with `$id` from the
  route's parameters.

### 7d. Renderer: the list

**Goal.** The list renderer of the `/renderer` section of
[design/08-frontend.md](design/08-frontend.md) on the frame of step 7c. The list renderer composes
`phx-page-header` with the title and the toolbar actions (`new`, `button`, `refresh`, `upload`, and
`export` hidden while `keel.export` is absent from the descriptor index) and `phx-data-table` over
the adapter of step 4 with the screen parameters as `scope`. Columns map from the closed
`column.type` onto `PhxColumn.type` by this table: `text` to `text`; `number` to `number`, with
`total` shown in the footer from `totals`; `link` to `link`, the target `/f/<screen>` with `$id`
substituted from the row's key, opened as an object tab through `PhxTabsService`; `status` to
`status`, the value mapped through `map` onto the tone and labelled through
`<module>.<entity>.<column>.<value>`; `toggle` to `toggle`, edited in the cell through
`PhxCellEditService` by calling `edit.function` with the key and the new value when `edit` is
present and read-only otherwise; `date` to `date`; `datetime` to `date` with the time in the
principal's zone; `boolean` to `icon`. `sortable` and `primary` pass through; a static `filter`
becomes `filterOptions` with translated labels, a `remote` filter a `phx-column-filter` fed by the
remote select over the named options function; `search` drives the table's search box. Row actions
come from `row_actions` with `hidden_if` evaluated per row, the `default` action opens on row click,
and an action with a `screen` navigates while one with a `function` runs through the action runner
of step 7c with the selection's ids or the one row's, the table reloading on success. The table is
selectable when any non-row action is a `delete` or `group`, and the selection drives
`phx-selection-bar` of step 5 with those actions, the link the design places in the runtime. `empty`
sets the empty state and its action, `invalidate_on` subscribes to `dataInvalidate$` of the stub and
reloads on a matching mask.

**Touches.** `frontend/libs/keel/renderer/src/` (`list/`, `columns.ts`, `list.spec.ts`). phoenix-ui:
`phx-page-header`, `phx-data-table`, `phx-column-filter`, `phx-selection-bar`, `PhxCellEditService`,
`PhxTabsService`, `phx-empty`.

**Out of scope.** The export button and `keel.export` (M3, as step 4 says); column order and
visibility in `settings_pref` (M3, with `columnOrder` of batch 2); the `report`, `dashboard`,
`tree`, `master-detail` and `stream` renderers (M5); the wizard renderer and the demo's view library
(step 7e).

**Acceptance.**

- `list.spec.ts` against `descriptor-dns-zone-list.json` with `KeelMockClient`: "the list renderer
  builds the table from the descriptor" — five columns with the types `link`, `number`, `status`,
  `toggle` and `text`, `records` with a footer total, `status` filterable with the translated
  options, `account` with a remote filter; "row actions follow row_actions and hidden_if" —
  `suspend` is absent on a suspended row; "a delete from the selection bar hands the selected rows
  to the runner and reloads" — two selected zones reach the runner of step 7c as `{ ids }` with
  their names and the table reloads on success; "a data.invalidate matching invalidate_on reloads
  the table" and "the export action is hidden while keel.export is absent".

### 7e. Renderer: wizard and frontend/products/demo

**Goal.** The wizard renderer and the first consumer of the registry of step 7c. The wizard renderer
owns the step container the design keeps out of phoenix-ui: `phx-wizard` shows the pages as its
steps, one page of the form of steps 6a and 6b is open at a time, `next` calls `pages[].validate`
with the values so far when the page names one and shows a `422` on the step's fields, `back` keeps
the values, the last step submits the wizard function with every page's values, and `ui.result`
`redirect` navigates back to the opener while `task` and `download` wait for M3.
`frontend/products/demo` is the Angular library `@globalart/keel-demo` with `provideKeelViews([{
func: "note.tag.list", component: NoteTagCloudComponent }])`: a tag cloud sized by count over the
pages of `note.tag.list` through the context's client, which opens `note.list` searched by the tag
when clicked; `frontend/products/demo/views.json` lists that one name for the metric of step 11a.
The library is a project of `angular.json` under `products/*` of the workspace, built from source
through the `paths` of `tsconfig.base.json` like the entry points, and step 8b's `main.ts` is what
loads it.

**Touches.** `frontend/libs/keel/renderer/src/wizard/` and its spec, `frontend/products/demo/` (the
library, the component, `views.json`, its spec), `frontend/angular.json`,
`frontend/tsconfig.base.json` (the path of `@globalart/keel-demo`). phoenix-ui: `phx-wizard`.

**Out of scope.** `ui.result` `task` and `download` (M3); the shell's registration of the library
(step 8b); a second custom view of the demo.

**Acceptance.**

- `wizard.spec.ts` with `KeelMockClient`: "a step is validated on the server before the next opens"
  — the validate function answers a `422` that lands on the step's field and the page stays, then
  answers `ok` and the next page opens; "the last step submits every page and redirect navigates
  back".
- `frontend/products/demo`: "the tag cloud lists every tag of note.tag.list sized by count and opens
  the list searched by the tag", and "views.json names exactly what the library registers".

### 8a. auth: stored grants through PermissionSource, the auth menu nodes and the dictionaries

**Goal.** The server half of the shell group: the permissions screen's source, the menu nodes the
`auth` module lacks, the dictionaries the shell and phoenix-ui read, and the third `/boot` fixture.
The `PermissionSource` of [design/03-access.md](design/03-access.md): the hidden
`keel_sdk::system::PermissionSource` answers `grants(principal, db)` as a map from permission key to
`bool`, the transports overlay it on the principal once per request next to the preferences of M1,
as `Principal.grants`, an in-process member the wire never carries, and `guards::holds` answers a
stored grant over the registered default, with `admin` and `owner` holding every key as before;
`auth` provides it over the table `auth_grant` (`id`, `principal_id`, `key`, `granted`,
`created_at`, `created_by`) and registers `auth.grant.list` (`list`, `reseller`, permission
`auth.grant.read` for `[reseller]`, scoped to the caller's subtree, search on `key` and
`principalLogin`), `auth.grant.set { principal, key, granted }` (`form`, `reseller`,
`auth.grant.manage` for `[reseller]`; the key must be registered, else `422` on `key` with
`auth.error.unknown_permission`, held by the caller, else `403` `auth.error.grant_not_held`, and the
target below the caller inside its scope under the `not_below` rule of M1) and
`auth.grant.delete { id }` (`action`); both writes bump the target's `acl_gen` through the hidden
`PrincipalSource::touch_acl`, so the descriptor cache and the menu of the target follow the design's
invalidation rule. The `auth` module gains the menu nodes it lacks, `system/sessions` on
`auth.session.list`, `system/tokens` on `auth.token.list`, `system/ip-rules` on `auth.ip_rule.list`
and `system/grants` on `auth.grant.list`, each projected by level and permission as any node. The
`phx.*` keys phoenix-ui needs, every key of `PHX_DEFAULT_LABELS`, join the kernel's dictionaries in
`crates/kernel/i18n/`, since every installation serves them and OwnLate manages them with the core
project, and the shell's own strings live under `keel.shell.*` in the same dictionaries. The kernel
test `boot_without_a_product_shows_the_platform_brand_and_the_platform_group_only` in
`crates/kernel/tests/kernel/boot.rs` writes `contracts/fixtures/boot-none.json` under the snapshot rule of
`contracts/fixtures/descriptors/`, the no-product payload of the design's three `/boot` fixtures,
and `crates/types/tests/types/fixtures.rs` and `fixtures.spec.ts` gain it.

**Touches.** `crates/sdk/src/system.rs` (`PermissionSource`, `PrincipalSource::touch_acl`),
`crates/types/src/principal.rs` (`grants`), `crates/kernel/src/dispatcher/guards.rs`, `http/auth.rs`
and `socket/` (the overlay), `crates/system/` (auth: the migration, `grant/`, the menu nodes,
dictionaries, tests, snapshots), `crates/kernel/i18n/en.json` and `ru.json` (`phx.*`,
`keel.shell.*`), `crates/kernel/tests/kernel/boot.rs` and `crates/types/tests/types/fixtures.rs`
(`boot-none.json`), `contracts/fixtures/boot-none.json`,
`frontend/libs/keel/meta/src/fixtures.spec.ts` (the mirror entry).

**Out of scope.** The shell itself (steps 8b to 8d); the permission matrix as a custom view (a
product's, later); the first-administrator screen `keel.setup.admin` (M4); on-behalf banners from a
peer (M6).

**Acceptance.**

- `crates/system/tests/system/auth_grants.rs`:
  `a_stored_grant_overrides_the_default_and_a_denial_takes_a_default_key_away_until_deleted` over
  HTTP — a test module registers a `user`-level function under a permission whose defaults name
  `reseller` alone; the reseller grants that key to A1, whose call is then admitted, a denial row on
  `keel.account.read` makes `keel.account.list` answer `403 keel.error.permission` for A1 until the
  row is deleted, and A1's descriptor `ETag` changes with each write; the reseller is refused a key
  it does not hold, a key nobody registered, and the `not_below` targets of M1;
  `assert_reseller_sees_only_subtree` holds for `auth.grant.list`; the `auth.grant.*` descriptors
  and the four menu nodes match their snapshots, and `/boot` for a `user` lists `system/sessions`
  and `system/tokens` and not `system/ip-rules`.
- `every_fixture_file_has_a_test` and its Jest mirror cover `boot-none.json`, whose `products` and
  `views` are empty and whose `menu` carries the group `system` alone.
- `crates/kernel/i18n/en.json` and `ru.json` carry `phx.*` and `keel.shell.*`, and the freeze's
  `en` check and the `ru` fallback of M0 hold for them.

### 8b. Shell: bootstrap, routes, guard, login and the labels bridge

**Goal.** The application frame of the `/shell` section of
[design/08-frontend.md](design/08-frontend.md) in its bootstrap, routing and login half, against the
`auth` module of M1; the layout, the sidebar rule and the rail are step 8c, and until it lands the
shell stands on a bare `phx-layout` whose sidebar is the menu as a flat list of `phx-sidebar-item`.
`bootstrapKeel({ products, icons, widgets })` bootstraps `KeelShellComponent` with the router,
`provideHttpClient(withFetch())`, the animations, `providePhoenixIcons(icons)`,
`provideKeelClient()`, `provideKeelWidgets(widgets)` and the labels bridge, and registers the
product libraries lazily: `products` lists `{ id, load }` pairs, the shell resolves `load()` — a
dynamic `import` that becomes a chunk of its own — after `/boot` for the ids `/boot.products` names
and for those alone, and collects their `provideKeelViews` and `provideKeelWidgets` providers;
`frontend/apps/shell/src/main.ts` is that one call with `demo`. Loading the chunks of the enabled
products at bootstrap rather than on the first route of each product is the plan's choice, since
`/boot` carries no map from a function's module to its product before `modules` reaches the product
rows in step 11a and the enabled products are the only ones a route can belong to; the commit
message names the sentence of the design and step 12 amends it. Routes: `/login` on `phx-logon` with
the password step posting `auth.login { login, password }`, the code step shown when the answer is
`{ state: "totp", challenge }` and posting `auth.totp.verify { challenge, code }`, where a recovery
code goes into the same box, `returnTo` honoured after either, and the lockout, rate-limit and
credential errors shown from their keys; `/login?key=` posting `auth.key.redeem { key }` same-origin
on load and navigating to the answered `redirect`, the page the design places here so that a
cross-site navigation never carries the key; `/logout`; `/f/:func` on `KeelScreen` of step 7c behind
a guard that loads `/boot` once and sends a guest to `/login` with `returnTo`; `/` to the first item
of the menu; anything else to `phx-not-found`. The `unauthenticated` signal of the client drops the
boot state and navigates to `/login`. After boot the shell loads the catalogue for `principal.lang`,
`en` for a guest. The labels bridge provides `PHX_LABELS` with a live object reading the `phx.*`
namespace of the loaded catalogue, so a language switch changes the labels in place;
`providePhoenixLabels` snapshots its argument into a static value provider, which cannot be
replaced at the root after bootstrap.

**Touches.** `frontend/libs/keel/shell/src/` (`bootstrap.ts`, `routes.ts`, `guard.ts`, `login/`,
`labels.ts`, `shell.ts` with the bare layout, specs), `frontend/apps/shell/src/main.ts`. Kernel
routes consumed: `/api/v1/boot`, `/api/v1/i18n`, `/api/v1/call/auth.*`. phoenix-ui: `phx-layout`,
`phx-sidebar-item`, `phx-logon`, `phx-not-found`, `PHX_LABELS` provided by the bridge.

**Out of scope.** The layout, the sidebar rule, the rail and the user menu (step 8c);
the settings value form and the security page (step 8d); the notification centre, tasks, problems
and badges (M3); the `/brand/logo.svg` route and white-label (step 9b); `keel-dev i18n pull` and
`missing`, which need the OwnLate projects for Keel that do not exist yet — they land in M3, where
the scope line names them with the projects for the core and `demo`, and the freeze's `en` check and
the `ru` fallback stand until then, which step 12 records against the M0 note that pointed at M2;
loading a product's chunk on its first route (with the module-to-product map, later).

**Acceptance.**

- `bootstrap.spec.ts`: with `demo` and a second product in `products`, a boot naming `demo` alone
  resolves one `load()` and registers its views, and the other is never called.
- `login.spec.ts` with `KeelMockClient`: the password step posts `auth.login`, a `totp` answer opens
  the code step, which posts `auth.totp.verify` with the challenge, and `returnTo` is honoured;
  `/login?key=` posts `auth.key.redeem` on load and navigates to `redirect`; a `401` from any call
  lands on `/login` with `returnTo` set to the screen it came from.
- `labels.spec.ts`: `PHX_LABELS` is filled from `phx.*` of the catalogue and switches with the
  language; the kernel's `en` and `ru` dictionaries carry every key of `PHX_DEFAULT_LABELS`, checked
  by a Jest test reading `crates/kernel/i18n/`.

### 8c. Shell: layout, sidebar rule, rail and the user menu

**Goal.** The layout half of the `/shell` section of [design/08-frontend.md](design/08-frontend.md),
replacing the bare layout of step 8b. After boot the shell feeds `hue` and `chroma` of `/boot.brand`
to `PhxThemeService`, shows the brand through `phx-brand-logo` with the `logo` path when the brand
carries one and the translated `name` otherwise, and builds `phx-layout`, `phx-sidebar` and
`phx-rail` from `menu`: with one product enabled the sidebar shows that product's
groups followed by the platform group `system`; with several, a `phx-sidebar-group` heading per
product in the order of `/boot.products`, each with its groups and the platform group last, a
heading without a visible item omitted; with none, the platform brand and the platform group alone.
Items are `phx-sidebar-item` with the icon, `t(label)` and the route `/f/<func>`; a `badge.problem`
stays hidden until `problems` exists. The level label follows the design's rule:
`product.<id>.level.<level>` while one product is enabled, `keel.level.<level>` otherwise. A
`phx-banner` in the page area above the screen names the actor's login while `principal.actor` is
set and offers `auth.su.exit` — `phx-topbar` is the phone chrome `phx-layout` renders itself and
has no slot the shell could fill; the rail carries the disabled placeholders for notifications,
tasks and help until M3 and the user menu on `phx-user-menu`: the login and level, the language
switch between the languages the catalogue serves, `en` and `ru`, writing
`settings.pref.set { key: "lang", value }` and reloading the catalogue in place; the theme switch,
`light`, `dark` and `system`, writing
`settings.pref.set { key: "theme" }` and `PhxThemeService`; "Act as", a dialog searching
`keel.principal.list` by login and posting `auth.su { principal }`, shown while `auth.su` is in the
descriptor index; the links to Sessions, Tokens and Security; and Log out. The system screens are
the descriptors that exist, rendered by steps 7c and 7d through the menu: accounts
(`keel.account.*`), principals (`keel.principal.*`), products (`keel.product.list` with its
confirmed row actions `enable` and `disable`), settings (`settings.list`), audit (`audit.list`,
`audit.get` as the object screen), sessions, tokens, IP rules and grants (the `auth` nodes of step
8a), and notes (`note.*` under the demo group). A language switch reloads the catalogue in place,
which the live labels bridge of step 8b follows without being re-provided.

**Touches.** `frontend/libs/keel/shell/src/` (`layout/`, `rail/`, `shell.ts`, specs). Kernel routes
consumed: `/api/v1/call/settings.pref.*`, `/api/v1/call/auth.su`, `/api/v1/call/auth.su.exit`,
`/api/v1/call/keel.principal.list`. phoenix-ui: `phx-layout`, `phx-sidebar`, `phx-sidebar-group`,
`phx-sidebar-item`, `phx-rail`, `phx-rail-item`, `phx-user-menu`, `phx-brand-logo`,
`phx-banner`, `phx-modal` through `PhxModalService`, `PhxThemeService`.

**Out of scope.** The settings value form and the security page the user menu links to (step 8d);
the notification centre, tasks, problems and badges (M3); the `/brand/logo.svg` route and
white-label (step 9b); the permission matrix as a custom view (a product's, later).

**Acceptance.**

- `sidebar.spec.ts` against `contracts/fixtures/boot.json`, `contracts/fixtures/boot-none.json` of
  step 8a and the one-product payload, which is `boot.json` with its second product and that
  product's menu groups removed by the spec until the dump of step 11a replaces it: "one product
  shows its groups then system under the product brand", "two products show a heading per product
  in boot order with the platform group last and the platform brand", "no product shows the platform
  group alone", "a heading without a visible item is omitted", and the level label key switches
  between `product.<id>.level.*` and `keel.level.*`.
- `rail.spec.ts`: the language switch posts `settings.pref.set { key: "lang", value: "ru" }` and
  reloads the catalogue, the `su` banner appears with `principal.actor` and posts `auth.su.exit`,
  the theme switch posts `settings.pref.set { key: "theme", value: "dark" }` and sets
  `PhxThemeService`, and "Act as" is absent when `auth.su` is not in the index.

### 8d. Shell: the settings value form and the security page

**Goal.** The two screens that are the shell's own, registered through the same `provideKeelViews`
and listed in `frontend/apps/shell/views.json`: the settings value form for `settings.set`, which
reads the row of `settings.get { key }` and builds the `value` control of the form of steps 6a and
6b from the `schema` the row carries, the automatic form the `settings` section of
[design/05-modules.md](design/05-modules.md) describes; and the security page, registered for
`auth.totp.enroll` and reached from the user menu of step 8c, which edits the password through
`auth.password.set`, enrols the second factor through `auth.totp.enroll` showing the `secret`, the
`uri` as a QR code drawn with the `qrcode` package and copy controls, confirms through
`auth.totp.confirm { code }` showing the recovery codes the one time they arrive, and disables
through `auth.totp.disable { code }`.

**Touches.** `frontend/libs/keel/shell/src/` (`security/`, `settings-value/`, `views.ts`, their
specs), `frontend/apps/shell/views.json`, `frontend/package.json` (`qrcode`). phoenix-ui:
`phx-clipboard`, `phx-modal` through `PhxModalService`.

**Out of scope.** The permission matrix as a custom view (a product's, later); a third view of the
shell.

**Acceptance.**

- `settings-value.spec.ts` with `KeelMockClient`: the form of `settings.set` for
  `auth.password.min_length` renders `phx-number-field` from the row's `schema` and posts `{ key,
  value }`.
- `security.spec.ts`: enrolment shows the secret and a QR, the confirm shows the recovery codes
  once, and `frontend/apps/shell/views.json` names exactly the two views the shell registers.

### 9a. embed_frontend!: assets, SPA fallback, CSP, the CI reorder, size check, publish-npm.yml

**Goal.** The embedding of [design/08-frontend.md](design/08-frontend.md) and
[design/09-operations.md](design/09-operations.md) and the publication of the library.
`keel::embed_frontend!("../../frontend/dist/shell")` in the facade expands to a `rust-embed` type
over the directory, with the `debug-embed` feature so the debug binary CI proves carries the assets
as the release does, and answers a `keel::Frontend` the runtime takes through
`Runtime::frontend(...)`; `bin/keel/src/main.rs` gains that line. The kernel's HTTP transport serves
it beside the API: a path that names an asset is answered with its content type by extension, the
brotli twin — `<name>.br`, written by the shell build, which runs `node tools/compress.mjs` over
`dist/shell` after `ng build` — when `Accept-Encoding` admits `br`, with `Content-Encoding: br` and
`Vary: Accept-Encoding`; a hashed asset carries `Cache-Control: public, max-age=31536000,
immutable`, `index.html` and every other unhashed file `no-store`; a `GET` that names no asset, is
not under `/api/`, `/healthz`, `/readyz` or `/brand/`, and whose last segment does not end in an
extension the shell build emits — `.js`, `.mjs`, `.css`, `.map`, `.woff2`, `.svg`, `.png`, `.ico`,
`.json`, `.txt`, `.br` — is answered `index.html`, the SPA fallback; function names carry dots, so
`/f/note.list` falls back while `/missing.png` answers `404`, and the list of extensions is one
constant in `crates/kernel/src/http/assets.rs` with a unit test over both paths, while `/api/v1/`
keeps its `404` envelope. `index.html` is answered with `Content-Security-Policy: default-src
'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self';
connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'`,
`X-Content-Type-Options: nosniff` and `Referrer-Policy: same-origin`, and the shell carries no
inline script: the phoenix theme boot script is shipped as a file the index loads first, Angular's
inline styles are what `style-src` admits, and the `production` configuration of `apps/shell` sets
`optimization.styles.inlineCritical` to `false`, since the critical-CSS inliner of the `application`
builder emits a stylesheet link with an inline `onload` handler that `script-src 'self'` refuses.
Publication is `.github/workflows/publish-npm.yml`, the mirror of the phoenix-ui workflow: on a tag
`keel-vX.Y.Z` or by `workflow_dispatch`, `pnpm install --frozen-lockfile`, `ng build keel`, the
version stamped from the tag into `dist/keel/package.json`, the binary's version under the rule step
1 records until SDK 1.0, `publishConfig.registry` set to Nexus, the token written to `~/.npmrc` and
`pnpm publish --no-git-checks` from `dist/keel`; the job `frontend` runs `pnpm publish --dry-run`
from the same build on every push. CI reorders itself for the macro: the job `check` sets up pnpm
and node, installs with the token and runs `pnpm build` before `cargo clippy`, since `bin/keel` now
compiles against `frontend/dist/shell`, and `.github/workflows/build.yml` does the same before its
release build and asserts the size of `target/x86_64-unknown-linux-musl/release/keel`, which carries
the `demo` bundle, under 40 MB, the M2 exit criterion, printing the number.

**Touches.** `Cargo.toml` (the pinned `rust-embed`), `crates/keel/src/` (`embed_frontend!`,
`Frontend`, `Runtime::frontend`), `crates/kernel/src/http/` (`assets.rs`, the fallback, the
headers), `crates/keel/tests/runtime.rs`, `bin/keel/src/main.rs`, `bin/keel/tests/binary.rs`,
`frontend/tools/compress.mjs`, `frontend/apps/shell/src/index.html` and `theme-boot.js`,
`frontend/angular.json` (`inlineCritical` off in the `production` configuration of `shell`),
`frontend/package.json`, `.github/workflows/ci.yml`, `.github/workflows/build.yml`,
`.github/workflows/publish-npm.yml`, `hack/prove-binary.sh` (the routes). Kernel routes added:
`GET /`, the assets, the SPA fallback.

**Out of scope.** The brand routes, `ProductManifest.assets` and white-label (step 9b); TLS and
`trusted_proxies` (M3); the shell's rendering of the brand (step 8c); a size cap for a binary with
`dns` and `panel` (set at the start of M5).

**Acceptance.**

- `crates/kernel/tests/kernel/assets.rs` with a test `Frontend` of three files: `GET /` and
  `GET /f/note.list` answer `index.html` with `no-store`, the CSP above, `nosniff` and the referrer
  policy; `GET /main-abc123.js` answers the file `immutable` with a year of `max-age`, and with
  `Accept-Encoding: br` the `.br` twin with `Content-Encoding: br` and `Vary`; `GET /missing.png`
  answers `404`; `GET /api/v1/nothing` keeps the `keel.error.route_not_found` envelope; `/healthz`
  is untouched; the unit test of the extension constant holds `/f/keel.account.list` as a fallback
  path and `/assets/x.woff2` as an asset path.
- `bin/keel/tests/binary.rs`: the built binary answers `GET /` with the real `index.html` of
  `frontend/dist/shell` and its hashed `main-*.js` as `immutable`, and the served `index.html`
  carries no `<script>` without `src`, no `onload` attribute and no `<style>` the CSP does not
  admit; `hack/prove-binary.sh` checks `/` and the fallback for `/f/note.list`.
- The job `check` builds the shell before the cargo steps and is green; `build.yml` prints the size
  of the musl release binary with the demo and fails at 40 MB and above, and the number of the
  closing run of M2 is recorded in the exit checklist.
- `publish-npm.yml` passes its dry run in the job `frontend`: `dist/keel/package.json` carries the
  version, the six `exports` and no `scripts`.
- `cargo-semver-checks` stays green on the facade: `embed_frontend!`, `Frontend` and
  `Runtime::frontend` are additions.

### 9b. Brand assets, ManifestSource, settings_brand and the /brand routes

**Goal.** The brand routes and white-label by hostname, the third server half of the M2 scope. The
brand routes are the kernel's: `ProductManifest` gains `assets`, a list of `Asset { id,
content_type, bytes }`, and `/brand/<id>/logo.svg` answers the asset the bundle's `Brand.logo`
names or `404`; `/brand/logo.svg` answers the effective brand's logo — the single enabled product's
asset, or the kernel's own mark embedded in `crates/kernel/assets/keel.svg` for the platform brand —
with `Cache-Control: no-store`; `products/demo` ships `brand/logo.svg` as `product.demo.logo`.
White-label is the `ManifestPatch` of [design/05-modules.md](design/05-modules.md) in the shape M2
needs: the hidden `keel_sdk::system::ManifestSource` answers `patch(host, db)` as
`Option<ManifestPatch { name, logo, hue, chroma }>`, every member optional, and `/boot` applies the
patch of the request's host name, the `Host` header without its port, onto `brand`;
`/brand/logo.svg` answers the patch's `logo`, an SVG document, for that host. `settings` provides
the source over the table `settings_brand` (`host`, `name`, `logo`, `hue`, `chroma`, `updated_at`,
`updated_by`) and registers `settings.brand.list` (`list`, `admin`, `.unowned()`),
`settings.brand.set { host, name?, logo?, hue?, chroma? }` (`form`, `admin`, `.unowned()`; `logo`
on the `code` control, refused unless it parses as an SVG document with no script element, `hue`
within 0 to 359 and `chroma` within 0 to 0.4) and `settings.brand.delete { host }` (`action`),
behind the menu node `system/brand`; a patched brand shows after a page load, since the shell
fetches `/boot` fresh on every load and the meta version does not name the host.

**Touches.** `crates/kernel/src/http/brand.rs`, `crates/kernel/assets/keel.svg`,
`crates/kernel/src/boot.rs` (the patch), `crates/sdk/src/product.rs` (`Asset`, `assets`) and
`system.rs` (`ManifestSource`, `ManifestPatch`), `crates/kernel/src/loader/`,
`crates/kernel/tests/kernel/boot.rs`, `crates/keel/tests/runtime.rs` and
`crates/sdk-probe/tests/harness.rs` (the test bundles gain `assets`), `crates/system/` (settings:
the migration, `brand/`, the menu node, dictionaries, tests, snapshots), `products/demo/`
(`brand/logo.svg`, the manifest), `hack/prove-binary.sh` (`/brand/logo.svg`). Kernel routes added:
`GET /brand/logo.svg`, `GET /brand/{id}/logo.svg`.

**Out of scope.** A patch of the bundle fields beyond the brand; an uploaded logo through `upload`
(a raw route, later); the shell's rendering of the brand (step 8c).

**Acceptance.**

- `crates/kernel/tests/kernel/brand.rs`: `/brand/demo/logo.svg` answers the bundle's asset as
  `image/svg+xml`, `/brand/logo.svg` the same with one product enabled and the kernel's mark with
  none, and `404` for a product without an asset.
- `crates/system/tests/system/settings_brand.rs`:
  `a_brand_patch_for_a_host_changes_boot_and_the_effective_logo_for_that_host_alone` — after
  `settings.brand.set` for `panel.example`, `/boot` with `Host: panel.example:1500` carries the
  patched `name`, `hue` and `chroma` and `/brand/logo.svg` the patched SVG, while `Host: 127.0.0.1`
  keeps the demo brand; an SVG with a script element and a `hue` of 400 answer `422` on their
  fields; the reseller is answered `403`; the descriptors match their snapshots.
- `hack/prove-binary.sh` fetches `/brand/logo.svg` and gets the demo's SVG; `cargo-semver-checks`
  stays green, `Asset` and `assets` being additions.

### 10. Host spike in `bin/spike` proven on the demo machine

**Goal.** The thin agent spike of [design/06-host.md](design/06-host.md), the test of the riskiest
hypothesis of [design/11-decisions.md](design/11-decisions.md) before any product depends on it: an
unprivileged process drives the operating system through a root agent that admits it by peer
credentials and executes a typed operation under a policy, with a configuration template validated
and rolled back on nginx. `bin/spike` (package `keel-spike`, binary `keel-spike`) depends on no
workspace crate and speaks the agent protocol of [design/10-contracts.md](design/10-contracts.md)
over JSON Lines: a request `{ id, op, actor, request_id, product, args }` and an answer
`{ id, ok, data }` or `{ id, ok: false, error: { key, detail } }`. `keel-spike agent --socket
/run/keel-spike/agent.sock --uid <uid> --policy spike.yaml --templates <dir>` runs as root under the
unit `keel-spike-agent.service`, creates the socket with mode `0666` in its `RuntimeDirectory` (mode
`0755`), so any local uid can connect and `SO_PEERCRED` is the one gate, which is what the `nobody`
scenario proves, reads `SO_PEERCRED` on every connection and closes one whose uid is not the named
one, logging the refusal; the policy file is `deploy/spike.yaml`, in the shape of the design's
`agent.yaml`, `file_roots { vhosts: /etc/nginx/conf.d }`, `services [nginx]`,
`checks { nginx.test: [nginx, -t] }` and `products { demo: { user_prefix: kd_, drop_in_prefix:
keel-demo- } }`, and a request whose `product` has no section is refused with `host.error.policy`.
Two operations: `host.user.create { name, shell? }`, which requires the product's `user_prefix`,
runs `useradd` as a process and answers `{ uid, created }`, `created: false` for a user that exists;
and `host.template.apply { template, vars, dest: { root, name }, mode, owner, validate, reload }`,
which requires a root of `file_roots` and a name with the product's `drop_in_prefix` and no path
separator, renders the named template with minijinja, writes the result to a temporary file beside
the target, swaps it in while keeping the previous content, runs the named check as a process,
restores the previous file, or removes one that did not exist, and answers
`host.error.validate_failed` with the check's output as `detail` when it fails, reloads the named
service through `systemctl reload` when it passes, restores and reloads again on a failed reload
answering `host.error.reload_failed`, and answers `{ changed }`, `false` when the rendered file
equals what was there. The spike swaps before it validates, where the design's order is render,
validate, swap, reload, rollback: `nginx -t` checks the whole configuration in place and cannot be
pointed at one drop-in, so the temporary file is validated by being in place, and the rollback
restores what was there; the commit message names the catalogue row and the paragraph of the design
that fix the order, and step 12 amends them to say that a check may need the file in place. Every
operation, admitted or refused, is written to standard error as one line with `op`, `actor`,
`request_id` and the outcome, which the unit's journal keeps, the audit rule of the design.
`keel-spike call --socket <path> --op <op> --args <json> --actor <login> --product demo` is the
client, run as the unprivileged uid, printing the answer and exiting non-zero on `ok: false`. The
template `bin/spike/templates/vhost.conf.j2` renders an nginx server block from `server_name`,
`listen` and `root`. The binary is built with `cargo build --release --target
x86_64-unknown-linux-musl -p keel-spike` on the developer machine with the musl toolchain and copied
with `scp` next to `hack/spike-vm.sh`, the unit, the policy and the templates into
`/root/keel-spike/` on `keel-demo`; the script runs from there. `hack/spike-vm.sh` runs the proof on
a VM as root: it refuses to run while `dpkg -s nginx` already succeeds, so the nginx `--cleanup`
purges is always the one the script installed, installs nginx from the distribution and writes the
marker `/root/keel-spike/.nginx-installed` that `--cleanup` requires before purging, creates
`/var/www/keel-demo-spike/index.html` with a known body as root before the first apply, installs the
unit and the policy, starts the agent, runs the scenario below as the `keel` user through `runuser`,
prints one line per check and exits non-zero on any failure, and `--cleanup` removes the unit, the
socket and `/run/keel-spike`, the policy, the created user, the vhost and its web root under
`/var/www/keel-demo-spike`, the nginx package with `apt-get purge` and `/etc/nginx`, and
`/root/keel-spike/` itself, and prints what remains of `id kd_spike`,
`systemctl status keel-spike-agent` and `dpkg -l nginx` so the transcript shows a clean machine. The
proof runs on `keel-demo`, the Debian 13 machine that carries the M1 installation, next to the
running `keel.service`, which it never touches, and its transcript goes into the commit message; the
spike's traces are removed from the machine at the end of the step, while the crate stays in the
workspace as the reference the M4 agent replaces, and M4's step that lands `crates/host-agent`
deletes `bin/spike`.

**Touches.** `Cargo.toml` (the pinned `minijinja`), `bin/spike/` (`Cargo.toml`, `src/main.rs`,
`src/agent.rs`, `src/policy.rs`, `src/ops.rs`, `src/client.rs`, `templates/vhost.conf.j2`,
`deploy/keel-spike-agent.service`, `deploy/spike.yaml`, `tests/agent.rs`), `hack/spike-vm.sh`,
`hack/check-deps.sh` (the `keel-spike` rule).

**Out of scope.** `keel-host`, `keel-host-agent`, the `host` module, `HostExecutor`, the
`privileged` subcommand and the `update` policy (M4); the other operation families; impersonation by
re-exec; `AgentOp`; any kernel code calling the spike — the spike is driven by its client alone; a
check that validates the temporary file before the swap (the M4 agent, for checks that admit a
path).

**Acceptance.**

- `bin/spike/tests/agent.rs`, Linux only, spawns the agent on a temporary socket with the test's own
  uid and a fake `nginx`, `useradd` and `systemctl` on `PATH` that record their arguments and fail
  on demand:
  `a_valid_template_is_rendered_swapped_in_validated_and_reloaded_and_a_repeat_changes_nothing`,
  `a_failed_check_restores_the_previous_file_and_answers_validate_failed_with_the_detail`,
  `a_failed_reload_restores_the_file_reloads_again_and_answers_reload_failed`,
  `a_foreign_root_an_unprefixed_name_or_user_and_an_unknown_product_are_refused_as_policy`,
  `a_user_that_exists_answers_created_false`; the policy and frame parsing have unit tests;
  `hack/check-deps.sh` asserts that `keel-spike` depends on no `keel-*` crate;
  `cargo test --workspace` in CI runs the Linux test.
- On `keel-demo`, `hack/spike-vm.sh` as root from `/root/keel-spike/`: the agent runs as root on
  `/run/keel-spike/agent.sock`; a call from a user without the agent's uid — `nobody`, through
  `runuser -u nobody` — is refused and the refusal is in `journalctl -u keel-spike-agent`; as
  `keel`, `host.user.create { name: "kd_spike" }` answers `created: true` and `id -u kd_spike`
  works; `host.template.apply` with `server_name spike.example`, `listen 80` and the root
  `/var/www/keel-demo-spike` writes `/etc/nginx/conf.d/keel-demo-spike.conf`, answers
  `changed: true`, `nginx -t` passes and `curl -H 'Host: spike.example' http://127.0.0.1/` answers
  the known body of `/var/www/keel-demo-spike/index.html`; the same apply answers `changed: false`;
  an apply with `listen eighty` answers `host.error.validate_failed` with nginx's message, the file
  is byte-identical to the previous one and the vhost still answers; `dest.root: etc` and
  `name: spike.conf` answer `host.error.policy`; `curl http://127.0.0.1:1500/healthz` of
  `keel.service` answers before and after; a second run before `--cleanup` refuses to start, since
  `dpkg -s nginx` succeeds; `--cleanup` leaves no unit, socket, runtime directory, user, vhost, web
  root, nginx package, `/etc/nginx` or `/root/keel-spike/`, and prints the three probes that show
  it.
- The transcript of that run is in the commit message of the step, and the M2 exit checklist cites
  it.

### 11a. keel-dev descriptors, contracts/dump, hack/descriptors.sh and views share

**Goal.** The fixture set taken from the binary and the custom-views share the design puts into CI
from M2. `keel-dev descriptors dump --url <url> --out <dir> [--as <level>=<login>:<password>]...`
logs in through `auth.login` for each named level, writes `<dir>/<level>/boot.json`, `products.json`
(the rows of `keel.product.list`), `i18n/en.json`, `i18n/ru.json` and `functions/<func>.json` for
every descriptor of `GET /api/v1/functions` as that principal, and `guest` without a login;
`keel-dev descriptors diff <a> <b>` prints a per-file unified diff of two dump directories, files
present on one side included, and exits non-zero on any difference; `keel-dev descriptors check`
dumps to a temporary directory and runs `diff` against `<dir>`, so a failure names every file that
differs with its diff. `hack/descriptors.sh <binary>` prepares a site as `hack/conformance.sh` does,
with an admin in the root account, a reseller in an account under it and a user in an account under
the reseller, starts the binary and runs the check against the committed `contracts/dump/`, which
`KEEL_UPDATE_DUMP=1` rewrites; the dump is the projected contract of the real binary per level and
what the mock client's dump loader, Storybook and the metric read, and
`contracts/dump/admin/boot.json` is the one-product payload of the design's three `/boot` fixtures,
the binary with `demo`. For the metric `keel.product.list` rows gain `modules`, the ids of the
bundle's modules, and `keel-dev views share --dump contracts/dump --frontend frontend` computes, per
product and for the composition, the share of custom views among functions: the numerator is the
names of `boot.views` whose module belongs to the product plus the names of
`frontend/products/<id>/views.json`, and for the composition all of them plus
`frontend/apps/shell/views.json`; the denominator is the functions of the admin dump whose `module`
belongs to the product, or all of them; one line per product and one for the composition are
printed, and a share above 20 % exits non-zero.

**Touches.** `crates/dev/src/` (`descriptors.rs`, `views.rs`, `main.rs`), `crates/kernel/src/keel/`
and `crates/kernel/src/products.rs` (`modules` on the product rows, its snapshot under
`contracts/fixtures/descriptors/`), `contracts/dump/`, `hack/descriptors.sh`, `hack/prove-binary.sh`
(`modules`), `.github/workflows/ci.yml` (`hack/descriptors.sh` in the job `check` after
`hack/conformance.sh`).

**Out of scope.** `keel-dev i18n` (M3, step 8b's note); a dump at the `owner` level, which no HTTP
client reaches; `keel-dev check`, `new-module`, `new-product`, `migrate create` and `docs` (the
milestones that need them).

**Acceptance.**

- `hack/descriptors.sh target/debug/keel` is green in CI after `hack/conformance.sh`: the committed
  `contracts/dump/` equals the binary's answers for `guest`, `user`, `reseller` and `admin`, and
  `keel-dev views share` prints `demo 1/8` and the composition line and stays under 20 %; a unit
  test of `keel-dev` shows that a ninth view over eight functions exits non-zero, and another that
  `descriptors diff` over two directories differing in one file prints that file's diff and exits
  non-zero while identical directories exit zero.
- `keel.product.list` answers `modules: ["note"]` for `demo` in `hack/prove-binary.sh`, and its
  snapshot under `contracts/fixtures/descriptors/` carries the column.

### 11b. keel-dev openapi from the dump

**Goal.** The OpenAPI description of [design/10-contracts.md](design/10-contracts.md), the document
clients in other languages read, generated from the dump of step 11a rather than from the running
binary, so it is committed and diffed like the dump itself. `keel-dev openapi --dump contracts/dump
--out contracts/openapi.json` writes it from the admin dump, one `POST /api/v1/call/<func>` per
function with the parameter schema as its request body and the envelope around the result schema as
its response; `--check` rewrites nothing and exits non-zero on a difference from the committed file,
and `hack/descriptors.sh` runs it after the dump check, so a descriptor change that reaches the dump
reaches the document in the same commit.

**Touches.** `crates/dev/src/openapi.rs` and `main.rs`, `contracts/openapi.json`,
`hack/descriptors.sh` (the currency check).

**Out of scope.** Serving the document from the binary (a route the design does not name); a
document per level; a client in another language generated from it (M7).

**Acceptance.**

- `contracts/openapi.json` is current in `hack/descriptors.sh`, names one `POST /api/v1/call/<func>`
  per function of the admin dump, and a unit test of `keel-dev` shows a changed parameter schema
  fails the check.

### 11c. contracts/icons.json and the freeze check

**Goal.** The `PhxIconName` check M0 left to M2. `pnpm generate` also runs `node tools/icons.mjs`,
which imports `PHX_ICON_NAMES` from `@globalart/phoenix-ui` and writes them sorted to
`contracts/icons.json`; the kernel embeds that file with `include_str!`, and the freeze refuses a
descriptor or a menu node naming an icon outside it, so the shell never receives an unknown icon;
the job `frontend` regenerates the file and fails on a difference, and the job `check` needs the
frontend install it already has since step 9a.

**Touches.** `frontend/tools/icons.mjs`, `frontend/package.json` (`generate`),
`contracts/icons.json`, `crates/kernel/src/loader/freeze.rs` (the icon check),
`.github/workflows/ci.yml`.

**Out of scope.** A check of the icons a custom view uses in its own template (the product's
build).

**Acceptance.**

- `contracts/icons.json` is current in the job `frontend`, sorted, and names `plus`, which the
  descriptor fixtures use; `pnpm generate` rewrites it and `git diff --exit-code` holds.
- `an_icon_outside_the_phoenix_set_in_a_descriptor_or_a_menu_node_fails_freeze` holds in
  `crates/kernel/src/loader/freeze.rs`, and every descriptor and menu node of the system modules
  and the demo passes the check.

### 11d. @globalart/keel/testing and Storybook in CI

**Goal.** The rest of the `/testing` entry point of [design/08-frontend.md](design/08-frontend.md)
and Storybook from fixtures. `KeelMockClient` of step 4 gains `fromDump(dir, level)`, which loads
`contracts/dump/<level>/` — `boot.json`, the descriptors, the catalogue — so a story or a spec runs
over the projected contract of the binary; the principal fixtures `KEEL_PRINCIPALS` for `guest`,
`user`, `reseller` and `admin` match the dump levels and the testkit's `Who`; the Storybook
decorator `withKeel({ level, boot, calls })` provides the mock client, the boot and the catalogue to
a story; and the three `/boot` fixtures the design asks for are exported as `KEEL_BOOT`:
`contracts/fixtures/boot.json`, the two-product payload since M0, `contracts/dump/admin/boot.json`,
the one-product payload of step 11a, and `contracts/fixtures/boot-none.json` of step 8a. Storybook
10 lives in `frontend/.storybook` on the Angular builder, through the targets `storybook` and
`build-storybook` of the project `keel` in `frontend/angular.json` on
`@storybook/angular:start-storybook` and `@storybook/angular:build-storybook`,
`configDir: .storybook`, `outputDir: storybook-static`, the phoenix styles and the IBM Plex fonts in
`styles`, as the `phoenix-ui` project of the sibling monorepo declares them, so `pnpm storybook` and
`pnpm build-storybook` are `ng run keel:storybook` and `ng run keel:build-storybook`, with the
`argTypesEnhancer` for function arguments and the test-runner shim for `module.register` copied from
phoenix-ui: the stories `Renderer/List`, `Renderer/Form`, `Renderer/Wizard`, `Renderer/Object` over
the dump's `note.list`, `note.edit`, `note.import`, `keel.account.list`, `settings.list`,
`audit.list` and `audit.get`, `Shell/NoProduct`, `Shell/OneProduct` and `Shell/TwoProducts` over the
three boot fixtures, and `Shell/Login`, `Shell/Totp` and `Shell/ActingAs`. `pnpm build-storybook`
writes `frontend/storybook-static`; the job `frontend` installs chromium with `pnpm exec playwright
install --with-deps chromium`, serves the static build with `http-server` on 6006, waits on
`index.json` with `wait-on` and runs `pnpm test-storybook` (`test-storybook --url
http://127.0.0.1:6006 --ci` under `frontend/test-runner-jest.config.js`, which loads the
`module.register` shim), the static build kept as an artifact — the same three steps as
`phoenix-ui-tests.yml`.

**Touches.** `frontend/libs/keel/testing/src/` (`mock-client.ts`, `principals.ts`, `storybook.ts`,
`boot.ts`, `public-api.ts`), `frontend/.storybook/`, `frontend/angular.json` (the targets
`storybook` and `build-storybook` of the project `keel`), `frontend/test-runner-jest.config.js`,
`frontend/libs/keel/renderer/src/**/*.stories.ts`, `frontend/libs/keel/shell/src/**/*.stories.ts`,
`frontend/package.json` (`storybook`, `build-storybook`, `test-storybook`; the packages
`@storybook/angular`, `@storybook/test-runner`, `playwright`, `http-server`, `wait-on`,
`axe-playwright`), `.github/workflows/ci.yml`, `.gitignore` (`frontend/storybook-static`).

**Out of scope.** Visual regression beyond the test-runner's play functions and a11y checks; a
deployed Storybook; the Playwright fixture and suite (step 11e); `FakeHost` (M4).

**Acceptance.**

- `mock-client.spec.ts` gains "fromDump answers the admin boot and every descriptor of the dump" and
  fails when a level directory lacks `boot.json`.
- `pnpm test-storybook` is green in the job `frontend` over the stories above, and
  `Shell/OneProduct` shows the demo brand with its groups then `system`, `Shell/TwoProducts` the
  platform brand with a heading per product and `Shell/NoProduct` the platform group alone — the
  Storybook exit criterion; `frontend/storybook-static` is uploaded as an artifact of the job.

### 11e. Playwright and hack/e2e.sh

**Goal.** Playwright against the built binary, the browser-driven half of the exit criteria. The
Playwright fixture `keelTest` of `@globalart/keel/testing` reads `KEEL_E2E_URL` as `baseURL`, offers
`login(login, password)` through the real `/login` screen and `auth.login`, since the test
authenticator of `keel-testkit` is not in the binary, and `asAdmin()`, `asReseller()` and `asUser()`
over the logins and the passwords the variables below carry. `hack/e2e.sh <binary>` prepares a site
as `hack/conformance.sh` does — a database created on `KEEL_TEST_DATABASE_URL` and dropped on exit,
`product enable demo`, the admin `root` in the root account, the reseller `res1` in an account under
it and the user `alice` in an account under the reseller, each with a generated password set over
`ctl auth.password.set` — starts `serve` on a free port of `127.0.0.1`, exports
`KEEL_E2E_URL=http://127.0.0.1:<port>` — the origin on which the `__Host-` cookie is accepted over
plain HTTP — and `KEEL_E2E_ROOT_PASSWORD`, `KEEL_E2E_RES1_PASSWORD`, `KEEL_E2E_ALICE_PASSWORD`, runs
`pnpm e2e` from `frontend/` and stops the process; the job `check` installs chromium and runs it
after `hack/descriptors.sh`.

**Touches.** `frontend/libs/keel/testing/src/playwright.ts`, `frontend/e2e/`
(`playwright.config.ts`, `login.spec.ts`, `key.spec.ts`, `notes.spec.ts`, `system.spec.ts`,
`language.spec.ts`), `frontend/package.json` (`e2e`, `@playwright/test`, `otpauth` for the codes
`login.spec.ts` computes from the shown secret), `hack/e2e.sh`, `.github/workflows/ci.yml` (chromium
and `hack/e2e.sh` in the job `check`).

**Out of scope.** A run against a remote installation; a second browser; a login through a bearer
token.

**Acceptance.**

- `hack/e2e.sh target/debug/keel` is green in the job `check`: `login.spec.ts` (a wrong password
  shows `auth.error.credentials`, `alice` logs in and out, a guest opening `/f/note.list` lands on
  the login with `returnTo` and returns there after it, and `root` enrols the second factor from the
  security page, the test computing the codes from the shown secret, logs in through the code step
  and disables it); `key.spec.ts` (`root` issues a key for `alice` through the API and `/login?key=`
  lands her on the redirect); `notes.spec.ts`, the exit criterion driven without product TypeScript:
  `alice` creates notes through the form, the list searches, sorts by title, filters `kind` to
  `todo`, selects two rows and deletes them through the destructive confirm by typing `2`; the form
  has no `owner` page for `alice` and has it for `root`, `due` appears when `kind` turns `todo` with
  a date a week ahead from `note.recompute`, and a duplicate title shows `note.error.exists` under
  `title`; the wizard `note.import` refuses a 201-character line on its first step and creates three
  notes from three lines; `/f/note.tag.list` renders the tag cloud of `@globalart/keel-demo` with
  the tags of those notes; `system.spec.ts` (`root` opens accounts, principals, products, settings,
  audit, sessions, tokens and grants from the menu and each lists, sets `auth.password.min_length`
  through the value form, creates a token whose plaintext is shown once, acts as `alice` from the
  user menu and sees the banner and the narrowed list, and exits); `language.spec.ts` (`alice`
  switches to `ru`, the menu, the column headers and the form labels change, and the choice survives
  a reload).
- With `KEEL_E2E_URL` unset, `pnpm e2e` fails at once naming the variable rather than against a
  default port.

### 12. M2 closure

**Goal.** Close the milestone the way the M0 and M1 checklists did, apply the amendments steps 1 to
11e owed the design, and put the built M2 in front of the owner. Run every M2 "Done when" item by
hand or through CI and record for each the test name or the command that shows it; add the section
"M2 exit checklist" after the M1 checklist, in the same form, citing Rust tests by name, Jest and
Playwright specs by file, and the commands of `hack/` and `ci.yml`; and make the consistency pass
over `docs/design/` and this plan against the built M2. The pass applies what the commit messages of
steps 1 to 11e recorded: the menu item of a view without a `kind` in the `/boot` table of
[design/10-contracts.md](design/10-contracts.md); the `action` kind in `mode: read` of `recompute`
and `validate` helpers in the form-dynamics table and in
[design/02-functions.md](design/02-functions.md); the object renderer of a `get` reached through a
link in the M2 row of the renderer table of [design/08-frontend.md](design/08-frontend.md) and in
the `ui` row of the descriptor table of [design/02-functions.md](design/02-functions.md); the chunks
of the enabled products loaded at bootstrap in the products paragraph of
[design/08-frontend.md](design/08-frontend.md), with the first-route loading kept as the aim for the
milestone that carries the module-to-product map; the "Group editing" bullet of the `/forms` section
of [design/08-frontend.md](design/08-frontend.md), which gains that `ui.multi` lands in M3 with
`batch_threshold`, as the M3 scope line names it; `keel.export` and the export button of the
`/client` row and the list-renderer sentence of the same document, deferred to M3 with the row
threshold; the numerator of the custom-views share in its "The ceiling" paragraph, widened from the
`r.view` registry alone to the replacements of `frontend/products/<id>/views.json` and the shell's
own views of `frontend/apps/shell/views.json`, as step 11a computes it; the version rule of the
library in the same document — its major equals the descriptor version — which gains that it applies
from SDK 1.0 in M7, the `0.x` releases carrying the binary's version until then;
`ProductManifest.assets` and the brand routes in [design/05-modules.md](design/05-modules.md);
`PermissionSource` as `grants` overlaid on the principal and `Principal.grants` in
[design/03-access.md](design/03-access.md); `ManifestSource` and the `settings_brand` shape in the
`settings` section; the provisional `/api/v1/events` path recorded for M3 in the endpoint map; the
control mapping table, the column mapping, the shell's own views, the `auth` menu nodes and the
labels bridge in [design/08-frontend.md](design/08-frontend.md); the template order of
[design/06-host.md](design/06-host.md), the catalogue row of `host.template.apply` and the
"Configuration templates" paragraph, which gain that a check needing the file in place validates
after the swap and the rollback restores the previous content, as the spike showed; the
pre-compressed assets, the cache rules, the extension rule of the SPA fallback, the CSP and the
`pnpm build` that precedes `cargo build` in [design/09-operations.md](design/09-operations.md); the
measured size of the binary against the 40 MB cap in
[design/11-decisions.md](design/11-decisions.md); `keel-dev` with the subcommands that exist and
`bin/spike` in the tooling table and the repository layout of
[design/01-overview.md](design/01-overview.md); the `contracts/` README with `schema/`, `dump/`,
`icons.json` and `openapi.json`; the M0 notes that pointed at M2 — `keel-dev i18n pull` and
`missing`, now deferred to M3 with the OwnLate projects, and the `PhxIconName` check, done by step
11c; the opening paragraph of this plan, the "How we work" section, which gains
`pnpm lint && pnpm test && pnpm build`, `hack/descriptors.sh` and `hack/e2e.sh` in CI's order, the
M2 scope lines that named the server halves, and any one-line mismatch the pass reveals. `README.md`
follows the code: the M2 row is set to done, the command list gains the frontend commands, the
`pnpm build` prerequisite of `cargo build -p keel-bin`, `hack/descriptors.sh` and `hack/e2e.sh`, and
the layout table names `frontend/`, `crates/dev` and `bin/spike`. `hack/check-docs.sh` carries the
list of M2 names — `embed_frontend!`, `Frontend`, `ViewSpec`, `PermissionSource`, `ManifestSource`,
`ManifestPatch`, `Asset`, `grants`, `keel-dev`, `provideKeelViews`, `KeelViewContext`,
`bootstrapKeel`, `PHX_LABELS`, `PhxDataTableFetch`, `KeelMockClient` — and asserts that each is
found under `docs/design/` and under `crates/` or `frontend/libs/keel`, that every Rust test the M2
checklist cites is defined, that every `*.spec.ts` it cites exists under `frontend/`, that every
command it cites exists, and that the README's check list, "How we work" and `ci.yml` agree.
`@globalart/keel@0.1.0` is published to Nexus by `publish-npm.yml` through `workflow_dispatch` from
the closing commit, the publication the M2 scope names. The demo machine is updated by hand: the
musl artifact of the closing commit's `build.yml` run is unpacked into
`/opt/keel/versions/<version>` on `keel-demo` beside the M1 installation, `bin/current` is switched,
`keel.service` is restarted and `/readyz` answers `ready`; the panel is opened through an SSH port
forward to the machine's listener, `http://localhost:1500/`, since the `__Host-` cookie needs that
origin on plain HTTP, and the owner logs in as the administrator `deploy/install.sh` created, the
password reset over `keel ctl auth.password.set` when it is no longer at hand, walks the notes list,
the form, the wizard and the tag cloud, and switches to `ru`.

**Touches.** `docs/plan.md`, `docs/design/*.md` (the consistency pass), `README.md`,
`contracts/README.md`, `hack/check-docs.sh`, `.github/workflows/ci.yml`; the demo machine
`keel-demo`.

**Out of scope.** Any code change beyond what a document alignment reveals as a one-line mismatch;
anything of M3.

**Acceptance.**

- Every "Done when" item of M2 is ticked in the new checklist with the name of the test, the spec or
  the command that demonstrates it, and every item is green on `main`: the browser-driven `demo`
  list, form and wizard (`notes.spec.ts`), the custom view (`views.spec.ts` and `notes.spec.ts`),
  Storybook from the three boot fixtures (`pnpm test-storybook`), the `en` and `ru` switch
  (`language.spec.ts`), the spike on `keel-demo` (the transcript of step 10), and the binary under
  40 MB (the size `build.yml` printed for the closing commit).
- `hack/check-docs.sh` is green in CI with the M2 names and the frontend specs; the README's check
  list, "How we work" and `ci.yml` agree, `hack/descriptors.sh` and `hack/e2e.sh` included.
- `@globalart/keel@0.1.0` answers `pnpm view` against Nexus with the six entry points and the nested
  `testing/playwright` in `exports`.
- On `keel-demo`, `keel --version` under `/opt/keel/bin/current` names the closing commit's version,
  `/readyz` answers `ready`, and the owner has opened the panel through the port forward and logged
  in, which the checklist records with the date-free note "confirmed by hand on keel-demo".

## How we work

- The code lives in the monorepo `GlobalArtInc/keel` on the `main` branch until the SDK reaches
  1.0; a product's crates may move to their own repository after M7 and are pulled into the
  `keel` build as dependencies ([design/01-overview.md](design/01-overview.md)).
- One task is one PR-sized change; a task is not split across branches.
- Verify with the same commands CI runs before pushing, in CI's order, which the README lists.
  The job `check`: `cargo fmt --all --check`; in `frontend/`, `pnpm install --frozen-lockfile`
  and `pnpm build`, since `cargo build -p keel-bin` embeds `frontend/dist/shell`; then
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` against the
  Postgres server `KEEL_TEST_DATABASE_URL` names, `env -u KEEL_TEST_DATABASE_URL cargo test -p
  keel-testkit` against the container the testkit starts,
  `cargo run -p keel-dev -- types --out contracts/schema --check`, `cargo test -p keel-bin
  --no-default-features`, `cargo build -p keel-bin`, `hack/prove-binary.sh target/debug/keel`,
  `hack/check-install.sh target/debug/keel`, `hack/conformance.sh target/debug/keel`,
  `hack/descriptors.sh target/debug/keel`, `pnpm exec playwright install --with-deps chromium`
  once and `hack/e2e.sh target/debug/keel`, then `hack/check-test-databases.sh`,
  `hack/check-deps.sh` and `hack/check-docs.sh`. The job `frontend`, in `frontend/`:
  `pnpm install --frozen-lockfile`, `pnpm generate` followed by `git diff --exit-code`,
  `pnpm lint`, `pnpm format`, `pnpm test`, `pnpm build`, `pnpm build-storybook`,
  `pnpm exec http-server storybook-static --port 6006 --silent &`,
  `pnpm exec wait-on http://127.0.0.1:6006/index.json` and `pnpm test-storybook`,
  `node tools/stamp.mjs` and, in `frontend/dist/keel`, `pnpm publish --dry-run --no-git-checks`.
  The job `system` runs the `keel-system` matrix, and the Build workflow the musl release with
  its size check.
- English everywhere in the repository: code, documentation, tests, CI, commit messages.
- No comments in code. Explanations go into the commit message or into `docs/`; the only
  exception is a link to an RFC next to the code that implements it.
- The project's own vocabulary only: `kind`, `role`, `control`, `level` and every other name is
  ours. No other control-panel product is named in code, documentation or commit messages, and
  none of its identifiers is borrowed; a name audit runs before the first release.
- Postgres is the only database; tests get a database per test from a template, and there is no
  in-memory substitute.

## M0 exit checklist

Every item below is checked by CI on `main`: the tests of the workspace, the build of `keel-bin`
with and without the `demo` feature, and `hack/prove-binary.sh` against the built binary.

- [x] `keel ctl note.edit title=x` returns the envelope with the `demo` bundle enabled
      (`hack/prove-binary.sh`).
- [x] `POST /api/v1/call/note.list` returns the envelope (`hack/prove-binary.sh`, the
      `401 unauthenticated` envelope without an authenticator; the `200` page through the test
      authenticator in `products/demo/tests`).
- [x] Descriptors match the committed snapshot (`contracts/fixtures/descriptors/` for the
      kernel, `products/demo/tests/snapshots/` for `note`).
- [x] The test "a user neither sees nor edits another account's notes" passes.
- [x] The test "a reseller sees its subtree" passes.
- [x] A nested `invoke` reuses the transaction of the call (`note.edit`).
- [x] The binary starts without `config.yaml` against the default database `keel`: a missing
      file yields the defaults (`a_missing_config_file_yields_the_platform_defaults`), CI
      starts the binary against its own database through `--config`, and the start against a
      host's `keel` database over the Postgres socket with peer authentication was checked by
      hand on the Debian 13 machine `keel-demo` through `deploy/install.sh` (the config it
      wrote carries `listen` only); the installer of M4 runs it in CI.
- [x] `keel serve` with no bundle enabled starts and answers on the socket
      (`crates/keel/tests/runtime.rs` and the second run of `hack/prove-binary.sh`).
- [x] `keel product disable demo` leaves the `note_` table and flips `note` to `orphaned`
      (the second run of `hack/prove-binary.sh`, where `note.list` answers `not_found`).
- [x] `cargo fmt --all --check` is green.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [x] `cargo test --workspace` is green against Postgres, and `hack/check-test-databases.sh`
      finds no database left behind.

## M1 exit checklist

Every item below is checked by CI on `main`: the tests of the workspace, the `keel-system`
matrix, the testkit's own tests against the container it starts, the build of `keel-bin` with and
without the `demo` feature, `hack/prove-binary.sh` and `hack/conformance.sh` against the built
binary, and `hack/check-docs.sh` over this checklist.

- [x] Over HTTP: login, then boot, then `keel.account.list`, then `auth.su`, after which the
      list is narrowed and `actor` is filled, then `su.exit`
      (`login_boot_list_su_narrows_the_list_and_fills_the_actor_then_su_exit_returns` in
      `crates/system/tests/system/auth_su.rs`; the auth group of `hack/conformance.sh`; the `su`
      sequence of `hack/prove-binary.sh`).
- [x] A token scoped to one function receives `403` on another
      (`a_token_scoped_to_one_function_is_admitted_there_and_refused_elsewhere_after_the_level_guard`
      in `crates/system/tests/system/auth_tokens.rs`;
      `a_token_scoped_to_note_edit_alone_edits_a_note_through_its_nested_invokes` in
      `products/demo/tests/demo/tokens.rs` for the nested invokes; the token group of
      `hack/conformance.sh`; `note.edit` under a token in `hack/prove-binary.sh`).
- [x] Writing a field above the caller's level yields `forbidden_field`
      (`login_sets_the_cookie_and_the_session_drives_boot_lists_and_forbidden_field` in
      `crates/system/tests/system/auth_sessions.rs`, over a real session; the errors group of
      `hack/conformance.sh`).
- [x] The audit trail contains both sides of `su` and masks the password, including for
      failed calls (`the_su_sequence_records_both_sides_of_su_and_the_calls_made_inside_it` and
      `logins_and_password_changes_are_masked_and_no_session_token_key_or_factor_reaches_the_trail`
      in `crates/system/tests/system/audit.rs`;
      `a_wrong_password_and_an_unknown_login_are_one_answer_and_the_audit_masks_the_password` in
      `crates/system/tests/system/auth_sessions.rs` for the event of the failed call; the `audit_call`
      queries of `hack/prove-binary.sh`).
- [x] The conformance skeleton is green against the built binary (`hack/conformance.sh
      target/debug/keel` in CI after `hack/prove-binary.sh`; a broken expectation exits non-zero:
      `a_broken_expectation_makes_the_suite_exit_non_zero_with_one_line_per_check` in
      `crates/conformance/tests/exit_code.rs`).
- [x] `auth`: argon2id passwords
      (`the_ledger_and_the_tables_exist_and_a_set_password_is_stored_as_argon2id` in
      `crates/system/tests/system/auth_passwords.rs`); opaque sessions and the `__Host-` cookie with the
      cross-site check (`login_is_refused_cross_site_and_replaces_the_session_it_arrived_with` and
      `a_session_slides_with_every_request_and_ends_after_the_idle_and_absolute_lifetimes` in
      `crates/system/tests/system/auth_sessions.rs`;
      `every_request_without_a_bearer_token_is_checked_for_cross_site` and
      `authenticators_are_asked_in_module_order_until_one_answers_or_fails` in
      `crates/kernel/tests/kernel/http.rs`); Bearer tokens with scope and IP restrictions
      (`ip_rules_revocation_expiry_and_last_used_govern_a_token_and_the_plaintext_is_shown_once`
      in `crates/system/tests/system/auth_tokens.rs`); TOTP with recovery codes
      (`a1_enrols_confirms_and_logs_in_through_a_challenge_with_the_step_moving_forward_only` and
      `a_recovery_code_logs_in_once_without_moving_the_step_and_is_stored_as_its_hash` in
      `crates/system/tests/system/auth_totp.rs`, the sealed secret through
      `a_handler_seals_through_the_cipher_and_only_the_composition_with_the_same_key_opens_it` in
      `crates/kernel/tests/kernel/cipher.rs`); login keys
      (`a_key_issued_for_a1_is_redeemed_once_and_refused_when_spent_expired_or_made_up` in
      `crates/system/tests/system/auth_keys.rs`); the socket side of delegation
      (`a_frame_with_as_runs_as_the_named_principal_with_the_owner_as_actor_via_socket` in
      `crates/kernel/tests/kernel/socket.rs`); IP rules
      (`ip_rules_refuse_by_address_and_level_take_effect_at_once_and_never_apply_to_the_socket`
      in `crates/system/tests/system/auth_guards.rs`, and the rule surviving a restart in the second run
      of `hack/prove-binary.sh`); rate limiting
      (`the_thirty_first_guest_call_from_one_address_within_a_minute_is_rate_limited_and_the_bucket_leaves_with_its_window`);
      lockout and unlock
      (`five_wrong_passwords_lock_the_login_from_that_address_only_until_the_window_passes_or_the_admin_unlocks`
      and `five_wrong_codes_lock_the_challenge_pair_for_the_address_and_the_code_passes_from_another`);
      the session list and revocation
      (`each_principal_lists_its_own_sessions_the_reseller_its_subtree_the_admin_all_and_the_current_row_is_marked`
      and `revoking_a_parent_takes_its_su_child_and_revoking_the_child_alone_ends_the_delegation`
      in `crates/system/tests/system/auth_session_list.rs`); the password policy
      (`the_policy_refuses_a_short_password_and_one_equal_to_the_login_wherever_it_is_set` in
      `crates/system/tests/system/auth_policy.rs`).
- [x] `settings`: values, inheritance and `Call::setting`
      (`the_admin_lists_the_six_auth_definitions_and_a_raised_minimum_makes_the_password_refuse_twelve_characters`
      and
      `a_value_inherits_from_the_root_down_the_account_path_and_a_composition_without_settings_resolves_the_default`
      in `crates/system/tests/system/settings.rs`;
      `without_a_settings_module_the_platform_answers_the_contributed_defaults_and_a_provided_source_replaces_it`
      in `crates/kernel/tests/kernel/settings.rs`); preferences and the caller's time zone
      (`a1_sets_tz_and_lang_and_boot_and_the_catalogue_follow_while_a2_reads_nothing_of_it` in
      `crates/system/tests/system/settings_pref.rs`;
      `the_transport_overlays_the_preferred_language_and_hands_the_zone_to_every_call_of_the_request`
      in `crates/kernel/tests/kernel/preferences.rs`;
      `a_bare_date_filter_takes_the_day_boundary_in_the_callers_zone_and_utc_without_one` in
      `products/demo/tests/demo/day_boundary.rs`).
- [x] `audit`: the trail from a background worker, flushed and drained
      (`flush_returns_after_the_batch_is_written_and_a_stop_leaves_no_event_unwritten` and
      `audit_list_filters_by_function_principal_status_request_and_time_within_the_callers_scope`
      in `crates/system/tests/system/audit.rs`;
      `a_read_leaves_no_row_a_replay_leaves_no_second_row_and_request_id_ip_and_via_are_filled`).
- [x] `ConfigSection`: a section validated and typed at freeze
      (`a_valid_section_reaches_start_and_the_handlers_an_absent_one_yields_the_defaults` and
      `a_section_of_the_wrong_shape_fails_the_boot_naming_the_module_the_section_and_the_path` in
      `crates/sdk-probe/tests/harness.rs`;
      `a_claimed_section_is_parsed_into_the_services_and_an_unclaimed_key_is_logged` and
      `a_section_claimed_twice_and_one_of_the_wrong_shape_are_freeze_problems` in
      `crates/kernel/src/loader/freeze.rs`; the unclaimed `host` section of
      `hack/prove-binary.sh`).
- [x] Postgres through testcontainers when no server is named
      (`the_server_is_the_named_one_when_the_variable_is_set_and_a_started_container_otherwise` in
      `crates/testkit/tests/harness.rs`, run by CI as
      `env -u KEEL_TEST_DATABASE_URL cargo test -p keel-testkit`).
- [x] The `keel-system` matrix is green: the `system` job of `.github/workflows/ci.yml` runs
      `cargo clippy` and `cargo test -p keel-system --no-default-features --features <set>` for
      `auth`, `settings`, `audit`, `settings,audit`, `auth,audit` and `auth,settings`
      (`settings_alone_boots_with_its_tables_and_the_admin_lists_and_sets_a_value_over_http` in
      `crates/system/tests/system/settings_alone.rs`,
      `audit_alone_boots_with_its_table_and_one_owner_write_leaves_one_row_after_a_flush` in
      `crates/system/tests/system/audit_alone.rs`).
- [x] `cargo fmt --all --check` is green.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [x] `cargo test --workspace` is green against Postgres, and `hack/check-test-databases.sh`
      finds no database left behind.
- [x] `hack/check-deps.sh` is green: `keel-system` depends on `keel-sdk` alone and
      `keel-conformance` on `keel-types` alone.
- [x] `hack/check-docs.sh` is green: every M1 name it carries is found under both `docs/design/`
      and `crates/`, every test name this checklist cites is defined, every command it cites
      exists, and the README's check list, the "How we work" section and `ci.yml` agree.

## M2 exit checklist

Every ticked item below is checked by CI on `main` — the tests of the workspace, the `keel-system`
matrix, the build of `keel-bin` with and without the `demo` feature after `pnpm build`,
`hack/prove-binary.sh`, `hack/conformance.sh`, `hack/descriptors.sh` and `hack/e2e.sh` against the
built binary, the job `frontend` with Jest, the static Storybook and its runner, and
`hack/check-docs.sh` over this checklist — except the spike's run on `keel-demo`, which the
transcript in the message of commit `205b8fb` records. The three items left unticked are run by
the operator on `keel-demo`, by the owner in a browser and by `publish-npm.yml`; each is ticked
once it holds, the `keel-demo` ones with the date-free note "confirmed by hand on keel-demo".

- [x] The `demo` bundle, enabled on the `keel` binary, is driven entirely from the browser
      without product-specific TypeScript: `frontend/e2e/notes.spec.ts`, run by
      `hack/e2e.sh target/debug/keel` against the built binary, creates notes through the form
      with `due` filled a week ahead by `note.recompute`, searches, sorts by title, filters by
      `kind`, selects two rows and deletes them through the destructive confirmation by typing
      their count, shows the owner page to `root` alone (`show_if` on the level), lands a
      duplicate title under the `title` field, and walks the import wizard, which refuses a
      201-character line on its first page and imports three lines on its last. The demo's only
      TypeScript is the tag cloud: `frontend/products/demo/src/demo.spec.ts` holds `views.json` to
      what the library registers and `hack/descriptors.sh` prints `demo 1/8`. The renderers under
      Jest against the golden fixtures: `frontend/libs/keel/renderer/src/list.spec.ts`,
      `frontend/libs/keel/forms/src/form.component.spec.ts`,
      `frontend/libs/keel/forms/src/conditions.spec.ts`,
      `frontend/libs/keel/forms/src/recompute.spec.ts`, `frontend/libs/keel/forms/src/errors.spec.ts`,
      `frontend/libs/keel/forms/src/submit.spec.ts`, `frontend/libs/keel/renderer/src/wizard.spec.ts`
      and `frontend/libs/keel/renderer/src/action.spec.ts`; the server side of the shapes in
      `products/demo/tests/demo/note.rs`
      (`note_list_searches_and_sorts_and_note_delete_removes_a_batch_unless_a_note_is_locked`,
      `note_edit_refuses_a_second_note_with_the_same_title_as_a_field_error_on_title`,
      `note_recompute_fills_due_a_week_ahead_when_kind_turns_todo_and_patches_nothing_otherwise`,
      `note_import_checks_the_source_page_and_creates_one_note_per_line_inside_the_limit`,
      `note_tag_list_counts_tags_within_the_callers_scope_with_a_sum_total`).
- [x] One screen is replaced by a custom view through `provideKeelViews`: the demo's tag cloud
      replaces `note.tag.list` (`frontend/products/demo/src/tag-cloud.spec.ts`; "the tag cloud
      shows the tags of the notes" in `frontend/e2e/notes.spec.ts`); the registry itself in
      `frontend/libs/keel/renderer/src/views.spec.ts` ("a component registered through
      provideKeelViews replaces the generic screen and receives the context with the descriptor",
      "a view of boot.views is created without a descriptor and with the screen parameters", "a
      view of boot.views without a component shows phx-not-found naming the product"); the
      server half, `r.view` behind `/boot.views`, in
      `a_registered_view_is_listed_in_boot_for_the_levels_its_access_admits_and_menus_without_a_kind`
      and `a_view_with_a_foreign_prefix_an_unknown_permission_or_an_unknown_group_fails_freeze` in
      `crates/kernel/tests/kernel/views.rs`; the custom-views share in CI, `demo 1/8 12.5%` and
      `composition 3/54 5.6%` under the 20 % ceiling, from `keel-dev views share` through
      `hack/descriptors.sh target/debug/keel`
      (`a_ninth_view_over_eight_functions_exceeds_the_ceiling_while_one_over_eight_holds` and
      `registered_views_count_for_the_product_owning_their_module_and_the_shell_views_for_the_composition_alone`
      in `crates/dev/src/views.rs`).
- [x] Storybook renders the shell from the `/boot` fixtures: the stories `NoProduct`, `OneProduct`
      and `TwoProducts` of `frontend/libs/keel/shell/src/shell.stories.ts` over `KEEL_BOOT` of
      `@globalart/keel/testing` show the platform group alone, the demo's brand with its logo at
      `/brand/logo.svg` and its groups before `system`, and the platform brand with a heading per
      product in `/boot` order, run with their accessibility checks by `pnpm test-storybook`
      against the static build of `pnpm build-storybook` (5 story files, 21 stories); the same
      rule under Jest in `frontend/libs/keel/shell/src/sidebar.spec.ts` ("one product shows its
      groups then system under the product brand", "two products show a heading per product in
      boot order with the platform group last and the platform brand", "no product shows the
      platform group alone", "a heading without a visible item is omitted"); the fixtures
      themselves from the kernel:
      `boot_over_the_socket_as_the_owner_carries_the_enabled_product_its_brand_and_both_groups` and
      `boot_without_a_product_shows_the_platform_brand_and_the_platform_group_only` in
      `crates/kernel/tests/kernel/boot.rs`, and `frontend/libs/keel/testing/src/boot.spec.ts` holding
      `KEEL_BOOT` equal to the three files.
- [x] `en` and `ru` switch: `frontend/e2e/language.spec.ts` ("alice switches to ru and the choice
      survives a reload" — the menu, the column headers and the form labels change and the
      choice survives a reload, every expectation resolved from the served catalogues so no
      translated word is repeated in the test); `frontend/libs/keel/shell/src/rail.spec.ts` (the
      language switch posts `settings.pref.set { key: "lang", value: "ru" }` and reloads the
      catalogue) and `frontend/libs/keel/shell/src/labels.spec.ts` (`PHX_LABELS` switches with
      the language); the catalogue on the server in
      `the_catalogue_merges_the_bundle_dictionaries_and_ru_falls_back_to_en_per_key` in
      `crates/kernel/tests/kernel/boot.rs` and
      `a1_sets_tz_and_lang_and_boot_and_the_catalogue_follow_while_a2_reads_nothing_of_it` in
      `crates/system/tests/system/settings_pref.rs`; `ctl_note_list_out_table_shows_translated_headers_in_en_and_ru`
      in `products/demo/tests/demo/note.rs`.
- [x] On a VM the spike, running as an unprivileged uid, creates an OS user and a vhost through
      the agent, and an invalid template is rolled back: the transcript of step 10 in the message
      of commit `205b8fb` records the run of `hack/spike-vm.sh` on `keel-demo` — the call as
      `nobody` refused with `refused peer_uid=65534 admitted_uid=999`, `host.user.create`
      answering `created: true` and `id -u kd_spike` working, the vhost written and answering the
      known body, the apply with `listen eighty` answering `host.error.validate_failed` with
      nginx's message and the vhost byte-identical to the previous one, and `--cleanup` leaving
      no trace; nothing under `bin/spike`, `deploy/spike.yaml` or `hack/spike-vm.sh` changed
      between that commit and the closing one. The mechanism is checked in CI by
      `bin/spike/tests/agent.rs`
      (`a_peer_with_another_uid_is_closed_without_an_answer_and_the_refusal_is_logged`,
      `a_valid_template_is_rendered_swapped_in_validated_and_reloaded_and_a_repeat_changes_nothing`,
      `a_failed_check_restores_the_previous_file_and_answers_validate_failed_with_the_detail`,
      `a_failed_reload_restores_the_file_reloads_again_and_answers_reload_failed`,
      `a_foreign_root_an_unprefixed_name_or_user_and_an_unknown_product_are_refused_as_policy`,
      `useradd_runs_with_the_shell_and_its_failure_answers_exec_failed_with_its_output`), and the
      Build workflow ships the musl `keel-spike` with its unit, policy, templates and
      `hack/spike-vm.sh` as the artifact `keel-spike-x86_64-linux-musl`, from which the operator
      may repeat the run on `keel-demo` at will; that repeat is not a condition of this tick.
- [x] The binary with the embedded frontend is under 40 MB: the Build workflow's step "The binary
      with the demo bundle and the frontend is under 40 MB" prints the size of the musl release
      build and fails at 40,000,000 bytes; the closing commit's build measures 19,822,864 bytes
      (19.8 MB) as a stripped `x86_64-unknown-linux-musl` release with thin LTO, the `demo` bundle
      and the shell included.
- [x] The shell is embedded and served beside the API:
      `a_runtime_with_an_embedded_frontend_serves_the_index_and_its_assets_beside_the_api` in
      `crates/keel/tests/runtime.rs` through `keel::embed_frontend!`;
      `the_root_and_a_screen_path_answer_the_index_no_store_with_the_policy_headers`,
      `a_hashed_asset_is_immutable_for_a_year_and_has_a_brotli_twin_when_accepted`,
      `a_missing_asset_is_404_while_the_api_keeps_its_envelope_and_the_probes_are_untouched` and
      `without_a_frontend_every_unknown_path_keeps_the_route_not_found_envelope` in
      `crates/kernel/tests/kernel/assets.rs`; the index, the SPA fallback and the policy headers of
      the real build in `hack/prove-binary.sh target/debug/keel`; `frontend/apps/shell/src/theme-boot.spec.ts`
      for the theme boot script the index loads in place of an inline script.
- [x] The server halves of the two shell screens: white-label by hostname through
      `ManifestSource` (`a_manifest_patch_for_the_request_host_changes_boot_and_the_effective_logo`
      and `a_product_logo_is_served_from_its_asset_and_the_effective_one_follows_the_brand_rule` in
      `crates/kernel/tests/kernel/brand.rs`;
      `a_brand_patch_for_a_host_changes_boot_and_the_effective_logo_for_that_host_alone` and
      `the_brand_descriptors_match_their_snapshots` in `crates/system/tests/system/settings_brand.rs`;
      the `/brand` routes, the patched `/boot` for `panel.example` and the refused scripted logo in
      `hack/prove-binary.sh`); stored grants through `PermissionSource`
      (`the_transports_overlay_the_stored_grants_and_the_guard_reads_them_over_the_defaults` in
      `crates/kernel/tests/kernel/grants.rs`;
      `a_stored_grant_overrides_the_default_and_a_denial_takes_a_default_key_away_until_deleted`,
      `grants_are_rows_of_the_principals_account_and_the_reseller_sees_its_subtree_only` and
      `the_grant_descriptors_and_the_auth_menu_nodes_match_their_snapshots` in
      `crates/system/tests/system/auth_grants.rs`); the grants screen, like every system screen,
      opened from the menu in "root opens every system screen from the menu" of
      `frontend/e2e/system.spec.ts`, which also sets `auth.password.min_length` through the shell's
      value form, creates a token whose plaintext is shown once and acts as `alice` with the
      banner; the security page in `frontend/libs/keel/shell/src/security.spec.ts` and the
      second-factor enrolment through the real screen in `frontend/e2e/login.spec.ts`.
- [x] The library and the contract tooling: `@globalart/keel/meta` agrees with `keel-types` on
      every case of the shared fixtures (`frontend/libs/keel/meta/src/expr.spec.ts`,
      `frontend/libs/keel/meta/src/check.spec.ts`, `frontend/libs/keel/meta/src/validate.spec.ts`,
      `frontend/libs/keel/meta/src/fixtures.spec.ts` against `contracts/schema/`, which
      `cargo run -p keel-dev -- types --out contracts/schema --check` keeps current);
      `@globalart/keel/client` in `frontend/libs/keel/client/src/client.spec.ts`,
      `frontend/libs/keel/client/src/list.spec.ts` and `frontend/libs/keel/client/src/auth.spec.ts`;
      the dump loader of `@globalart/keel/testing` in `frontend/libs/keel/testing/src/mock-client.spec.ts`
      ("fromDump answers the admin boot and every descriptor of the dump") over `contracts/dump/`,
      which `hack/descriptors.sh target/debug/keel` holds to the binary
      (`dump_logs_in_per_level_writes_the_projected_files_and_check_diffs_against_a_directory` and
      `descriptors_diff_over_two_directories_differing_in_one_file_prints_that_files_diff_and_identical_directories_exit_zero`
      in `crates/dev/src/descriptors.rs`) together with `contracts/openapi.json`
      (`the_committed_document_is_current_and_names_one_post_per_function_of_the_admin_dump` and
      `a_changed_parameter_schema_fails_the_check_without_rewriting_the_document` in
      `crates/dev/src/openapi.rs`); the icon set embedded from `contracts/icons.json`
      (`an_icon_outside_the_phoenix_set_in_a_descriptor_or_a_menu_node_fails_freeze` in
      `crates/kernel/src/loader/freeze.rs` and `the_embedded_icon_set_is_sorted_unique_and_names_plus`
      in `crates/kernel/src/loader/icons.rs`), regenerated by `pnpm generate` and held by
      `git diff --exit-code`.
- [x] The frontend job is green: `pnpm install --frozen-lockfile`, `pnpm generate`, `pnpm lint`,
      `pnpm format`, `pnpm test` (55 suites, 263 tests), `pnpm build`, `pnpm build-storybook`,
      `pnpm test-storybook`, `node tools/stamp.mjs` (the workspace version stamped, the six entry
      points and the nested `testing/playwright` exported) and
      `pnpm publish --dry-run --no-git-checks` against Nexus.
- [ ] `@globalart/keel@0.1.0` is published to Nexus by `publish-npm.yml` through
      `workflow_dispatch` from the closing commit, after which the `exports` of the published
      package, read from Nexus with pnpm's `view`, name the six entry points and the nested
      `testing/playwright` — operator-run; the dry run above is CI's rehearsal.
- [x] `cargo fmt --all --check` is green.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [x] `cargo test --workspace` is green against Postgres, `env -u KEEL_TEST_DATABASE_URL cargo test
      -p keel-testkit` against the container, `cargo test -p keel-bin --no-default-features`
      without the demo, and `hack/check-test-databases.sh` finds no database left behind.
- [x] `hack/check-deps.sh` is green: `keel-dev` depends on `keel-types` alone and `keel-spike` on
      no workspace crate, next to the M0 and M1 rules.
- [x] `hack/check-docs.sh` is green: every M2 name it carries is found under `docs/design/` and
      under `crates/` or `frontend/libs/keel`, every Rust test this checklist cites is defined,
      every `*.spec.ts` it cites exists under `frontend/`, every command it cites exists, and the
      README's check lists, the "How we work" section and `ci.yml` agree, `hack/descriptors.sh` and
      `hack/e2e.sh` included.
- [ ] On `keel-demo`, `keel --version` under `/opt/keel/bin/current` names the closing commit's
      version and `/readyz` answers `ready`, after the musl artifact of the closing commit's Build
      run was unpacked into `/opt/keel/versions/<version>` beside the M1 installation,
      `bin/current` switched and `keel.service` restarted — operator-run; to be ticked with the
      note "confirmed by hand on keel-demo".
- [ ] The owner has opened the panel of `keel-demo` through the SSH port forward at
      `http://localhost:1500/`, logged in as the administrator `deploy/install.sh` created — the
      password reset over `keel ctl auth.password.set` when no longer at hand — walked the notes
      list, the form, the wizard and the tag cloud, and switched to `ru` — operator-run; to be
      ticked with the note "confirmed by hand on keel-demo".
