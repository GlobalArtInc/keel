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
- `products/demo` with the `note` module.
- Five to six crates. No TLS, no metrics, no macros.

Done when:

- `demo ctl note.edit title=x` and `POST /api/v1/call/note.list` return the envelope;
- descriptors match the committed snapshot;
- the tests "a user neither sees nor edits another account's notes" and "a reseller sees its
  subtree" pass;
- a nested `invoke` reuses the transaction of the call;
- the binary starts without `config.yaml` against the default database;
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
  wizard, tabs), `/shell`, `/testing`; embedding through rust-embed; Storybook with fixtures;
  phoenix-ui batch 1; the OwnLate to `PHX_LABELS` bridge; publication to Nexus.
- In parallel, a thin agent spike in a separate `products/spike`: unix socket with
  `SO_PEERCRED`, `host.user.create`, `host.template.apply` with validate and rollback on nginx.

Done when:

- `demo` is driven entirely from the browser without product-specific TypeScript: a list with
  search, sorting, filters and bulk delete with confirmation; a form with pages, `show_if`,
  `recompute` and a field-level error; a wizard;
- one screen is replaced by a custom view through `provideKeelViews`;
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

- The operator plane in full: `install.sh`, directories, systemd units with hardening, a uid per
  product, `privileged` with the update policy (check, apply, rollback, a pre-update dump of the
  database, anti-rollback), `backup` and `restore` with `--with-secret-key`, `doctor`,
  `support-bundle`.
- `keel-host` in full: users, files, services, templates, packages, `AgentOp`, the `host`
  module.
- The feature guard without a lease.
- The product `dns`: `dns-zones`, `dns-records`, `dns-templates` on bind, account limits, an
  OwnLate project, the release build and channel.
- The first extraction rehearsal of a product from the facade.

Done when:

- on a clean Debian or Ubuntu, `curl | sh` installs `dns`, and no process other than
  `privileged` runs as root;
- a zone is created from the UI, from the CLI and with an API token, and bind serves it;
- a broken template is rolled back without downtime;
- the update 0.1 to 0.2 runs from a button and rolls back without losing the database;
- a backup taken on one VM restores on another with 2FA still working;
- an operation outside `file_roots` is refused by the agent and lands in the audit trail.

### M5 — KeelPanel and the commercial layer

Scope:

- `license`: Ed25519 lease, editions, trial, grace and read-only degradation, monotonic time.
- `stats`.
- Renderers `report`, `dashboard`, `tree`, `master-detail`, `stream`.
- phoenix-ui batch 3: code editor, log view, key-value field, transfer list.
- The product `panel`: `hosting-users`, `hosting-web` (nginx, php-fpm), `hosting-mail`,
  `hosting-db`, `hosting-ftp`, `hosting-ssl` (ACME), `hosting-backup`, `hosting-services`; the
  file manager and the configuration editor as custom views.

Done when:

- a site with a Let's Encrypt certificate is set up by the wizard and is served;
- there are exactly two custom views;
- with the license server unreachable the panel works until `expires`, then in grace, then
  read-only, verified on a shifted clock;
- a function with `feature: dns.dnssec` is hidden from the `lite` menu and answers `402`;
- the share of custom views is at most 10 % of functions;
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
- `panel` to `dns` through `dns-peer`.

Done when:

- two instances are paired by code;
- creating a site in `panel` creates a zone in `dns` on behalf of the client, and a DNS
  error arrives in the hosting form as a key on a field;
- the DNS panel opens from the hosting panel by a one-time key, with `actor` in the audit trail;
- `svc.dns.zones.open` called twice with the same key produces one service;
- an event arrives signed, and a repeat creates no duplicate;
- conformance is green against both products;
- revoking a token breaks its calls with `401`.

### M7 — Plugins, SDK 1.0, extraction of products

Scope:

- `plugins`: protocol 1, `daemon` and `exec` modes, the rights `observe`, `hook` and `patch`,
  the kv store, a separate uid, timeouts.
