# Plan

This document orders the work: the milestones M0 to M8 with their scope and exit criteria, the
breakdowns of M0 and M1 into PR-sized steps, the working rules of the repository, and the M0 and
M1 exit checklists. The design itself lives in `design/`; this document only says in which order
it is built and how each step is shown to be done. Nothing here carries a date or a duration.

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
  `ManifestPatch` in `settings`, served in `/boot.brand` next to the `/brand/logo.svg` route the
  embedded frontend brings; stored permission grants in `auth` through `PermissionSource`,
  behind the shell's permissions screen.

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

**Out of scope.** `ManifestPatch` and white-label by hostname (`settings`, M2); dashboard
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
cookie), `crates/kernel/tests/http.rs`, `crates/testkit/src/auth.rs` and `http.rs` (contribution
instead of service, connect info), `docs/design/10-contracts.md` and `docs/design/08-frontend.md`
(the cross-site rule), `docs/design/05-modules.md` (the hidden hooks).

**Out of scope.** Every table, function and dictionary of `auth` (steps 1b and 1c);
`trusted_proxies` (M3); `kpt_` (M6); `PermissionSource` and stored grants, which the M2 scope owns
next to the shell's permissions screen, since the design makes the registered defaults the answer
while no source is composed and M1 composes none.

**Acceptance.**

- In `crates/kernel/tests/http.rs`, a stub authenticator contributed by a test module answers a
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
- In `crates/system/tests/auth_passwords.rs`, driven through `X-Keel-Test-Principal` and the socket:
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
- Over HTTP, in `crates/system/tests/auth_sessions.rs`: after an admin sets A1's password, `POST
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
  `note.get`, so the scope guard admits nested invokes: `products/demo/tests/tokens.rs`.
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
  `crates/system/tests/auth_su.rs`, the M1 exit sequence over HTTP: login as the admin, `boot`,
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

## How we work

- The code lives in the monorepo `GlobalArtInc/keel` on the `main` branch until the SDK reaches
  1.0; a product's crates may move to their own repository after M7 and are pulled into the
  `keel` build as dependencies ([design/01-overview.md](design/01-overview.md)).
