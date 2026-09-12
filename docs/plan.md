# Plan

This document orders the work: the milestones M0 to M8 with their scope and exit criteria, a
breakdown of M0 into PR-sized steps, the working rules of the repository, and the M0 exit
checklist. The design itself lives in `design/`; this document only says in which order it is
built and how each step is shown to be done. Nothing here carries a date or a duration.

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
- Postgres through testcontainers in CI.
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
- `ui.result = task`; a batch above the threshold becomes a task.
- Self-signed TLS and `trusted_proxies`.
- phoenix-ui batch 2.

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
  OwnLate project, the release build and channel.
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
the frontend workspace, `keel-dev`, `keel-conformance`, testcontainers (M1 replaces the service
container with them).

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
8 and 9); `ConfigSection` (M1); the icon check against the `PhxIconName` list, which needs the
list generated from phoenix-ui and is enabled in M2.

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

**Out of scope.** `ManifestPatch` and white-label by hostname (`settings`, M1); dashboard
widgets, problems and license state (M3, M5); custom views in `views` (M2); OwnLate export at
build time and `keel-dev i18n missing` in CI (M2); the `PhxIconName` check (M2); the lease check
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

## How we work

- The code lives in the monorepo `GlobalArtInc/keel` on the `main` branch until the SDK reaches
  1.0; a product's crates may move to their own repository after M7 and are pulled into the
  `keel` build as dependencies ([design/01-overview.md](design/01-overview.md)).
- One task is one PR-sized change; a task is not split across branches.
- Verify with the same commands CI runs before pushing: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` against the
  Postgres server `KEEL_TEST_DATABASE_URL` names, `cargo test -p keel-bin --no-default-features`,
  `cargo build -p keel-bin` and `hack/prove-binary.sh target/debug/keel`, then
  `hack/check-test-databases.sh` and `hack/check-deps.sh`; the README lists them in CI's order.
  Once `frontend/` exists, `pnpm lint && pnpm test && pnpm build` join the list.
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
- [ ] The binary starts without `config.yaml` against the default database `keel`: a missing
      file yields the defaults (`a_missing_config_file_yields_the_platform_defaults`), and CI
      starts the binary against its own database through `--config`; the start against a host's
      `keel` database over the Postgres socket is checked by hand until the installer of M4
      runs it in CI.
- [x] `keel serve` with no bundle enabled starts and answers on the socket
      (`crates/keel/tests/runtime.rs` and the second run of `hack/prove-binary.sh`).
- [x] `keel product disable demo` leaves the `note_` table and flips `note` to `orphaned`
      (the second run of `hack/prove-binary.sh`, where `note.list` answers `not_found`).
- [x] `cargo fmt --all --check` is green.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [x] `cargo test --workspace` is green against Postgres, and `hack/check-test-databases.sh`
      finds no database left behind.