- A Python SDK and a sample plugin.
- Stabilisation of `keel-sdk` 1.0 and descriptor v1: CHANGELOG, semver policy.
- Extraction of `dns` and `panel` into their own repositories by tag.
- A load test of lists at 100k rows.
- Security review.
- Operator and developer documentation.

Done when:

- a bash plugin adds a field to the zone form, vetoes a deletion with a field-level error and
  shows its own list in the menu, all without a rebuild;
- a plugin hook on `auth.*` is rejected at start;
- a crash of a daemon plugin does not take the kernel down;
- the tags `keel-v1.0.0`, `dns-v1.0.0` and `panel-v1.0.0` exist and the products build
  against the published tag;
- a list of 100k zones answers in under 300 ms;
- the security checklist is closed;
- the first external operator installs `panel` from the documentation without the author's
  help;
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
directory skeleton for the fixtures of step 2.

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
with its HTTP status mapping; `ListQuery`, `Sort`, `Filter`, `Page<T>`; the event shape; the
condition AST (`eq`, `ne`, `in`, `gt`, `lt`, `truthy`, `empty`, `matches`, `and`, `or`, `not`;
variables are fields, `$mode`, `$principal.level`, `$screen.params.*`), its parser from the
mini-language and its interpreter; the `check` preset evaluator; the ULID identifier type. The
types derive `JsonSchema` so that M2 can generate TypeScript from them. Golden fixtures land in
`contracts/fixtures/`: `expr-cases.json`, check cases, the `dns.zone.list` descriptor from the
contracts document, envelope examples for `200`, `422` and `403`, a `ListQuery` and a `Page`.

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
`database`, `tls`, `trusted_proxies` and `log`; defaults for `listen`, `socket` and `data_dir`
come from the product manifest, and `database` defaults to
`postgres:///keel_<product>?host=/run/postgresql` for peer authentication under the product's
uid, with `${<PRODUCT>_DB_PASS}` substituted for a TCP URL. The connection goes through sea-orm
1.1 (sqlx-postgres, runtime-tokio-rustls): a pool of 20, connect and acquire timeouts of 10 s,
`search_path` set to the product's schema. The kernel's migrations are an embedded sqlmig 0.2
catalogue with the ledger `keel_migration`; they create the eight kernel tables and insert the
root account (`kind = system`, `path = /`), and they run under a session-level
`pg_advisory_lock`. On first start the kernel creates `data_dir`, generates `secret.key` (mode
0600) and `instance.id`, and writes the instance id and the wrapped key into `keel_kv`. Logging
is initialised from the `log` key (tracing, `json` or `text`).

**Touches.** `crates/kernel/` (config, storage, migrations), `crates/kernel/migrations/`.

**Out of scope.** Running module migrations (step 5), the pre-update dump, `schema prune`,
`ConfigSection` (M1), `tls` and `trusted_proxies` beyond parsing (M3), backups (M4).

**Acceptance.**

- On a fresh database the migration creates the eight tables and the ledger; a second run
  changes nothing, and the ledger row count is identical.
- The root account row exists with `kind = system` and `path = /` and is the only account.
- A missing `config.yaml` yields the defaults; `${DEMO_DB_PASS}` is substituted in a TCP URL; an
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
reserved point `r.invoker` are part of this step.

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

- The descriptors of `keel.account.*` and `keel.principal.*` match committed snapshots.
- `keel.account.list` as a reseller returns only its subtree; as a user, only the caller's
  account; as owner, every account including the root.
- Changing a principal's role or account increments its `acl_gen`; changing a reseller account
  increments `acl_gen` of every principal in its subtree.
- `resolve_account` on the `account` parameter: a reseller may name any account in its subtree
  and is refused outside it; a user may name only its own account.

### 8. HTTP transport