- One task is one PR-sized change; a task is not split across branches.
- Verify with the same commands CI runs before pushing: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` against the
  Postgres server `KEEL_TEST_DATABASE_URL` names, `env -u KEEL_TEST_DATABASE_URL cargo test -p
  keel-testkit` against the container the testkit starts, `cargo test -p keel-bin
  --no-default-features`, `cargo build -p keel-bin`, `hack/prove-binary.sh target/debug/keel`,
  `hack/check-install.sh target/debug/keel` and `hack/conformance.sh target/debug/keel`, then
  `hack/check-test-databases.sh`, `hack/check-deps.sh` and `hack/check-docs.sh`; the README
  lists them in CI's order. Once
  `frontend/` exists, `pnpm lint && pnpm test && pnpm build` join the list.
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

## M1 exit checklist

Every item below is checked by CI on `main`: the tests of the workspace, the `keel-system`
matrix, the testkit's own tests against the container it starts, the build of `keel-bin` with and
without the `demo` feature, `hack/prove-binary.sh` and `hack/conformance.sh` against the built
binary, and `hack/check-docs.sh` over this checklist.

- [x] Over HTTP: login, then boot, then `keel.account.list`, then `auth.su`, after which the
      list is narrowed and `actor` is filled, then `su.exit`
      (`login_boot_list_su_narrows_the_list_and_fills_the_actor_then_su_exit_returns` in
      `crates/system/tests/auth_su.rs`; the auth group of `hack/conformance.sh`; the `su`
      sequence of `hack/prove-binary.sh`).
- [x] A token scoped to one function receives `403` on another
      (`a_token_scoped_to_one_function_is_admitted_there_and_refused_elsewhere_after_the_level_guard`
      in `crates/system/tests/auth_tokens.rs`;
      `a_token_scoped_to_note_edit_alone_edits_a_note_through_its_nested_invokes` in
      `products/demo/tests/tokens.rs` for the nested invokes; the token group of
      `hack/conformance.sh`; `note.edit` under a token in `hack/prove-binary.sh`).
- [x] Writing a field above the caller's level yields `forbidden_field`
      (`login_sets_the_cookie_and_the_session_drives_boot_lists_and_forbidden_field` in
      `crates/system/tests/auth_sessions.rs`, over a real session; the errors group of
      `hack/conformance.sh`).
- [x] The audit trail contains both sides of `su` and masks the password, including for
      failed calls (`the_su_sequence_records_both_sides_of_su_and_the_calls_made_inside_it` and
      `logins_and_password_changes_are_masked_and_no_session_token_key_or_factor_reaches_the_trail`
      in `crates/system/tests/audit.rs`;
      `a_wrong_password_and_an_unknown_login_are_one_answer_and_the_audit_masks_the_password` in
      `crates/system/tests/auth_sessions.rs` for the event of the failed call; the `audit_call`
      queries of `hack/prove-binary.sh`).
- [x] The conformance skeleton is green against the built binary (`hack/conformance.sh
      target/debug/keel` in CI after `hack/prove-binary.sh`; a broken expectation exits non-zero:
      `a_broken_expectation_makes_the_suite_exit_non_zero_with_one_line_per_check` in
      `crates/conformance/tests/exit_code.rs`).
- [x] `auth`: argon2id passwords
      (`the_ledger_and_the_tables_exist_and_a_set_password_is_stored_as_argon2id` in
      `crates/system/tests/auth_passwords.rs`); opaque sessions and the `__Host-` cookie with the
      cross-site check (`login_is_refused_cross_site_and_replaces_the_session_it_arrived_with` and
      `a_session_slides_with_every_request_and_ends_after_the_idle_and_absolute_lifetimes` in
      `crates/system/tests/auth_sessions.rs`;
      `every_request_without_a_bearer_token_is_checked_for_cross_site` and
      `authenticators_are_asked_in_module_order_until_one_answers_or_fails` in
      `crates/kernel/tests/http.rs`); Bearer tokens with scope and IP restrictions
      (`ip_rules_revocation_expiry_and_last_used_govern_a_token_and_the_plaintext_is_shown_once`
      in `crates/system/tests/auth_tokens.rs`); TOTP with recovery codes
      (`a1_enrols_confirms_and_logs_in_through_a_challenge_with_the_step_moving_forward_only` and
      `a_recovery_code_logs_in_once_without_moving_the_step_and_is_stored_as_its_hash` in
      `crates/system/tests/auth_totp.rs`, the sealed secret through
      `a_handler_seals_through_the_cipher_and_only_the_composition_with_the_same_key_opens_it` in
      `crates/kernel/tests/cipher.rs`); login keys
      (`a_key_issued_for_a1_is_redeemed_once_and_refused_when_spent_expired_or_made_up` in
      `crates/system/tests/auth_keys.rs`); the socket side of delegation
      (`a_frame_with_as_runs_as_the_named_principal_with_the_owner_as_actor_via_socket` in
      `crates/kernel/tests/socket.rs`); IP rules
      (`ip_rules_refuse_by_address_and_level_take_effect_at_once_and_never_apply_to_the_socket`
      in `crates/system/tests/auth_guards.rs`, and the rule surviving a restart in the second run
      of `hack/prove-binary.sh`); rate limiting
      (`the_thirty_first_guest_call_from_one_address_within_a_minute_is_rate_limited_and_the_bucket_leaves_with_its_window`);
      lockout and unlock
      (`five_wrong_passwords_lock_the_login_from_that_address_only_until_the_window_passes_or_the_admin_unlocks`
      and `five_wrong_codes_lock_the_challenge_pair_for_the_address_and_the_code_passes_from_another`);
      the session list and revocation
      (`each_principal_lists_its_own_sessions_the_reseller_its_subtree_the_admin_all_and_the_current_row_is_marked`
      and `revoking_a_parent_takes_its_su_child_and_revoking_the_child_alone_ends_the_delegation`
      in `crates/system/tests/auth_session_list.rs`); the password policy
      (`the_policy_refuses_a_short_password_and_one_equal_to_the_login_wherever_it_is_set` in
      `crates/system/tests/auth_policy.rs`).
- [x] `settings`: values, inheritance and `Call::setting`
      (`the_admin_lists_the_six_auth_definitions_and_a_raised_minimum_makes_the_password_refuse_twelve_characters`
      and
      `a_value_inherits_from_the_root_down_the_account_path_and_a_composition_without_settings_resolves_the_default`
      in `crates/system/tests/settings.rs`;
      `without_a_settings_module_the_platform_answers_the_contributed_defaults_and_a_provided_source_replaces_it`
      in `crates/kernel/tests/settings.rs`); preferences and the caller's time zone
      (`a1_sets_tz_and_lang_and_boot_and_the_catalogue_follow_while_a2_reads_nothing_of_it` in
      `crates/system/tests/settings_pref.rs`;
      `the_transport_overlays_the_preferred_language_and_hands_the_zone_to_every_call_of_the_request`
      in `crates/kernel/tests/preferences.rs`;
      `a_bare_date_filter_takes_the_day_boundary_in_the_callers_zone_and_utc_without_one` in
      `products/demo/tests/day_boundary.rs`).
- [x] `audit`: the trail from a background worker, flushed and drained
      (`flush_returns_after_the_batch_is_written_and_a_stop_leaves_no_event_unwritten` and
      `audit_list_filters_by_function_principal_status_request_and_time_within_the_callers_scope`
      in `crates/system/tests/audit.rs`;
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
      `crates/system/tests/settings_alone.rs`,
      `audit_alone_boots_with_its_table_and_one_owner_write_leaves_one_row_after_a_flush` in
      `crates/system/tests/audit_alone.rs`).
- [x] `cargo fmt --all --check` is green.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [x] `cargo test --workspace` is green against Postgres, and `hack/check-test-databases.sh`
      finds no database left behind.
- [x] `hack/check-deps.sh` is green: `keel-system` depends on `keel-sdk` alone and
      `keel-conformance` on `keel-types` alone.
- [x] `hack/check-docs.sh` is green: every M1 name it carries is found under both `docs/design/`
      and `crates/`, every test name this checklist cites is defined, every command it cites
      exists, and the README's check list, the "How we work" section and `ci.yml` agree.