**Goal.** Serve the kernel over plain HTTP with axum, per
[design/10-contracts.md](design/10-contracts.md): `GET /healthz`; `GET /readyz` (database
reachable, migrations applied, not draining); `GET /api/v1/boot`; `GET /api/v1/functions` and
`GET /api/v1/functions/{name}`, projected for the principal, with
`ETag = hash(build, modules, edition, lang, principal_id, acl_gen)` and `private, no-store`;
`POST /api/v1/call/{func}`, with `GET` allowed for `kind: list | get`; `GET /api/v1/i18n` for
the catalogue in the principal's language. Every response carries `X-Keel-Request-Id`, calls
carry `X-Keel-Meta-Version`, replays carry `X-Keel-Idempotent-Replay`. Error codes map to HTTP
statuses as the contracts document lists; `body_limit` is enforced. The principal is resolved
through `keel_sdk::system::Authenticator`; with no authenticator registered every request is a
guest. Requests that would be authenticated by cookie are checked by `Sec-Fetch-Site` / `Origin`;
Bearer requests are not.

**Touches.** `crates/kernel/` (HTTP transport, routes, status mapping, headers).

**Out of scope.** TLS and `trusted_proxies` (M3); SSE (M3); raw routes `/api/v1/x/<module>/…`;
metrics; the embedded frontend (M2); cookies, CSRF enforcement and rate limiting (M1);
`GET /api/v1/instance` (M6).

**Acceptance.**

- `/healthz` answers `200` as soon as the process listens; `/readyz` answers `503` before
  migrations finish and `200` after.
- `/api/v1/functions` as a guest lists only guest-level functions.
- An unknown function name yields the `404 not_found` envelope.
- `POST /api/v1/call/keel.account.list` returns the envelope: as a guest, the
  `401 unauthenticated` error envelope with `ok: false`; with the test authenticator of step 11
  acting as a user, `200` with a `Page`. The same check against `note.list` is part of step 12.
- Every error code of the contract maps to the documented status in a table-driven test.
- Every response carries `X-Keel-Request-Id`; the `ETag` of a descriptor changes after the
  principal's `acl_gen` is bumped.

### 9. Control socket and ctl

**Goal.** Serve the same call protocol on the unix socket named by `socket` (the manifest
default is `/run/keel/<product>/core.sock`). The kernel reads `SO_PEERCRED`; the design fixes
only that the local socket is the owner level and that `ctl` is identified by peer credentials.
As a plan-level arrangement, M0 accepts only the kernel's own uid, which becomes the owner
principal of the root account; whether root or other uids are admitted is not fixed by the
design and stays open. The `ctl` subcommand is a client of that socket:
`<product> ctl <func> key=value … --out json|table`, returning the same envelope as HTTP;
`--out table` renders `ui.columns` for `kind: list`, translating keys through the catalogue of
step 10. The `serve` subcommand starts the kernel; `migrate` runs the migrations of step 5 and
exits. Running `migrate` only while the service is stopped is a plan-level arrangement for M0;
the migrations serialise on the advisory lock of step 4 either way. This step also adds the test
"a product of one bare kernel starts and answers on the socket".

**Touches.** `crates/kernel/` (socket transport, peer credentials, subcommands `serve`, `ctl`,
`migrate`).

**Out of scope.** `--as <login>`, which is delegation and belongs to `auth.su` in M1; the full
CLI parameter grammar (repeated keys, `a.b=v`, `@file.json` versus JSON only), an open question
in [design/11-decisions.md](design/11-decisions.md) — M0 implements the `key=value` form the
exit criteria use and leaves the rest open; the subcommands `privileged`, `backup`, `restore`,
`doctor` and `support-bundle` (M4).

**Acceptance.**

- `ctl keel.account.create name=x` returns the envelope with `ok: true` and the new id; the
  `note.edit` form of the exit criteria is checked in step 12.
- A connection from a uid other than the kernel's is refused (unit test on the credential check).
- For the same call the envelope over the socket and over HTTP differ only in `meta`.
- `--out table` for `keel.account.list` renders the columns declared in `ui.columns`.
- A product composed of the bare kernel with no modules starts and answers `keel.account.list`
  on the socket.

### 10. i18n catalogue, manifest, menu and /boot

**Goal.** The `ProductManifest` type in `keel-sdk`. The design does not fix a field list; it
mentions the product id, name, version, `api`, the brand, the level labels and disabled levels,
the editions, `license` and the menu groups, and it says the defaults for `listen`, `socket` and
`data_dir` come from the manifest. Any further field (languages, update channel, host settings,
account kinds) is proposed at M0 and not fixed by the design. The default listen ports stay open, per
[design/11-decisions.md](design/11-decisions.md); M0 picks provisional values. At M0 the kernel
consumes id, name, version, `api`, `listen`, `socket`, `data_dir`, brand, levels, menu groups
and languages. The i18n
catalogue merges the `r.i18n(lang, json)` dictionaries of all modules, applies the key
convention of [design/08-frontend.md](design/08-frontend.md) (`<module>.<entity>.<field>`,
`.hint`, `.placeholder`, `.<enum>`, `.action.<id>`, `.page.<id>`, `.column.<key>`, shared
`keel.*` with inheritance) and the single placeholder format `{name}`; freeze fails on a key
missing for `en`. Menu nodes registered with `r.menu` are placed under the manifest's groups and
projected by level and permission. `GET /api/v1/boot` returns `product`, `brand`, `principal`,
`features`, `menu`, `views`, `meta` and the fields fed by modules absent from the composition
(`license`, `dashboard`, `problems`) as null or empty. With `license: None` the license
mechanism is off; the feature guard arrives in M4 and is not exercised at M0.

**Touches.** `crates/sdk/` (`ProductManifest`), `crates/kernel/` (i18n, menu, boot,
meta-version).

**Out of scope.** `ManifestPatch` and white-label by hostname (`settings`, M1); dashboard
widgets, problems and license state (M3, M5); custom views in `views` (M2); OwnLate export at
build time and `keel-dev i18n missing` in CI (M2); the `PhxIconName` check (M2).

**Acceptance.**

- `/api/v1/boot` over the socket as owner returns the product block from the manifest, the brand,
  the owner principal with `account` set to the root account, the menu with the `keel` module's
  own node and a `meta` value; the same request as a guest returns no menu entry above the guest
  level. The demo's `note` node is checked in step 12.
- A function registered without an `en` title key fails start with the key named.
- `/api/v1/i18n` returns the merged catalogue for the requested language. Whether a key missing
  in `ru` falls back to `en` or is reported as missing is a plan-level expectation for M0 (fall
  back), not fixed by the design, which only requires every key to exist for `en`.
- `meta` changes when the composition changes and when the principal's `acl_gen` changes.

### 11. keel-testkit

**Goal.** The public test harness of [design/05-modules.md](design/05-modules.md):
`Harness::new().module(&M).boot()`; a temporary Postgres database per test, created with
`CREATE DATABASE` from a template and dropped afterwards, with the server taken from the
environment; `call_as(Level, "func", json!{})` in process; fixture principals for every level and
a fixture account tree (root, a reseller with a subtree, a user); a fixed clock; descriptor
snapshots per level; the two standard tests as reusable helpers, "a reseller sees only its
subtree" and "a user cannot edit a foreign row by id". For HTTP tests the harness registers a
test `Authenticator` that maps a request header to a fixture principal; this is a test-only
arrangement, not part of the design's authentication model.

**Touches.** `crates/testkit/`.

**Out of scope.** `FakeHost` (M2 spike, M4); testcontainers (M1); Playwright and the frontend
`/testing` entry point (M2).

**Acceptance.**

- A module test that depends only on `keel-testkit` and `keel-sdk` boots a harness with one
  module against Postgres in CI.
- Every test gets its own database, and no database remains after the test run.
- A changed descriptor makes the snapshot test fail with a diff.
- The two standard helpers pass against the `keel` module's own functions before the demo exists.

### 12. products/demo with the note module

**Goal.** The first product. The facade crate `keel` gains `Product::new(&MANIFEST)`, `.module()`,
`.run()` returning `keel::ExitCode`, and re-exports of `keel-sdk` and `keel-types`.
`products/demo` carries `MANIFEST` and a `main.rs` that composes the kernel with the `note`
module. The module `note` is a dogfood module without host access: one table with the `note_`
prefix and an account column, the functions `note.list` (`list`, read), `note.get` (`get`,
read), `note.edit` (`form`, write, `.owned`) and `note.delete` (`action`, write, `.owned`), the
permissions `note.read` and `note.manage`, a menu node, dictionaries for `en` and `ru`, and one
`before` and one `within` hook so that both hook kinds are exercised by a real module.
`note.edit` invokes `keel.account.get` through `call.invoke` to exercise nested invocation.

**Touches.** `crates/keel/`, `products/demo/`.

**Out of scope.** `embed_frontend!` and any frontend (M2); `note.export` (M3); a `products/spike`
(M2); `dns` and `panel`.

**Acceptance.**

- `products/demo` depends only on the facade `keel`; `hack/check-deps.sh` still passes.
- `demo serve` starts without `config.yaml` against the default database and answers on the
  socket and on HTTP.
- `demo ctl note.edit title=x` and `POST /api/v1/call/note.list` return the envelope.
- The `note.*` descriptors match the committed snapshot.
- The standard tests pass against `note`: a user neither sees nor edits another account's notes;
  a reseller sees the notes of its subtree.
- The test "a nested `invoke` reuses the transaction of the call" passes on `note.edit`.
- The i18n check passes for `en` and `ru`, and `note.list --out table` shows translated headers.

### 13. hack/prove-binary.sh

**Goal.** A script CI runs against the built `demo` binary: start `serve` with a temporary
`data_dir` and a fresh database, wait for `/readyz`, call `ctl keel.account.create`, `/healthz`,
`/api/v1/boot`, `ctl note.edit title=x` and `ctl note.list --out json`, then
`POST /api/v1/call/note.list` over HTTP, where no authenticator is registered, asserting the
`401 unauthenticated` error envelope with `ok: false`; stop the process and exit non-zero on
any failure. CI runs it after the build.

**Touches.** `hack/prove-binary.sh`, `.github/workflows/ci.yml`.

**Out of scope.** The musl target and the `ldd` check, `hack/vm-test.sh`, `hack/release.py`,
`keel-conformance`.

**Acceptance.**

- The CI job runs the script and is green.
- The script fails when any of its checks fails, and it stops the process and removes its
  temporary `data_dir` on both success and failure.

## How we work

- The code lives in the monorepo `GlobalArtInc/keel` on the `main` branch until the SDK reaches
  1.0; products are extracted by tag in M7 ([design/01-overview.md](design/01-overview.md)).
- One task is one PR-sized change; a task is not split across branches.
- Verify with the same commands CI runs before pushing: `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` against a
  Postgres service. Once `frontend/` exists, `pnpm lint && pnpm test && pnpm build` join the
  list.
- English everywhere in the repository: code, documentation, tests, CI, commit messages.
- No comments in code. Explanations go into the commit message or into `docs/`; the only
  exception is a link to an RFC next to the code that implements it.
- The project's own vocabulary only: `kind`, `role`, `control`, `level` and every other name is
  ours. No other control-panel product is named in code, documentation or commit messages, and
  none of its identifiers is borrowed; a name audit runs before the first release.
- Postgres is the only database; tests get a database per test from a template, and there is no
  in-memory substitute.

## M0 exit checklist

- [ ] `demo ctl note.edit title=x` returns the envelope.
- [ ] `POST /api/v1/call/note.list` returns the envelope.
- [ ] Descriptors match the committed snapshot.
- [ ] The test "a user neither sees nor edits another account's notes" passes.
- [ ] The test "a reseller sees its subtree" passes.
- [ ] A nested `invoke` reuses the transaction of the call.
- [ ] The binary starts without `config.yaml` against the default database.
- [ ] `cargo fmt --check` is green.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [ ] `cargo test --workspace` is green against Postgres.
