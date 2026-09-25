# Plan

This document orders the work: the milestones M0 to M8 with their scope and exit criteria, the
breakdowns of M0, M1, M2 and M3 into PR-sized steps, the working rules of the repository, and the
M0, M1, M2 and M3 exit checklists. The design itself lives in `design/`; this document only says in
which order it is built and how each step is shown to be done. Nothing here carries a date or a
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
- `keel.account.delete` as a task walking the `AccountLifecycle` contributions with a checkpoint
  per module, and `keel.account.suspend` and `resume`, which the M0 and M1 breakdowns pointed at
  M3.
- Self-signed TLS and `trusted_proxies`.
- phoenix-ui batch 2.
- The OwnLate projects for the core and `demo`, `keel-dev i18n pull` at build time and
  `keel-dev i18n missing` in CI, deferred from M2.
- File uploads: the `upload` kind over a guarded raw route `/api/v1/x/<module>/…` with size and
  content-type limits and an audit event, `control: file` rendered by `phx-file-field`, and the
  brand logo of `settings.brand` becoming an upload of an SVG that the server sanitises, instead
  of the text field of M2. Owner feedback on the M2 panel, same screen: the menu node and the
  screen title of `settings.brand` say white-label by host name, the number fields of `hue` and
  `chroma` say "from the product" when empty rather than "Unlimited", and the demo logo takes
  the palette of the platform.
- Not in M3, against the `auth` sentence of the design that placed it here: password reset
  through `notify`, which needs an address per principal that no table of the design carries and
  a delivery to one recipient that the channels do not make, moves to M5 with `panel`, the first
  product whose users reach the panel without an operator at hand; the M3 breakdown records the
  move here so the owner confirms it with the breakdown, and step 15 of the breakdown amends the
  design sentence and the M5 scope list.

Done when:

- `note.export` runs as a task; progress and log are visible in the browser and in
  `ctl task.get`;
- cancellation works;
- a task with a checkpoint survives `systemctl restart`;
- a cron job raises a problem, the problem is visible on the dashboard and reaches an SMTP
  catcher as an email in the end-to-end test;
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
- The two items M3 left undone, recorded in its exit checklist: `keel.account.delete` as a task
  walking the `AccountLifecycle` contributions with `keel.account.suspend` and `resume`, which
  `dns` is the first to need for its `on_delete`; and the per-session cap of the event stream
  with `SSE_MAX_CLIENTS`, the credential re-check at keep-alive and the `events` group of the
  conformance suite.

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
- Renderers `report`, `dashboard` in full (`stat` widgets and the widget order in preferences,
  the `list` slice having arrived with M3), `tree`, `master-detail`, `stream`.
- phoenix-ui batch 3: code editor, log view, key-value field, transfer list.
- Password reset through `notify`, moved here from M3 together with an address per principal
  that no table of the design carries yet and a delivery to one recipient the channels of M3 do
  not make; `panel` is the first product whose users reach the panel without an operator at hand.
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
`keel_host::api`); `LimitDef` and `check_limit` (M4); `AccountLifecycle` (planned for M3, left
undone there, see the M3 exit checklist); the contribution
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
`ConfigSection` (M1), `tls` and `trusted_proxies` beyond parsing (M3, done by step 9), backups (M4).

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
threshold turning a call into a task, `keel.export` and rate limiting (M3, done by steps 2a, 7b
and M1 step 5a).

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
`AccountLifecycle`, and `suspend` / `resume` (planned as M3 step 2b, left undone there, see the M3
exit checklist); `keel.setup.admin`
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
otherwise the platform brand `keel.brand`, hue 264, chroma 0.18, no logo), `principal` (`null`
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
widgets, problems and license state (M3 step 4a, M5); custom views in `views` (M2); OwnLate export
at build time and `keel-dev i18n missing` in CI (deferred from M2 to M3, done by M3 step 10); the
icon check against
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
- The installation listens on plain HTTP until TLS arrives in M3 (done by M3 step 9b), while the
  session cookie is `__Host-keel_session` and carries `Secure` from its first version, as the
  design fixes. The clients of M1 — the testkit, `hack/prove-binary.sh`, `keel-conformance` — keep
  and return the cookie themselves; a browser is not a client before M2.
- There is no `trusted_proxies` before M3 (done by M3 step 9c), so the client address of a request
  is the peer address of its TCP connection, which the HTTP transport starts recording in step 1a.
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
beyond guest calls and `sse_max_clients` (M3 step 6b, left undone there); geo rules; persistence
of counters (no milestone fixes it).

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
`DashboardWidget` contributions and the `dashboard` block of `/boot` (the contributions in M3 step
4a, the order with the dashboard renderer of M5); the
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
`Secure` cookie over the plain HTTP the binary listened on until M3 step 9b. It takes `--url`, the
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
  in M3 (done by M3 step 9b, which retired the localhost rule of this remark), while the session
  cookie is `__Host-keel_session` with `Secure`. A browser accepts such a
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
kernel own that check. Retries are the design's rule: a call that carries an idempotency key is
retried once on a network failure without a response, a call without one never. `KeelBoot.load()`
fetches `GET /api/v1/boot` and keeps the payload as signals — `principal`, `products`, `brand`,
`menu`, `views`, `features`, `meta` — sending `If-None-Match` only for a refetch inside one page
life; `KeelDescriptors.get(func)` fetches `GET /api/v1/functions/{func}` and `list()` the index,
cached in memory under the meta version; `KeelI18n.load(lang)` fetches `GET /api/v1/i18n?lang=` and
exposes `t(key, params)` through the helpers of `/meta`, the catalogue's `ETag` kept for the same
refetch rule. `KeelAuth` wraps the login functions of `crates/system/src/auth` by their real shapes:
`login({ login, password })` answers `{ state: "authenticated", principal }` or `{ state: "totp",
challenge }`, `verifyTotp({ challenge, code })`, `redeemKey({ key })` answering `{ redirect,
principal, issuedBy }`, `logout()`, `su({ principal })` and `suExit()`, each followed by a boot
reload. The list adapter is `listFetch(func, scope)` answering a `PhxDataTableFetch<Row>`: it maps
`PhxDataTableQuery { page, pageSize, search, sort, filters }` onto `ListQuery { page, pageSize,
search, sort: { key, dir }, filters: [{ key, value }], scope }`, drops the table's `offset` and
`limit`, which are derived, calls the function and maps `Page { data, total, totals }` onto `{ data,
total }`, keeping `totals` on a companion signal for the footer; it posts, since `GET` is permitted
for lists and not required. `KeelDates` converts the wire's RFC 3339 through the principal's `tz`
from `/boot` with `Intl.DateTimeFormat` and produces the bare `YYYY-MM-DD` of a date filter in that
zone, the client side of the day-boundary rule of M1. In-flight calls are counted into
`PhxGlobalProgressService`. The event stream is a stub, since the kernel serves none before M3 (the
real subscription is M3 step 12): `KeelEvents.subscribe()` opens an `EventSource` on the path the
token `KEEL_EVENTS_PATH` names, `/api/v1/events` as the plan's provisional value for M3 to confirm,
with credentials, dispatches frames by type into `taskProgress$`, `notification$`, `dataInvalidate$`
and `metaInvalidate$`, reconnects with a backoff doubling from one second to thirty and relies on
the browser to resend `Last-Event-ID`; against the M1 kernel the route answers `404`, the stub stays
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
threshold in M3, where the scope line names them, done by M3 steps 7b and 14a, and step 12 records
the deferral against the `/client` row of the design); a real event stream, the progress modal and
the notification centre (M3); the login route and what the shell does on `unauthenticated` (step
8b); a browser session on a `Bearer` token; the dump loader of the mock client and the principal
fixtures (step 11d).

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
`columnOrder`; M3 step 11, shipped as `0.3.1`) and batch 3 (M5); any Keel logic in phoenix-ui — the
link between selection and bulk actions, remote options from a descriptor and validation stay in
`@globalart/keel`.

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
with `batch_threshold` in M3, where the scope line names it (done by M3 steps 8a and 14b); the
wizard container and navigation (step 7e); the object screen (step 7c); a widget of a product.

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

**Out of scope.** The export button and `keel.export` (M3, as step 4 says; done by M3 step 14a);
column order and the cell editor in `settings_pref` (M3, with `columnOrder` of batch 2; done by M3
step 14a); the `report`, `dashboard`,
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

**Out of scope.** The layout, the sidebar rule, the rail and the user menu (step 8c); the settings
value form and the security page (step 8d); the notification centre, tasks, problems and badges (M3,
done by step 13); the `/brand/logo.svg` route and white-label (step 9b); `keel-dev i18n pull` and
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
`phx-rail` from `menu`: with one product enabled the sidebar shows that product's groups followed by
the platform group `system`; with several, a `phx-sidebar-group` heading per product in the order of
`/boot.products`, each with its groups and the platform group last, a heading without a visible item
omitted; with none, the platform brand and the platform group alone. Items are `phx-sidebar-item`
with the icon, `t(label)` and the route `/f/<func>`; a `badge.problem` stays hidden until `problems`
exists. The level label follows the design's rule: `product.<id>.level.<level>` while one product is
enabled, `keel.level.<level>` otherwise. A `phx-banner` in the page area above the screen names the
actor's login while `principal.actor` is set and offers `auth.su.exit` — `phx-topbar` is the phone
chrome `phx-layout` renders itself and has no slot the shell could fill; the rail carries the
disabled placeholders for notifications, tasks and help until M3 (alive since M3 step 13) and the
user menu on `phx-user-menu`: the login and level, the language switch between the languages the
catalogue serves, `en` and `ru`, writing `settings.pref.set { key: "lang", value }` and reloading
the catalogue in place; the theme switch, `light`, `dark` and `system`, writing `settings.pref.set {
key: "theme" }` and `PhxThemeService`; "Act as", a dialog searching `keel.principal.list` by login
and posting `auth.su { principal }`, shown while `auth.su` is in the descriptor index; the links to
Sessions, Tokens and Security; and Log out. The system screens are the descriptors that exist,
rendered by steps 7c and 7d through the menu: accounts (`keel.account.*`), principals
(`keel.principal.*`), products (`keel.product.list` with its confirmed row actions `enable` and
`disable`), settings (`settings.list`), audit (`audit.list`, `audit.get` as the object screen),
sessions, tokens, IP rules and grants (the `auth` nodes of step 8a), and notes (`note.*` under the
demo group). A language switch reloads the catalogue in place, which the live labels bridge of step
8b follows without being re-provided.

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

**Out of scope.** `keel-dev i18n` (M3 step 10, step 8b's note); a dump at the `owner` level, which
no HTTP client reaches; `keel-dev check`, `new-module`, `new-product`, `migrate create` and `docs`
(the milestones that need them).

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

## M3 work breakdown

M3 is split into thirty-three PR-sized steps in fifteen groups, ordered by dependency; a step is one
change to `main` and is done when its acceptance holds in CI. The letters of a group (`1a`, `1b`)
name the steps of one delivery that land separately, as the M1 and M2 breakdowns named them; ten
groups are split that way, each along the line between a kernel seam and the module that consumes
it, between a mechanism and the functions or checks that expose it, or between two mechanisms of one
design section — the tasks module behind its kernel seams and before its functions (1a, 1b, 1c), the
task entrance beside the kernel's first task (2a, 2b), the notify module into its kernel seams, its
in-app half, the outbox seam, its channels and the mail catcher (4a to 4e), the webhooks module
behind the subject rule of `emit` and before its conformance check (5a, 5b, 5c), the event stream
into its route and its caps (6a, 6b), the download kind beside `keel.export` (7a, 7b), group editing
beside the upload kind and its first upload (8a, 8b, 8c), TLS into its material, its switch and
`trusted_proxies` (9a, 9b, 9c), the browser chrome of tasks beside that of notifications (13a, 13b),
and the list's export beside the multi-row form and the file control (14a, 14b, 14c) — each side
being a change to one crate or one mechanism, and no other step is above PR size once the server
half and the browser half of a mechanism are steps of their own. The steps derive strictly from the
design; where a step names a mechanism the design leaves open, it says so, and the commit message of
the step records the choice. As in M2, steps 1 to 14 touch nothing under `docs/`: where a step
diverges from a sentence of the design or fills a gap the design leaves, its commit message names
the sentence and the choice, and step 15 applies every owed amendment together with the exit
checklist, `hack/check-docs.sh` and the consistency pass, so the design changes once, against the
built M3. Steps 9a, 9c, 10 and 11 depend on nothing before them and may land at any point before
step 12; step 9b lands after 4d and 5b, whose provider lines it removes, and a module step landing
after 9b does not add the line.

Eight remarks apply to the whole breakdown.

- The four system modules of M3 join `crates/system` as the cargo features `tasks`, `cron`, `notify`
  and `webhooks`, whose module ids are `task`, `cron`, `notify` and `webhook`, as the table of
  [design/05-modules.md](design/05-modules.md) fixes. Each exports `pub static MODULE` from
  `keel_system::<feature>`, embeds its migrations from `crates/system/migrations/<feature>/` under
  the ledger `keel_migration_<id>` the loader demands, ships `crates/system/i18n/<feature>/en.json`
  and `ru.json`, and is tested under `crates/system/tests/system/` behind `cfg(feature)` as the M1
  modules are. The features nest where the modules do: `notify` needs `cron` for its poll job and
  turns that feature on, `webhooks` needs `notify` for its dead letter; `tasks` and `cron` stand
  alone. The default feature set and `bin/keel/src/main.rs` compose all four, the testkit boots the
  ones a test names, and the `keel-system` matrix of `ci.yml` grows with each module by the rule
  step 1c states.
- What a module needs from the kernel beyond the public SDK goes into the hidden `keel_sdk::system`,
  as in M1: `TaskRunner`, `Caller`, `LiveEvents`, `AdvisoryLock` and `ContextExt::data_dir` (step
  1a), `ProblemCounter` and two lookups on `PrincipalSource` (step 4a), `OutboxSubscriber` (step
  4c), the name the hidden-hooks list of [design/05-modules.md](design/05-modules.md) reserves for
  the outbox seam. The public SDK grows by additions alone — `Call::task()` with `TaskHandle` and
  its `stored`, `Call::spawn_self_as_task`, `Call::emit_about`, `Download` and `DownloadOutcome`,
  `DashboardWidget`, `AccountLifecycle`, the `resumable`, `class` and `internal` members of the
  function builder, `keel_sdk::multipart` — and `cargo-semver-checks` on the facade stays green
  throughout.
- The outbox is written by the dispatcher alone, inside the transaction of a call, as the "Emit to
  outbox" stage of [design/02-functions.md](design/02-functions.md) says; `outbox::write` stays
  private to the dispatcher. A module that needs a row and its event outside a request — the problem
  registry of step 4b, fed by a cron job — runs an internal function of its own through the hidden
  `Caller`, so the dispatcher writes both. Three consumers read the outbox in M3, each with a cursor
  of its own: the channel delivery of `notify` (`notify_cursor`), the deliveries of `webhooks`
  (`webhook_cursor`) and the browser stream (in memory per connection, resumed by `Last-Event-ID`).
  They read through the kernel's hidden `OutboxSubscriber`, never by SQL on `keel_outbox`, and `seq`
  is commit-ordered from step 4c on, so a cursor that passed a `seq` has seen every row below it.
  Retention of the outbox stays out of M3: nothing discards a row, so no consumer can be left
  behind, and the retention that consults the cursors arrives with the operator plane of M4.
- The raw routes of M3 are the kernel's own projection: a `kind: download` or `kind: upload`
  function registered with `r.function` is served at `/api/v1/x/<module>/<name>`, `<name>` being the
  function name without its module prefix and the dot, inside the `api` router behind
  `auth::authenticate`, through the full pipeline and with an audit event whatever its mode. A
  free-form raw route a module registers itself, for the terminal and VNC of `panel`, waits for that
  consumer in M5; the design sentence that names raw routes as a module's registration is amended by
  step 15 to say so.
- TLS arrives with steps 9a and 9b. Steps 1a to 8c keep the plain listener of M2 and its clients;
  step 9a lands the certificate material, `Http::bind_tls` and clients that can speak https while
  the runtime still binds plain, and from step 9b the installation listens on TLS alone, every
  script and suite of `hack/` speaks https with the generated certificate, and the `__Host-` cookie
  of M1 is accepted from any origin, which retires the localhost rule the M1 and M2 breakdowns
  recorded.
- The frontend keeps its six entry points. `@globalart/phoenix-ui` stays at `0.2.2` until step 11
  ships batch 2 as `0.3.0`, then `0.3.1` (the time picker commits once), which step 14a pins as its
  first commit, being its first consumer; steps
  12, 13a and 13b build on `0.2.2`, whose `phx-progress-modal`, `phx-drawer` and
  `phx-notification-center` already exist. The three disabled rail placeholders of M2 come alive in
  step 13, as the rail sentence of the design says of M3: tasks in step 13a, notifications and help
  in step 13b — help as an about drawer over `/boot`, the smallest target the sentence admits before
  the support screen of `panel` in M5, the plan's choice.
- Every screen of the new modules is a descriptor rendered generically: `task.list` and `task.get`,
  `cron.job.list` and `cron.run.list`, `notify.problem.list`, `notify.channel.*`,
  `webhook.subscription.*` and `webhook.delivery.list`. The internal functions of step 4b are the
  one exception: they have no screen, appear in no menu, dump or OpenAPI document, and answer `403
  keel.error.internal_only` to every transport. The shell adds chrome, not views: the progress
  modal, the task drawer, the notification centre and the about drawer hang off the action runner
  and the rail, and the home screen at `/` renders the widgets `/boot.dashboard` names — the
  `list`-widget slice of the M5 dashboard renderer the exit criteria pull forward (step 13b), a
  route without a function, so `frontend/apps/shell/views.json` and the custom-views share, which
  counts views registered against functions, do not move.
- The `demo` bundle gains what the exit criteria need and nothing else, in `products/demo`:
  `note.export` as the first task (step 2a) with its stored file (step 7b), a `batch_threshold` on
  `note.delete` and `note.edit` and `ui.multi` on `note.edit` (steps 2a and 8a), an
  `AccountLifecycle` whose `on_delete` drops the notes of a deleted account (step 2b), a
  `ProblemSource` with the `badge.problem` on its menu node (step 4b), `emit_about` on `note.edit`
  (step 5a), and the export button, the group edit and the logo in the platform palette driven from
  the browser (steps 14a to 14c). Where the demo consumes the `api` of a system module it names that
  module in `requires` and depends on `keel-system` with that feature alone, the rule of
  [design/05-modules.md](design/05-modules.md) the `dns` example applies to `keel_host::api`;
  nothing else of the demo changes.

### 1a. tasks: the seams — TaskRunner, Caller, LiveEvents, AdvisoryLock and the testkit's restart

**Goal.** What the `tasks` module of the "tasks and cron" section of
[design/05-modules.md](design/05-modules.md) needs from the kernel, the SDK and the testkit, landed
first so that the module of step 1b is a change to `crates/system` alone, as the M1 breakdown opened
`auth` with the kernel seams. A job is a function call kept for later, so the seams are about
running a call outside a request and reporting from inside one. The hidden
`keel_sdk::system::TaskRunner` — `spawn(TaskSpec { function, params, principal, class, resumable,
request_id }) -> Id`, `progress(id, done, total, message)`, `log(id, line)`, `checkpoint(id,
state)`, `cancel(id)`, `state(id)` — is the trait the module provides and the dispatcher,
`keel.export` and the batch rule consume. The hidden `keel_sdk::system::Caller { call(principal,
func, params, options) }` runs a function under a handed principal through the dispatcher with `via:
internal`, the `Via` value the principal's actor carries; the kernel provides it as a handle the
runtime fills once the dispatcher exists, since the dispatcher is built after `Kernel::boot` and a
start hook must not depend on it, and a call made before it is filled answers `unavailable` with
`keel.error.caller_unavailable`. The dispatcher's `CallOptions` gain `task: Option<TaskHandle>`, and
the public `Call::task()` answers that handle to a handler running inside a job — `progress(done,
total, message)`, `log(line)`, `checkpoint(&Serialize)`, `resume_from::<T>()` with the checkpoint of
an interrupted run, `is_cancelled()`, `output()` as a writer on `<data_dir>/tasks/<id>.out` for step
7a — and `None` inline, so one handler serves both entrances; `TaskHandle` is a thin public type
over an `Arc<dyn TaskSink>` the module implements, so the SDK keeps no runtime.
`FunctionBuilder::class(&str)` names the worker class of a function's jobs, `default` when absent,
and `FunctionBuilder::resumable()` marks a function whose job may restart from its checkpoint; both
reach the descriptor nowhere and `Limits` stays as it is. Progress goes out as `TaskProgress { id,
function, state, done, total, message }` frames, with `TaskState` the closed enum `queued`,
`running`, `done`, `failed`, `cancelled`, `interrupted`, both types of `keel-types`, through the
hidden `keel_sdk::system::LiveEvents { publish(Frame { principal, account, kind, data }) }` the
kernel provides as an in-process broadcast that step 6a turns into the `task.progress` event;
without a listener a frame is dropped. The hidden `keel_sdk::system::AdvisoryLock { async fn
take(&self, key: i64) -> Result<LockGuard> }` is provided by the kernel over the pattern of
`storage/lock.rs`, since a module sees the database through `keel_sdk::Db` alone and can neither pin
a pool connection nor keep a transaction open across a `start` hook, and a session lock taken
through the pooled connection would land on a random connection and leak to its next borrower:
`take` acquires a `PoolConnection` from the pool, marks it `close_on_drop`, runs `SELECT
pg_advisory_lock($key)` on it and hands the connection back inside `LockGuard`, whose drop closes
the connection and so releases the lock; a caller that drops the pending future while blocked closes
the connection as well, so a waiting taker leaves nothing behind. The keys `keeltask` and `keelcron`
join the constants of `storage/lock.rs` and its `every_lock_key_is_distinct` test.
`ContextExt::data_dir()` hands a module the data directory next to `db()`, `Composition` gains
`.data_dir(path)` and the runtime passes `config.data_dir`; `Running::stop` sets
`Readiness::draining`, then stops the socket and the listener, then the modules, so `/readyz` is not
reachable while step 1b drains, and no test polls it during a drain. The testkit gains
`Harness::new().data_dir(path)` — a temporary directory per harness by default —
`Harness::new().database(db)`, which boots a second kernel on the `TestDatabase` a `stop()`
returned, and `Harness::database_handle()`, a `DatabaseHandle` (URL and name, neither dropping nor
finishing the database) that `Harness::new().database(handle)` also accepts, since `TestDatabase` is
neither `Clone` nor free of its `Drop`; so a test can restart a composition on its data, or run two
harnesses on one database while the creating harness keeps ownership and finishes it last.

**Touches.** `crates/sdk/src/system.rs` (`TaskRunner`, `TaskSpec`, `TaskSink`, `Caller`,
`LiveEvents`, `Frame`, `AdvisoryLock`, `LockGuard`, `ContextExt::data_dir`),
`crates/sdk/src/task.rs` (`TaskHandle`), `crates/sdk/src/call.rs` (`Call::task`),
`crates/sdk/src/function.rs` (`class`, `resumable`), `crates/sdk/src/context.rs`,
`crates/types/src/task.rs` (`TaskProgress`, `TaskState`) with `contracts/schema/` and the generated
types, `crates/kernel/src/loader/composition.rs` (`data_dir`), `crates/kernel/src/storage/lock.rs`
(`AdvisoryLock`, the two keys), `crates/kernel/src/dispatcher/mod.rs` (`CallOptions.task`, the
`Caller` handle, the `LiveEvents` broadcast), `crates/keel/src/runtime.rs` (the data directory, the
handle), `crates/testkit/src/harness.rs` (`data_dir`, `database`, `database_handle`),
`crates/testkit/src/database.rs` (`DatabaseHandle`), `crates/kernel/tests/kernel/dispatcher.rs`,
`crates/kernel/tests/kernel/storage.rs`, `crates/testkit/tests/harness.rs`.

**Out of scope.** The module, its tables, worker and functions (steps 1b and 1c); the `task` and
batch entrances of the dispatcher (step 2a); the `task.progress` event on the wire (step 6a);
`output()` having a reader, which is `task.download` (step 7a).

**Acceptance.**

- `crates/kernel/tests/kernel/dispatcher.rs`:
  `the_caller_runs_as_the_handed_principal_via_internal_and_answers_unavailable_until_filled`;
  `a_handler_inside_a_job_reports_through_its_task_handle_while_an_inline_call_sees_none` with a
  `TaskSink` double;
  `a_published_frame_reaches_every_live_events_listener_and_is_dropped_without_one`.
- `crates/kernel/tests/kernel/storage.rs`:
  `a_second_taker_blocks_until_the_guard_drops_and_a_dropped_pending_take_closes_its_connection`.
- `crates/testkit/tests/harness.rs`:
  `a_harness_reboots_on_a_returned_database_shares_it_with_another_and_keeps_its_data_dir`;
  `crates/kernel/src/loader/composition.rs`:
  `a_composition_without_a_data_dir_hands_modules_the_configured_default`.
- `pnpm generate` leaves `TaskProgress` and `TaskState` committed in the generated types;
  `cargo-semver-checks` stays green on the facade, `Call::task`, `TaskHandle`, `class` and
  `resumable` being additions.

### 1b. tasks: the runner, worker classes, leader and drain

**Goal.** The `tasks` module itself, as the feature `tasks` of `crates/system` with the module id
`task`, on the seams of step 1a, without its functions, which step 1c adds, so this step is the
runner alone. The table `task_job` (`id`, `class`, `function`, `params_json`, `principal_id`,
`account_id`, `state` — the `TaskState` values — `resumable`, `progress_done`, `progress_total`,
`message`, `checkpoint_json`, `result_json`, `error_json`, `request_id`, `created_at`, `started_at`,
`finished_at`) holds one row per job, and `task_step` (`job_id`, `seq`, `at`, `done`, `total`,
`message`, `checkpoint_json`) one row per progress report or checkpoint, so the last step is at hand
and a restart finds the checkpoint; the log is the file `<data_dir>/tasks/<id>.log`, one line per
`log` call stamped by the kernel's `Clock`, as the layout table of
[design/09-operations.md](design/09-operations.md) places it. The module provides `TaskRunner`
through `r.provide::<dyn TaskRunner>`, implements `TaskSink` for the handles it hands out, and runs
the stored function under the stored principal through `Caller`, with `CallOptions.task` set. Worker
classes: the worker runs jobs of distinct classes concurrently and jobs of one class in order, so a
heavy export never delays a quick batch; the worker itself is single per installation, held on the
eight-byte key `keeltask` through the `AdvisoryLock` seam of step 1a, the mechanism and the module's
counterpart of the kernel's keys of [design/04-storage.md](design/04-storage.md); a second process
on the same database blocks in the same `take` and takes over when the first releases the lock or
its connection closes, so a waiting takeover needs no poll, and `stop` on the waiting side drops the
pending call. The guard holds one of the pool's twenty connections for the process lifetime, and the
leader of step 3 a second; the count is recorded in the commit message. At `start` every `running`
row is marked `interrupted`; the resumable ones are re-queued with their checkpoint kept, the others
stay `interrupted` with `task.error.interrupted`. `stop` drains: no queued job starts, running jobs
see `is_cancelled()` as a request to stop at their next checkpoint, the hook waits up to
`TASK_STOP_GRACE` for them — `20` seconds, below the unit's `TimeoutStopSec` and the scripts'
SIGTERM wait, so a checkpoint is written before anything kills the process — and marks what still
runs `interrupted`. Cancellation is cooperative in the same way: `TaskRunner::cancel(id)` sets the
flag, and the worker marks `cancelled` when the handler returns `task.error.cancelled` or finishes.
Every progress report and state change is published as a `TaskProgress` frame to the job's principal
and account through `LiveEvents`. `api::Drain` awaits the queue for tests, as `audit::api::Flush`
awaits the sink.

**Touches.** `crates/system/Cargo.toml` (the feature `tasks`), `crates/system/src/lib.rs`,
`crates/system/src/tasks/` (`mod.rs`, `api.rs`, `entity.rs`, `runner.rs`, `worker.rs`, `leader.rs`,
`log.rs`), `crates/system/migrations/tasks/`, `crates/system/tests/system/` (`main.rs`, `tasks.rs`).

**Out of scope.** `task.list`, `task.get`, `task.cancel`, the menu node, the dictionaries, the
matrix sets and `hack/prove-binary.sh` (step 1c); `ui.result = task`, the batch rule and
`note.export` (step 2a); `task.download` and the export file (step 7a); the drain before an update,
which arrives with the operator plane of M4; retention of finished jobs and their files; a worker on
another node (M8).

**Acceptance.**

- `crates/system/tests/system/tasks.rs`, through the runner and the tables, since the functions
  arrive with step 1c:
  `a_spawned_job_runs_under_the_stored_principal_and_records_progress_steps_and_log_lines` — a test
  module registers a function whose handler reports three progress steps and two log lines through
  `call.task()`; spawned through the runner for A1, the job ends `done` with `task_step` rows,
  `state(id)` answers `done`, the log file under the harness's data directory holds the two lines,
  and the frames reached a `LiveEvents` listener;
  `a_cancelled_job_stops_at_its_next_checkpoint_and_ends_cancelled`;
  `a_job_running_at_stop_is_interrupted_and_a_resumable_one_restarts_from_its_checkpoint` — the
  harness stops mid-job, a second harness on the same database and data directory marks the row
  `interrupted`, re-queues the resumable one, whose handler reads `resume_from` and finishes without
  repeating the first half, and leaves a non-resumable one `interrupted`;
  `stop_lets_a_running_job_reach_its_checkpoint_and_leaves_queued_jobs_for_the_next_start`;
  `jobs_of_two_classes_run_concurrently_while_jobs_of_one_class_run_in_order`;
  `two_kernels_on_one_database_run_one_worker_and_the_second_takes_over_when_the_first_stops`.
- `cargo test -p keel-system --features tasks` is green; the matrix of `ci.yml` waits for step 1c.

### 1c. tasks: the functions, the menu node and the matrix

**Goal.** The screens of the `tasks` module over the runner of step 1b, every one a descriptor
rendered generically: `task.list` (`list`, `Level::User`, permission `task.read` for user and
reseller, `.owned()` over `task_job.account_id`, columns `function`, `state` as `status` with a tone
map, `progress` as `text`, `createdAt`, `finishedAt`, a filter on `state`, search on `function`, row
actions `open` on `task.get` and `cancel` hidden unless the state is `queued` or `running`),
`task.get { id }` (`get`, the row with `logTail`, the last `TASK_LOG_TAIL` lines of the file),
`task.cancel { id }` (`action`, permission `task.manage` for user and reseller, calling
`TaskRunner::cancel`); the menu node `system/tasks`, the node the `/boot` example of
[design/10-contracts.md](design/10-contracts.md) places in the platform group; the dictionaries
`crates/system/i18n/tasks/en.json` and `ru.json`. The `keel-system` matrix of `ci.yml` follows one
rule from here on: every module alone, and the crate without each module in turn, the features a
removed module implies removed with it; this step adds `tasks` and the four sets without one of
`auth`, `settings`, `audit` and `tasks`, and every later module step adds its own by the same rule.

**Touches.** `crates/system/src/tasks/functions.rs` and `mod.rs`, `crates/system/i18n/tasks/`,
`crates/system/tests/system/` (`tasks.rs`, `tasks_alone.rs`, snapshots), `bin/keel/src/main.rs`,
`.github/workflows/ci.yml` (the matrix), `hack/prove-binary.sh` (`ctl task.list`).

**Out of scope.** `ui.result = task`, the batch rule and `note.export` (step 2a); `task.download`
(step 7a); the task drawer (step 13a).

**Acceptance.**

- `crates/system/tests/system/tasks.rs` gains
  `task_get_shows_the_last_step_the_log_tail_and_task_cancel_ends_a_paced_job_cancelled` — the job
  of the first test of step 1b is read back as A1 with its state, `3/3` and the two lines in
  `logTail`, and a paced job is cancelled through `task.cancel`; `assert_reseller_sees_only_subtree`
  and `assert_user_cannot_edit_foreign_row` hold for `task.list` and `task.cancel` with a `Fixture`
  inserting a job row; every `task.*` descriptor matches its snapshot.
- `crates/system/tests/system/tasks_alone.rs` boots the module with `auth` alone and answers
  `task.list` on the socket; the matrix job `keel-system with tasks` is green, and so are
  `settings,audit,tasks`, `auth,audit,tasks`, `auth,settings,tasks` and `auth,settings,audit`.
- `hack/prove-binary.sh` runs `ctl task.list` and gets an empty page.

### 2a. tasks: ui.result = task, batch_threshold and note.export

**Goal.** The two rules that turn a call into a job and the first task of the product line.
`ui.result = task` on an `action` or `wizard` function, the `task` member of `ScreenResult` of
[design/02-functions.md](design/02-functions.md): the dispatcher runs the stages up to the before
hooks — resolve, guards, strip and validation, idempotency, before hooks — and then, instead of the
transaction and the handler, asks the `TaskRunner` of step 1b to spawn the same function with the
enriched parameters under the caller's principal, in the function's `class`, and answers `{ task: {
id } }`, the `TaskRef` type of `keel-types`; the worker's call of the function then runs the whole
pipeline again with `CallOptions.task` set, so a handler sees `call.task()` and reports; the task
entrance and the batch rule are skipped when `CallOptions.task` is `Some`, and the worker's call
carries no `Idempotency-Key`, so the stored parameters run once and the job never spawns itself. The
freeze refuses a function with `ui.result = task` in a composition without a `TaskRunner` provider,
naming the function and the module, and refuses `ui.result = task` on a function with a `secret`
field, since a job stores its parameters, the plan's choice for a case the design does not name.
`batch_threshold` of the limits table of the same document: a `delete` or `group` action whose key
parameter is an array with more ids than the threshold, after `OwnedGuard` admitted every id,
becomes a job whose body calls the function once per chunk of `batch_threshold` ids, each chunk one
call and one transaction, with progress per chunk and a log line per chunk; the answer is a
`TaskRef`; a composition without a runner runs the batch inline, the threshold being inert without
one. The demo's `note.export { format, pace }` is the first task, the one the M3 exit criteria name:
a `kind: action` in `mode: read`, `Level::User`, permission `note.read`, `.unowned()`, whose screen
block is the new `ActionScreen::new("note.export.title").result(ScreenResult::Task)` of the SDK's
`ui` builders — `ScreenUi.result` on a `kind: action` is the plan's choice for a gap the design
leaves: the `ui` table of [design/02-functions.md](design/02-functions.md) has rows for list, form
and wizard alone and gives `result` to the wizard, while
[design/05-modules.md](design/05-modules.md) and [design/08-frontend.md](design/08-frontend.md) say
`ui.result = task` without naming a kind; the commit message records the choice and step 15 adds an
`action` row to the table — offered from the toolbar of `note.list` as `{ id: "export_task", role:
"button", function: "note.export", params: { format: "csv" }, icon: "file-down" }`, whose static
`params` the action runner of M2 posts, and reachable as `/f/note.export?format=csv&pace=300`
through the action route of M2 once step 13a coerces the route's query values by the descriptor's
schema, which is how a browser test asks for a slow run; until then the tests of this step ask for
it over HTTP with a JSON body, since the route of M2 posts a query value as a string and `pace` is
an integer. `format` is `csv` or `json`, `pace` the milliseconds the job waits after each note, `0`
by default and at most `2000`; the handler walks the caller's notes in id order, writes one record
per note into `call.task().output()`, logs `exported <title>`, reports `done/total`, checkpoints `{
lastId, written }` after every note, resumes from that checkpoint, stops at `is_cancelled()`, and
answers `{ exported }`, beside which step 7b puts the download of the written file. The function is
`.resumable()` and in the class `export`. `note.delete` declares `.batch_threshold(10)`, and `note`
names `tasks` in `requires`, so the runner starts before the demo.

**Touches.** `crates/kernel/src/dispatcher/mod.rs` (the task and batch entrances),
`crates/kernel/src/loader/freeze.rs` (the two checks), `crates/types/src/task.rs` (`TaskRef`) with
`contracts/schema/` and the generated types, `crates/sdk/src/ui.rs` (`ActionScreen`),
`products/demo/` (`Cargo.toml`, `src/note/mod.rs`, `src/note/export.rs`, `src/note/functions.rs`,
the dictionaries, `tests/demo/tasks.rs`, snapshots), `crates/kernel/tests/kernel/tasks.rs` with a
runner double, `hack/prove-binary.sh`.

**Out of scope.** The progress modal (step 13a); the download result of `note.export` and
`task.download` (steps 7a and 7b); the group-edit request over `ids` and its threshold (step 8a);
`keel.account.delete` as a task (step 2b).

**Acceptance.**

- `crates/kernel/tests/kernel/tasks.rs` with a `TaskRunner` double recording spawns:
  `a_ui_result_task_function_answers_a_task_ref_and_its_handler_runs_in_the_worker`,
  `a_composition_with_a_ui_result_task_function_and_no_runner_fails_freeze_naming_the_function`,
  `a_secret_field_on_a_ui_result_task_function_fails_freeze`,
  `a_batch_above_the_threshold_becomes_a_job_of_chunks_and_one_at_the_threshold_runs_inline`,
  `a_batch_with_one_foreign_id_is_not_found_before_any_job_is_spawned`,
  `a_job_run_by_the_worker_does_not_spawn_a_second_job` — the runner double calls the function back
  with `CallOptions.task` set and no idempotency key, and the handler runs once with no further
  spawn.
- `products/demo/tests/demo/tasks.rs` with the real `tasks` module composed:
  `note_export_runs_as_a_task_with_progress_a_log_line_per_note_and_a_checkpoint_after_each` — A1
  calls `note.export { format: "csv" }` over HTTP, receives a `TaskRef`, and `task.get` ends `done`
  with `exported` equal to A1's notes, a `logTail` naming them and `task_step` rows for each;
  `note_export_is_cancelled_between_two_notes_when_paced` — with `pace: 500` in the JSON body,
  `task.cancel` after the first frame leaves `cancelled` with `exported` below the count;
  `note_export_interrupted_by_a_restart_resumes_from_its_checkpoint_and_writes_no_note_twice`;
  `note_delete_above_ten_ids_becomes_a_task_that_deletes_in_chunks_of_ten`; the `note.export`
  descriptor matches its snapshot, and `note.list` carries the `export_task` action.
- `hack/prove-binary.sh`: `ctl note.export format=csv` answers a `TaskRef`, and within the prove
  timeout `ctl task.get id=<id>` answers `done`, `progress` equal to the note count and the log
  lines; then, with at least four notes present, the script runs `ctl note.export format=csv
  pace=500`, reads the `TaskRef`, calls `stop_serve` while `ctl task.get id=<id>` still answers
  `running`, calls `start_serve resumed`, and within the prove timeout reads `done` from `ctl
  task.get id=<id>` with `progress` equal to the note count and each `exported <title>` line exactly
  once in `logTail` — the CI counterpart of `systemctl restart` that the M3 "Done when" and step 15
  cite; `pnpm generate` leaves `TaskRef` committed.

### 2b. keel.account.delete as a task walking AccountLifecycle, suspend and resume

**Goal.** The `AccountLifecycle` section of [design/03-access.md](design/03-access.md) and the
`keel.account.delete` sentence of the "tasks and cron" section of
[design/05-modules.md](design/05-modules.md), which the M0 and M1 breakdowns pointed at M3: the
kernel's first task, on the entrance of step 2a, needing nothing of `keel-host`. `AccountLifecycle {
on_suspend, on_resume, on_delete, on_rename }` joins the public SDK as a contribution of `keel-sdk`,
each member an optional handler of the shape of a `within` hook, `fn(Call<Value>) ->
Future<Result<()>>`, run inside the call of the kernel function that drives the change, with the
account as the call's `id` parameter and the module's rows reached through `call.tx()`; `on_rename`
is a member without a caller in M3, since no account column is a name and a rename arrives with the
origin sync of M6, and the freeze accepts a contribution without it. `keel.account.suspend { id }`
and `keel.account.resume { id }` (`action`, `Level::Reseller`, permission `keel.account.manage`,
`.owned()`) set `state` to `suspended` and `active`, run the matching handler of every contributing
module in `requires` order inside the transaction, and emit `keel.account.suspended` and
`keel.account.resumed`; the root account answers `409 keel.account.error.root_immutable` and the
caller's own account `409 keel.account.error.own_account` to all three functions, and a principal of
a `suspended` or `deleting` account is refused by the kernel after the authenticator chain with `401
keel.error.account_suspended`, as a `suspended` principal is. `keel.account.delete { id }` is an
`action` with `ui.result = task`,
`ActionScreen::new("keel.account.delete.title").result(ScreenResult::Task)`, `Level::Reseller`,
`keel.account.manage`, `.owned()`, in the class `default`, `.resumable()`, offered as the row action
`delete` of `keel.account.list` with `confirm_with(ConfirmDetails { destructive: true, .. })`: the
call refuses an account with child accounts with `409 keel.account.error.has_children` and sets
`state` to `deleting` before the job is spawned, so its principals are refused at the authenticator
at once; the job walks the modules in reverse `requires` order, calling `on_delete` of each with a
checkpoint `{ done: [module] }` after every module and a log line `deleted <module>`, resumes from
that checkpoint after an interruption — the job's function runs in `mode: read` and the walk opens a
transaction of its own per module on the kernel's pool, which the kernel's functions have and a
module's do not, handing the handler a `Call` built over it, so every `on_delete` commits before its
checkpoint and a resumed job repeats none — then deletes the account's principals and the
`keel_account` row and emits `keel.account.deleted`; a module without an `on_delete` whose foreign
key holds the row makes the last step fail with `task.error.failed` naming the constraint, the loud
failure the design wants, and a module may refuse up front through `r.within("keel.account.delete",
…)` as the design shows. The modules that hold rows of an account contribute their handler here:
`auth` drops the passwords, sessions, tokens, login keys, challenges, second factors, recovery codes
and grants of the account's principals and the sessions of the account; `settings` drops the values
scoped to the account and the preferences of its principals; `tasks` drops the account's finished
jobs with their steps, logs and output files and refuses through a `within` hook with `409
task.error.jobs_running` while a job of the account is `queued` or `running`; `audit` keeps its
records, which reference no row; the demo drops the account's notes through `lifecycle::drop_notes`,
the `dns` example of the design read against `note`. The modules of steps 4b and 5b contribute
theirs where they land.

**Touches.** `crates/sdk/src/lifecycle.rs` (`AccountLifecycle`, `LifecycleHandler`),
`crates/sdk/src/lib.rs`, `crates/kernel/src/keel/accounts.rs` (the three functions, the row action),
`crates/kernel/src/keel/lifecycle.rs` (the walk, the checkpoint), `crates/kernel/src/keel/source.rs`
(the account state on the principal lookup), `crates/kernel/src/http/live.rs` and
`crates/kernel/src/socket/mod.rs` (the refusal after the chain),
`crates/kernel/src/loader/freeze.rs` (the contribution), `crates/kernel/i18n/`,
`crates/system/src/auth/lifecycle.rs`, `crates/system/src/settings/lifecycle.rs`,
`crates/system/src/tasks/lifecycle.rs` and the three `mod.rs`, `products/demo/src/note/lifecycle.rs`
and `mod.rs`, `contracts/fixtures/descriptors/` (the `keel.account.*` snapshots),
`crates/kernel/tests/kernel/accounts.rs`, `crates/system/tests/system/` (`auth_sessions.rs`,
`settings.rs`, `tasks.rs`), `products/demo/tests/demo/lifecycle.rs`, `hack/prove-binary.sh`.

**Out of scope.** `on_rename` having a caller (M6); `keel.account.upsert` and the `keel.account.*`
events of the origin sync (M6); `close` with a retention period for service accounts (M6); the OS
side of a lifecycle in `keel-host` (M4); a retention of `deleting` accounts whose job failed, which
the operator plane's `doctor` reports (M4).

**Acceptance.**

- `crates/kernel/tests/kernel/accounts.rs` with two test modules contributing `on_suspend`,
  `on_resume` and `on_delete` and one contributing nothing:
  `suspend_and_resume_run_the_handlers_in_requires_order_and_emit_their_events`;
  `a_principal_of_a_suspended_or_deleting_account_is_refused_after_the_chain_until_resume`;
  `delete_marks_the_account_deleting_and_the_job_walks_on_delete_in_reverse_order` with a `TaskRef`
  answered and a checkpoint per module;
  `an_interrupted_deletion_resumes_with_the_remaining_modules_and_calls_no_handler_twice` on the
  `database()` builder of step 1a;
  `rows_without_on_delete_fail_the_job_loudly_and_a_within_hook_refuses_the_call_up_front`;
  `the_root_account_the_callers_own_account_and_an_account_with_children_are_refused`;
  `assert_reseller_sees_only_subtree` and `assert_user_cannot_edit_foreign_row` hold for the three
  functions; the `keel.account.*` descriptors match their snapshots.
- `crates/system/tests/system/auth_sessions.rs`:
  `deleting_an_account_drops_the_auth_rows_of_its_principals`; `settings.rs`:
  `deleting_an_account_drops_its_values_and_its_principals_preferences`; `tasks.rs`:
  `deleting_an_account_drops_its_finished_jobs_and_files_and_is_refused_while_one_runs`.
- `products/demo/tests/demo/lifecycle.rs`:
  `deleting_an_account_drops_its_notes_through_the_demos_on_delete_and_the_account_row_after_them`.
- `hack/prove-binary.sh` creates a throwaway user account with two notes, deletes it over `ctl
  keel.account.delete`, and within the prove timeout `ctl task.get id=<id>` answers `done` with
  `deleted note` in `logTail` and `ctl keel.account.get id=<id>` answers `404`.

### 3. cron: CronJob contributions, the schedule, the leader lock and cron_run

**Goal.** The `cron` module as the feature `cron`, running the `CronJob` contributions of the
contribution table of [design/05-modules.md](design/05-modules.md). `CronJob` and `JobRun` live in
`keel_system::cron::api`, since the design keeps the contribution points other than `SettingDef`,
`LimitDef`, `AccountLifecycle` and `ConfigSection` in the modules' own `api`: `CronJob::new(id,
schedule, handler)` names a job by an id under the contributing module's prefix, a schedule string
and a handler `fn(JobRun) -> Future<Result<()>>`; `JobRun` carries the job id, the scheduled time,
`invoke(func, params)`, which runs a function as the owner principal with `via: internal` through
the hidden `Caller` of step 1a and is what a product module's job uses, and, behind
`keel_sdk::system`, `db()` and `caller()` for a system module's job, the latter being how the poll
job of step 4b reaches the internal functions of `notify`. The schedule is a five-field cron
expression parsed by `croner`, added to `[workspace.dependencies]` at a pinned major because no
listed crate parses one and the design fixes the `"0 */6 * * *"` form; the module's `start` parses
every contributed schedule and fails the boot naming the job on the first one that does not parse.
Leadership is the key `keelcron` taken through the `AdvisoryLock` seam of step 1a, the follower
blocking in `take` until the leader lets go, as the worker of step 1b holds `keeltask`; the leader
ticks every second against the kernel's `Clock`, so the frozen clock of the testkit drives it, runs
every job whose next occurrence has passed, and records each run in `cron_run` (`id`, `job`,
`scheduled_at`, `started_at`, `finished_at`, `outcome` — `ok`, `failed`, `skipped`, `interrupted` —
`error`). A job is single-flight: an occurrence that arrives while the previous run of the same job
has not finished is recorded `skipped`, and a run left without `finished_at` at start is marked
`interrupted`. Intervals are overridable through settings by one definition the module contributes
itself, `cron.overrides`, an object from job id to schedule string at `Level::Admin`, read at every
tick through `SettingsSource`; an override that does not parse is logged once and the contributed
schedule stands, the plan's choice, since a `SettingDef` per job cannot be contributed before the
contributions are known. Functions, all `Level::Admin` and `.unowned()`: `cron.job.list` (`list`
over the frozen contributions joined with their last run: `id`, `module`, `schedule` as contributed,
`effective`, `lastRunAt`, `lastOutcome` as `status`, `nextRunAt`; row action `run`), `cron.job.run {
id }` (`action`, runs the job now on the leader, `409 cron.error.running` while it runs),
`cron.run.list` (`list` over `cron_run`, sorted by `started_at` descending, filter on `job` and
`outcome`); the menu node `system/cron` labelled `cron.job.list.title`.

**Touches.** `crates/system/Cargo.toml` (the feature `cron`, `croner`), `Cargo.toml` (`croner`),
`crates/system/src/cron/` (`mod.rs`, `api.rs`, `entity.rs`, `functions.rs`, `leader.rs`,
`schedule.rs`, `scheduler.rs`), `crates/system/migrations/cron/`, `crates/system/i18n/cron/`,
`crates/system/tests/system/` (`cron.rs`, `cron_alone.rs`, snapshots), `.github/workflows/ci.yml`
(the matrix by the rule of step 1c), `bin/keel/src/main.rs`.

**Out of scope.** The `cron` sections of plugin manifests (M7); the aggregation job of `stats` (M5);
a job pinned to a node (M8); the poll job of `notify`, which is the first contribution (step 4b).

**Acceptance.**

- `crates/system/tests/system/cron.rs` with a test module contributing two jobs:
  `a_job_runs_when_the_clock_passes_its_next_occurrence_and_records_an_ok_run` — the frozen clock is
  advanced past `*/5 * * * *` and the handler runs once with a `cron_run` row;
  `a_job_still_running_is_not_started_again_and_the_occurrence_is_recorded_skipped`;
  `a_run_left_unfinished_at_start_is_marked_interrupted`;
  `two_kernels_on_one_database_elect_one_leader_and_the_other_takes_over_after_a_stop` on the
  `database()` builder of step 1a;
  `a_cron_overrides_entry_replaces_the_contributed_schedule_and_an_unparsable_one_is_ignored`;
  `cron_job_run_starts_a_job_now_and_answers_conflict_while_it_runs`;
  `a_products_job_invokes_a_function_as_the_owner_through_the_run`;
  `a_schedule_that_does_not_parse_fails_the_start_naming_the_job`; the `cron.*` descriptors match
  their snapshots and the reseller is answered `403`.
- `crates/system/tests/system/cron_alone.rs` boots the module with `auth` alone; the matrix gains
  `cron` and `auth,settings,audit,tasks` becomes the set without `cron`, the earlier "without" sets
  gaining `cron`.

### 4a. kernel: DashboardWidget, BootProblems, ProblemCounter and the PrincipalSource lookups

**Goal.** What the in-app half of `notify` needs from the kernel, the SDK and `keel-types`, landed
alone so that step 4b is a change to `crates/system` and `products/demo`, the line the M1 breakdown
drew between a seam and its consumer. `DashboardWidget { kind, func, limit }` joins the public SDK
as a contribution: the kernel fills `/boot.dashboard` from the contributions whose function the
principal may call — level, permission and feature as the menu applies them — and types it as
`BootWidget { kind, func, limit }` of `keel-types`, the `dashboard` member of the `/boot` table of
[design/10-contracts.md](design/10-contracts.md) leaving `Value`. `/boot.problems` becomes
`BootProblems { open, critical }`, answered by the hidden `keel_sdk::system::ProblemCounter {
counts(principal, db) }` when a module provides it and `null` without a provider as before. The
hidden `PrincipalSource` gains `at_or_above(level, db)` and `of_account(account, db)`, the two
lookups a module needs to address the admins and the principals of an account, over the kernel's own
`keel_principal`, since a module never reads that table by SQL. The fixtures
`contracts/fixtures/boot.json` and `boot-none.json` carry the typed members.

**Touches.** `crates/sdk/src/dashboard.rs` (`DashboardWidget`), `crates/sdk/src/lib.rs`,
`crates/sdk/src/system.rs` (`ProblemCounter`, `PrincipalSource::at_or_above`, `of_account`),
`crates/types/src/boot.rs` (`BootWidget`, `BootProblems`) with the fixtures, `contracts/schema/` and
the generated types, `crates/kernel/src/boot.rs` (the widgets, the counter),
`crates/kernel/src/keel/source.rs` (the two lookups), `crates/kernel/tests/kernel/boot.rs`.

**Out of scope.** The module that provides the counter and contributes the first widget (step 4b);
the home screen that renders the widgets (step 13b); `stat` widgets and the widget order (M5).

**Acceptance.**

- `crates/kernel/tests/kernel/boot.rs`:
  `dashboard_widgets_and_problem_counts_come_from_the_contributions_and_stay_empty_without_them` — a
  test module contributes a `list` widget over a `Level::Admin` list and provides a `ProblemCounter`
  double; the admin's `/boot` carries the widget and the counts, A1's carries neither, and a
  composition without them answers `[]` and `null`;
  `at_or_above_and_of_account_answer_the_principals_the_menu_and_the_scope_rules_would`;
  `contracts/fixtures/boot.json`, `boot-none.json` and the dump still round-trip with the typed
  members.
- `pnpm generate` leaves `BootWidget` and `BootProblems` committed; `cargo-semver-checks` stays
  green, `DashboardWidget` being an addition.

### 4b. notify: messages, problems and the poll job

**Goal.** The in-app half of the `notify` module of the "notify" section of
[design/05-modules.md](design/05-modules.md), as the feature `notify` requiring `cron`, on the seams
of step 4a: the messages, the problem registry with its sources and poll job, and what the dashboard
and `/boot` show of them. Tables: `notify_message` (`id`, `principal_id`, `account_id`, `key`,
`params_json`, `level` — `info`, `warning`, `error` — `source`, `problem_id`, `read_at`,
`created_at`) and `notify_problem` (`id`, `key`, `subject`, `severity` — `warning`, `critical` —
`params_json`, `account_id`, `source`, `count`, `raised_at`, `updated_at`, `cleared_at`). Every row
and its event are written by the dispatcher, never by the module outside a call: the module
registers three internal functions — `notify.problem.raise { key, subject, severity, params,
account, source }`, `notify.problem.clear { key, subject }` and `notify.message.create { recipients,
key, params, level, source, problem }` — `kind: action`, `mode: write`, `Level::Admin`,
`.unowned()`, marked `.internal()`, a new member of the function builder that keeps the function out
of every menu, the dump and the OpenAPI document and makes the dispatcher answer `403
keel.error.internal_only` unless the caller's `via` is `internal`, so HTTP and the socket never
reach them; `keel.error.internal_only` joins the kernel's dictionary. The module's `api` wraps them:
`Problems { raise(caller, Raise { key, subject, severity, params, account, source }), clear(caller,
key, subject) }` and `Notifier { notify(caller, Recipients, key, params, level) }` take the hidden
`Caller` of step 1a and run the internal functions as the owner principal with `via: internal`, so
the rows, the `notify.message.created` events and the `notify.problem.raised` and
`notify.problem.cleared` events are written inside one transaction of the dispatcher and pass the
commit order of step 4c once it lands. `raise` of an open `(key, subject)` pair updates `params`,
`count` and `updated_at` and notifies nobody twice, `clear` of a pair that is not open is a no-op,
so both are idempotent under the keys the design names; `notify.message.create` writes one
`notify_message` per recipient principal, `Recipients` being `Admins`, `Principal(id)` or
`Account(id)`, resolved through the two lookups of step 4a, and publishes a `NotificationFrame { id,
key, params, level, createdAt }`, a type of `keel-types`, through `LiveEvents` from an
`after_commit` callback, so a listener never sees the frame of a rolled-back message.
`ProblemSource::new(id, poll)`, where `poll(&Db)` answers the `Raise` set the source sees now, is
the contribution a module makes. A raised problem notifies the admins, an account-scoped one the
account's principals as well, with `notify.problem.raised` and the problem as params, and a cleared
one with `notify.problem.cleared`. The poll job `notify.problems.poll`, a `CronJob` on `* * * * *`
and the first contribution to `cron`, runs every `ProblemSource`, raises what a source reports and
clears what a source reported before and reports no more, source by source, through the `Caller` its
`JobRun` carries, and a source that fails keeps its problems and is logged. The module provides the
`ProblemCounter` of step 4a, counting problems without an account for admins and owners and
account-scoped ones within the caller's scope, and contributes a `list` widget over
`notify.problem.list` with `limit: 5`, the problems block of the M3 exit criteria. Functions:
`notify.message.list` (`list`, `Level::User`, permission `notify.read` for user and reseller,
`.owned()`, narrowed to the caller's own rows, sorted by `created_at` descending, filter `unread`),
`notify.message.mark { ids, read }` (`action`, key `ids`, `notify.manage`), `notify.message.clear`
(`action`, deletes the caller's read messages), `notify.problem.list` (`list`, `Level::User`,
`notify.read`, the scoping the counter applies, columns `key`, `severity` as `status`, `subject`,
`count`, `raisedAt`, `clearedAt`, filter `open`) and `notify.problem.get { id }`; the menu node
`system/problems`. The module contributes `AccountLifecycle { on_delete }` dropping the messages and
problems of a deleted account, as step 2b demands of every module with an `account_id` column. The
demo contributes its first source, `note.overdue`, a `ProblemSource` reporting `note.overdue` per
account with the count of `todo` notes whose `due` has passed, `subject` the account id and
`severity: warning`, and its menu node `demo/notes` gains `.badge("note.overdue")`; `note` names
`notify` in `requires` and `products/demo/Cargo.toml` gains `keel-system` with the `notify` feature
for `notify::api`. The matrix: `notify` alone turns `cron` on, so the set is `notify`; the set
without `notify` is `auth,settings,audit,tasks,cron`; the sets without `cron` lose `notify` with it,
`auth,settings,audit,tasks` staying as it is.

**Touches.** `crates/system/Cargo.toml` (`notify = ["cron", ...]`), `crates/system/src/notify/`
(`mod.rs`, `api.rs`, `entity.rs`, `problems.rs`, `messages.rs`, `poll.rs`, `counter.rs`,
`lifecycle.rs`, `functions/` with `internal.rs`), `crates/system/migrations/notify/`,
`crates/system/i18n/notify/`, `crates/system/tests/system/` (`notify.rs`, `notify_alone.rs`,
snapshots), `crates/sdk/src/function.rs` (`internal`), `crates/kernel/src/dispatcher/mod.rs` (the
`internal_only` refusal), `crates/kernel/src/http/descriptors.rs` and `crates/kernel/src/boot.rs`
(internal functions left out of the index and the menu), `crates/kernel/i18n/`,
`crates/types/src/event.rs` (`NotificationFrame`) with `contracts/schema/` and the generated types,
`crates/kernel/tests/kernel/dispatcher.rs`, `products/demo/` (`Cargo.toml`, `src/note/mod.rs`,
`src/note/problems.rs`, `tests/demo/problems.rs`), `.github/workflows/ci.yml`,
`bin/keel/src/main.rs`.

**Out of scope.** The channels, the templates and the mail (steps 4d and 4e); the `notification`
event on the wire and the notification centre (steps 6a and 13b); the sources disk, TLS, license,
migrations, outbox and nodes, which arrive with the operator plane (M4), the license (M5) and the
cluster (M8); password reset through `notify`, which the M3 scope list moves to M5 with `panel` — it
needs an address per principal that no table of the design carries and a delivery to one recipient
that the channels of step 4d do not make, and `panel` is the first product whose users reach the
panel without an operator at hand — step 15 amending the `auth` sentence of the design that placed
it in M3 and the M5 scope list gaining it; the `problems` and `widgets` sections of plugin manifests
(M7).

**Acceptance.**

- `crates/kernel/tests/kernel/dispatcher.rs`:
  `an_internal_function_admits_the_caller_alone_and_is_absent_from_the_index` — refused over HTTP
  and the socket with `403 keel.error.internal_only`, run through the `Caller`, missing from
  `/functions` and the dump.
- `crates/system/tests/system/notify.rs`:
  `raise_is_idempotent_by_key_and_subject_and_clear_closes_the_problem_once`;
  `a_source_polled_by_the_cron_job_raises_what_it_reports_and_clears_what_it_stops_reporting`;
  `a_raised_problem_notifies_the_admins_once_counts_in_boot_and_feeds_the_dashboard_widget` —
  `/boot` for the admin carries `problems: { open: 1, critical: 0 }` and a `dashboard` entry `{
  kind: "list", func: "notify.problem.list", limit: 5 }`, for A1 `problems: { open: 0, critical: 0
  }` and no widget, and the outbox holds one `notify.message.created` per admin written by the
  dispatcher;
  `an_account_scoped_problem_reaches_the_accounts_principals_and_shows_in_their_boot_counts`;
  `a_notification_frame_is_published_after_commit_and_not_for_a_rolled_back_message`;
  `notify_message_create_is_refused_over_http_and_the_socket_and_runs_through_the_caller`;
  `a_user_lists_marks_and_clears_own_messages_only` with `assert_reseller_sees_only_subtree` for
  `notify.problem.list` and `assert_user_cannot_edit_foreign_row` for `notify.message.mark`;
  `deleting_an_account_drops_its_messages_and_problems`; the `notify.*` descriptors match their
  snapshots and the internal functions have none.
- `products/demo/tests/demo/problems.rs`:
  `an_overdue_todo_raises_note_overdue_for_its_account_after_the_poll_and_a_done_one_clears_it`, and
  `/boot` for A1 lists `demo/notes` with `badge.problem` `note.overdue`.
- `crates/system/tests/system/notify_alone.rs` boots with `auth` alone and its `cron`; the matrix
  sets `notify`, `auth,settings,audit,tasks,cron` and the others named above are green; `pnpm
  generate` leaves `NotificationFrame` committed; `hack/descriptors.sh` still prints the same share,
  the internal functions counting nowhere.

### 4c. kernel: OutboxSubscriber and the commit watch

**Goal.** The seam every outbox consumer of M3 reads through, landed alone as the seams of step 1a
were, so that step 4d is a change to `crates/system`. The hidden `keel_sdk::system::OutboxSubscriber
{ after(seq, limit) -> Vec<Event>, latest() -> watch::Receiver<u64> }` — the name the hidden-hooks
list of [design/05-modules.md](design/05-modules.md) reserves for the seam the outbox consumers plug
into — is provided by the kernel: the dispatcher publishes the latest `seq` after every commit that
wrote events, and `after` fills `instanceId` from the instance. `seq` is made commit-ordered:
`outbox::write` takes `pg_advisory_xact_lock` on the eight-byte key `keeloutbox` before its first
insert, so transactions that write events commit in `seq` order and `after(seq)` never passes a row
that is still uncommitted — an identity value is assigned at insert, and without the lock a consumer
that read past a higher `seq` would lose the lower one committing later — the plan's choice for a
guarantee the design's "no consumer can be left behind" needs and names no mechanism for; the lock
serialises the commit tail of the calls that emit events alone, and the key joins the kernel's
advisory keys in [design/04-storage.md](design/04-storage.md) through step 15. The runtime provides
the subscriber with the instance id, and a kernel test composes it as the runtime does.

**Touches.** `crates/sdk/src/system.rs` (`OutboxSubscriber`),
`crates/kernel/src/dispatcher/outbox.rs` (the transaction lock, `after`) and `mod.rs` (the watch),
`crates/keel/src/runtime.rs` (the subscriber provided with the instance),
`crates/kernel/tests/kernel/outbox.rs`.

**Out of scope.** Every consumer (steps 4d, 5b and 6a); retention of the outbox (M4).

**Acceptance.**

- `crates/kernel/tests/kernel/outbox.rs`:
  `outbox_subscriber_answers_events_after_a_seq_with_the_instance_id_and_wakes_a_waiter_on_commit`;
  `a_transaction_writing_an_event_waits_for_the_earlier_writer_to_commit_so_after_never_skips_a_row`
  — two transactions, the first writes an event and stays open, the second's write blocks until the
  first commits, and `after` answers the rows in `seq` order with no gap that fills later.

### 4d. notify: channels, templates and the sender

**Goal.** The delivery half of `notify`: channels, templates, the outbox consumer that feeds them,
the sender and the test email button, tested against a closed port and a refused connection; the
mail catcher is step 4e. Tables: `notify_channel` (`id`, `kind` — `email`, `telegram`, `webhook` —
`name`, `enabled`, `config_json` with its secrets sealed through `SecretCipher`, `last_sent_at`,
`last_error`, `created_at`, `created_by`), `notify_delivery` (`id`, `channel_id`, `message_id`,
`attempt`, `state` — `pending`, `sent`, `failed` — `next_at`, `error`, `sent_at`, `created_at`) and
`notify_cursor` (`consumer`, `seq`). `MailTemplate::new(id, subject, body)` joins the module's
`api`: a pair of minijinja templates rendered under a `SyntaxConfig` whose variable delimiters are
`{` and `}`, so a template obeys the one placeholder form of
[design/08-frontend.md](design/08-frontend.md) and the freeze's placeholder check applies to it,
with the block delimiters the commit message records; the module ships `notify.problem_raised`,
`notify.problem_cleared` and `notify.test` under `crates/system/templates/notify/`, and a
contributed id that collides fails the freeze. The email channel goes over `lettre`, added to
`[workspace.dependencies]` at a pinned major with its rustls features and without `native-tls`, so
the musl build stays free of C libraries: `AsyncSmtpTransport` with `host`, `port`, `security`
(`none`, `starttls`, `tls`), `username`, `password` (sealed), `from` and `to`; the module's `start`
runs `rustls::crypto::ring::default_provider().install_default()` before the first transport is
built, ignoring the `Err` of a provider already installed, since the workspace's `rustls` pin
installs none on its own and a `starttls` or `tls` channel would otherwise fail at its first send —
the line step 9b moves to the runtime, so a step landing after 9b does not add it; the `telegram`
and `webhook` kinds are members of the enum, listed by the descriptor, and refused by
`notify.channel.set` with `422 notify.error.channel_kind_unavailable` on `kind` until their
milestone. Delivery goes through the outbox, as the design says: a consumer with the cursor
`channels` wakes on `OutboxSubscriber::latest()` of step 4c, reads the `notify.message.created`
events after its `seq` and creates one `notify_delivery` per enabled channel for a message whose
`source` is `problem` or `test`; the sender wakes on a `tokio::sync::Notify` the consumer signals
and on the clock for the retries, so a message becomes an attempt within a second of its commit and
the retry ladder alone waits on the clock; it renders the template the message's key names with its
params and the recipient's language, and retries a failure on the ladder of
[design/07-ecosystem.md](design/07-ecosystem.md), 1m, 5m, 30m, 2h, 12h, after which the delivery is
`failed` and `notify.delivery_failed` is raised for the channel through `Problems`, with the
`Caller` the module took from its context at `start`. The test email button is `notify.channel.test
{ id }`, an action that writes a message of source `test` to the caller with one delivery on that
channel, so the outcome shows in `notify.delivery.list` and on the channel row. Functions, all
`Level::Admin` and `.unowned()`: `notify.channel.list`, `notify.channel.get`, `notify.channel.set {
id?, kind, name, enabled, email: { host, port, security, username, password, from, to } }` (`form`,
`password` a `secret`), `notify.channel.delete { id }`, `notify.channel.test { id }` and
`notify.delivery.list` (columns `channel`, `messageKey`, `state` as `status`, `attempt`, `nextAt`,
`error`, `sentAt`); the menu node `system/channels`.

**Touches.** `crates/system/Cargo.toml` (`lettre`, `minijinja`, `rustls`), `Cargo.toml` (`lettre`),
`crates/system/src/notify/` (`channels/` with `mod.rs` and `email.rs`, `templates.rs`,
`consumer.rs`, `sender.rs`, `functions/channels.rs`, `functions/deliveries.rs`),
`crates/system/migrations/notify/` (the three tables), `crates/system/i18n/notify/`,
`crates/system/templates/notify/`, `crates/system/tests/system/` (`notify_channels.rs`, snapshots);
`deploy/README.md` is untouched until step 15.

**Out of scope.** The mail catcher in CI and `hack/e2e.sh` (step 4e); the telegram and webhook
channels beyond their declaration; a template per language beyond the catalogue's languages; an
address per principal (M5, by the note of step 4b).

**Acceptance.**

- `crates/system/tests/system/notify_channels.rs`:
  `an_unimplemented_channel_kind_is_refused_on_kind_and_the_email_password_is_sealed_and_masked`;
  `a_template_renders_with_single_brace_placeholders_and_a_double_brace_fails_the_freeze`;
  `a_message_becomes_a_delivery_per_channel_within_a_second_and_the_cursor_survives_a_restart` on
  the `database()` builder of step 1a;
  `a_refused_smtp_connection_follows_the_ladder_and_the_fifth_failure_raises_notify_delivery_failed`
  under the frozen clock;
  `a_tls_channel_to_a_closed_port_fails_on_the_connection_and_not_for_want_of_a_provider`; the
  `notify.channel.*` and `notify.delivery.list` descriptors match their snapshots.

### 4e. notify: the mail catcher in CI and hack/e2e.sh

**Goal.** The email of the M3 exit criteria seen arriving: an SMTP catcher with an HTTP API reaches
the tests the way Postgres does, and `hack/e2e.sh` walks the cron job, the problem and the mail end
to end. The catcher is the `mailpit` image, the plan's choice of a maintained catcher with an SMTP
listener and a JSON API; the M3 "Done when" names no product. `.github/workflows/ci.yml` adds to the
jobs `check` and `system` the service `mailpit` (that image, `ports: ['1025', '8025']`, the health
check `/mailpit readyz`, the binary the image ships and checks itself with) and exports
`KEEL_TEST_SMTP_URL=smtp://127.0.0.1:${{ job.services.mailpit.ports['1025'] }}` and
`KEEL_TEST_SMTP_API=http://127.0.0.1:${{ job.services.mailpit.ports['8025'] }}` to the steps Test,
the `keel-system` Test and `hack/e2e.sh`, the arrangement of the Postgres service, so no job starts
a container on the runner. `crates/system/tests/system/notify_mail.rs` uses the two variables when
set and starts the image through `testcontainers` otherwise, as the testkit starts Postgres.
`hack/e2e.sh` reads the two variables and uses that catcher; when both are unset it starts the image
itself through the CLI of the container runtime `KEEL_CONTAINER_RUNTIME` names, `podman` when it is
on the path and `docker` otherwise — the daemon `testcontainers` reaches through `DOCKER_HOST` — on
two `free_port`s published as `-p <port>:1025 -p <port>:8025`, waits for `/readyz` of the API,
removes the container in `cleanup`, and fails naming the runtime when neither is on the path; the
`for tool in` check of the script gains the runtime under that condition alone. With the catcher
reachable the script creates an email channel to it over `ctl notify.channel.set`, creates a `todo`
note of `alice` with `due` in the past, runs `ctl cron.job.run id=notify.problems.poll`, asserts
`ctl notify.problem.list` shows `note.overdue`, and polls `GET /api/v1/messages` of the catcher up
to the prove timeout until it lists one message whose subject names the key, rather than asserting
once, since the sender of step 4d wakes within a second and the script must not race it;
`notify_mail.rs` polls the same way.

**Touches.** `.github/workflows/ci.yml` (the catcher service and its variables),
`crates/system/Cargo.toml` (`testcontainers` as a dev-dependency),
`crates/system/tests/system/notify_mail.rs`, `hack/e2e.sh`.

**Out of scope.** The browser side of the mail (none; the catcher is checked by the script and the
test); `deploy/README.md` (step 15).

**Acceptance.**

- `crates/system/tests/system/notify_mail.rs`:
  `a_raised_problem_reaches_the_mail_catcher_and_the_test_button_sends_a_test_email` against the
  catcher `KEEL_TEST_SMTP_URL` and `KEEL_TEST_SMTP_API` name, the service container in CI, and
  against one started through `testcontainers` when they are unset; the subject of the received mail
  is the rendered `notify.problem_raised` in `en`.
- `hack/e2e.sh target/debug/keel` is green with the catcher, in the job `check` with the service
  container and locally with the container the script starts: the poll raises `note.overdue` and the
  catcher lists the email within the prove timeout; with the two variables unset and neither
  `podman` nor `docker` on the path the script fails naming the runtime.

### 5a. kernel: emit_about and the subject rule

**Goal.** Events need a subject for a subscription and a stream to be scoped, the seam of steps 5b
and 6a landed alone. The public `Call::emit_about(kind, EventSubject, &data)` names the row an event
is about, and the dispatcher fills a missing subject for a plain `emit` with `kind` the function
name without its verb, `id` the key parameter when it is a single id, and `accountId` the account
the call resolved through `Call::resolve_account`, which the call state records on its last
successful resolution, and the caller's account when the handler resolved none — the plan's choice
for the subject rule the design leaves to the emitter, and the resolved account rather than the
caller's because an owner editing a user's row over `ctl note.edit … account=<alice>`, as
`hack/prove-binary.sh` does, must produce an event alice's stream and subscriptions match — so
`keel_outbox.subject_json` is filled from here on, and `note.edit` names the note's account through
`emit_about`.

**Touches.** `crates/sdk/src/call.rs` (`emit_about`, `Emitted.subject`, the recorded resolution),
`crates/kernel/src/dispatcher/outbox.rs` and `mod.rs` (the subject fill),
`crates/kernel/tests/kernel/dispatcher.rs`, `products/demo/src/note/functions.rs` (`emit_about`),
`products/demo/tests/demo/note.rs`.

**Out of scope.** Every consumer of the subject (steps 5b and 6a).

**Acceptance.**

- `crates/kernel/tests/kernel/dispatcher.rs`:
  `an_emitted_event_carries_the_subject_the_handler_named_or_the_one_the_dispatcher_derives`;
  `a_derived_subject_names_the_resolved_account_when_an_admin_acts_on_another_accounts_row`; the
  outbox fixture of `contracts/fixtures/event.json` still round-trips.
- `products/demo/tests/demo/note.rs`: `note_saved_names_the_notes_account_as_its_subject` for a note
  the owner edits in A1's account.

### 5b. webhooks: subscriptions, HMAC signatures, retries and the dead letter

**Goal.** The `webhooks` module of the "webhooks" section of
[design/05-modules.md](design/05-modules.md) and the event-delivery section of
[design/07-ecosystem.md](design/07-ecosystem.md), as the feature `webhooks` with the module id
`webhook`, requiring `notify`; its conformance check is step 5c. Tables: `webhook_subscription`
(`id`, `account_id`, `principal_id`, `name`, `url`, `secret` sealed through `SecretCipher`, `events`
as a text array of masks, `enabled`, `created_at`, `created_by`, `updated_at`), `webhook_delivery`
(`id`, `subscription_id`, `account_id`, `event_id`, `seq`, `attempt`, `state` — `pending`,
`delivered`, `failed`, `dead` — `next_at`, `last_status`, `last_error`, `delivered_at`,
`created_at`) and `webhook_cursor` (`consumer`, `seq`). The consumer with the cursor `deliveries`
reads the outbox through the `OutboxSubscriber` of step 4c and, for every event, creates one
`webhook_delivery` per enabled subscription whose masks match the event type by the rule of
`invalidate_on` and whose scope admits the event: a subscription is scoped to its subscriber,
`AccountScope::of` the principal loaded through `PrincipalSource::by_id`, and an event is admitted
when its `subject.accountId` of step 5a lies within that scope, an event without one reaching
admins' subscriptions alone, which is how a user-level subscription is scoped to own events. The
sender posts due deliveries through `reqwest` over rustls with the event as the body, `Content-Type:
application/json`, `X-Keel-Event-Id` and `X-Keel-Signature: t=<unix seconds>,v1=<hex HMAC-SHA256 of
"<t>.<body>" under the secret>`, the format of the design; `t` is the unix time of the attempt, so a
retry carries a new `t` and a new `v1`; `signature.rs` exposes `sign(secret, t, body)` and
`verify(secret, header, body, now)`, where `verify` compares through `hmac::Mac::verify_slice`, the
constant-time comparison, and refuses a `t` more than 300 seconds from `now`, the window the design
sketches and the plan fixes, so the inbound check of M6 reuses it. The sender's client is built with
`redirect(Policy::none())`, ten seconds to connect and thirty in all, a 1 MiB cap on the response
body it reads, and `last_error` truncated to 512 bytes; a `2xx` marks `delivered`, anything else or
a transport failure moves `attempt` on and sets `next_at` from the ladder 1m, 5m, 30m, 2h, 12h, and
the failure after the fifth attempt marks the delivery `dead` and raises `webhook.dead_letter` for
the subscription through `Problems` of step 4b, with the subscription's account and the `Caller` the
module took from its context at `start`, which clears when a later `webhook.delivery.retry`
delivers. The module's `start` installs the ring provider as step 4d does, the line step 9b moves to
the runtime, so an `https://` subscription does not fail at its first send for want of a
process-level provider. A subscriber chooses the target, so the module guards the installation's own
network: the `SettingDef` `webhook.private_targets` (`bool`, `Level::Admin`, default `false`) the
module contributes and reads through `SettingsSource`, the default standing in a composition without
`settings`, governs private targets — while it is `false`, `webhook.subscription.set` refuses on
`url` with `422 webhook.error.url_private` a scheme other than `https`, a host that is an IP
literal, and a host that resolves through `tokio::net::lookup_host` to a loopback, link-local,
private (RFC 1918), unique-local or unspecified address, an admin's subscription checked the same
way; the sender resolves the host through `tokio::net::lookup_host` again at send time, applies the
same rule to every answered address, marks the attempt failed with `webhook.error.url_private` when
any is private, and pins the checked addresses on the request through
`reqwest::ClientBuilder::resolve_to_addrs(host, &addrs)` on a client built per attempt, so the
connection goes to an address the check saw and to nothing else, and a name that answers differently
to the check and to the connection — a rebinding name with a zero time to live — reaches nothing.
`X-Keel-Peer` waits for the peers of M6, since no installation has an identity to name yet.
Functions: `webhook.subscription.list` (`list`, `Level::User`, permission `webhook.manage` for user
and reseller, `.owned()`, columns `name`, `url`, `events` as `text`, `enabled` as `toggle` edited
through `webhook.subscription.toggle`, `createdAt`; row actions `edit`, `deliveries` as a `child`
screen on `webhook.delivery.list` with `subscription_id`, `delete`), `webhook.subscription.get`,
`webhook.subscription.set { id?, name, url, secret?, events, enabled, account? }` (`form`; `url`
under the `url` format and the private-target rule, `events` on the `tags` control with each mask
checked against the grammar `<module>.*` or a dotted name, `secret` a `secret` generated when absent
— 32 bytes from the OS RNG encoded base64url — and answered `once`, a caller-chosen one shorter than
16 bytes answering `422 webhook.error.secret_short` on `secret`), `webhook.subscription.toggle { id,
enabled }`, `webhook.subscription.delete { id }`, `webhook.delivery.list` (`list`, `.owned()` over
the copied `account_id`, scoped by `subscription_id`, columns `eventId`, `type`, `state` as
`status`, `attempt`, `nextAt`, `lastStatus`, `lastError`; row action `retry`),
`webhook.delivery.get`, `webhook.delivery.retry { id }` (`action`, re-queues a `failed` or `dead`
delivery); the menu node `system/webhooks`. The module contributes `AccountLifecycle { on_delete }`
dropping the subscriptions and deliveries of a deleted account, as step 2b demands.

**Touches.** `crates/system/Cargo.toml` (`webhooks = ["notify", ...]`, `reqwest`, `rustls`, `hmac`,
`sha2`), `crates/system/src/webhooks/` (`mod.rs` with the setting, `entity.rs`, `functions/`,
`consumer.rs`, `sender.rs`, `signature.rs`, `masks.rs`, `targets.rs`, `lifecycle.rs`),
`crates/system/migrations/webhooks/`, `crates/system/i18n/webhooks/`, `crates/system/tests/system/`
(`webhooks.rs`, `webhooks_alone.rs`, snapshots), `.github/workflows/ci.yml` (the matrix),
`bin/keel/src/main.rs`.

**Out of scope.** The conformance check (step 5c); the peer subscription created at pairing,
`X-Keel-Peer` and the inbox (M6); the pull feed (M6); a signature check on inbound deliveries (M6),
which reuses `verify`; retention of delivered rows.

**Acceptance.**

- `crates/system/tests/system/webhooks.rs` with an `axum` listener in the test and
  `webhook.private_targets` set:
  `a_subscription_receives_a_signed_delivery_for_a_matching_event_and_the_signature_verifies`;
  `an_event_outside_the_masks_or_outside_the_subscribers_scope_creates_no_delivery` — A1's
  subscription to `note.*` receives A1's `note.saved` and not A2's, the admin's receives both, and
  an event without a subject reaches the admin's alone;
  `a_failed_delivery_follows_the_ladder_and_its_fifth_failure_is_a_problem_a_retry_clears` under the
  frozen clock, each retry carrying a later `t`;
  `the_consumer_resumes_from_its_cursor_after_a_restart` on the `database()` builder;
  `a_subscription_secret_is_answered_once_sealed_at_rest_and_masked_in_audit`;
  `a_private_or_plain_url_is_refused_on_url_unless_webhook_private_targets_is_set`;
  `the_sender_connects_to_the_address_it_checked_and_not_to_one_the_name_answers_later` with a test
  resolver override answering a public address to the check and a loopback one afterwards;
  `a_short_secret_is_refused_on_secret_and_a_generated_one_is_32_bytes`;
  `the_sender_follows_no_redirect` — a listener answering `302` to a second listener leaves the
  delivery failed and the second untouched;
  `deleting_an_account_drops_its_subscriptions_and_deliveries`; `assert_reseller_sees_only_subtree`
  and `assert_user_cannot_edit_foreign_row` for `webhook.subscription.list` and
  `webhook.subscription.set`; the descriptors match their snapshots. The unit tests of
  `signature.rs`:
  `a_signature_verifies_through_verify_slice_and_a_stale_or_tampered_one_is_refused`.
- `crates/system/tests/system/webhooks_alone.rs` boots with `auth` alone; the matrix gains
  `webhooks` and `auth,settings,audit,tasks,cron,notify` as the set without it.

### 5c. webhooks: the conformance check

**Goal.** The webhooks check the design binds to `keel-conformance`, over the module of step 5b:
`suite/webhooks.rs` opens a listener of its own, subscribes as the admin to `note.*` with a secret
it chose, edits a note through the API, receives the delivery within the suite's timeout, verifies
the signature with the rule of `signature.rs` — the window and the comparison — asserts a tampered
body and a stale `t` fail that verification, checks the event id header equals the body's `id`,
answers `200`, then subscribes as the user, edits a note of the user and one of the admin, and
receives the user's alone. `hack/conformance.sh` runs `ctl settings.set key=webhook.private_targets
value=true` before the suite so the suite's loopback listener is reachable.

**Touches.** `crates/conformance/` (`Cargo.toml` with `axum`, `hmac`, `sha2` as dependencies,
`src/suite/webhooks.rs`, `src/suite/mod.rs`), `hack/conformance.sh` (the setting before the suite),
`.github/workflows/ci.yml`.

**Out of scope.** An inbound check (M6); a check of the dead letter, which the frozen clock of the
module tests covers.

**Acceptance.**

- `hack/conformance.sh target/debug/keel` is green with the `webhooks` check printing its lines: the
  signed delivery verified, the tampered and stale variants refused, the user's subscription
  receiving the user's event alone; `hack/check-deps.sh` still holds `keel-conformance` to its
  allowed dependencies.

### 6a. SSE: /api/v1/events with scope filtering and Last-Event-ID

**Goal.** The server-sent event stream of the endpoint map of
[design/10-contracts.md](design/10-contracts.md) at the provisional path M2 recorded, `GET
/api/v1/events`, now confirmed, in the kernel's HTTP transport: the route, the filtering and the
catch-up; the caps, the re-check at keep-alive and `meta.invalidate` are step 6b. The route sits
behind the authentication middleware: a guest is answered `401`, a cookie session or a Bearer token
opens a stream of `text/event-stream` with `Cache-Control: no-store`, a `retry: 3000` line and a
comment every fifteen seconds as keep-alive. Three events here: `data.invalidate`, one frame per
outbox event the principal may see, `id:` the outbox `seq` and `data:` the event in the shape of
[design/07-ecosystem.md](design/07-ecosystem.md); `task.progress` and `notification`, live frames
from the `LiveEvents` broadcast of step 1a — the tasks module publishes `TaskProgress`, `notify`
publishes `NotificationFrame`, the types steps 1a and 4b put into `keel-types` — without an `id`,
since they are not outbox rows; the fourth, `meta.invalidate`, `data: { meta }`, is sent by the
stream itself on a catch-up outside the window here and by the kernel on `touch_acl` from step 6b.
Filtering is per connection by `ScopePolicy` and principal: a `data.invalidate` is sent when the
event's `subject.accountId` lies within `AccountScope::of` the stream's principal, an event without
one to admins and owners alone; a live frame when its `principal` is the stream's principal, or when
its `account` lies within `AccountScope::of` the stream's principal and that principal is at
`Level::Reseller` or above, or when the stream's principal is an admin — so a user sees its own jobs
and notifications alone, a reseller those of its subtree, an admin every one, and a user receives
`data.invalidate` only for its own objects, the M3 exit criterion. Catch-up: a stream opened with
`Last-Event-ID`, or with the query member `after`, which the browser client of step 12 sends on a
reconnect it makes itself, replays the outbox events after that `seq` through the `OutboxSubscriber`
of step 4c, filtered the same way, before going live, within a window of `SSE_REPLAY_WINDOW` events
behind the latest `seq`; an id older than the window is answered with one `meta.invalidate` first
and the stream goes live from the latest `seq`, so the client refetches everything rather than miss
something. The connection registry watches `Readiness`: when the runtime drains, every stream sends
a final `retry: 3000` comment and closes, so axum's graceful shutdown completes and `Running::stop`
never waits on a browser, which would otherwise hold the process until the scripts' and the unit's
SIGKILL and kill the worker before the checkpoint the drain of step 1b protects. Over `su` the
stream belongs to the effective principal. The testkit's `TestHttp` gains `events(who,
last_event_id)`, a reader of frames for tests.

**Touches.** `crates/kernel/src/http/events.rs` and `mod.rs` (the route, the connection registry,
the `Readiness` watch handed to the registry), `crates/testkit/src/http.rs` (`events`),
`crates/kernel/tests/kernel/events.rs`.

**Out of scope.** The per-session cap, `SSE_MAX_CLIENTS`, the re-authentication at keep-alive,
`meta.invalidate` on `touch_acl`, the conformance suite and `hack/prove-binary.sh` (step 6b); the
browser client and the shell (steps 12, 13a and 13b); the `stream` kind and the log tail (M5); the
peers feed (M6); a stream on the control socket.

**Acceptance.**

- `crates/kernel/tests/kernel/events.rs`:
  `a_user_receives_data_invalidate_for_its_own_objects_alone_while_an_admin_receives_every_one` —
  A1, A2 and the admin hold streams, a test module's owned write in A1's account and one in A2's
  produce two outbox events, A1's stream carries one frame with `id` equal to its `seq`;
  `a_reconnect_replays_after_last_event_id_and_an_id_outside_the_window_gets_meta_invalidate`;
  `the_after_query_member_is_read_like_the_header`;
  `a_guest_is_refused_and_a_bearer_token_gets_a_stream`;
  `live_frames_reach_the_addressed_principal_and_the_admins`;
  `live_frames_of_an_account_reach_its_reseller_and_not_a_user_of_another_account`;
  `every_open_stream_closes_when_the_runtime_drains_so_stop_returns_within_a_second`.
- `crates/system/tests/system/tasks.rs` gains
  `task_progress_frames_of_a_job_arrive_on_the_owners_stream_in_order` and `notify.rs` gains
  `a_notification_frame_arrives_on_the_recipients_stream_when_a_message_is_written`.

### 6b. SSE: caps, the keep-alive re-check, meta.invalidate and the checks

**Goal.** What makes the stream of step 6a safe to leave open. `meta.invalidate` gains its kernel
sender: it is published to the principal whose `acl_gen` the kernel bumps through `touch_acl` and to
every stream of an account it bumps by path, beside the catch-up case of step 6a. A session holds at
most `SSE_STREAMS_PER_SESSION` streams, `4` as the plan's value, and a token the same, so two tabs
of one login each keep a stream; the stream beyond the cap closes the oldest of that session, which
is how a reconnect never leaves a stale connection counted. At every keep-alive the stream re-runs
the authenticator chain with its credential and closes on `unauthenticated`, so a revoked session, a
logout or an expired token loses its stream within fifteen seconds, and the tasks and notify frames
are scoped at send time by the principal the last check answered. The global limit of
[design/02-functions.md](design/02-functions.md), which that document groups with the body limit, is
the kernel constant `SSE_MAX_CLIENTS` beside `BODY_LIMIT` in `http/mod.rs`, `1000`, and a member
`sse_max_clients` of `HttpOptions` the tests lower — not a key of `config.yaml`, which
[design/09-operations.md](design/09-operations.md) keeps to what the process cannot start without;
the design names the limit in lower case as a configuration key would be named, and step 15 writes
the constant into that sentence; the stream beyond it is answered `503 unavailable` with
`keel.error.sse_limit`. `keel-conformance` gains `suite/events.rs`, which opens the stream as the
admin and as the user, edits a note of each through the API and asserts each receives its own
`data.invalidate` and the admin both.

**Touches.** `crates/kernel/src/http/events.rs` and `mod.rs` (`SSE_MAX_CLIENTS`,
`HttpOptions.sse_max_clients`, the per-session cap, the re-check at keep-alive),
`crates/kernel/src/keel/source.rs` (`meta.invalidate` on `touch_acl`),
`crates/kernel/src/keel/acl.rs` (the subtree bump), `crates/kernel/i18n/` (`keel.error.sse_limit`),
`crates/kernel/tests/kernel/events.rs`, `crates/conformance/src/suite/events.rs`,
`hack/prove-binary.sh` (the stream answers a frame and survives the stop).

**Out of scope.** The browser client and the shell (steps 12, 13a and 13b); a limit an operator
edits, which the operator plane of M4 may turn into a `SettingDef` of the `keel` module if an
installation ever needs another value.

**Acceptance.**

- `crates/kernel/tests/kernel/events.rs`:
  `a_session_beyond_its_cap_loses_its_oldest_stream_and_the_one_beyond_sse_max_clients_is_refused`
  with `HttpOptions.sse_max_clients` at `6` — five streams of one session leave four open with the
  first closed, and the seventh stream across sessions is refused;
  `a_revoked_session_loses_its_stream_at_the_next_keep_alive` under the frozen clock;
  `meta_invalidate_follows_a_touch_acl_of_the_streams_principal_and_a_bump_of_its_account_subtree`;
  `crates/kernel/src/http/mod.rs`: `sse_max_clients_defaults_to_the_constant`.
- `hack/conformance.sh` prints the `events` lines green; `hack/prove-binary.sh` opens the stream
  with the admin's cookie, edits a note over `ctl` and reads a `data.invalidate` frame naming
  `note.saved` within the timeout, then keeps a stream open through `stop_serve`, and the stop still
  completes on SIGTERM within the timeout.

### 7a. the download kind, the raw route and task.download

**Goal.** The `download` kind of [design/02-functions.md](design/02-functions.md) in the SDK, the
kernel and the tasks module; `keel.export` over it is step 7b. A handler of a `kind: download`
function answers `DownloadOutcome`, either `Body(Download { filename, content_type, body })` with a
byte stream or `Stored { filename, content_type }` for a body written to `call.task().output()`
inside a job, the descriptor's `result` being the `{ filename, contentType }` the design fixes; a
handler of any kind running inside a job hands its output file the same way through the public
`TaskHandle::stored(filename, content_type)`, which `Stored` calls for a `download` handler, and the
worker records the job's result as the handler's answer extended with the `download` member of
`DownloadRef { download: { function: "task.download", params: { id } } }`, a type of `keel-types`,
when the job has a stored file; the kernel serves every `download` function at `GET` and `POST
/api/v1/x/<module>/<name>`, the parameters read as the call route reads them, inside the `api`
router behind `auth::authenticate` as remark 4 places the raw routes, through the whole pipeline,
and streams the body with `Content-Type`, `Content-Disposition: attachment; filename="…"` with a
`filename*` for a name outside ASCII, `Cache-Control: no-store` and `X-Keel-Request-Id`, an error
before the first byte answering the envelope; `/api/v1/call/<name>` and the socket answer `422
validation` with `keel.error.download_route` naming the raw route, and every `download` and `upload`
call raises the audit event whatever its mode, the "always behind a guard and an audit event" of the
design. `task.download { id }` is the tasks module's `kind: download`, `.owned()` over
`task_job.account_id`, streaming `<data_dir>/tasks/<id>.out` under the filename and content type the
job recorded and `404` for a job without a file. `note.export` of step 2a stays a `kind: action`
with `ui.result = task`, so its `ui` block and the `/f/note.export` route step 13a opens stand: its
handler calls `call.task().stored(filename, content_type)` with the name and type of its `format`
after its last note and keeps answering `{ exported }`, so the job's result carries `exported` as
the tests of step 2a assert and the `download` member beside it. `keel ctl` keeps `--out
json|table`, the surface the design fixes; a download from the terminal goes through the raw route
with a token.

**Touches.** `crates/sdk/src/download.rs` (`Download`, `DownloadOutcome`), `crates/sdk/src/task.rs`
(`TaskHandle::stored`), `crates/sdk/src/system.rs` (`TaskSink::stored`),
`crates/sdk/src/function.rs` (the download builder), `crates/types/src/task.rs` (`DownloadRef`) with
`contracts/schema/` and the generated types, `crates/kernel/src/http/raw.rs` and `mod.rs` (the raw
routes of `download` functions), `crates/kernel/src/dispatcher/mod.rs` (the audit rule, the download
refusal on the call route), `crates/kernel/src/socket/mod.rs` (the refusal), `crates/kernel/i18n/`
(the error key), `crates/system/src/tasks/` (`download.rs`, the `download` member of the result),
`products/demo/src/note/export.rs`, `crates/kernel/tests/kernel/download.rs`,
`crates/system/tests/system/tasks_download.rs`, `products/demo/tests/demo/tasks.rs`.

**Out of scope.** `keel.export` and the row threshold (step 7b); the export button (step 14a); the
`upload` kind (step 8b); a `download` over the control socket; retention of output files; the
`stream` kind (M5).

**Acceptance.**

- `crates/kernel/tests/kernel/download.rs` with a test module whose `download` function streams a
  synthetic body:
  `a_download_is_served_and_audited_on_its_raw_route_and_refused_on_the_call_route_and_the_socket`;
  `an_error_before_the_first_byte_answers_the_envelope_and_a_non_ascii_name_gets_filename_star`;
  `a_guest_is_refused_on_the_raw_route`.
- `crates/system/tests/system/tasks_download.rs` with the real `tasks` module and a test job whose
  handler writes to `output()` and calls `stored`:
  `a_jobs_stored_file_ends_in_a_download_ref_that_task_download_serves`;
  `task_download_of_another_accounts_job_or_of_a_job_without_a_file_is_not_found`.
- `products/demo/tests/demo/tasks.rs`:
  `note_export_ends_with_exported_and_a_download_that_task_download_streams_as_the_callers_csv`; the
  `note.export` snapshot is unchanged, the function keeping its kind and screen block; `pnpm
  generate` leaves `DownloadRef` committed.

### 7b. keel.export with the row threshold

**Goal.** The generic export of the lists section of
[design/10-contracts.md](design/10-contracts.md) over the `download` kind of step 7a. `keel.export {
func, query, columns, format }` is a `kind: download` in `mode: read` at `Level::User`,
`.unowned()`, of the `keel` module: `func` must name a `kind: list` function the caller may call —
level, permission, feature and token scope run as its guards would, an unknown or uncallable one
answering `not_found` — `query` is a `ListQuery` whose `page` and `pageSize` are ignored, `columns`
an optional subset of the columns the caller's projected descriptor carries, `422` on `columns`
otherwise, and `format` the `ExportFormat` enum; the handler streams every page of `max_page_size`
of the list through the dispatcher under the caller's principal, so the scope of `ListSource` and
the strip apply as on the screen, and writes `csv` under RFC 4180 with a header row of the column
labels translated through the catalogue in the caller's language by the key rule of
[design/08-frontend.md](design/08-frontend.md), or `json` as one array of the stripped rows; the
file is named `<func>-<date>.<format>`. The row threshold is `.batch_threshold(EXPORT_TASK_ROWS)` on
`keel.export`, the design's rule read against the `total` of the first page, `10000` as the plan's
value: above it, and not already inside a job, the handler calls `Call::spawn_self_as_task()`, a
public SDK addition that spawns the same function with the same parameters under the same principal
through the runner and answers a `TaskRef`, or `unavailable` with `keel.error.tasks_unavailable`
when no runner is composed, and the transport answers the `TaskRef` envelope in place of the stream;
inside the job the handler writes to `output()` with progress per page and answers `Stored`, so the
job's result is a `DownloadRef`. `keel ctl` gains no `--out csv`; an export from the terminal goes
through the raw route with a token. The csv writer, `keel_kernel::export::Csv`, is unit-tested once.
`keel-conformance` gains `suite/export.rs`: the user's csv of `note.list` lacks `accountId`,
`columns: ["accountId"]` answers `422`, and `Content-Disposition` names the file.

**Touches.** `crates/sdk/src/call.rs` (`spawn_self_as_task`), `crates/kernel/src/export/` (`mod.rs`,
`csv.rs`, `json.rs`, `function.rs`), `crates/kernel/src/keel/mod.rs` (`keel.export`),
`crates/kernel/i18n/` (`keel.export.*`, the error keys), `contracts/fixtures/descriptors/` (the
`keel.export` snapshot), `crates/kernel/tests/kernel/export.rs`,
`crates/system/tests/system/tasks_download.rs`, `crates/conformance/src/suite/export.rs`,
`hack/prove-binary.sh`.

**Out of scope.** The export button (step 14a); a third format; retention of output files.

**Acceptance.**

- `crates/kernel/tests/kernel/export.rs` with a test module whose `ListSource` answers synthetic
  rows and carries a reseller-level column:
  `an_export_streams_every_page_under_the_screens_scope_and_strip_with_translated_headers` — A1's
  `csv` lacks the reseller-level column and holds A1's rows alone, the reseller's holds its subtree,
  a `columns` entry A1 cannot see answers `422` on `columns`, and `json` is one array;
  `an_export_above_the_row_threshold_answers_a_task_ref_and_writes_the_file_inside_the_job` with a
  runner double; `keel_export_of_a_function_the_caller_may_not_call_is_not_found`; the unit test of
  `crates/kernel/src/export/csv.rs` quotes commas, quotes and line breaks under RFC 4180.
- `crates/system/tests/system/tasks_download.rs` with the real `tasks` module and a test list of
  twenty thousand synthetic rows:
  `an_export_job_above_the_threshold_ends_in_a_download_ref_that_task_download_serves`.
- `hack/prove-binary.sh` fetches `/api/v1/x/keel/export` for `note.list` as csv with the admin's
  cookie and reads the header row; `hack/conformance.sh` prints the `export` lines green.

### 8a. group editing through ui.multi

**Goal.** The group-edit request of the form-dynamics table of
[design/10-contracts.md](design/10-contracts.md). A `form` function whose `ui.multi` is non-empty
accepts the body `{ ids, fields, mask }` next to its single-row body, told apart by the presence of
`ids` and the absence of the key parameter; the dispatcher checks `mask` against the projected
`ui.multi` — `422` on `mask` with `keel.error.multi_field` for a field outside it — and `fields`
against `mask` and against the schema of each named field, runs `OwnedGuard` over `ids` element by
element as it already does for an array key, and, inside one transaction, for every id reads the
current object through the `get` of `ui.read`, merges the masked `fields` over it and runs the
function's own handler with the merged parameters as an edit, all or nothing, answering `GroupResult
{ updated }`, a type of `keel-types`; the merge over `ui.read` is the plan's choice for how a
per-row form applies to several rows, since the design fixes the request and `ui.multi` alone. Above
`batch_threshold` the group call becomes a job of chunks by the rule of step 2a, each chunk one
group call, and a foreign id fails the whole batch before any job. The freeze checks `ui.multi`
against the schema — every name exists, none is the key, none is `secret`. The demo's `note.edit`
declares `.multi(["kind", "due", "tags"]).batch_threshold(10)`, `due` among them because the handler
demands a `due` for `kind: todo` and a group edit to `todo` without one would be refused on every
row.

**Touches.** `crates/kernel/src/dispatcher/` (`group.rs`, `mod.rs`),
`crates/kernel/src/loader/freeze.rs` (`ui.multi`), `crates/types/src/task.rs` (`GroupResult`) with
`contracts/schema/` and the generated types, `crates/kernel/i18n/` (the error key),
`products/demo/src/note/functions.rs`, `products/demo/tests/demo/note.rs`,
`crates/kernel/tests/kernel/group.rs`.

**Out of scope.** The multi-row form in the browser (step 14b); the upload kind (step 8b).

**Acceptance.**

- `crates/kernel/tests/kernel/group.rs` with a test form carrying `ui.multi`:
  `a_group_edit_merges_the_masked_fields_over_ui_read_into_every_row_inside_one_transaction`;
  `a_mask_outside_ui_multi_or_a_field_outside_the_mask_answers_422_on_the_offending_path`;
  `a_group_edit_with_one_foreign_id_is_not_found_and_changes_nothing`;
  `a_group_edit_above_the_threshold_becomes_a_job_of_chunks` with the runner double;
  `ui_multi_naming_the_key_a_secret_or_an_unknown_field_fails_freeze`.
- `products/demo/tests/demo/note.rs`:
  `note_edit_as_a_group_sets_kind_due_and_tags_of_the_selected_notes_and_becomes_a_task_above_ten` —
  two memos become `todo` with one `due`; `pnpm generate` leaves `GroupResult` committed.

### 8b. the upload kind over a guarded raw route

**Goal.** The `upload` kind of [design/02-functions.md](design/02-functions.md) over the raw route
of remark 4, in the kernel, the SDK, `keel-types` and the testkit; its first consumer is step 8c. A
`kind: upload` function's parameters arrive as `multipart/form-data` on `POST
/api/v1/x/<module>/<name>`: a `params` part with the JSON of the other fields and one part per field
whose `x-ui.control` is `file`; such a field carries `x-keel.file { maxBytes, types }`, a new member
of `KeelAttrs` the kernel enforces and the freeze demands on every `control: file` field, and the
kernel checks each part's `Content-Type` against `types` and its size against `maxBytes` while
reading it, answering `422` on the field's path with `keel.error.file_type` or
`keel.error.file_too_large`, then places the part into the parameters — the text of a `string`
field, checked as UTF-8, or base64 for a field whose schema says `contentEncoding: base64` — and
runs the ordinary pipeline in `mode: write` with `Idempotency-Key` from the header, the audit event
as for every upload, and the multipart limit the sum of the fields' `maxBytes` plus `64 KiB` for
`params`, in place of the 1 MiB of a JSON body — the plan's one exception to the body-limit
constant, which step 15 writes into the three sentences of the design that state it. The raw routes
sit inside the `api` router behind `auth::authenticate`, so the `Origin` and `Sec-Fetch-Site` check
of M1 applies to the multipart `POST` as to every browser write. `/api/v1/call/<name>` answers `422`
`keel.error.upload_route`; over the socket the field value comes inline, the owner's path for `ctl`,
and the socket applies `maxBytes` to the inline value as the raw route does to the part. An `upload`
carries a `Form` screen block, so the shell renders it like a form and posts multipart — the plan's
choice against the `ui` sentence of the descriptor table of
[design/02-functions.md](design/02-functions.md), which lists `upload` among the kinds without a
screen, and the `download · upload · stream` row of the form-dynamics table of
[design/10-contracts.md](design/10-contracts.md): an upload with a second field and a read-only
parameter needs the pages and conditions of a form, and the shell has one form renderer; the commit
message names the two sentences and step 15 amends them. The testkit's `TestHttp` gains `upload(who,
path, params, files)`.

**Touches.** `crates/kernel/src/http/raw.rs` (multipart), `crates/kernel/src/dispatcher/validate.rs`
(the file checks), `crates/kernel/src/socket/mod.rs` (`maxBytes` inline),
`crates/kernel/src/loader/freeze.rs` (`x-keel.file`, the `Form` block on an `upload`),
`crates/types/src/descriptor.rs` (`FileAttrs`) with `contracts/schema/` and the generated types,
`Cargo.toml` (`axum` with the `multipart` feature), `crates/sdk/src/multipart.rs` (`FileAttrs`
re-export and the builder helper), `crates/sdk/src/function.rs` (the upload builder),
`crates/kernel/i18n/` (the error keys), `crates/testkit/src/http.rs` (`upload`),
`crates/kernel/tests/kernel/upload.rs`.

**Out of scope.** The brand logo (step 8c); the file control's submit in the browser (step 14c); a
streamed upload to disk beyond `maxBytes`; a free-form raw route (M5).

**Acceptance.**

- `crates/kernel/tests/kernel/upload.rs` with a test upload of a text file:
  `a_multipart_upload_on_its_raw_route_lands_the_file_in_the_parameters_and_the_audit_record`;
  `a_part_above_max_bytes_or_of_another_type_or_not_utf8_is_refused_on_the_field`;
  `a_control_file_field_without_x_keel_file_fails_freeze`;
  `the_call_route_refuses_an_upload_and_the_socket_takes_the_field_inline_under_max_bytes`;
  `a_cross_site_multipart_post_is_refused_403`; `an_upload_descriptor_carries_its_form_block`; `pnpm
  generate` leaves `FileAttrs` committed.

### 8c. the brand logo as the first upload

**Goal.** The brand logo of the M3 scope list as the first `upload`, over step 8b:
`settings.brand.logo { host, logo }`, `Level::Admin`, `.unowned()`, `logo` a string with `control:
file` and `x-keel.file { maxBytes: 65536, types: ["image/svg+xml"] }`,
`Form::new("settings.brand.logo.title").page("main", ["host",
"logo"]).field("host").readonly_if("$screen.params.host")`; the server sanitises by refusing, the
plan's choice over rewriting, so an operator never gets a logo other than the one uploaded:
`check_svg` parses with `ParsingOptions { allow_dtd: false, .. }`, so any document type is refused
whole, compares namespaces and not local names alone — it refuses a root element outside the
namespace `http://www.w3.org/2000/svg`, any element outside that namespace, which is what makes
`foreignObject` content refused whether the element is named or not, and any attribute in a
namespace other than none, `http://www.w3.org/1999/xlink` and
`http://www.w3.org/XML/1998/namespace`, so a document that smuggles an `iframe` or a `form` under
another namespace fails the check itself and not only the `default-src 'none'` of the served page —
and grows to refuse `script` and `foreignObject` elements, any attribute whose name starts with
`on`, an `href` or `xlink:href` that is not a fragment reference, an `attributeName` attribute
naming `href`, `xlink:href` or a name starting with `on` — the `animate` and `set` path to the
refused attributes — a `style` element or attribute containing `url(` or `@import`, and an
`xml-stylesheet` processing instruction, each with its reason in `params`; the accepted document is
stored on `settings_brand.logo` and served from `/brand/logo.svg` for that host as in M2.
`settings.brand.set` drops `logo` from its parameters and pages, the column staying under the
additive rule, `settings.brand.list` gains the column `logo` as `boolean` and the row action `{ id:
"logo", role: "upload", screen: "settings.brand.logo", params: { host: "$id" } }`, and the M2 check
of `hack/prove-binary.sh` moves to the upload over `ctl`.

**Touches.** `crates/system/src/settings/brand/` (`functions.rs`, `svg.rs`, `mod.rs`),
`crates/system/i18n/settings/`, `crates/system/tests/system/settings_brand.rs` and snapshots,
`hack/prove-binary.sh`.

**Out of scope.** The file control and the white-label screen in the browser (step 14c); the brand
text field's return.

**Acceptance.**

- `crates/system/tests/system/settings_brand.rs`:
  `a_plain_svg_logo_is_served_for_its_host_and_an_unsafe_one_is_refused_naming_the_reason` through
  `TestHttp::upload`, each refusal naming its reason;
  `settings_brand_set_no_longer_takes_a_logo_and_leaves_a_stored_one_in_place`; the unit tests of
  `svg.rs` gain `an_animate_or_set_element_targeting_href_or_an_event_attribute_is_refused`,
  `an_element_or_attribute_outside_the_svg_xlink_and_xml_namespaces_is_refused` and
  `a_document_type_is_refused_whole`; the `settings.brand.*` snapshots follow.
- `hack/prove-binary.sh` refuses `ctl settings.brand.logo host=panel.example
  logo='<svg…><script/></svg>'` and accepts a plain one, then fetches it from `/brand/logo.svg` with
  `Host: panel.example`.

### 9a. TLS material, bind_tls and https-capable clients

**Goal.** The first half of the TLS section of [design/09-operations.md](design/09-operations.md),
brought forward from the M1 and M2 remarks that kept the plain listener: everything the switch of
step 9b needs, landed while the runtime still binds plain, so that the clients can speak https
before the server does and no partial landing reddens CI. `crates/kernel/src/tls.rs` holds the
material: `tls.cert` and `tls.key` name an operator's PEM pair, one without the other failing the
start naming the missing key; with both empty the kernel uses `<data_dir>/tls/cert.pem` and
`key.pem` and generates them with `rcgen` at first start when they are missing, added at a pinned
major as the design names it: `tls.rs` creates `<data_dir>/tls` with mode `0700`, writes an ECDSA
P-256 key with mode `0600` and a certificate for ten years whose subject alternative names are the
host name, `localhost`, `127.0.0.1`, `::1`, the addresses the host name resolves to through std's
`ToSocketAddrs`, so no interface crate is needed, and the `listen` address when it is neither
unspecified nor loopback, so the installer's probe of that address and the operator's first visit by
it match; each PEM goes to a temporary file in `<data_dir>/tls` and is renamed into place, so a
reader polling `/readyz` with `--cacert` never sees a partial file; the material answers its
certificate's SHA-256 fingerprint for the log line of step 9b. A non-empty `tls.acme` fails the
start with `keel.error.acme_unsupported` until the operator plane of M4, the empty string of the
sample config being unset, so `hack/check-install.sh`'s `migrate` of the sample still passes.
`Http::bind_tls(addr, TlsMaterial)` binds through `axum-server` with its rustls feature that
installs no provider, added to `[workspace.dependencies]` at a pinned major because it serves an
axum router with graceful shutdown and connection info over rustls in one call, and passes a
five-second grace to `axum_server::Handle::graceful_shutdown`, beside the drain close of step 6a;
`Served::url` answers the scheme of its binding, so `Http::bind` and `bind_local` keep the plain
listener for the kernel tests and the testkit, whose `TestHttp` stays on the in-process router. The
clients learn https without using it yet: `keel-conformance` and `keel-dev descriptors dump` gain
`--cacert` and add the certificate to their `reqwest` client; the `http()` and `start_serve` helpers
of `hack/prove-binary.sh`, `hack/conformance.sh`, `hack/descriptors.sh` and `hack/e2e.sh` are
parameterised on a `scheme` variable that stays `http` and on an optional `cacert` argument;
`frontend/e2e/playwright.config.ts` passes `--ignore-certificate-errors-spki-list=$KEEL_E2E_SPKI` to
chromium when `KEEL_E2E_SPKI` is set and keeps `ignoreHTTPSErrors` off, so any other certificate
still fails the suite; `products/demo/tests/demo/runtime.rs` and `crates/keel/tests/runtime.rs` move
their `http` helper to a `reqwest` client that trusts a handed certificate and keep asserting
`http://` until step 9b.

**Touches.** `Cargo.toml` (`axum-server`, `rcgen`), `crates/kernel/Cargo.toml` (`axum-server`,
`rcgen`, `rustls`), `crates/kernel/src/tls.rs` (the material, the generation, the atomic write, the
fingerprint), `crates/kernel/src/http/mod.rs` (`bind_tls`, `Served::url` by scheme, the grace),
`crates/kernel/src/config.rs` (the `acme` refusal, the pair check),
`crates/kernel/tests/kernel/tls.rs`, `crates/keel/Cargo.toml`, `bin/keel/Cargo.toml` and
`products/demo/Cargo.toml` (`reqwest` and `rustls` as dev-dependencies),
`crates/keel/tests/runtime.rs`, `products/demo/tests/demo/runtime.rs`, `crates/conformance/src/`
(`--cacert`), `crates/dev/src/descriptors.rs` (`--cacert`), `hack/prove-binary.sh`,
`hack/conformance.sh`, `hack/descriptors.sh`, `hack/e2e.sh` (the `scheme` and `cacert` parameters),
`frontend/e2e/playwright.config.ts`.

**Out of scope.** The runtime binding TLS, the installer, `hack/check-install.sh` and the scripts'
switch to https (step 9b); `trusted_proxies` (step 9c); ACME and the certificate button in the
settings (M4); the TLS expiry problem, which needs the certificate parsed and lands with the same
operator plane; mTLS between nodes (M8); `sd_notify` and the watchdog (M4).

**Acceptance.**

- `crates/kernel/src/tls.rs`:
  `a_missing_pair_is_generated_for_host_listen_address_and_loopback_under_0700_with_a_0600_key`;
  `a_configured_cert_without_its_key_fails_the_start_naming_tls_key`;
  `an_unreadable_pair_under_data_dir_is_regenerated`; `a_pem_is_written_whole_or_not_at_all`;
  `crates/kernel/src/config.rs`:
  `a_non_empty_acme_value_fails_the_start_until_it_is_supported_and_the_empty_string_is_unset`.
- `crates/kernel/tests/kernel/tls.rs`:
  `bind_tls_serves_healthz_to_a_client_trusting_the_certificate_and_refuses_one_that_does_not` and
  `bind_tls_stops_within_its_grace_with_a_stream_open`; `crates/kernel/tests/kernel/http.rs` and
  `crates/testkit/tests/harness.rs` keep their `http://` assertions on the plain binding.
- `hack/prove-binary.sh`, `hack/conformance.sh`, `hack/descriptors.sh` and `hack/e2e.sh` are green
  over `scheme=http` with the parameterised helpers; `keel-conformance --cacert` and `keel-dev
  descriptors dump --cacert` accept the flag against a plain server and read the file.

### 9b. the installation listens on TLS alone

**Goal.** The switch: from this step the installation listens on TLS from its very first start, as
the TLS section of [design/09-operations.md](design/09-operations.md) says. `keel serve` binds
`listen` through `Http::bind_tls` with the material of step 9a, and the ring provider of the
workspace's `rustls` pin is installed once at start by the runtime — the line steps 4d and 5b
carried in their modules' `start` until here, removed from `crates/system/src/notify/` and
`crates/system/src/webhooks/` where it exists, and what `keel-conformance` already does on the
client side; `keel serve` logs the certificate's SHA-256 fingerprint at every start so an operator
can recognise it in a browser. There is no plain listener and no redirect port, the plan's choice
against the redirect sentence of the design, which step 15 amends, and the choice answers the case
the sentence guards against: the design fears a first visit over plain HTTP by IP address that shows
a login page whose `__Host-` cookie the browser then refuses to store, leaving the operator unable
to log in without knowing why; with a single TLS listener a plain request gets no page at all — the
browser reports a protocol error and the operator corrects the scheme — `deploy/install.sh` prints
`https://` addresses and the fingerprint, and the operator plane of M4 brings ACME, after which a
name and not an address is what an operator types; a second port would need a second address the
single `listen` key of the design does not carry, and a redirect on the same port would need an
accept loop of our own in front of `axum-server`. Every client of the repository speaks https from
here: `hack/prove-binary.sh`, `hack/conformance.sh`, `hack/descriptors.sh` and `hack/e2e.sh` set
`scheme=https` and `--cacert "$site/lib/tls/cert.pem"`, the `tls/` directory of the `data_dir` the
scripts configure, and the readiness loop of `start_serve` treats a missing certificate file, which
the first `serve` writes, as not ready; `keel-conformance` and `keel-dev descriptors dump` are run
with `--cacert`; `products/demo/tests/demo/runtime.rs` and `crates/keel/tests/runtime.rs` assert
`https://`. The browser suite trusts the generated certificate rather than ignoring certificate
errors: `hack/e2e.sh` exports `KEEL_E2E_URL=https://127.0.0.1:<port>`, `KEEL_E2E_SPKI`, the base64
SHA-256 of the certificate's subject public key computed by the script with node's
`crypto.X509Certificate`, which the browser suite has on the path already, rather than a tool the
runner need not carry, and `NODE_EXTRA_CA_CERTS=$site/lib/tls/cert.pem` for the fixture's own
`fetch`. `deploy/install.sh` keeps creating `/var/lib/keel/tls` and generates nothing, adds `curl`
to the `need` list of `check_host` and installs it with `apt_install curl ca-certificates` when
absent, waits for `/readyz` with `curl --cacert /var/lib/keel/tls/cert.pem` in place of its raw
socket, prints `https://` addresses and the fingerprint, and shows the API example over https. The
`__Host-` cookie of M1 is accepted from any origin from here, which retires the localhost rule.

**Touches.** `crates/keel/src/runtime.rs` (the provider, the material, `bind_tls`, the fingerprint
line), `crates/system/src/notify/` and `crates/system/src/webhooks/` (the provider line removed),
`crates/keel/tests/runtime.rs`, `bin/keel/tests/binary.rs`, `products/demo/tests/demo/runtime.rs`,
`hack/prove-binary.sh`, `hack/conformance.sh`, `hack/descriptors.sh`, `hack/e2e.sh`,
`deploy/install.sh`, `hack/check-install.sh`, `frontend/libs/keel/testing/src/playwright.ts`.

**Out of scope.** A plain redirect port; `trusted_proxies` (step 9c); ACME (M4); `deploy/README.md`
(step 15).

**Acceptance.**

- `crates/keel/tests/runtime.rs`:
  `the_runtime_listens_on_tls_alone_and_a_plain_request_gets_no_http_answer` — a `reqwest` client
  trusting the generated certificate reads `/healthz`, one that does not is refused, and a plain
  `http://` request fails at the protocol;
  `the_runtime_installs_the_ring_provider_before_the_modules_start`; `bin/keel/tests/binary.rs`
  reads `/healthz` from the built binary over TLS with the certificate the site generated and finds
  the fingerprint in the log.
- `hack/prove-binary.sh`, `hack/conformance.sh`, `hack/descriptors.sh`, `hack/check-install.sh` and
  `hack/e2e.sh` are green over https in the job `check`, the browser suite with `KEEL_E2E_SPKI` set
  and `ignoreHTTPSErrors` off; `deploy/install.sh` prints the https address and the fingerprint at
  the end of an installation, and `hack/check-install.sh` asserts that `need` names `curl`, the
  `--cacert` wait and the https form of the printed address.

### 9c. trusted_proxies

**Goal.** The `trusted_proxies` key of the configuration file of
[design/09-operations.md](design/09-operations.md). The list of `config.yaml` is parsed as CIDRs at
load, a bad entry failing the start naming it; `HttpOptions` gains `trusted_proxies: Vec<IpNet>`,
filled by the runtime from `Config`, since `http/auth.rs::client(headers, extensions)` has no access
to the configuration and `Shared` carries `HttpOptions` alone; `client` reads the client address
from `X-Forwarded-For` only when the peer address lies inside one of them — the rightmost entry that
is not itself a trusted proxy — and, under the same condition, the host for the brand patch and for
the `Origin` comparison of `cross_site` comes from `X-Forwarded-Host`, the plan's choice for a
header the design does not name: the host is derived by `host_name` in `http/brand.rs`, which
`boot.rs` imports, so the forwarded host lands there once, and the CSRF check reads it too because a
reverse proxy that rewrites `Host` to the upstream would otherwise have every browser `POST` refused
`403 keel.error.cross_site`, the opposite of the key's purpose. The two headers are input from a
proxy and are read defensively: an entry that does not parse as an IP address, or a header whose
every entry is a trusted proxy, leaves `Client.ip` at the peer address and is logged once per
connection at debug; `X-Forwarded-Host` is taken only when it is a single `host[:port]` authority —
no scheme, no path, no comma — and the `Host` header stands otherwise; the peer address itself is
never replaced by anything from an untrusted peer. The IP rules, the lockout and the rate limit of
`auth` and the audit record read `Client.ip` already and follow.

**Touches.** `crates/kernel/Cargo.toml` (`ipnet`), `crates/kernel/src/config.rs` (`trusted_proxies`
as CIDRs), `crates/kernel/src/http/mod.rs` (`HttpOptions.trusted_proxies`),
`crates/kernel/src/http/auth.rs` (`client`, `cross_site`), `crates/kernel/src/http/brand.rs`
(`host_name`), `crates/keel/src/runtime.rs` (the options from `Config`),
`crates/kernel/tests/kernel/http.rs`, `crates/system/tests/system/auth_guards.rs`,
`crates/system/tests/system/settings_brand.rs`.

**Out of scope.** A proxy protocol below HTTP; the peers behind a proxy (M6).

**Acceptance.**

- `crates/kernel/src/config.rs`:
  `trusted_proxies_are_parsed_as_cidrs_and_a_bad_entry_fails_the_start_naming_it`.
- `crates/kernel/tests/kernel/http.rs`:
  `x_forwarded_for_is_honoured_from_a_trusted_proxy_alone_and_the_rightmost_untrusted_entry_wins`;
  `a_same_site_post_is_admitted_by_the_forwarded_host_behind_a_trusted_proxy_alone`;
  `a_malformed_forwarded_for_or_host_falls_back_to_the_peer_and_the_host_header`.
- `crates/system/tests/system/auth_guards.rs`:
  `an_ip_rule_a_lockout_and_the_audit_record_see_the_forwarded_address_behind_a_trusted_proxy`;
  `crates/system/tests/system/settings_brand.rs`:
  `x_forwarded_host_from_a_trusted_proxy_selects_the_brand_patch`.

### 10. OwnLate projects and keel-dev i18n pull and missing

**Goal.** The translation pipeline of the bundles section of
[design/08-frontend.md](design/08-frontend.md), deferred from M2 with the projects that did not
exist. Two OwnLate projects in the GlobalArt workspace, `keel-core` for the kernel, the system
modules, the frontend runtime and the `phx.*` keys, and `keel-demo` for the demo, each with `en` as
source and `ru` as target, are created and seeded from the committed dictionaries, and
`contracts/i18n.json` records them: the project ids and the directories a key prefix lands in —
`keel.*`, `phx.*` and `menu.system` in `crates/kernel/i18n`, `auth.*` in `crates/system/i18n/auth`,
and so on for every module and for `note.*` and `product.demo.*` in `products/demo/i18n`. Three
subcommands of `keel-dev`, which stays on `keel-types` alone: `i18n extract --dump contracts/dump
--out <file>` collects every key the registry emits without a manual list — the derived keys of
every descriptor of the admin dump by the table of the design, fields, enum values, actions, pages,
columns and column values, hints and placeholders where `x-ui` names them, the explicit titles,
empty states and error keys the descriptors carry, the menu labels of `boot.json`, and every key of
the dump's `i18n/en.json`, so the `keel.error.*` keys of handlers and the shell's `keel.shell.*` are
covered by the dictionaries that carry them — with the `en` text of each as source, in the flat JSON
the OwnLate import takes, and `--push` uploads it to the named project with `OWNLATE_URL` and
`OWNLATE_TOKEN` from the environment; `i18n pull --out .` fetches the latest release of every
project of `contracts/i18n.json` and writes each language nested by key into the directory its
prefix names, prettified as the repository keeps them, so `pull` on a repository in sync changes
nothing; `i18n missing --dump contracts/dump [--langs en,ru]` compares the keys `extract` derives
with the catalogues of the dump for every language, a derived key satisfied by its own entry or the
shared `keel.<tail>` as the freeze reads it, prints every key without a translation per language and
exits non-zero on any. `pull` runs at build time in the sense the design means and the offline rule
allows: the pulled dictionaries are committed, so a build never reaches out, and
`.github/workflows/build.yml` runs `pull` before its build when `OWNLATE_TOKEN` is present and fails
on a difference, the plan's choice for a server that cannot fetch translations at runtime; the job
`check` runs `missing` after `hack/descriptors.sh`, the CI step the design asks for, which is what
makes a `ru` gap a red build where the freeze checks `en` alone.

**Touches.** `crates/dev/src/i18n/` (`mod.rs`, `keys.rs`, `extract.rs`, `pull.rs`, `missing.rs`),
`crates/dev/src/main.rs`, `crates/dev/Cargo.toml`, `contracts/i18n.json`, `.github/workflows/ci.yml`
(the `missing` step), `.github/workflows/build.yml` (the `pull` step), the dictionaries `pull`
rewrites where the projects differ from the repository; the OwnLate workspace outside the
repository.

**Out of scope.** A language beyond `en` and `ru`; `keel-dev check`, `new-module`, `new-product`,
`migrate create` and `docs`; translations fetched at runtime; a project per plugin (M7).

**Acceptance.**

- `crates/dev/src/i18n/`:
  `extract_derives_the_keys_of_a_dump_by_the_key_table_and_keeps_its_explicit_ones` over
  `contracts/dump/admin/`;
  `missing_reports_a_key_absent_from_ru_and_accepts_a_derived_key_the_shared_fallback_satisfies`;
  `pull_splits_a_release_by_prefix_into_the_dictionaries_and_leaves_a_synced_tree_unchanged` against
  a canned release.
- `cargo run -p keel-dev -- i18n missing --dump contracts/dump` is green in the job `check` after
  `hack/descriptors.sh`, and a test dictionary with one `ru` key removed makes it exit non-zero
  naming the key.
- `keel-core` and `keel-demo` exist in OwnLate with every key of `extract` and the `ru` translations
  of the repository, and `i18n pull --out .` on the closing commit of the step leaves `git status`
  clean; `hack/check-deps.sh` still holds `keel-dev` to `keel-types`.

### 11. phoenix-ui batch 2 in the globalart monorepo and its Nexus release

Done in another repository: batch 2 shipped as `@globalart/phoenix-ui@0.3.0` and then `0.3.1`,
which fixes the time picker so it commits once; step 14a pins `0.3.1`.

**Goal.** The M3 row of the phoenix-ui batch table of
[design/08-frontend.md](design/08-frontend.md), made in `platform/frontend/libs/phoenix-ui` of the
globalart monorepo under its conventions, as batch 1 was in M2, and released as
`@globalart/phoenix-ui@0.3.0`; this step lives in another repository, and its Keel-side change is
the pin, which step 14a makes as its first commit. `phx-datetime-field`
(`src/lib/date/datetime-field.component.ts`): a `ControlValueAccessor` over an RFC 3339 string in
the zone the consumer hands it, a date picker and a time field side by side. `select` and `number`
types in `PhxCellEditService` (`src/lib/table/cell-edit-popup.component.ts`):
`PhxCellEditOptions.type`, the input `edit(anchor, options)` takes, gains `'select'` with `options`
and `'number'` with `min`, `max` and `step` next to the text input it has. `columnOrder` on
`phx-list-table` and `phx-data-table`: an input of column keys that orders the rendered columns and
hides those absent, a header drag that reorders, and the output `columnOrderChange`. The searchable
remote mode of `phx-column-filter`: a `remote` input with `searchChange`, `loading` and `more`, the
surface of the remote select. The accessible name of a control inside `phx-form-row`, the heading
level of the table's empty state under a page title, and `stopPropagation` of the native `search`
event in `phx-searchbar`, the three findings the M2 renderers recorded. Nothing beyond the row: the
label a nullable number shows for its null is a matter of `PHX_LABELS`, which step 14c scopes in
`@globalart/keel`, so the batch table, the contract with the other repository, stands as written.
Stories with play functions next to each; the labels join `PHX_DEFAULT_LABELS` where a component
gains one; `libs/phoenix-ui/package.json` is bumped to `0.3.0` and the tag `phoenix-ui-v0.3.0` runs
`phoenix-ui-publish.yml`.

**Touches.** In the globalart monorepo:
`platform/frontend/libs/phoenix-ui/src/lib/date/datetime-field.component.ts` and its story,
`src/lib/table/cell-edit-popup.component.ts`, `list-table.component.ts`, `data-table.component.ts`,
`column-filter.component.ts` and their stories, `src/lib/form/form-section.component.ts`, where
`phx-form-row` is declared, `src/lib/navigation/searchbar/`, `src/lib/labels/labels.ts`,
`src/public-api.ts`, `libs/phoenix-ui/package.json`, the library's `README.md`; the tag
`phoenix-ui-v0.3.0`. In Keel: nothing; the pin is the first commit of step 14a.

**Out of scope.** Batch 3 (`@globalart/phoenix-ui/code`, `phx-log-view`, `phx-key-value-field`,
`phx-transfer-list`, the chart additions; M5); persistence of column order, the link between
selection and bulk actions and export, which stay in `@globalart/keel`; an input phoenix-ui does not
need for the empty label of a number field.

**Acceptance.**

- In the globalart CI (`phoenix-ui-tests.yml`): `typecheck`, `lint`, `format`, `build-storybook` and
  `test-storybook` are green with the new stories: `DatetimeField/Default` picks a date and a time
  and emits one RFC 3339 value, `CellEdit/Select` and `CellEdit/Number` open the popup with their
  types and resolve the chosen value, `ListTable/ColumnOrder` reorders by drag and emits
  `columnOrderChange`, `ColumnFilter/Remote` types a query and receives `searchChange`, and the
  searchbar story asserts the `search` event stops at the component.
- `pnpm view @globalart/phoenix-ui@0.3.0 --registry
  https://nexus.globalart.hu/repository/npm-hosted/` with a token answers the version, and the
  published `package.json` carries no `main` pointing into `src/`.

### 12. @globalart/keel/client: the real SSE subscription and data.invalidate

**Goal.** The SSE bullet of the `/client` section of [design/08-frontend.md](design/08-frontend.md)
over the stream of steps 6a and 6b, replacing the stub of M2 in
`frontend/libs/keel/client/src/events.ts` without changing what the renderers consume.
`KeelEvents.subscribe()` opens an `EventSource` on `KEEL_EVENTS_PATH`, confirmed as
`/api/v1/events`, with credentials; the shell calls it once the boot answers a principal and
`close()` on logout and on `unauthenticated`. The four frames are typed from the generated types:
`taskProgress$` as `Observable<TaskProgress>`, `notification$` as `Observable<NotificationFrame>`,
`dataInvalidate$` as `Observable<Event>` and `metaInvalidate$`; a frame that does not parse into its
type is dropped and counted on a signal. Catch-up is the client's part of the design's
`Last-Event-ID` rule: the service keeps `lastEventId` from every frame that carries an id, relies on
the browser to resend `Last-Event-ID` on the reconnects `EventSource` makes itself, and, when it
reconnects on its own after a `closed` state with the doubling backoff of M2, opens
`${path}?after=${lastEventId}`, so no reconnect loses the position; `meta.invalidate` feeds the same
path a changed `X-Keel-Meta-Version` does, `KeelClient.metaChanged`, on which `KeelDescriptors`
drops its cache and the shell refetches `/boot`, the handling that exists since M2.
`data.invalidate` reaches the lists as before through `invalidations` of the list renderer, which
now debounces a burst on one list into one refetch within `INVALIDATE_DEBOUNCE_MS`, the plan's
choice against a refetch per event of a batch; the `state` signal gains `lastEventId` and a
`reconnects` counter for the drawer of step 13a. The mock event source of `@globalart/keel/testing`
gains `emit(type, data, id)` and `fail()` so specs and stories drive frames.

**Touches.** `frontend/libs/keel/client/src/` (`events.ts`, `events.spec.ts`, `client.ts` for
`metaChanged` from the frame, `public-api.ts`), `frontend/libs/keel/renderer/src/list/invalidate.ts`
and `list.spec.ts`, `frontend/libs/keel/shell/src/shell.ts` (subscribe after boot, close on logout),
`frontend/libs/keel/testing/src/storybook.ts` (`QuietEventSource` gains `emit` and `fail`),
`frontend/e2e/events.spec.ts`.

**Out of scope.** The progress modal, the task drawer and the notification centre (steps 13a and
13b); the export call of the client (step 14a); a stream over a Bearer token in the browser.

**Acceptance.**

- `events.spec.ts` with the fake source: "the subscription opens with credentials after a boot with
  a principal and closes on logout"; "frames are dispatched by type and typed, and an unparsable one
  is dropped and counted"; "the last event id is kept and a self-made reconnect passes it as after";
  "meta.invalidate emits metaChanged, drops the descriptor cache and refetches boot once"; "the
  backoff doubles from one second to thirty and resets on open".
- `list.spec.ts`: "a burst of data.invalidate frames matching invalidate_on refetches the table
  once" and "a frame outside the masks refetches nothing".
- `frontend/e2e/events.spec.ts`: `alice` has `/f/note.list` open; the spec, through the `keelApi`
  fixture logged in as `root`, calls `note.edit` on one of her notes with `account` set to her
  account over the API, and the new title appears without a reload; a note of `res1`'s account
  edited the same way changes nothing on her screen.

### 13a. shell: the progress modal, the download and the task drawer

**Goal.** The task half of the `/shell` bullets of [design/08-frontend.md](design/08-frontend.md)
that M3 brings: `ui.result = task` in the browser and the rail's tasks, on
`@globalart/phoenix-ui@0.2.2`. The progress modal: `KeelTasks.follow(ref, opener)` in `/renderer`,
called by the action runner, the form and the wizard when an answer is a `TaskRef`, opens
`PhxModalService.openProgress` with the function's title, `cancelable`, and drives it from
`taskProgress$` — `done/total` as the value, `message` translated as the status — until a frame
reports `done`, `failed` or `cancelled`; `cancel` posts `task.cancel { id }`; `done` closes the
modal and hands back to the opener, which reloads its table or navigates as `redirect` would, and a
result carrying the `download` member of `DownloadRef` fetches the named download's raw route
through `KeelClient.download(func, params)`, a `fetch` with credentials that hands a blob to the
browser through an object URL and an anchor, or shows the envelope's error; `failed` shows the error
with the request id on a toast; a modal that misses frames for `TASK_POLL_MS` polls `task.get`, so a
stream that dropped never leaves it hanging. The action route coerces its query values by the
descriptor's schema before the runner posts them — a `string` that parses as JSON lands typed on an
`integer`, `number`, `boolean` or `array` field and stays a string otherwise, the rule `from_query`
applies on the server for GET — in `screen.ts` (`paramsOf(query, schema)`), which is what lets
`/f/note.export?format=csv&pace=300` of step 2a post `pace` as a number. The task drawer: the rail
item `tasks` shows the count of the principal's `queued` and `running` jobs as its badge, refreshed
from frames and from `task.list` at open, and opens a `phx-drawer` listing the recent jobs of
`task.list` with state, progress and time; a job opens its detail in the same drawer — the row of
`task.get` on `phx-prop-list`, a `phx-progress-bar`, the `logTail` in a read-only `phx-code-field`
until `phx-log-view` arrives in M5, refreshed on every frame of that job, and the cancel button
while it runs; the progress modal offers "details" that opens the drawer on its job. Dictionaries:
`keel.shell.tasks.*` in the kernel's `en` and `ru`.

**Touches.** `frontend/libs/keel/renderer/src/` (`tasks/` with `follow.ts`, `download.ts`,
`tasks.spec.ts`; `screen.ts` and `screen.spec.ts` for the coercion; `action/runner.ts`,
`form/form-screen.component.ts`, `wizard/wizard.component.ts` for the `TaskRef` answer),
`frontend/libs/keel/client/src/` (`download.ts`, `tasks.ts`), `frontend/libs/keel/shell/src/`
(`rail/rail.component.ts`, `tasks/` with the drawer, specs and stories), `crates/kernel/i18n/` (the
keys), `frontend/e2e/tasks.spec.ts`, `hack/e2e.sh` (the paced export before the browser specs).

**Out of scope.** The notification centre, the problem badges, the home dashboard and the about
drawer (step 13b); the export button, the multi-row form, the file control and the white-label
screen (steps 14a to 14c); `phx-log-view` (M5).

**Acceptance.**

- `tasks.spec.ts` with `KeelMockClient` and the fake source: "a TaskRef answer opens the progress
  modal, frames drive value and status, cancel posts task.cancel and done hands back to the opener";
  "a result with a download member fetches the download route and hands the blob to the browser"; "a
  failed frame shows the error and the request id"; "a modal without frames polls task.get".
- `screen.spec.ts`: "a query value on an integer field of the descriptor is posted as a number and
  one on a string field stays a string".
- `task-drawer.spec.ts`: "the rail badge counts queued and running jobs and the drawer lists them";
  "a job's detail shows the prop list, the progress and the log tail refreshed by its frames and
  offers cancel while it runs"; the story `Shell/Tasks` passes `pnpm test-storybook`.
- `hack/e2e.sh`: `frontend/e2e/tasks.spec.ts` — `alice` runs `note.export` from the notes toolbar
  and gets the csv through the modal, then opens `/f/note.export?format=csv&pace=300`, sees the
  modal advance with the note titles as its status, opens the details and reads the log lines in the
  drawer, cancels, runs the paced route again to the end, and the drawer lists the three jobs with
  their states.

### 13b. shell: the notification centre, the problem badges, the home dashboard and the about drawer

**Goal.** The other half of the `/shell` bullets of M3: the rail's notifications and help, the
dashboard's problems block and the last piece of the menu contract, on
`@globalart/phoenix-ui@0.2.2`. The notification centre: the rail item `notifications` carries the
unread count and opens a drawer with `phx-notification-center` fed from `notify.message.list` at
open and from `notification$` live, `read` posting `notify.message.mark`, `clear` posting
`notify.message.clear`, `select` navigating to `notify.problem.list` for a message whose params name
a problem; for a principal who may call `notify.problem.list` the drawer carries a problems section
from that list, and `KeelProblems` keeps the open problems by key, from which the sidebar fills
`badge.problem` of every menu item that names a key, the last piece of the menu contract of
[design/10-contracts.md](design/10-contracts.md). The home dashboard: when `/boot.dashboard` carries
widgets, `/` renders `KeelHomeComponent`, one `phx-widget-card` per widget over `phx-widget-grid` in
boot order, a `list` widget titled by its function's `title` and showing the first `limit` rows of
the list renderer in a compact mode without toolbar or pagination with a link to the full screen —
the problems block the M3 exit criteria name; with no widget `/` keeps redirecting to the first menu
item, so a composition without `notify` behaves as in M2. This is the `list`-widget slice of the
dashboard renderer the renderer table of [design/08-frontend.md](design/08-frontend.md) places in
M5, pulled forward by the "visible on the dashboard" of the M3 exit criteria; `stat` widgets,
reordering and the widget order preference stay in M5, and step 15 writes the slice into the table's
`dashboard` row. Its share accounting is decided here: the home screen is a route without a
function, so it is neither listed in `frontend/apps/shell/views.json` nor counted in the
custom-views share, which `keel-dev views share` computes over views registered against functions;
the drawers are chrome as the rail is. The about drawer: the rail item `help`, the third placeholder
of M2, opens a `phx-drawer` over `/boot.keel` and `/boot.products` — the platform version and API
version, the enabled products with their editions — the smallest target the rail sentence of the
design admits before the support screen of `panel` in M5, the plan's choice, so the sentence is
closed as written and the three placeholders are alive in M3. Dictionaries:
`keel.shell.notifications.*`, `keel.shell.home.*` and `keel.shell.about.*` in the kernel's `en` and
`ru`, the notification centre's labels handed from `phx.*`.

**Touches.** `frontend/libs/keel/renderer/src/list/list.component.ts` (the compact mode),
`frontend/libs/keel/client/src/` (`notifications.ts`, `problems.ts`),
`frontend/libs/keel/shell/src/` (`rail/rail.component.ts`, `notifications/` with the centre,
`about/` with the drawer, `home/` with the home component, `layout/sidebar.ts` for the badges,
`routes.ts`, `guard.ts`, specs and stories), `crates/kernel/i18n/` (the keys),
`frontend/e2e/notifications.spec.ts`, `hack/e2e.sh` (the poll before the browser specs).

**Out of scope.** The export button, the multi-row form, the file control and the white-label screen
(steps 14a to 14c); the dashboard renderer with `stat` widgets, reordering and the widget order
preference (M5, where step 15 moves the settings sentence that placed the order with
`DashboardWidget`); the support screen (M5, with `panel`).

**Acceptance.**

- `notifications.spec.ts` (Jest): "the centre lists messages, marks read, clears, and the badge
  counts unread"; "a notification frame prepends a message"; "open problems fill the sidebar badges
  by key for a principal who may list them and nothing for one who may not".
- `home.spec.ts`: "a boot with a list widget renders the home with one card showing the first rows
  and a link to the screen" and "a boot without widgets redirects to the first menu item";
  `about.spec.ts`: "the help item opens the about drawer with the version, the API version and the
  products of the boot"; the stories `Shell/Home`, `Shell/Notifications` and `Shell/About` pass
  `pnpm test-storybook`; `hack/descriptors.sh` prints the same share and
  `frontend/apps/shell/views.json` is unchanged.
- `hack/e2e.sh`: `frontend/e2e/notifications.spec.ts` — after the script's overdue todo and `ctl
  cron.job.run id=notify.problems.poll`, `root` sees `note.overdue` on the home dashboard's problems
  card and in the centre, the notes item of `alice`'s sidebar carries the badge `1`, and the help
  item opens the about drawer naming the platform version.

### 14a. the phoenix-ui 0.3.1 pin, the export button, column order and the cell editor

**Goal.** The first consumers of batch 2, opening with the pin: `frontend/libs/keel/package.json`
and `frontend/package.json` name `@globalart/phoenix-ui` `0.3.1`, the batch of step 11, and
`pnpm-lock.yaml` follows, the one Keel-side change of step 11, as the first commit. The export
button of the list renderer, the `/client` row of the design: the `export` action of a list, hidden
while `keel.export` is absent from the descriptor index since M2, becomes a toolbar `phx-dropdown`
of the action's `formats` that calls `KeelClient.download("keel.export", { func, query, columns,
format })` with the table's current `search`, `sort`, `filters` and `scope` as `query` and the
visible columns in their order as `columns`; a body of `application/json` is an envelope — a
`TaskRef` goes to the progress modal of step 13a, an error to a toast — and anything else is the
file, handed to the browser. Two consumers of batch 2 the M2 breakdown deferred to this milestone
land here as well: `columnOrder` bound to the preference `columns.<function>` of `settings_pref`,
read at load and written through `settings.pref.set` on `columnOrderChange`; and a `select` or
`number` column with `edit` edited in the cell through `PhxCellEditService` with the column's
options or bounds, as the toggle is since M2.

**Touches.** `frontend/libs/keel/package.json`, `frontend/package.json`, `frontend/pnpm-lock.yaml`
(the pin), `frontend/libs/keel/client/src/download.ts` (from step 13a, gains export),
`frontend/libs/keel/renderer/src/` (`list/export.ts`, `list/list.component.ts`, `list/columns.ts`,
`list/order.ts`, specs and stories), `frontend/e2e/notes.spec.ts`, `hack/e2e.sh`.

**Out of scope.** The multi-row form (step 14b); the file control and the white-label screen (step
14c); a third format of export; the dashboard renderer and the widget order (M5); the report and
stream renderers (M5).

**Acceptance.**

- `pnpm install --frozen-lockfile` resolves `@globalart/phoenix-ui@0.3.1` from Nexus and `pnpm
  build` is green on it.
- `export.spec.ts` with `KeelMockClient`: "the export dropdown lists the formats and posts
  keel.export with the table's query and the visible columns in order"; "a TaskRef answer opens the
  progress modal and a file answer is handed to the browser".
- `columns.spec.ts`: "the column order comes from columns.<function> and a reorder writes it back"
  and "a select column with edit opens the cell editor with its options and posts the function".
- `hack/e2e.sh`: `frontend/e2e/notes.spec.ts` gains "the export button downloads a csv whose header
  row names the visible columns".

### 14b. the multi-row form

**Goal.** The browser half of group editing over step 8a: when the list's `edit` action names a form
whose `ui.multi` is non-empty, the selection bar offers `edit`, which opens `KeelMultiFormComponent`
in a modal — one row per field of `ui.multi` with a "change" toggle that is the `mask` and the
field's control from the form's mapping, `submit` posting `{ ids, fields, mask }` to the form
function with an idempotency key, a `422` on `mask` or a field landing on its row, a `TaskRef` going
to the progress modal of step 13a and a `GroupResult` closing the modal and reloading the table.
Dictionaries: `keel.shell.multi.*` in the kernel's `en` and `ru`.

**Touches.** `frontend/libs/keel/renderer/src/` (`multi/multi-form.component.ts`,
`multi/multi-form.spec.ts`, `list/list.component.ts` for the selection bar, stories),
`crates/kernel/i18n/` (the keys), `frontend/e2e/notes.spec.ts`, `hack/e2e.sh`.

**Out of scope.** The file control and the white-label screen (step 14c); a group edit of a form
without `ui.multi`.

**Acceptance.**

- `multi-form.spec.ts`: "the selection bar offers edit when the form has ui.multi and the modal
  posts ids, fields and mask for the toggled fields alone"; "a 422 on mask lands on its row and a
  GroupResult reloads the table"; the story `Renderer/MultiForm` passes `pnpm test-storybook`.
- `hack/e2e.sh`: `frontend/e2e/notes.spec.ts` gains "two selected notes are edited as a group to
  kind todo with a due date".

### 14c. the file control, the upload submit and the white-label screen

**Goal.** The browser half of the upload kind over steps 8b and 8c, and the owner's feedback on the
M2 panel. The file control: `control: file` renders `phx-file-field` with `accept` from
`x-keel.file.types` and `maxSize` from `maxBytes`, and a form whose descriptor is `kind: upload`
submits through `KeelClient.upload(func, params, files)`, multipart to `/api/v1/x/<module>/<name>`
with the `params` part and one part per file field, `Idempotency-Key` in the header, the envelope
handled as a submit's; the row action `logo` of `settings.brand.list` opens `/f/settings.brand.logo`
with `host` from `$id`, the generic form with the file field, `host` read-only. The white-label
screen: the menu node `system/brand` and the titles of `settings.brand.*` say white-label by host
name in the `settings` dictionaries, `en` and `ru`; the nullable number fields `hue` and `chroma`
say "from the product" when empty through the convention `<module>.<entity>.<field>.empty`, a key
the field component looks up, falling back to `phx.unlimited`, the plan's choice over a new `x-ui`
member, and hands to `phx-number-field` by providing `PHX_LABELS` at the field component's element
injector with `unlimited` replaced by the translated key — the injection token phoenix-ui already
scopes by the nearest provider, so no input is added to the other repository and the batch table of
the design stands — with `settings.brand.hue.empty` and `settings.brand.chroma.empty` in the
dictionaries; and `products/demo/brand/logo.svg` takes the palette of the platform, hue 264 and
chroma 0.18 rendered as one sRGB hex fill the commit message records, so the demo looks like every
product of the line; the unit test of `products/demo/src/lib.rs` over the embedded logo asserts that
fill, since the `Shell/OneProduct` story references the file by path and renders nothing of its
colour.

**Touches.** `frontend/libs/keel/client/src/upload.ts`, `frontend/libs/keel/forms/src/`
(`field.component.ts` for the file field and the scoped labels, `submit.ts` for multipart,
`model.ts` for `x-keel.file`, specs and stories), `crates/system/i18n/settings/en.json` and
`ru.json`, `products/demo/brand/logo.svg`, `products/demo/src/lib.rs` (the fill test),
`frontend/e2e/system.spec.ts`, `hack/e2e.sh`.

**Out of scope.** A file control outside an `upload` function; a streamed upload; the brand text
field's return.

**Acceptance.**

- `field.component.spec.ts`: "control file renders phx-file-field with accept and maxSize from
  x-keel.file"; "a nullable number shows the field's .empty label through the scoped PHX_LABELS when
  the catalogue has it and phx.unlimited otherwise"; `submit.spec.ts`: "an upload descriptor submits
  multipart with a params part and one part per file field and the idempotency header".
- `hack/e2e.sh`: `frontend/e2e/system.spec.ts` gains "the white-label screen says white-label by
  host name, hue and chroma say from the product when empty, an uploaded svg logo shows after a
  reload and a scripted one is refused on the field"; `products/demo/src/lib.rs`:
  `the_demo_logo_is_filled_with_the_platform_palette` — every `fill` of the file other than white is
  the recorded hex; `hack/descriptors.sh` still prints the same share.

### 15. M3 closure

**Goal.** Close the milestone the way the M0, M1 and M2 checklists did, apply the amendments steps 1
to 14 owed the design, and put the built M3 in front of the owner. Run every M3 "Done when" item by
hand or through CI and record for each the test name or the command that shows it; add the section
"M3 exit checklist" after the M2 checklist, in the same form, citing Rust tests by name, Jest and
Playwright specs by file, and the commands of `hack/` and `ci.yml`; and make the consistency pass
over `docs/design/` and this plan against the built M3. The pass applies what the commit messages of
steps 1 to 14 recorded: the module ledger `keel_migration_task` in the ledger example of
[design/04-storage.md](design/04-storage.md), the module keys `keeltask` and `keelcron` and the
kernel's `keeloutbox` beside the kernel's advisory keys, with the commit order of `seq` in the
`keel_outbox` section and the `AdvisoryLock` seam as the way a module holds a session lock, the
`pg_advisory_lock` sentences standing as built; `TaskRunner`, `Caller`, `LiveEvents`,
`AdvisoryLock`, `ProblemCounter` and the `PrincipalSource` lookups beside the `OutboxSubscriber`
already named in the hidden-hooks list of [design/05-modules.md](design/05-modules.md),
`ContextExt::data_dir` next to `db()`, `Call::task`, `emit_about` and `spawn_self_as_task` in the
SDK list, `DashboardWidget` among what the SDK gives every module, the `internal` member of the
function builder with the functions it hides, and the raw routes as the kernel's projection of
`download` and `upload` kinds with `r.raw` deferred; the tables, functions, menu nodes and the
`cron.overrides` setting of the four modules in their sections, the poll job and the internal
functions the registry writes through, the channel kinds declared and the one implemented, the
`webhook.private_targets` setting, the pinned-address rule of the sender and the secret rule of
`webhooks`, `notify.problem.list` in place of the illustrative `problem.list` of the `/boot`
example, and the `keel.account.delete` walk, `suspend` and `resume` as built in the
`AccountLifecycle` section of [design/03-access.md](design/03-access.md); the two sentences that
pointed at M3 and now point elsewhere — "Password reset through `notify` arrives with M3" of the
`auth` section, moved to M5 with an address per principal, the M5 scope list gaining both, as the M3
scope list records; and "the widget order arrives with the `DashboardWidget` contributions (M3)" of
the `settings` section, moved to the dashboard renderer of M5 — and the rail sentence of
[design/08-frontend.md](design/08-frontend.md), "Notifications, tasks and help are disabled
placeholders until M3", closed as built with help as the about drawer; the `TaskRef`, `DownloadRef`,
`GroupResult`, `TaskProgress` and `NotificationFrame` shapes, the `after` query member, the replay
window, the per-session stream cap, the live-frame scope rule and the re-check at keep-alive of the
event stream, the subject rule of `emit`, the group-edit merge over `ui.read`, the multipart shape
with `x-keel.file`, the multipart exception to the 1 MiB body limit, the two raw-route refusals and
the `download · upload · stream` row of the form-dynamics table saying an `upload` carries a `Form`
block in [design/10-contracts.md](design/10-contracts.md), the ±300 s window of
[design/07-ecosystem.md](design/07-ecosystem.md) fixed as built, the `/boot` table's `dashboard` and
`problems` members now typed and the endpoint map's `/api/v1/events` no longer provisional; in
[design/02-functions.md](design/02-functions.md) `x-keel.file` in the attribute table,
`batch_threshold` on `keel.export` as the row threshold, the multipart exception in the body-limit
sentence, the stream cap named as the platform constant `SSE_MAX_CLIENTS` in that same global
sentence, the `secret` and `ui.multi` freeze rules, an `action` row in the `ui` table carrying
`title` and `result`, and the `ui` sentence of the descriptor table naming `get`, `options` and
`download` alone as the kinds without a screen; the TLS paragraph of
[design/09-operations.md](design/09-operations.md) — the pair generated by the binary at first
start, its names and validity, the `0700` directory, no plain listener and no redirect port with the
reasoning of step 9b, the fingerprint in the log — the installer step that generates nothing, the
multipart exception in the body-limit sentence, the `X-Forwarded-For` and `X-Forwarded-Host` rule of
`trusted_proxies` with the forwarded host in the CSRF check and the fallbacks for a malformed
header, and the "until M3" sentences of the `auth` section of
[design/05-modules.md](design/05-modules.md), the CSRF section of
[design/10-contracts.md](design/10-contracts.md) and the TLS section itself, which the built TLS
closes; in [design/08-frontend.md](design/08-frontend.md) the `/client` row's export and SSE as
built, the `<field>.empty` convention in the key table with the scoped `PHX_LABELS` as its carrier,
the home screen, the drawers and the about drawer in the `/shell` section, the `list`-widget slice
of M3 in the `dashboard` row of the renderer table with the share rule for a route without a
function, the debounce of `data.invalidate`, the M3 row of the batch table standing as written, the
M3 row of the frontend table, and the `pull` and `missing` paragraph as built with the committed
dictionaries and `contracts/i18n.json`; the tooling table and the layout of
[design/01-overview.md](design/01-overview.md) with the `i18n` subcommands; the M0, M1 and M2
remarks and notes of this plan that pointed at M3 — the plain listener, the localhost cookie rule,
the SSE stub, the export button, `ui.multi`, `keel-dev i18n`, column order and the cell editor,
`keel.account.delete` as a task and `AccountLifecycle` — each marked done in place; the opening
paragraph of this plan, which gains the M3 breakdown, and the "How we work" section, which gains
`i18n missing`, the https form of the `hack/` scripts and the grown matrix; and any one-line
mismatch the pass reveals. `README.md` follows the code: the M3 row set to done, the command list
with `i18n missing`, the layout table with `crates/dev/src/i18n`, and the https addresses.
`hack/check-docs.sh` carries the list of M3 names — `TaskRunner`, `TaskHandle`, `Caller`,
`LiveEvents`, `AdvisoryLock`, `OutboxSubscriber`, `ProblemCounter`, `CronJob`, `ProblemSource`,
`MailTemplate`, `DashboardWidget`, `Download`, `DownloadOutcome`, `FileAttrs`, `emit_about`,
`spawn_self_as_task`, `trusted_proxies`, `KeelEvents`, `KeelTasks`, `KeelProblems`,
`KeelMultiFormComponent` — and asserts each is found under `docs/design/` and under `crates/` or
`frontend/libs/keel` (`AccountLifecycle` and `SSE_MAX_CLIENTS` stay out of the list until step 2b
and 6b land, because the check demands each name under `crates/` or `frontend/libs/keel`), that
every Rust test the M3 checklist cites is defined, every `*.spec.ts` it cites exists, every command
it cites exists, and that the README's check list, "How we work" and `ci.yml` agree.
`@globalart/keel` is published to Nexus at the closing commit's version
by `publish-npm.yml` through `workflow_dispatch`. The demo machine is updated by hand: the musl
artifact of the closing commit's `build.yml` run is unpacked into `/opt/keel/versions/<version>` on
`keel-demo`, `bin/current` is switched, `keel.service` is restarted, the first start generates the
certificate under `/var/lib/keel/tls` and logs its fingerprint, `/readyz` answers `ready` over
https, and the owner opens `https://<host>:<port>/` directly — the SSH port forward of M2 is no
longer needed — accepts the certificate by its fingerprint, logs in, runs `note.export` and watches
the modal and the drawer, opens the notification centre, the dashboard and the about drawer, uploads
a logo for the machine's host name, and confirms the notes item badge after `keel ctl cron.job.run
id=notify.problems.poll`.

**Touches.** `docs/plan.md`, `docs/design/*.md` (the consistency pass), `README.md`,
`deploy/README.md` (TLS, the https wait and the mail catcher of the end-to-end script),
`contracts/README.md` (`i18n.json`), `hack/check-docs.sh`, `.github/workflows/ci.yml`; the demo
machine `keel-demo`.

**Out of scope.** Any code change beyond what a document alignment reveals as a one-line mismatch;
anything of M4.

**Acceptance.**

- Every "Done when" item of M3 is ticked in the new checklist with the name of the test, the spec or
  the command that demonstrates it, and every item is green on `main`: `note.export` as a task with
  progress and log in the browser (`frontend/e2e/tasks.spec.ts`) and in `ctl task.get`
  (`hack/prove-binary.sh`); cancellation (`note_export_is_cancelled_between_two_notes_when_paced`,
  `tasks.spec.ts`); the checkpoint surviving a restart
  (`note_export_interrupted_by_a_restart_resumes_from_its_checkpoint_and_writes_no_note_twice`, and
  the restart check step 2a put into `hack/prove-binary.sh`, which stops `serve` with `SIGTERM`
  during a paced `note.export`, starts it again and reads `done` with every note written once from
  `task.get`, the CI counterpart of `systemctl restart`, confirmed by hand on `keel-demo`); the cron
  job raising a problem that reaches the dashboard and the mail catcher
  (`an_overdue_todo_raises_note_overdue_for_its_account_after_the_poll_and_a_done_one_clears_it`,
  `a_raised_problem_reaches_the_mail_catcher_and_the_test_button_sends_a_test_email`,
  `frontend/e2e/notifications.spec.ts` and the catcher check of `hack/e2e.sh`); and
  `data.invalidate` for own objects alone
  (`a_user_receives_data_invalidate_for_its_own_objects_alone_while_an_admin_receives_every_one`
  and `frontend/e2e/events.spec.ts`; the `events` group of the conformance suite is left to step 6b,
  recorded unticked in the checklist).
- `hack/check-docs.sh` is green in CI with the M3 names and the new specs; the README's check list,
  "How we work" and `ci.yml` agree, `i18n missing` and the fourteen-set matrix included.
- `@globalart/keel` at the closing commit's version answers `pnpm view` against Nexus with the six
  entry points.
- On `keel-demo`, `keel --version` under `/opt/keel/bin/current` names the closing commit's version,
  `/readyz` answers `ready` over https with the generated certificate, and the owner has opened the
  panel directly, logged in and walked the task, the notification and the white-label screens, which
  the checklist records with the date-free note "confirmed by hand on keel-demo".

## M4 work breakdown

M4 is split into twenty-one PR-sized steps in eight groups, ordered by dependency; a step is one
change to `main` and is done when its acceptance holds in CI. As in M3, the letters of a group name
the steps of one delivery that land separately: the privileged code path into its root half, its
kernel half and the release channel that feeds it (1a, 1b, 1c); the operator commands into backup
and restore, diagnostics, the product switch on a running installation and the installer that ties
them together (2a to 2d); the host layer into the agent's core, the rest of its catalogue and the
kernel-side client (3a, 3b, 3c); the two items M3 left undone (6a, 6b), which `dns` needs before
its first zone; the `dns` bundle into its three modules and its browser and host proof (5a to 5d).
Every step that touches the operating system carries two kinds of proof: a test that runs
unprivileged in CI against a stub — a fake `systemctl` on the path, a temporary directory standing
for `file_roots`, an agent started in-process over a temporary socket — and a script under `hack/`
the operator runs on a virtual machine for what needs root, whose transcript the exit checklist
records. Steps 1 to 20 touch nothing under `docs/`; where a step names a mechanism the design
leaves open it says so, the commit message records the choice, and step 21 amends the documents.
Where the design names a crate this milestone does not build (`keel-storage`, the S3 target of a
backup), the step says what stands in its place and the closure moves the crate to the milestone
that needs it.

### 1a. the privileged unit and the update policy

**Goal.** The root half of the privilege boundary of [design/06-host.md](design/06-host.md) and
the update section of [design/09-operations.md](design/09-operations.md): `keel privileged
--socket <path> --policy <path>` is the one root code path of an installation, run by
`keel-privileged.service`; in this step it serves the `update` policy alone, and `keel privileged
update --from-request` is what `keel-update.service`, started by `keel-update.path` when
`/var/lib/keel/update.request` appears, runs as a oneshot. The update reads the version from the
request and nothing else: it downloads `versions/<ver>/manifest.json` and the archive from the
channel named in the request under `update.channel_url` (default `https://dl.globalart.hu/keel/`,
compiled in), verifies the Ed25519 signature of the manifest against the release key compiled into
the binary or one of `previous_keys`, verifies the archive's sha256 against the manifest, refuses a
version below `min_upgrade_from` of the channel head, unpacks into `/opt/keel/versions/<ver>/`,
switches `bin/current`, refreshes the unit files from the release's `deploy/systemd/` and restarts
`keel.service`; `keel privileged update rollback` switches the symlink to the previous version
within the same `contract`, and refuses across a contract boundary unless `--restore` names a
backup archive. The release manifest, its signing and its verification live in a new crate
`crates/release` (`keel-release`), depending on `ed25519-dalek` and `sha2` at pinned majors, which
`hack/release.py` of step 1c mirrors; the root paths (`/opt/keel`, `/var/lib/keel`, the unit
directory, the `systemctl` binary) are parameters of the update runner with the design's defaults,
so a test drives the whole sequence in a temporary tree with a stub `systemctl` and a channel served
from a local directory. The design leaves the exact flag spellings open; the step fixes them as
written here. The `deploy/systemd/` directory gains `keel-privileged.service`, `keel-update.path`
and `keel-update.service`; `deploy/install.sh` installs and enables them.

**Touches.** `crates/release` (new), `crates/keel/src/cli.rs`, `crates/keel/src/privileged/`
(new: the unit's main loop, the request reader, the update runner), `deploy/systemd/`,
`deploy/install.sh`, `hack/check-install.sh`, `Cargo.toml` (the two dependencies).

**Out of scope.** The `host` policy (3a); the kernel side that writes the request (1b); the
workflow that publishes a release (1c); ACME.

**Acceptance.**

- `crates/release` unit tests: `a_manifest_signed_with_the_release_key_verifies_and_a_changed_byte_does_not`,
  `a_manifest_signed_with_a_previous_key_still_verifies`, `an_archive_whose_sha256_differs_from_the_manifest_is_refused`,
  `a_version_below_min_upgrade_from_is_refused`.
- `crates/keel/tests/privileged.rs`: `an_update_request_installs_the_signed_version_switches_current_and_restarts`
  (temporary tree, local channel directory, stub `systemctl` that records its argument vectors),
  `a_request_naming_an_unsigned_or_tampered_version_leaves_current_untouched`,
  `rollback_switches_to_the_previous_version_within_the_contract_and_refuses_across_it`,
  `the_request_carries_no_command_the_root_side_obeys` (a request with extra keys installs the
  version and ignores the rest).
- `hack/check-install.sh` asserts the three units are installed and enabled and that
  `keel-privileged.service` runs as root while `keel.service` runs as `keel`.

### 1b. the update module: check, apply as a task, rollback

**Goal.** The kernel half: the system module `update` in `crates/system/src/update/` (cargo
feature `update`, in the default set and the matrix) with the functions `update.check`, reading
the channel head named by the setting `update.channel` (`stable` or `beta`, default `stable`,
admin level) and answering the running version, the available version, its `notes` and whether an
update is allowed by `min_upgrade_from` and `contract`; `update.apply`, an action with `ui.result =
task` whose task drains the queue through the tasks module, takes the pre-update logical dump into
`<data_dir>/backups/` with `COPY … TO STDOUT` over the kernel's own connection, one file per loaded
module table plus the kernel tables, writes `update.request` with the version, the channel and the
requesting principal, and ends in state `done` with the note that the root unit takes over; and
`update.rollback`, the same shape writing a rollback request. The problem `update.available` is
raised by a cron job polling the channel daily and cleared when the running version catches up.
The screen `update.check` under the system group ("Updates") shows the versions, the notes and
the two actions; the CLI `keel update check|apply|rollback` calls the same functions over the
socket. The task ends before the restart, so the task drawer shows `done` and the shell's boot
revalidation shows the new version after the restart.

**Touches.** `crates/system/src/update/`, `crates/system/i18n/update/`, `crates/system/Cargo.toml`,
`crates/keel/src/cli.rs`, `bin/keel/src/main.rs`, `.github/workflows/ci.yml` (the matrix set),
`contracts/dump`, `contracts/openapi.json`, `frontend/libs/keel/testing/src/generated/`.

**Out of scope.** The root side (1a); the S3 target of the dump; the button's end-to-end run on
the demo host (step 21, operator-run).

**Acceptance.**

- `crates/system/tests/system/update.rs`: `check_compares_the_running_version_with_the_channel_head_and_reads_min_upgrade_from`
  against a channel served by the test's own HTTP listener,
  `apply_drains_dumps_every_loaded_table_and_writes_the_request_with_the_principal`,
  `apply_refuses_a_version_below_min_upgrade_from_and_across_a_contract`,
  `rollback_writes_a_rollback_request`, `the_daily_check_raises_update_available_and_the_running_version_clears_it`.
- The matrix gains the set `update` alone and `tasks,cron,notify,update`; `keel update check
  --out json` answers over the socket in `hack/prove-binary.sh`.

### 1c. the release workflow and the channel

**Goal.** `.github/workflows/release.yml` on the tag `keel-vX.Y.Z`: the musl build of `Build`,
`hack/release.py` writing `versions/<ver>/manifest.json` (`version`, `sha256`, `min_upgrade_from`,
`contract`, `notes` from the tag's annotation) and signing it with the release key held as the
repository secret `KEEL_RELEASE_KEY`, and the upload of the archive, its `.sha256`, the version's
manifest and the channel head `stable/manifest.json` (or `beta/` when the tag carries a
pre-release suffix) into the Nexus raw repository `keel-releases` through its REST API under the
deploy account `keel-release`, whose password is the secret `KEEL_RELEASE_NEXUS_PASSWORD`. The
script verifies what it uploaded by reading it back through `https://dl.globalart.hu/keel/` when
that name answers and through the Nexus URL otherwise. The Nexus repository, the role and the
account, the secrets and the route `dl.globalart.hu` in gitops are operator-run and listed in the
step's return; the script and the workflow run against a local directory in CI (`--dry-run
--out <dir>`), which is what the test covers. The public key of the release key is what step 1a
compiled in; the step records in the commit message how the pair was generated and where the
private half lives.

**Touches.** `.github/workflows/release.yml` (new), `hack/release.py` (new), `hack/check-release.sh`
(new: the dry run against a directory and the manifest verified with `keel-release`'s test
binary), `.github/workflows/ci.yml` (the check).

**Out of scope.** Signing on an offline machine; the license server's download tokens (M5).

**Acceptance.**

- `hack/check-release.sh` in the job `check`: a dry run produces the layout of the design under a
  directory and `cargo run -p keel-release -- verify <dir>/versions/<ver>/manifest.json` accepts
  it with the compiled public key.
- `release.yml` is valid (`actionlint` or the parser CI already uses) and triggers only on
  `keel-v*` tags; the workflow's run on the closing tag of step 21 is operator-run.

### 2a. backup and restore

**Goal.** `keel backup [--out <path>] [--with-secret-key]` writes a `tar.zst` with a
`manifest.json` recording the keel version, the enabled products with their editions and the
schema version of every module ledger, the logical dump of the database — `COPY` per module
table over the kernel's connection, or `pg_dump -Fc` when the binary is on the host — `/etc/keel/`
in full, the brand assets and the TLS material, and the contribution of every loaded module
through the new SDK contribution `BackupSource` (a name and an async fn writing files into a
directory of the archive), which `dns-zones` uses in 5a for its zone files; `secret.key` is
included only with `--with-secret-key`, sealed with a passphrase read from the terminal or
`KEEL_BACKUP_PASSPHRASE`, since API tokens and sessions are stored as hashes and only `Secret`
values need the key. `keel restore <archive> [--with-secret-key]` runs with the service stopped,
consults a compatibility table by major version, refuses a binary that does not carry one of the
bundles the manifest names, restores the database, `/etc/keel/`, the assets and the `BackupSource`
directories, and the next start logs what the restored enabled set changed. Restore from the UI is
`backup.restore` as a task that ends with a restart request; `backup.create` as a task writes the
archive under `<data_dir>/backups/` and answers it as a download. The target is `local`; the S3
target and the `keel-storage` crate the design names move to M5 with the license server, which the
closure records.

**Touches.** `crates/sdk/src/backup.rs` (new, `BackupSource`), `crates/kernel/src/backup/` (new:
the archive writer and reader, the table dump), `crates/system/src/backup/` (new module `backup`
with the two functions and the screen), `crates/keel/src/cli.rs`, `deploy/uninstall.sh` (the
`--keep-backups` note), `contracts/`.

**Out of scope.** S3 and `keel-storage` (M5); the restore of a backup across a major (refused).

**Acceptance.**

- `crates/keel/tests/backup.rs`: `a_backup_restored_into_a_fresh_database_and_data_dir_answers_the_same_boot_and_logs_in_with_the_same_totp`
  (the two-factor secret is a `Secret`, so the test restores with `--with-secret-key`),
  `a_backup_without_the_key_restores_but_the_sealed_values_do_not_open`,
  `restore_refuses_an_archive_naming_a_bundle_the_binary_does_not_carry`,
  `a_backup_source_contribution_lands_in_the_archive_and_comes_back`.
- `hack/prove-binary.sh` takes a backup over the CLI and lists the archive's manifest.

### 2b. doctor, support-bundle and the log ring

**Goal.** `keel doctor` checks the installation end to end and prints one line per check with
`ok`, `warn` or `fail`: directory ownership and modes, the three units and their users, the TLS
material and its expiry, the database (server version, socket, role rights), the migration
ledgers against the binary, the host agent (in use, reachable, or not in use), the license (a stub
answering `not required` until M5), free space under `/var/lib/keel` and `/opt/keel`, that every
enabled product is carried by the binary, the orphaned modules whose tables remain after a
disable, and accounts left in `deleting` whose job failed; exit code 1 when anything fails.
`keel support-bundle [--out <path>]` collects `config.yaml` and `agent.yaml` with secrets redacted
by the same `secret` flag of the field schema that masks logs and audit, the last 10 000 log
entries from a new ring buffer in the kernel's `tracing` layer, the migration ledgers, the tail of
the audit table, open problems and the metrics text; `keel.logs.tail` is the function over the
same ring buffer, admin level, and the `doctor` checks are also `keel.doctor.run` for the screen
"Health" under the system group. CI runs a test that the bundle contains no secret value.

**Touches.** `crates/kernel/src/logs.rs` (new, the ring), `crates/kernel/src/doctor/` (new),
`crates/kernel/src/support.rs` (new), `crates/keel/src/cli.rs`, `crates/system/src/` (the
functions land in the `settings` module's crate area or a new `ops` module — the step chooses,
with `ops` preferred so the health screen has an owner), `contracts/`.

**Out of scope.** OTLP export; metrics beyond what exists.

**Acceptance.**

- `crates/keel/tests/doctor.rs`: `doctor_is_green_on_a_fresh_site_and_names_a_missing_unit_a_bad_mode_and_an_orphaned_module`,
  `the_support_bundle_carries_no_secret_value` (a channel with a sealed password and a token exist,
  and neither value appears in the bundle), `logs_tail_answers_the_newest_entries_in_order`.
- `hack/prove-binary.sh` runs `keel doctor` on its site and asserts exit 0.

### 2c. the product switch on a running installation

**Goal.** `keel product enable|disable <id>` and `keel.product.enable|disable` on a running
installation: the function records the change in `keel_kv` and requests a restart through a
`keel.restart` internal function whose task drains the queue, takes the same pre-change logical
dump as an update and asks the service manager to restart the unit — through `sd_notify`'s
`RELOADING`/exit with `Restart=always`, the kernel exiting with a distinguished code that the
unit restarts, which the step chooses and records — so the new product's modules migrate before
the port opens; `disable` leaves the product's tables in place, `enable` of a product whose ledger
already holds its migrations re-runs none of them, and `doctor` lists the orphaned modules; the
lease check the design places before `enable` is a stub that always allows, marked for M5. The
screen `keel.product.list` gains the two actions with the confirmation that names the restart.

**Touches.** `crates/kernel/src/keel/products.rs`, `crates/keel/src/runtime.rs` (the restart
exit), `crates/keel/src/cli.rs`, `deploy/systemd/keel.service` (the restart exit status),
`contracts/`.

**Out of scope.** The lease (M5); `install.sh` (2d).

**Acceptance.**

- `crates/keel/tests/products.rs`: `disable_leaves_the_tables_and_enable_reruns_no_applied_migration`
  (the ledger rows are compared before and after), `enable_on_a_running_installation_requests_a_restart_after_the_dump`.
- `hack/prove-binary.sh` disables and enables `demo` on its site and asserts the `note_*` tables
  and the ledger survive.

### 2d. install.sh in full, sd_notify and the setup link

**Goal.** The installer of [design/09-operations.md](design/09-operations.md) completed: the
release manifest's signature verified before anything is unpacked (with `keel-release`'s public
key embedded in the script by `hack/release.py`), the uid range allotted to the installation for
the host agent recorded in `/etc/keel/agent.yaml` under `uid_range`, `agent.yaml` installed from
the release with the sections of every first-party product, the three units, the container and
clean-host checks kept, and the first administrator no longer created over the socket: the kernel
prints a one-time link with a `kok_` login key to `keel.setup.admin`, a guest-level function that
exists only while the installation has no administrator and creates one from the login and
password of the form; the installer prints that link. The kernel speaks `sd_notify` — `READY=1`
after the port opens, `WATCHDOG=1` on a timer, `STOPPING=1` on drain — so `keel.service` becomes
`Type=notify` with `WatchdogSec=60`; the notify socket is written directly, no libsystemd.
`uninstall.sh --purge` removes the new units and the agent socket.

**Touches.** `deploy/install.sh`, `deploy/uninstall.sh`, `deploy/systemd/keel.service`,
`crates/keel/src/runtime.rs` (sd_notify), `crates/kernel/src/keel/setup.rs` (new,
`keel.setup.admin`), `frontend/libs/keel/shell/src/login/` (the setup form when the link is
followed), `hack/check-install.sh`, `hack/prove-binary.sh`.

**Out of scope.** ACME; the second product on the same host (M5).

**Acceptance.**

- `crates/keel/tests/setup.rs`: `the_setup_link_creates_the_first_administrator_once_and_the_function_is_gone_after`,
  `the_runtime_sends_ready_and_watchdog_over_the_notify_socket` (a test socket stands for
  systemd).
- `hack/check-install.sh` asserts the signature check, the uid range, `Type=notify`, the three
  units and the printed setup link; `hack/e2e.sh` follows a setup link once in the browser.
- Operator-run, recorded in the checklist: on a fresh VM `curl | sh -s -- --product demo`
  installs and `ps -eo user,comm` shows root only under `keel privileged`.

### 3a. keel-host-agent: the socket, the policy, the core catalogue and the audit

**Goal.** The crate `crates/host-agent` (`keel-host-agent`) replaces `bin/spike`: the unix socket
accepting only uid `keel` by `SO_PEERCRED`, the policy loader for `agent.yaml` (`file_roots`,
`services`, `checks`, `exec_allow`, `uid_range`, `products` with `user_prefix` and
`drop_in_prefix`), the frame protocol of [design/06-host.md](design/06-host.md) (`id`, `op`,
`actor`, `request_id`, `product`, `args` in, `ok`/`data` or `error` with `key` and `detail` out),
and the core catalogue: `host.user.*` and `host.group.*` inside the uid range and under the
product's `user_prefix`, `host.fs.*` addressed by `file_roots` key plus name with writes through a
temporary file and rename, `host.service.*` for units named in `services` through `systemctl` as
a process, and `host.template.apply` with the render (minijinja on the agent side), swap, named
check, reload and rollback sequence of the design, `changed: false` when the rendered file equals
what is there. Every operation, accepted or refused, is one journald record with the actor and the
request id; a request whose product has no section, whose path leaves `file_roots` or whose unit
is not listed is refused with `host.error.policy` and audited. Impersonation is the re-exec
`agent-exec --uid` with `setresuid` in `pre_exec`, and a test greps the crate for `setfsuid` and
for `spawn_blocking` around a credential call. `keel privileged` serves this policy beside the
update policy once the step lands; `bin/spike` and `hack/spike-vm.sh` are removed and
`hack/host-vm.sh` takes their place for the operator's VM proof.

**Touches.** `crates/host-agent` (new), `crates/keel/src/privileged/`, `bin/spike` (removed),
`hack/spike-vm.sh` (removed), `hack/host-vm.sh` (new), `hack/check-deps.sh` (the boundary:
`keel-kernel` and `keel-system` depend on neither host crate), `Cargo.toml`.

**Out of scope.** `pkg`, `cron`, `quota`, `exec` and `AgentOp` (3b); the kernel-side client (3c).

**Acceptance.**

- `crates/host-agent/tests/agent.rs`, unprivileged with a temporary policy, a temporary
  `file_roots` tree and a stub `systemctl`: `a_peer_that_is_not_the_configured_uid_is_refused`,
  `a_file_operation_outside_file_roots_is_refused_and_audited`,
  `template_apply_renders_swaps_validates_reloads_and_reports_changed`,
  `a_failing_check_rolls_the_file_back_and_a_failing_reload_reloads_again`,
  `an_unchanged_render_answers_changed_false_and_touches_nothing`,
  `a_request_whose_product_has_no_section_is_refused`,
  `no_setfsuid_and_no_credential_change_on_a_worker_thread`.
- Operator-run with `hack/host-vm.sh` on a VM: an operating-system user is created inside the uid
  range, a broken template rolls back on `named` without downtime, and an operation outside
  `file_roots` is refused and appears in `journalctl -u keel-privileged` — the M4 "Done when"
  items for the agent.

### 3b. the rest of the catalogue, AgentOp and ConfigTemplate

**Goal.** `host.pkg.*` over the distribution's package manager (apt on Debian and Ubuntu, the
only two the installer accepts), `host.cron.*` writing drop-ins under the product's
`drop_in_prefix` in `/etc/cron.d`, `host.quota.*` over `setquota` and `repquota` when present,
and `host.exec` for binaries in `exec_allow` with a fixed argument vector per entry; the SDK
contributions `AgentOp` (a name, the schemas of arguments and result, a handler compiled into the
agent, a policy class) and `ConfigTemplate` (an id, the template text, a named check), collected
by the registrar, handed to the agent at start and checked for unique ids at the freeze; template
overrides read from `/etc/keel/templates/<id>` before the compiled text. The demo bundle
contributes one `ConfigTemplate` and one `AgentOp` as the fixture the tests and the matrix need.

**Touches.** `crates/host-agent/src/ops/`, `crates/sdk/src/host.rs` (new: `AgentOp`,
`ConfigTemplate`), `crates/sdk/src/registrar.rs`, `products/demo/src/` (the fixtures),
`hack/host-vm.sh`.

**Out of scope.** A real product operation (libvirt, ipmi — later products).

**Acceptance.**

- `crates/host-agent/tests/catalogue.rs`: `exec_runs_only_allowlisted_binaries_with_their_fixed_arguments`,
  `pkg_install_calls_the_package_manager_and_reports_what_changed` (stub `apt-get`),
  `cron_writes_a_drop_in_under_the_product_prefix_and_removes_it`,
  `a_product_agent_op_is_executed_under_its_policy_class_and_audited`,
  `a_template_override_under_etc_wins_over_the_compiled_text`,
  `two_templates_with_one_id_fail_the_freeze`.

### 3c. keel-host: HostExecutor, the Local client and the host module

**Goal.** The crate `crates/host` (`keel-host`), linked into the kernel process: the trait
`HostExecutor` in `keel_host::api`, the `Local` implementation over `/run/keel/agent.sock` with
`request_id` propagation and the mapping of agent errors to `host.error.*` keys that surface in
the call envelope; the typed client API `users()`, `groups()`, `fs()`, `services()`, `pkg()`,
`cron()`, `quota()`, `template()`, `exec()` and `op::<A, R>(name)`; and the module
`keel_host::MODULE`, id `host`, registering `dyn HostExecutor` as a service, reading its
`ConfigSection` (the socket path, a call timeout the step fixes at 60 seconds and records), and
loaded only when an enabled bundle names `host`; a module that `requires: ["host"]` fails the
freeze when no module provides the service, and while `host` is not loaded the kernel never opens
the socket. The demo bundle gains `host` in its modules behind the cargo feature `demo-host`, so
the composition without `keel-host` stays in the matrix and `check-deps.sh` proves the kernel does
not depend on it.

**Touches.** `crates/host` (new), `bin/keel/src/main.rs`, `products/demo/`, `hack/check-deps.sh`,
`.github/workflows/ci.yml` (a composition with and without `host`), `crates/testkit` (a harness
starting the agent in-process over a temporary socket for module tests).

**Out of scope.** `Remote` (M8); host-level screens (`host.*` functions for files and logs), not
fixed by the design and left to the products that need them.

**Acceptance.**

- `crates/host/tests/executor.rs`: `a_module_applies_a_template_through_the_executor_and_the_agent_audits_the_request_id`,
  `an_agent_error_surfaces_in_the_envelope_as_its_key_with_the_detail`,
  `a_module_requiring_host_without_a_provider_fails_the_freeze`,
  `without_host_loaded_the_kernel_never_connects_to_the_socket`.
- `hack/check-deps.sh` asserts `keel-kernel` and `keel-system` depend on neither `keel-host` nor
  `keel-host-agent`; the matrix builds `keel-bin` with and without `demo-host`.

### 4. the feature guard without a lease

**Goal.** The third check of the access triple of [design/03-access.md](design/03-access.md): a
function declaring `.feature("dns.zones")` is refused with `keel.error.license` (402) unless the
installation's feature set names it, and its actions, columns and menu nodes are cut from the
descriptor projection and `/boot` the way level and permission already cut them. The feature set
comes from a service `dyn Features` the kernel provides with one implementation in M4: every
feature of every enabled product is enabled, and `keel product features` lists them; the setting
`license.features_override` is not created — M5 replaces the implementation with the lease and
the design's `features` in `/boot` starts carrying the real set. The guard runs in the dispatcher
in the order the design fixes: level, permission, feature, token scope, module guards, ownership.

**Touches.** `crates/kernel/src/dispatcher/guards.rs`, `crates/kernel/src/boot.rs`,
`crates/kernel/src/http/descriptors.rs`, `crates/sdk/src/function.rs` (`.feature`), `contracts/`.

**Out of scope.** The lease, editions and the license module (M5).

**Acceptance.**

- `crates/kernel/tests/kernel/features.rs`: `a_function_of_a_feature_not_in_the_set_answers_license_and_is_cut_from_the_projection_and_the_menu`,
  `the_m4_feature_set_names_every_feature_of_every_enabled_product`.
- `hack/conformance.sh` gains the `features` group: a probe function behind a feature the fixture
  disables answers 402.

### 6a. keel.account.suspend, resume and delete as a task over AccountLifecycle

**Goal.** The item M3 left undone, in the shape its 2b described: the SDK contribution
`AccountLifecycle` with `on_suspend`, `on_resume`, `on_delete` and `on_rename` handlers that run
after commit or as a task, the kernel functions `keel.account.suspend` and `keel.account.resume`
switching `state` and walking the contributions after commit, and `keel.account.delete` as a task
that moves the account to `deleting`, walks every module's `on_delete` in `requires` order with
progress per module, and removes the account row last; a failed step leaves the account in
`deleting` with the error on the task, which `doctor` of 2b reports. The demo bundle contributes
a lifecycle that suspends and drops notes, the fixture the tests need and the model `dns-zones`
follows in 5a.

**Touches.** `crates/sdk/src/lifecycle.rs` (new), `crates/sdk/src/registrar.rs`,
`crates/kernel/src/keel/accounts.rs`, `crates/kernel/src/keel/lifecycle.rs` (new),
`products/demo/src/note/`, `contracts/`.

**Out of scope.** The host side of a lifecycle (a product's `on_delete` that removes
operating-system users goes through the executor of 3c, which `panel` first needs in M5).

**Acceptance.**

- `crates/kernel/tests/kernel/lifecycle.rs`: `suspend_and_resume_switch_the_state_and_walk_the_contributions_after_commit`,
  `delete_runs_as_a_task_walking_on_delete_in_requires_order_and_removes_the_row_last`,
  `a_failing_on_delete_leaves_the_account_deleting_with_the_error_on_the_task`.
- `products/demo/tests/demo/lifecycle.rs`: notes of a suspended account refuse edits and
  disappear with the account.

### 6b. the event stream's caps and the conformance events group

**Goal.** The second item M3 left undone: `SSE_MAX_CLIENTS` per session (four, the step's choice
recorded in the commit) with the oldest stream closed with a final `retry: 0` frame when the cap
is exceeded, the credential re-check at every keep-alive so a revoked session or token closes its
stream within one keep-alive period, and the `events` group of `keel-conformance`: a subscription
receives the `data.invalidate` of a write, `Last-Event-ID` replays within the window, a guest is
refused, a revoked session is closed, and the fifth stream of a session closes the first.

**Touches.** `crates/kernel/src/http/events.rs`, `crates/conformance/src/suite/events.rs` (new),
`hack/conformance.sh`.

**Out of scope.** Nothing else of the stream changes.

**Acceptance.**

- `crates/kernel/tests/kernel/events.rs`: `the_fifth_stream_of_a_session_closes_the_first_with_retry_zero`,
  `a_revoked_session_is_closed_at_the_next_keep_alive`.
- `hack/conformance.sh` runs the `events` group green against the binary.

### 5a. the dns bundle and dns-zones on bind

**Goal.** `products/dns` — the bundle crate with `MANIFEST` (id `dns`, brand KeelDNS with the
platform palette, modules `dns-zones`, `dns-records`, `dns-templates`, `host`), `i18n/en.json`
and `ru.json`, the `dns` section of `agent.yaml` (`drop_in_prefix: keel-dns-`) and the module
crate `products/dns/crates/dns-zones` (`dns-zones`): the table `dns_zone` (account, name, kind
`primary`, serial, refresh/retry/expire/minimum, template, state), the functions
`dns.zone.list|get|create|edit|delete` owned by account with the list screen under the menu group
`dns`, the `LimitDef` `dns.zones` per account, the `DnsProvider` trait with the bind
implementation that renders `bind/zone.j2` and the include drop-in `keel-dns-<zone>.conf` under
the `zones` and `named` roots through the executor of 3c, validated by `named-checkzone` and
reloaded with `named`, called from `after_commit` as [design/02-functions.md](design/02-functions.md)
shows; the `AccountLifecycle` of 6a removing an account's zones on delete and the `BackupSource`
of 2a contributing the zone files. The `ConfigTemplate` for the zone and the include come with
the module; the agent's `named` check and the `named` service are what the operator's `agent.yaml`
names. The binary carries `dns` behind the cargo feature `dns`, in the default set beside `demo`.

**Touches.** `products/dns/` (new), `products/dns/crates/dns-zones/` (new), `bin/keel/`,
`contracts/`, `.github/workflows/ci.yml` (the composition with `dns`), `frontend/products/dns/`
(the product package with the brand, no custom views).

**Out of scope.** Records (5b), templates (5c), secondary zones and AXFR (later), DNSSEC (later).

**Acceptance.**

- `products/dns/tests/dns/zones.rs` against the in-process agent of 3c's testkit with a stub
  `named-checkzone` and `rndc`: `a_zone_created_by_a_user_renders_its_file_and_include_after_commit_and_lists_under_its_account`,
  `a_zone_over_the_account_limit_is_refused_before_the_transaction`,
  `a_failing_check_leaves_the_row_but_marks_the_zone_with_the_error_and_the_problem`,
  `deleting_the_account_removes_its_zones_and_their_files`.
- `hack/descriptors.sh` and `hack/conformance.sh` run with `dns` enabled; the dumps and the
  OpenAPI document gain the `dns.zone.*` functions.

### 5b. dns-records

**Goal.** The module crate `dns-records`: the table `dns_record` (zone, type `A`, `AAAA`, `CNAME`,
`MX`, `TXT`, `NS`, `SRV`, `CAA`, name, value, ttl, priority, weight, port), the functions
`dns.record.list|create|edit|delete` owned through the zone's account, the per-type validation of
the value as `x-keel` checks the form shows before the call (an address for `A` and `AAAA`, a host
name for `CNAME`, `NS` and `MX`, the quoted text for `TXT`, the tag and value shape for `CAA`), the
`LimitDef` `dns.records` per zone, the serial bump of the zone on every change and the re-render
through the provider of 5a after commit, and the list screen reached from a zone's row with the
zone as the parent filter.

**Touches.** `products/dns/crates/dns-records/` (new), `products/dns/src/` (the module in the
manifest), `contracts/`.

**Out of scope.** Wildcards beyond bind's own rules; record sets as one row (later).

**Acceptance.**

- `products/dns/tests/dns/records.rs`: `a_record_of_every_type_validates_its_value_and_lands_in_the_rendered_zone`,
  `an_invalid_value_is_refused_on_the_field_before_the_transaction`,
  `every_change_bumps_the_serial_once_per_transaction`, `records_of_a_zone_over_the_limit_are_refused`.

### 5c. dns-templates

**Goal.** The module crate `dns-templates`: the table `dns_template` (name, owner account or
platform, the record set with the placeholders `{zone}`, `{ip}` and `{ip6}`), the functions
`dns.template.list|create|edit|delete` and the option source `dns.template.options` the zone form
of 5a uses through `x-keel.options`, applying a template on zone creation by expanding its records
into `dns_record` rows, and two platform templates shipped with the module (a web host with `A`,
`AAAA`, `CNAME www` and `MX`, and an empty one).

**Touches.** `products/dns/crates/dns-templates/` (new), `products/dns/crates/dns-zones/` (the
template field and the expansion hook), `contracts/`.

**Out of scope.** Editing an applied template's records in bulk (the records of a zone are rows
of 5b once applied).

**Acceptance.**

- `products/dns/tests/dns/templates.rs`: `a_zone_created_with_a_template_receives_its_expanded_records`,
  `a_template_of_another_account_is_not_offered_and_not_applied`, `the_platform_templates_are_present_after_the_first_start`.

### 5d. dns end to end: the browser, the CLI, the token and the VM

**Goal.** The "Done when" of the zone: `hack/e2e.sh` with `dns` enabled creates a zone and a
record from the browser through the descriptor-driven list and form of the shell, the CLI
`keel ctl dns.zone.create` and an `kat_` API token create zones too, and the three land in the
same list; `hack/dns-vm.sh` is the operator's proof on a VM with bind: the zone the browser created
is served by `dig @127.0.0.1 <zone> SOA`, and a template broken on purpose rolls back without
`named` stopping. The frontend product package `frontend/products/dns` carries the brand and the
`dns` group's icon, the contracts `contracts/i18n.json` gain the project `keel-dns`, and the
OwnLate project is operator-run.

**Touches.** `frontend/e2e/dns.spec.ts` (new), `hack/e2e.sh`, `hack/dns-vm.sh` (new),
`contracts/i18n.json`, `frontend/products/dns/`.

**Out of scope.** A DNS-specific custom view (the descriptor screens suffice for the alpha).

**Acceptance.**

- `frontend/e2e/dns.spec.ts` in the browser suite: a zone and a record created through the UI, a
  zone created over the CLI in the fixture and one with a token, all three listed.
- Operator-run, recorded in the checklist: `hack/dns-vm.sh` on a VM installed with
  `--product dns` shows the zone answered by bind and the broken template rolled back.

### 7. the dependency rehearsal

**Goal.** The first proof that a product's module crates can live outside the workspace: a CI job
`rehearsal` that builds `keel-bin` with `dns-zones`, `dns-records` and `dns-templates` taken from a
git revision of this repository — the head the job runs on, or the tag it is given, fetched from
GitHub with the job's token since the repository is private — through a throwaway workspace
`hack/rehearsal.sh` writes outside the checkout, instead of the workspace paths, and runs
`hack/prove-binary.sh` on the result. The step records in `hack/rehearsal.sh` what
had to be true of the crates for the build to succeed — their `Cargo.toml` naming `keel-sdk` by
version, their tests not reaching into the workspace — and fixes what is not.

**Touches.** `hack/rehearsal.sh` (new), `.github/workflows/ci.yml` (the job, on `main` only),
`products/dns/crates/*/Cargo.toml`.

**Out of scope.** Moving the crates to their own repositories (M7).

**Acceptance.**

- The job `rehearsal` is green on `main` and `hack/rehearsal.sh` fails when a module crate
  reaches a workspace path.

### 8a. the security review of the privileged path

**Goal.** The review the threat model of [design/06-host.md](design/06-host.md) schedules for M4,
run as an adversarial pass over `crates/host-agent`, `crates/keel/src/privileged/` and
`crates/release`: path traversal and symlink races in `host.fs.*` and `host.template.apply`, the
exec allowlist, the argument vectors of `systemctl` and the package manager, the request parser
(depth, size, unknown fields), the uid range and the re-exec, the manifest verifier and the archive
unpacker (path entries, sizes, permissions), the request file the root unit reads; every finding
fixed in this step with a test that reproduces it, and the findings without a fix recorded for step
8b. Fuzz harnesses for the frame parser and the manifest verifier under `crates/host-agent/fuzz`
and `crates/release/fuzz`, run for a bounded time in the job `check`.

**Touches.** `crates/host-agent`, `crates/keel/src/privileged/`, `crates/release`, the fuzz
directories, `.github/workflows/ci.yml`.

**Out of scope.** The M7 review.

**Acceptance.**

- Every finding is a named test in the crate it concerns; the two fuzz targets run one minute each
  in `check` without a crash.

### 8b. M4 closure

**Goal.** The document pass and the exit checklist: the choices the steps recorded amended into
[design/06-host.md](design/06-host.md), [design/09-operations.md](design/09-operations.md),
[design/03-access.md](design/03-access.md) and [design/05-modules.md](design/05-modules.md)
(`keel-storage` and the S3 target moved to M5, the flag spellings, the restart exit, the cap of the
event stream, the feature set of M4), `deploy/README.md` rewritten for the full installer,
`README.md`'s M4 row, the version of the workspace and the frontend packages raised to `0.2.0`,
`hack/check-docs.sh` green; the `## M4 exit checklist` with the "Done when" items ticked by test,
spec or command, and the operator-run items unticked and named: the tag `keel-v0.2.0` published to
the channel, the demo host updated from 0.1.0 to 0.2.0 from the button and rolled back once, a
fresh VM installed with `--product dns`, the backup of one VM restored on another with the second
factor working, the review findings without a fix.

**Touches.** `docs/`, `README.md`, `deploy/README.md`, `Cargo.toml`, `frontend/package.json` and
the package versions, `hack/check-docs.sh`.

**Out of scope.** Code beyond the version bump.

**Acceptance.**

- `hack/check-docs.sh` is green with the M4 names; every "Done when" item of M4 is ticked or
  marked operator-run in the new checklist; `keel --version` answers `0.2.0`.

## M5 work breakdown

M5 is split into twenty-five PR-sized steps in eight groups; a step is one change to `main` and
is done when its acceptance holds on the runner. The groups run in the order written except
where a step names what it waits for: the license (1a to 1c) first, because the lease replaces
the feature set M4 stubbed and decides which products may be enabled; phoenix-ui batch 3 (2a,
2b) next, in the globalart repository, because the stream renderer, the configuration editor
and two hosting forms need its components; `stats` (3) before the renderers (4a to 4d) that
chart it; the password reset (5) before `panel`, the first product whose users reach the panel
without an operator; the `panel` bundle in nine steps (6a to 6i), one per module, `hosting-web`
and the bridge to `dns` early because the sites, certificates, mail and backups hang off them;
the two custom views (7a, 7b) once the functions they drive exist; and the installer, the
review and the closure (8a to 8c). Every step that touches the operating system proves itself
unprivileged against stubs and the in-process agent of the testkit, as in M4, and names a
script under `hack/` for the operator's VM run with the real daemons; the VM transcripts and
the Let's Encrypt certificate are recorded in the exit checklist. Steps 1a to 8b touch nothing
under `docs/`; a step that decides something the design leaves open says so in its commit
message and step 8c amends the documents. The terminal and VNC of `panel`, and with them the
free-form raw route `r.raw`, are not in this milestone: its two custom views are the file
manager and the configuration editor, which the `upload` and `download` kinds and ordinary
functions serve. `dns-peer` is built as the remote side of the bridge answering `unavailable`
until M6 pairs a neighbour; the license server stays an open question and leases are issued by
hand with `keel-dev license issue`.

### 1a. the license module and the lease

**Goal.** The system module `license` of [design/05-modules.md](design/05-modules.md): the lease
is an Ed25519-signed JSON document — `instance_id`, `issued_at`, `expires_at`, `grace_days`, and
`products: [{ id, edition }]` — verified against the license key compiled into the binary (a
second key pair beside the release key of M4, public half in `crates/release/keys/license.pub`,
reusing the signing and verifying code of `keel-release`), bound to the installation's
`instance_id` and stored in `keel_kv`. `keel license install <file>` over the socket and
`license.install` (admin) verify and store it; `license.get` answers the state (`none`,
`valid`, `grace`, `expired`, `read_only`), the products with their editions, the expiry and the
grace end. The SDK gains editions on `ProductManifest`: `editions: &[Edition { id, features }]`,
the features of an edition being the ones its bundle declares for it; the lease's products and
editions give the installation's feature set, which replaces the M4 implementation of
`dyn Features` when `license` is loaded, and `/boot.features` and `/boot.license` carry it.
`license: None` in the composition keeps the M4 behaviour (every feature of every enabled
product) for internal builds and the test fixtures. The `dns` bundle declares the editions
`lite` and `pro`; `pro` adds the feature `dns.dnssec`, which gates `dns.zone.dnssec.get`, a read
of the zone's signing state that answers `unsigned` until DNSSEC signing arrives later; the
`demo` bundle declares one edition carrying all its features.

**Touches.** `crates/system/src/license/` (new module, feature `license`, in the matrix),
`crates/sdk/src/manifest.rs` (`Edition`), `crates/kernel/src/boot.rs`,
`crates/release/keys/license.pub` (new), `crates/keel/src/cli.rs`, `products/dns/`,
`products/demo/src/`, `contracts/`.

**Out of scope.** Degradation and the enable gate (1b); issuing leases (1c); a license server.

**Acceptance.**

- `crates/system/tests/system/license.rs`: `a_lease_signed_with_the_license_key_installs_and_a_changed_byte_does_not`,
  `a_lease_for_another_instance_id_is_refused`, `the_feature_set_is_the_union_of_the_listed_editions`,
  `license_none_keeps_every_feature_of_every_enabled_product`.
- `products/dns/tests/dns/editions.rs`: `a_lite_lease_hides_dns_zone_dnssec_get_from_the_menu_and_it_answers_license`
  (402), and a `pro` lease shows and serves it.

### 1b. grace, read-only degradation and the enable gate

**Goal.** Time and consequences of the lease: monotonic time kept in `keel_kv` — the largest
wall-clock value the installation has seen, advanced by a cron job and at every start — so a
clock moved backwards cannot revive an expired lease; the states `valid` until `expires_at`,
`grace` for `grace_days` after it (everything works and the problem `license.expiring` is
raised), and `read_only` after the grace: mutating functions (every function whose mode is not
`Read`) answer `keel.error.license` (402), reads, login and `license.install` keep working.
`keel.product.enable` refuses a product the lease does not list with `keel.error.license`; an
enabled product that a newly installed lease no longer lists turns read-only for its modules
alone with the problem `license.product_unlisted`, the rest of the installation unaffected, and
nothing is ever deleted. The degradation is a guard in the dispatcher beside the feature guard
of M4, so descriptors project the mutating actions as disabled with the reason.

**Touches.** `crates/system/src/license/`, `crates/kernel/src/dispatcher/guards.rs`,
`crates/kernel/src/keel/products.rs`, `crates/kernel/src/http/descriptors.rs`, `contracts/`.

**Out of scope.** The license server and online renewal.

**Acceptance.**

- `crates/system/tests/system/license_time.rs` on a shifted clock of the testkit:
  `the_installation_works_until_expires_then_in_grace_then_read_only`,
  `a_clock_moved_backwards_does_not_revive_an_expired_lease`,
  `read_only_keeps_reads_login_and_license_install`.
- `crates/keel/tests/license.rs`: `a_lease_that_lists_dns_alone_refuses_keel_product_enable_panel`
  (with a fixture product standing for `panel` until 6a),
  `an_enabled_product_the_new_lease_drops_turns_read_only_with_a_problem`.

### 1c. issuing leases and the size cap of the release binary

**Goal.** `keel-dev license keygen --out <dir>` and `keel-dev license issue --key <file>
--instance <id> --product dns:pro --product panel:lite --days 365 --grace 30 --out <file>`, the
operator's tool for the long offline leases of the first releases; the key pair for production
is generated by the operator and its private half never enters the repository, while the tests
use a throwaway pair and a test-only verifying key injected through the testkit. The cap of the
release binary, which [design/11-decisions.md](design/11-decisions.md) sets at the start of M5:
the Build workflow measures the stripped musl release binary carrying `demo`, `dns` and the
empty `panel` bundle skeleton against a cap written in `hack/size-cap` — the current size plus
the headroom `panel` needs, the step records both numbers — and fails above it; the shell's
product chunks are loaded lazily so a composition without a product's frontend pays nothing.

**Touches.** `crates/dev/src/license.rs` (new), `hack/size-cap` (new), `.github/workflows/build.yml`,
`frontend/apps/shell/` (lazy product chunks), `crates/testkit/`.

**Out of scope.** A license server; the storage of the production private key (operator).

**Acceptance.**

- `crates/dev/tests/license.rs`: `an_issued_lease_verifies_with_the_matching_key_and_installs`.
- The Build workflow fails when the release binary exceeds `hack/size-cap` (checked by a
  script test that feeds it a larger size); the frontend build emits one chunk per product.

### 2a. phoenix-ui batch 3: the code editor and the log view

**Goal.** In the globalart repository (`platform/frontend/libs/phoenix-ui`): `phx-code-editor`,
a text editor with line numbers, syntax highlighting for the configuration languages `panel`
edits (nginx, ini, yaml, json, shell) and a read-only mode, built on CodeMirror 6 as a
dependency of the library, never copied; and `phx-log-view`, a virtualised, follow-the-tail view
of log lines with levels, a pause and a filter, fed by an observable. Stories and tests as the
library's CI requires, the README sections, the version raised to 0.4.0.

**Touches.** `platform/frontend/libs/phoenix-ui/src/lib/` of globalart, its stories, README and
package.json.

**Out of scope.** The key-value field and the transfer list (2b); the pin in keel (2b).

**Acceptance.**

- The library's test and Storybook jobs green in globalart with stories `CodeEditor` and
  `LogView`, each with a test that types or appends and asserts the rendered result.

### 2b. phoenix-ui batch 3: the key-value field and the transfer list, the release and the pin

**Goal.** `phx-key-value-field` (a form control editing a map of string pairs, with key
validation and duplicates refused) and `phx-transfer-list` (two lists with move buttons and
search, a form control over the chosen ids); stories and tests; the release `phoenix-ui-v0.4.0`
tagged and published to Nexus through the library's workflow (the one push this group makes in
globalart), confirmed with `npm view @globalart/phoenix-ui@0.4.0`; and in keel the pin raised to
0.4.0 with the lockfile, the `/forms` library mapping the `x-keel` widgets `code`, `key_value`
and `transfer` onto the new controls.

**Touches.** globalart's library as in 2a; `frontend/package.json`, `frontend/libs/keel/forms/`,
`crates/types/` (the widget enum), `contracts/`.

**Out of scope.** Screens using them (4d, 6b, 7b).

**Acceptance.**

- globalart: stories `KeyValueField` and `TransferList` with tests; 0.4.0 on Nexus.
- keel: `frontend/libs/keel/forms` tests `a_code_widget_renders_the_code_editor`,
  `a_key_value_widget_edits_a_map`, `a_transfer_widget_edits_a_list_of_ids`.

### 3. stats

**Goal.** The system module `stats` of [design/05-modules.md](design/05-modules.md): the SDK
contribution `StatCollector { key, unit, interval, collect }`, the tables `stats_sample` (key,
subject, at, value) and `stats_agg` (key, subject, bucket, resolution `hour` or `day`, min, max,
sum, count) with rollover — raw samples kept for a setting-defined number of days, hourly
buckets for longer, daily beyond — and the aggregation and pruning as cron jobs; the functions
`stats.series { key, subject?, from, to, resolution }` answering points for charts and
`stats.keys` listing the collectors the principal may read, scoped by account for per-account
keys. The kernel contributes `keel.accounts` and `keel.principals` counts, `dns-zones` the zone
count per account, the fixture the dashboard and report steps need.

**Touches.** `crates/system/src/stats/` (new, feature `stats`, in the matrix), `crates/sdk/`,
`products/dns/crates/dns-zones/`, `contracts/`.

**Out of scope.** Host metrics beyond what a collector reads through the executor.

**Acceptance.**

- `crates/system/tests/system/stats.rs`: `a_collector_is_sampled_on_its_interval_and_aggregated_into_hours_and_days`,
  `old_samples_roll_over_into_buckets_and_are_pruned`, `a_per_account_series_is_scoped_to_the_callers_account`.

### 4a. the report renderer

**Goal.** The `report` kind end to end: the descriptor's `bands` (tables with totals) and
`charts` (`line`, `bar`, `pie` over a series function) as [design/10-contracts.md](design/10-contracts.md)
fixes them, the report renderer mapping bands onto `phx-list-table` and charts onto `phx-chart`,
the report's parameters as a form above it (period, account for an admin), export of a band
through `keel.export`. The first report is `dns.report.zones` (zones per account over time from
`stats`), and the demo bundle keeps a fixture report for the renderer's own tests.

**Touches.** `crates/types/` (report shapes), `crates/kernel/src/http/descriptors.rs`,
`frontend/libs/keel/renderer/src/report/` (new), `products/dns/`, `contracts/`.

**Out of scope.** Scheduled reports sent by mail.

**Acceptance.**

- Renderer tests: `a_report_renders_its_bands_with_totals_and_its_charts`, `the_parameters_form_refetches_the_report`.
- `frontend/e2e/report.spec.ts`: the dns zones report opens from the menu and shows a chart.

### 4b. the dashboard in full

**Goal.** The dashboard of `/`: `stat` widgets (a number with its unit and a sparkline from
`stats.series`) beside the `list` widgets of M3, laid out on `phx-widget-grid`, the widget order
and the hidden widgets kept in the preference `dashboard.layout` through `settings.pref.*`, a
drag to reorder and a menu to hide or restore; `DashboardWidget` contributions of kind `stat`
from `dns-zones` (zones) and the kernel (accounts), and `/boot.dashboard` in the widget order
the preference gives.

**Touches.** `frontend/libs/keel/shell/src/home/`, `crates/kernel/src/boot.rs`,
`crates/types/`, `products/dns/`, `contracts/`.

**Out of scope.** Widgets written by products in TypeScript.

**Acceptance.**

- Shell tests: `a_stat_widget_shows_its_value_and_sparkline`, `the_widget_order_follows_the_preference_and_a_drag_writes_it`.
- `frontend/e2e/dashboard.spec.ts`: reorder, reload, the order stays.

### 4c. tree and master-detail

**Goal.** The two layouts [design/08-frontend.md](design/08-frontend.md) names as `ui` choices:
`tree` — a list whose rows carry a parent field, rendered as an expandable tree with lazy
loading of children through the same list function filtered by parent — and `master-detail` — a
list on the left and the object renderer or a child list of the selected row on the right, the
selection in the route so it survives a reload. The account tree of the kernel
(`keel.account.list` with `parent`) takes the tree layout and `dns.zone.list` with its records
the master-detail layout, the first real uses.

**Touches.** `frontend/libs/keel/renderer/src/tree/`, `.../master-detail/` (new),
`crates/types/` (the `ui.layout` values), `crates/kernel/src/keel/accounts.rs` (the descriptor),
`products/dns/`, `contracts/`.

**Out of scope.** Drag and drop between tree nodes.

**Acceptance.**

- Renderer tests: `a_tree_loads_children_on_expand`, `master_detail_keeps_the_selection_in_the_route`.
- `frontend/e2e/layouts.spec.ts`: the account tree expands; a zone selected in master-detail
  shows its records and survives a reload.

### 4d. the stream kind and renderer

**Goal.** The `stream` kind: a function registered with `r.stream(name, handler)` yields lines
(`{ at, level, text }`) that the kernel serves as SSE on the raw route of the function behind
authentication and the guarded caller, with the access, audit and projection of any function;
the stream renderer shows them in `phx-log-view` from 2a with follow, pause and filter. The first
streams are `keel.logs.stream` (the log ring of M4, admin) and `dns.zone.log` (the provider's
apply history of a zone), both reachable from the menu and from a row action.

**Touches.** `crates/sdk/src/registrar.rs` (`stream`), `crates/kernel/src/http/` (the stream
route), `frontend/libs/keel/renderer/src/stream/` (new), `frontend/libs/keel/client/`,
`products/dns/crates/dns-zones/`, `contracts/`.

**Out of scope.** Streams writing back (the terminal).

**Acceptance.**

- `crates/kernel/tests/kernel/streams.rs`: `a_stream_serves_lines_as_sse_behind_auth_and_audits_the_open`,
  `a_stream_of_another_account_is_refused`.
- Renderer test `the_stream_renderer_follows_and_pauses`; e2e opens the kernel log stream.

### 5. password reset through notify

**Goal.** The reset [design/05-modules.md](design/05-modules.md) moves to M5: an address per
principal (`auth_principal_address`: principal, kind `email`, value, verified_at), set by the
principal on the security page with a verification mail and by an admin; a delivery to one
recipient in `notify` (`notify.send_to { principal, template, params }` over the email channel,
internal only); the guest functions `auth.password.reset.request { login }` — always answering
the same, rate-limited per address and per login, mailing a single-use link with a `kpr_` token
valid for a setting-defined time to a verified address — and `auth.password.reset.confirm {
token, password }`, which sets the password, revokes every session of the principal and
consumes the token; the shell's "Forgot password" link and the reset form.

**Touches.** `crates/system/src/auth/`, `crates/system/src/notify/`, `crates/system/i18n/`,
`frontend/libs/keel/shell/src/login/`, `contracts/`.

**Out of scope.** Reset over SMS; reset for principals without a verified address (the operator
resets them over the socket).

**Acceptance.**

- `crates/system/tests/system/password_reset.rs`: `a_reset_mails_a_single_use_link_that_sets_the_password_and_revokes_sessions`,
  `an_unknown_login_answers_the_same_and_sends_nothing`, `the_reset_request_is_rate_limited`.
- `frontend/e2e/reset.spec.ts` against the mail catcher: request, open the link from the mail,
  set the password, sign in.

### 6a. the panel bundle and hosting-users

**Goal.** `products/panel` — `MANIFEST` (id `panel`, brand KeelPanel, editions `lite` and `pro`,
the bridge field of [design/05-modules.md](design/05-modules.md) declared but served in 6c),
`i18n`, the `panel` section of `deploy/agent.yaml` (`user_prefix: hp_`, `drop_in_prefix:
keel-panel-`, the `file_roots` `www`, `vhosts`, `pools`, `mail`, `certs`, the services `nginx`,
`php*-fpm`, `postfix`, `dovecot`, `mariadb`, `vsftpd`) — and the module `hosting-users`: an
operating-system user per hosting user (`hosting_user`: account, name, uid, home, state) created
through `host.user.*` inside the uid range, the functions `hosting.user.list|create|edit|delete`
owned by account, the limit `hosting.users` per account, an `AccountLifecycle` that suspends
(locks the OS user) and removes them, and the `panel` product package in `frontend/products/panel`.
The binary carries `panel` behind the feature `panel` in the default set.

**Touches.** `products/panel/` (new), `products/panel/crates/hosting-users/` (new),
`deploy/agent.yaml`, `bin/keel/`, `frontend/products/panel/` (new), `.github/workflows/ci.yml`,
`contracts/`, `contracts/i18n.json` (project `keel-panel`).

**Out of scope.** Sites (6b) and everything else of the bundle.

**Acceptance.**

- `products/panel/tests/panel/users.rs` against the in-process agent with stubs: `a_hosting_user_is_an_os_user_inside_the_uid_range_under_hp`,
  `suspending_the_account_locks_its_users_and_deleting_removes_them`, `the_users_limit_is_enforced`.
- `a_lease_that_lists_dns_alone_refuses_keel_product_enable_panel` of 1b runs against the real
  `panel` bundle instead of its fixture.

### 6b. hosting-web: sites on nginx and php-fpm

**Goal.** The module `hosting-web`: a site (`hosting_site`: account, hosting user, domain,
aliases, document root under the user's home, php version or none, state) rendered through
`ConfigTemplate`s — the nginx server block `keel-panel-<site>.conf` validated by `nginx -t` and
reloaded, the php-fpm pool `keel-panel-<site>.conf` validated by `php-fpm -t` — through the
executor after commit, with rollback on a failed check as the agent does; the functions
`hosting.site.list|get|create|edit|delete`, a create wizard (user, domain, php, DNS zone yes or
no), the limit `hosting.sites`, the lifecycle. The module declares `DnsProvider` as the trait it
requires; until 6c lands the tests provide a fake.

**Touches.** `products/panel/crates/hosting-web/` (new), its templates, `products/panel/src/`.

**Out of scope.** Certificates (6d); the bridge (6c).

**Acceptance.**

- `products/panel/tests/panel/sites.rs`: `a_site_renders_its_server_block_and_pool_after_commit_and_reloads`,
  `a_failing_nginx_check_leaves_the_previous_config_and_marks_the_site`,
  `deleting_a_site_removes_its_files_and_reloads`.

### 6c. dns-local, dns-peer and the bridge

**Goal.** The bridge of [design/07-ecosystem.md](design/07-ecosystem.md): `Bridge { to: "dns",
local: "dns_local", remote: "dns_peer" }` on `ProductManifest`, the loader choosing at start —
`dns-local` when `dns` is enabled on the installation, `dns-peer` otherwise — and both modules
providing `DnsProvider` to `hosting-web`: `dns-local` through `call.invoke` of `dns.zone.*`
inside the hosting call's transaction under the same principal and account, so the site and its
zone commit together and a DNS validation error comes back as a field error of the hosting
form; `dns-peer` answering `keel.error.unavailable` with the reason that no neighbour is paired
(M6). `keel product disable dns` while `panel` bridges to it locally is refused unless
`--switch-remote` is passed.

**Touches.** `crates/sdk/src/manifest.rs` (`Bridge`), `crates/kernel/src/loader/`,
`products/panel/crates/dns-local/`, `products/panel/crates/dns-peer/` (new),
`crates/kernel/src/keel/products.rs`, `contracts/`.

**Out of scope.** Peers (M6).

**Acceptance.**

- `products/panel/tests/panel/bridge.rs`: `a_site_with_a_zone_commits_both_in_one_transaction_under_the_same_account`,
  `a_dns_validation_error_reaches_the_hosting_form_as_a_field_key`,
  `without_dns_the_bridge_falls_to_dns_peer_which_answers_unavailable`,
  `disabling_dns_under_a_local_bridge_needs_switch_remote`.

### 6d. hosting-ssl: ACME

**Goal.** The module `hosting-ssl`: certificates per site from an ACME directory (Let's Encrypt
production and staging, the directory a setting), the HTTP-01 challenge served from a
`keel-panel-acme` location in the site's server block, the account key sealed as a `Secret`,
issuance and renewal as tasks (renewal by a daily cron job thirty days before expiry), the
certificate and key written into the `certs` root through the executor and the server block
re-rendered with TLS; an uploaded certificate as the alternative; the problem
`hosting.certificate.expiring`. The create wizard of 6b gains "Let's Encrypt certificate".
The ACME client is a pinned crate (`instant-acme` or an equal), the step says which.

**Touches.** `products/panel/crates/hosting-ssl/` (new), `products/panel/crates/hosting-web/`
(the TLS block), `Cargo.toml`.

**Out of scope.** DNS-01 and wildcard certificates (later, through `dns-local`).

**Acceptance.**

- `products/panel/tests/panel/ssl.rs` against a local ACME test server (pebble in a container
  in CI, as the mail catcher is): `a_site_gets_a_certificate_over_http01_and_serves_tls`,
  `renewal_runs_thirty_days_before_expiry`.
- Operator-run in 8a: a site on a real domain with a Let's Encrypt certificate is served.

### 6e. hosting-db

**Goal.** The module `hosting-db`: MariaDB databases and database users per account
(`hosting_database`, `hosting_db_user`), created and dropped through an `AgentOp` `mariadb.exec`
compiled into the agent that runs a fixed statement set with quoted identifiers through the
`mariadb` client under the policy's `exec_allow`, never a statement from a request; passwords
as `Secret`s; grants per database; the limits; the lifecycle; the backup source dumping each
database.

**Touches.** `products/panel/crates/hosting-db/` (new), `crates/host-agent/src/ops/` (the op),
`deploy/agent.yaml`.

**Out of scope.** PostgreSQL databases (later).

**Acceptance.**

- `products/panel/tests/panel/db.rs` with a stub `mariadb`: `a_database_and_user_are_created_with_quoted_identifiers`,
  `a_name_outside_the_account_prefix_is_refused`, `deleting_the_account_drops_its_databases`.

### 6f. hosting-mail

**Goal.** The module `hosting-mail`: mail domains and mailboxes per account (`hosting_mail_domain`,
`hosting_mailbox` with a password hash in the format dovecot reads, quota, aliases and
forwards), rendered as the virtual maps of postfix and the passwd-file of dovecot through
`ConfigTemplate`s validated by `postfix check` and `doveconf -n`, with `postmap` through
`exec_allow`; the MX and the SPF record through the bridge when the domain has a zone; limits,
lifecycle, the backup source of the maildirs.

**Touches.** `products/panel/crates/hosting-mail/` (new), its templates, `deploy/agent.yaml`.

**Out of scope.** Webmail; DKIM signing (later).

**Acceptance.**

- `products/panel/tests/panel/mail.rs`: `a_mailbox_lands_in_the_virtual_maps_and_the_passwd_file`,
  `a_domain_with_a_zone_gets_mx_and_spf_through_the_bridge`, `a_failing_postfix_check_rolls_back`.

### 6g. hosting-ftp

**Goal.** The module `hosting-ftp`: FTP accounts chrooted to a directory of a site
(`hosting_ftp_user`), served by vsftpd with virtual users from a user file rendered through a
`ConfigTemplate` and validated, mapped onto the site's operating-system user; limits and
lifecycle.

**Touches.** `products/panel/crates/hosting-ftp/` (new), its templates.

**Out of scope.** SFTP-only accounts (the hosting user's shell covers them).

**Acceptance.**

- `products/panel/tests/panel/ftp.rs`: `an_ftp_account_is_chrooted_under_its_site`,
  `a_directory_outside_the_site_is_refused`.

### 6h. hosting-backup

**Goal.** The module `hosting-backup`: per-site backups — the site's files, its databases and
mailboxes — as tasks into the `backups` root of the account, a schedule per site as a cron
job, retention by count, restore of a site from an archive as a task, and the archives offered
for download; the installation backup of M4 carries the module's own tables through
`BackupSource`.

**Touches.** `products/panel/crates/hosting-backup/` (new).

**Out of scope.** Remote targets (with `keel-storage`, later).

**Acceptance.**

- `products/panel/tests/panel/backup.rs`: `a_site_backup_contains_files_databases_and_mail_and_restores`,
  `retention_keeps_the_newest_archives`.

### 6i. hosting-services

**Goal.** The module `hosting-services`: the services the policy lists for `panel` with their
state, `start`, `stop`, `restart` and `reload` for an admin through `host.service.*`, the stat
collectors of their memory and uptime, and the problem `hosting.service.down` raised by a poll.

**Touches.** `products/panel/crates/hosting-services/` (new).

**Out of scope.** Installing packages from the UI.

**Acceptance.**

- `products/panel/tests/panel/services.rs`: `a_listed_service_restarts_and_an_unlisted_one_is_refused`,
  `a_stopped_service_raises_a_problem_the_next_poll_clears`.

### 7a. the file manager

**Goal.** The first custom view of `panel`, `hosting.file-manager`, registered with `r.view` and
written in `frontend/products/panel`: browsing a hosting user's home through the functions
`hosting.file.list|mkdir|rename|delete|chmod` and the `upload` and `download` kinds, all through
the executor under the hosting user's uid (the agent's impersonation of M4), never outside the
home; phoenix-ui components only.

**Touches.** `products/panel/crates/hosting-users/` (the file functions), `frontend/products/panel/src/file-manager/` (new).

**Out of scope.** Archives packed in the browser; editing files (7b edits configurations only).

**Acceptance.**

- `products/panel/tests/panel/files.rs`: `a_path_escaping_the_home_is_refused`, `files_are_written_as_the_hosting_user`.
- `frontend/e2e/file-manager.spec.ts`: upload, rename, download, delete.

### 7b. the configuration editor

**Goal.** The second custom view, `hosting.config-editor`: the overrides of a site's server
block and pool — the `ConfigTemplate` overrides of M4 per site — edited in `phx-code-editor`
with the rendered result beside it, saved through `hosting.config.set` which renders, validates
with the named check and reloads through the executor, answering the check's output as a field
error on failure and keeping the previous file.

**Touches.** `products/panel/crates/hosting-web/` (the config functions), `frontend/products/panel/src/config-editor/` (new).

**Out of scope.** Editing arbitrary files of the host.

**Acceptance.**

- `products/panel/tests/panel/config.rs`: `an_override_that_fails_nginx_t_is_refused_with_the_output_on_the_field`.
- `frontend/e2e/config-editor.spec.ts`: edit, save, see the rendered block; `keel-dev views share`
  reports exactly two views for `panel` and a share of at most 10 %.

### 8a. install --product panel, the end-to-end and the VM

**Goal.** `install.sh --product panel` on a clean host (the packages `panel` needs installed by
the installer through the package manager), `keel product enable panel` on a host running `dns`
yielding one login with the platform brand and a group per product in the menu; `hack/e2e.sh`
with both products: a site with a zone created by the wizard through `dns-local`, a DNS error
on the hosting form's field; `hack/panel-vm.sh` for the operator: a clean Debian VM, the
install, a site on a real domain with a Let's Encrypt certificate served over TLS.

**Touches.** `deploy/install.sh`, `hack/e2e.sh`, `frontend/e2e/panel.spec.ts` (new),
`hack/panel-vm.sh` (new), `hack/check-install.sh`.

**Out of scope.** Upgrading a host from a foreign panel.

**Acceptance.**

- `frontend/e2e/panel.spec.ts` green; `hack/check-install.sh` asserts the `panel` packages and
  units; operator-run: `hack/panel-vm.sh` transcript with the served certificate.

### 8b. the security review of panel

**Goal.** An adversarial pass over the modules of `panel` and the license: paths of files, FTP
chroots and backups; template injection through domains, aliases and overrides; the `mariadb`
statement set; mail maps; the ACME challenge location; the lease verifier and the clock; the
reset tokens. Every finding fixed with a test that reproduces it; findings without a fix recorded
for 8c.

**Touches.** the modules of `products/panel`, `crates/system/src/license/`, `crates/system/src/auth/`.

**Out of scope.** The M7 review.

**Acceptance.**

- Every finding is a named test in the crate it concerns.
- `the_support_bundle_carries_no_secret_value` covers an installation with `panel`: no mailbox
  or database password and no ACME account key appears in any entry.

### 8c. M5 closure

**Goal.** The document pass and the exit checklist: the choices the steps recorded amended into
the design documents, `deploy/README.md` for `panel`, `README.md`'s M5 row, the versions raised to
`0.3.0`, `hack/check-docs.sh` green; the `## M5 exit checklist` with the "Done when" items ticked
by test, spec or command and the operator-run items named: the production license key pair and a
lease issued for the demo host, the Let's Encrypt site on the VM, the tag `keel-v0.3.0` on the
channel and the demo host updated from the button, the OwnLate project `keel-panel`.

**Touches.** `docs/`, `README.md`, `deploy/README.md`, the versions.

**Out of scope.** Code beyond the version bump.

**Acceptance.**

- `hack/check-docs.sh` green with the M5 names; every "Done when" item of M5 ticked or marked
  operator-run; `keel --version` answers `0.3.0`.

## How we work

- The code lives in the monorepo `GlobalArtInc/keel` on the `main` branch until the SDK reaches
  1.0; a product's crates may move to their own repository after M7 and are pulled into the
  `keel` build as dependencies ([design/01-overview.md](design/01-overview.md)).
- One task is one PR-sized change; a task is not split across branches.
- Verify with the same commands CI runs before pushing, in CI's order, which the README lists. The
  job `check`: `cargo fmt --all --check`; in `frontend/`, `pnpm install --frozen-lockfile` and `pnpm
  build`, since `cargo build -p keel-bin` embeds `frontend/dist/shell`; then `cargo clippy
  --workspace --all-targets -- -D warnings`, `cargo test --workspace` against the Postgres server
  `KEEL_TEST_DATABASE_URL` names, `env -u KEEL_TEST_DATABASE_URL cargo test -p keel-testkit` against
  the container the testkit starts, `cargo run -p keel-dev -- types --out contracts/schema --check`,
  `cargo test -p keel-bin --no-default-features`, `cargo build -p keel-bin --no-default-features
  --features demo`, `cargo build -p keel-bin --no-default-features --features dns`, `cargo build
  -p keel-bin`, `hack/prove-binary.sh target/debug/keel`, `hack/check-install.sh
  target/debug/keel`, `hack/check-release.sh target/debug/keel`, `hack/conformance.sh
  target/debug/keel`, `hack/descriptors.sh target/debug/keel`, `cargo run -p keel-dev -- i18n
  missing --dump contracts/dump`, `pnpm exec playwright install chromium` once and `hack/e2e.sh
  target/debug/keel`, then `hack/check-test-databases.sh`, `hack/check-deps.sh` and
  `hack/check-docs.sh`; the scripts that start the binary speak https to it with the certificate it
  generates under the site's data directory. The job `frontend`, in `frontend/`: `pnpm install
  --frozen-lockfile`, `pnpm generate` followed by `git diff --exit-code`, `pnpm lint`, `pnpm
  format`, `pnpm test`, `pnpm build`, `pnpm build-storybook`, `pnpm exec http-server
  storybook-static --port 6006 --silent &`, `pnpm exec wait-on http://127.0.0.1:6006/index.json` and
  `pnpm test-storybook`, `node tools/stamp.mjs` and, in `frontend/dist/keel`, `pnpm publish
  --dry-run --no-git-checks`. The job `system` runs the `keel-system` matrix, `cargo clippy` and
  `cargo test -p keel-system --no-default-features --features <set>` over eighteen sets — each of
  the nine modules alone, `tasks,cron,notify,update` and `tasks,backup`, the seven modules of M3
  without each of the M1 modules and without `tasks` in turn, and the three nested prefixes
  `auth,settings,audit,tasks`, `…,cron` and `…,cron,notify` — against Postgres and the mail
  catcher. The job `rehearsal` builds the shell with `pnpm install --frozen-lockfile` and `pnpm
  build` in `frontend/`, then runs `hack/rehearsal.sh "$GITHUB_SERVER_URL/$GITHUB_REPOSITORY" "$GITHUB_SHA"`
  and `hack/check-test-databases.sh` against Postgres. The Build workflow builds the musl release
  with its size check, after `keel-dev i18n pull` when the translation token is present, and
  `release.yml` publishes an annotated `keel-v*` tag to the release channel.
- A gap in `ru` is a red build: `keel-dev i18n missing` runs in the job `check` after
  `hack/descriptors.sh`, and the dictionaries are the per-language source between pulls.
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
      of commit `205b8fb` records the run of the spike's script `spike-vm.sh` on `keel-demo` — the call as
      `nobody` refused with `refused peer_uid=65534 admitted_uid=999`, `host.user.create`
      answering `created: true` and `id -u kd_spike` working, the vhost written and answering the
      known body, the apply with `listen eighty` answering `host.error.validate_failed` with
      nginx's message and the vhost byte-identical to the previous one, and `--cleanup` leaving
      no trace; nothing of the spike changed between that commit and the M2 closing one. The
      mechanism was checked in CI by the spike's own tests and shipped by the Build workflow as
      the artifact `keel-spike-x86_64-linux-musl` until the close of M4 removed the spike, its
      script and the artifact; `keel-host-agent` carries the mechanism since, in
      `crates/host-agent/tests/agent.rs` (`a_peer_that_is_not_the_configured_uid_is_refused`,
      `template_apply_renders_swaps_validates_reloads_and_reports_changed`,
      `a_failing_check_rolls_the_file_back_and_a_failing_reload_reloads_again`,
      `a_request_whose_product_has_no_section_is_refused`,
      `users_and_groups_are_created_inside_the_uid_range_under_the_product_prefix`), proven on a
      machine by `hack/host-vm.sh`.
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
      no workspace crate, next to the M0 and M1 rules; the second rule left with the spike at the
      close of M4.
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

## M3 exit checklist

Every ticked item below is checked by CI on `main` — the tests of the workspace, the fourteen-set
`keel-system` matrix against Postgres and the mail catcher, the build of `keel-bin` with and
without the `demo` feature after `pnpm build`, `hack/prove-binary.sh`, `hack/check-install.sh`,
`hack/conformance.sh`, `hack/descriptors.sh`, `keel-dev i18n missing` and `hack/e2e.sh` against
the built binary over https, the job `frontend` with Jest, the static Storybook and its runner,
and `hack/check-docs.sh` over this checklist. The items left unticked are run by the operator on
`keel-demo`, by the owner in a browser, by `publish-npm.yml` and in the translation service; each
is ticked once it holds, the `keel-demo` ones with the date-free note "confirmed by hand on
keel-demo". Two items of the M3 scope were planned and left undone; they are listed last, unticked,
so the milestone that needs them picks them up.

- [x] `note.export` runs as a task, and its progress and log are visible in the browser and in `ctl
      task.get`:
      `note_export_runs_as_a_task_with_progress_a_log_line_per_note_and_a_checkpoint_after_each` and
      `note_export_ends_with_exported_and_a_download_that_task_download_streams_as_the_callers_csv`
      in `products/demo/tests/demo/tasks.rs`; the task entrance of the dispatcher in
      `a_ui_result_task_function_answers_a_task_ref_and_its_handler_runs_in_the_worker`,
      `a_job_run_by_the_worker_does_not_spawn_a_second_job`,
      `a_within_hook_vetoes_inside_the_job_and_an_idempotency_key_replays_the_task_ref`,
      `a_composition_with_a_ui_result_task_function_and_no_runner_fails_freeze_naming_the_function`
      and `a_secret_field_on_a_ui_result_task_function_fails_freeze` in
      `crates/kernel/tests/kernel/tasks.rs`; the worker in
      `a_spawned_job_runs_under_the_stored_principal_and_records_progress_steps_and_log_lines`,
      `task_progress_frames_of_a_job_arrive_on_the_owners_stream_in_order`,
      `jobs_of_two_classes_run_concurrently_while_jobs_of_one_class_run_in_order`,
      `two_kernels_on_one_database_run_one_worker_and_the_second_takes_over_when_the_first_stops`,
      `task_get_shows_the_last_step_the_log_tail_and_task_cancel_ends_a_paced_job_cancelled`,
      `task_get_masks_the_secret_fields_of_the_parameters_while_the_job_ran_them_raw` and
      `the_task_descriptors_match_their_snapshots_and_the_menu_carries_the_node` in
      `crates/system/tests/system/tasks.rs`; `ctl task.get` reading `done`, `4/4`, the download and
      one `exported <title>` line per note in `hack/prove-binary.sh target/debug/keel`; the browser
      in `frontend/e2e/tasks.spec.ts` ("alice exports the notes through the progress modal", "the
      paced export shows the titles, opens the details and cancels", "the paced export runs to the
      end and the drawer lists the jobs") through `hack/e2e.sh target/debug/keel`; the modal and the
      drawer under Jest in `frontend/libs/keel/renderer/src/tasks.spec.ts` ("a TaskRef answer opens
      the progress modal, frames drive value and status, cancel posts task.cancel and done hands
      back to the opener", "a result with a download member fetches the download route and hands the
      blob to the browser", "a failed frame shows the error and the request id", "a modal without
      frames polls task.get") and `frontend/libs/keel/shell/src/task-drawer.spec.ts`.
- [x] Cancellation works: `note_export_is_cancelled_between_two_notes_when_paced` in
      `products/demo/tests/demo/tasks.rs`,
      `a_cancelled_job_stops_at_its_next_checkpoint_and_ends_cancelled` in
      `crates/system/tests/system/tasks.rs`, and "the paced export shows the titles, opens the
      details and cancels" in `frontend/e2e/tasks.spec.ts`.
- [x] A task with a checkpoint survives a restart:
      `note_export_interrupted_by_a_restart_resumes_from_its_checkpoint_and_writes_no_note_twice`
      in `products/demo/tests/demo/tasks.rs`,
      `a_job_running_at_stop_is_interrupted_and_a_resumable_one_restarts_from_its_checkpoint` and
      `stop_lets_a_running_job_reach_its_checkpoint_and_leaves_queued_jobs_for_the_next_start` in
      `crates/system/tests/system/tasks.rs`; the CI counterpart of `systemctl restart` in
      `hack/prove-binary.sh target/debug/keel`, which stops `serve` with `SIGTERM` during a paced
      `note.export`, reads the `interrupted` row with its checkpoint, starts `serve` again and
      reads `done` from `task.get` with every note written once in the log tail and in the export
      file. The `systemctl restart` itself on `keel-demo` is part of the operator-run item below.
- [x] A cron job raises a problem, the problem is visible on the dashboard and reaches an SMTP
      catcher as an email in the end-to-end test:
      `an_overdue_todo_raises_note_overdue_for_its_account_after_the_poll_and_a_done_one_clears_it`
      in `products/demo/tests/demo/problems.rs`;
      `a_raised_problem_reaches_the_mail_catcher_and_the_test_button_sends_a_test_email` in
      `crates/system/tests/system/notify_mail.rs` against the mail catcher of `KEEL_TEST_SMTP_URL`;
      the registry, the poll job and the counters in
      `raise_is_idempotent_by_key_and_subject_and_clear_closes_the_problem_once`,
      `a_source_polled_by_the_cron_job_raises_what_it_reports_and_clears_what_it_stops_reporting`,
      `a_raised_problem_notifies_the_admins_once_counts_in_boot_and_feeds_the_dashboard_widget`,
      `an_account_scoped_problem_reaches_the_accounts_principals_and_shows_in_their_boot_counts`,
      `a_notification_frame_arrives_on_the_recipients_stream_when_a_message_is_written` and
      `notify_message_create_is_refused_over_http_and_the_socket_and_runs_through_the_caller` in
      `crates/system/tests/system/notify.rs`; the channels and the ladder in
      `a_message_becomes_a_delivery_per_channel_within_a_second_and_the_cursor_survives_a_restart`,
      `a_refused_smtp_connection_follows_the_ladder_and_the_fifth_failure_raises_notify_delivery_failed`
      and
      `an_unimplemented_channel_kind_is_refused_on_kind_and_the_email_password_is_sealed_and_masked`
      in `crates/system/tests/system/notify_channels.rs`; the scheduler in
      `a_job_runs_when_the_clock_passes_its_next_occurrence_and_records_an_ok_run`,
      `a_job_still_running_is_not_started_again_and_the_occurrence_is_recorded_skipped`,
      `two_kernels_on_one_database_elect_one_leader_and_the_other_takes_over_after_a_stop`,
      `a_cron_overrides_entry_replaces_the_contributed_schedule_and_an_unparsable_one_is_ignored`
      and `cron_job_run_starts_a_job_now_and_answers_conflict_while_it_runs` in
      `crates/system/tests/system/cron.rs`;
      `dashboard_widgets_and_problem_counts_come_from_the_contributions_and_stay_empty_without_them`
      in `crates/kernel/tests/kernel/boot.rs`; the end-to-end walk in `hack/e2e.sh
      target/debug/keel`, which creates the email channel to the catcher, an overdue todo and runs
      `ctl cron.job.run id=notify.problems.poll`, checks that the catcher lists the mail whose
      subject names `note.overdue` from and to the channel's addresses, and runs
      `frontend/e2e/notifications.spec.ts` ("root sees the problem on the dashboard and in the
      centre", "the notes item of alice carries the badge and the help item opens the about drawer
      with the version"); the centre, the badges and the home under Jest in
      `frontend/libs/keel/shell/src/notifications.spec.ts` and
      `frontend/libs/keel/shell/src/home.spec.ts`.
- [x] A user receives `data.invalidate` only for their own objects:
      `a_user_receives_data_invalidate_for_its_own_objects_alone_while_an_admin_receives_every_one`,
      `a_reconnect_replays_after_last_event_id_and_an_id_outside_the_window_gets_meta_invalidate`,
      `the_after_query_member_is_read_like_the_header`,
      `a_guest_is_refused_and_a_bearer_token_gets_a_stream`,
      `live_frames_reach_the_addressed_principal_and_the_admins`,
      `live_frames_of_an_account_reach_its_reseller_and_not_a_user_of_another_account` and
      `every_open_stream_closes_when_the_runtime_drains_so_stop_returns_within_a_second` in
      `crates/kernel/tests/kernel/events.rs`; the subject of an event in
      `note_saved_names_the_notes_account_as_its_subject` in `products/demo/tests/demo/note.rs`; the
      browser in `frontend/e2e/events.spec.ts` ("an edit over the API reaches the open list of its
      account alone"); the client in `frontend/libs/keel/client/src/events.spec.ts` ("the
      subscription opens with credentials after a boot with a principal and closes on logout",
      "frames are dispatched by type and typed, and an unparsable one is dropped and counted", "the
      last event id is kept and a self-made reconnect passes it as after", "meta.invalidate emits
      metaChanged, drops the descriptor cache and refetches boot once"). The conformance suite has
      no `events` group: the stream is covered by the kernel tests and the browser suite, and the
      group joins the suite with the per-session cap listed below.
- [x] The outbox is read through one seam in commit order:
      `outbox_subscriber_answers_events_after_a_seq_with_the_instance_id_and_wakes_a_waiter_on_commit`
      and
      `a_transaction_writing_an_event_waits_for_the_earlier_writer_to_commit_so_after_never_skips_a_row`
      in `crates/kernel/tests/kernel/outbox.rs`; the webhooks consumer in
      `a_subscription_receives_a_signed_delivery_for_a_matching_event_and_the_signature_verifies`,
      `an_event_outside_the_masks_or_outside_the_subscribers_scope_creates_no_delivery`,
      `a_subscription_placed_in_another_account_receives_that_accounts_events_alone`,
      `a_failed_delivery_follows_the_ladder_and_its_fifth_failure_is_a_problem_a_retry_clears`,
      `the_consumer_resumes_from_its_cursor_after_a_restart`,
      `a_private_or_plain_url_is_refused_on_url_unless_webhook_private_targets_is_set`,
      `the_sender_connects_to_the_address_it_checked_and_not_to_one_the_name_answers_later`,
      `the_sender_follows_no_redirect` and
      `a_short_secret_is_refused_on_secret_and_a_generated_one_is_32_bytes` in
      `crates/system/tests/system/webhooks.rs`,
      `a_subscription_secret_is_answered_once_sealed_at_rest_and_masked_in_audit` in
      `crates/system/tests/system/webhooks_secret.rs`, and the `webhook` group of the conformance
      suite, which subscribes over the wire, receives a signed delivery on a listener of its own and
      verifies `X-Keel-Signature` within the window, through `hack/conformance.sh
      target/debug/keel`.
- [x] `keel.export` with the row threshold, the `download` kind and the export button:
      `an_export_streams_every_page_under_the_screens_scope_and_strip_with_translated_headers`,
      `an_export_above_the_row_threshold_answers_a_task_ref_and_writes_the_file_inside_the_job` and
      `keel_export_of_a_function_the_caller_may_not_call_is_not_found` in
      `crates/kernel/tests/kernel/export.rs`;
      `a_download_is_served_and_audited_on_its_raw_route_and_refused_on_the_call_route_and_the_socket`
      in `crates/kernel/tests/kernel/download.rs`;
      `a_jobs_stored_file_ends_in_a_download_ref_that_task_download_serves`,
      `task_download_of_another_accounts_job_or_of_a_job_without_a_file_is_not_found` and
      `an_export_job_above_the_threshold_ends_in_a_download_ref_that_task_download_serves` in
      `crates/system/tests/system/tasks_download.rs`; the `export` group of the conformance suite
      and the export of `note.list` as `root` and as `alice` in `hack/prove-binary.sh
      target/debug/keel`; the dropdown in `frontend/libs/keel/renderer/src/export.spec.ts` ("the
      export dropdown lists the formats and posts keel.export with the table’s query and the visible
      columns in order", "a TaskRef answer opens the progress modal and a file answer is handed to
      the browser") and "the export button downloads a csv whose header row names the visible
      columns" in `frontend/e2e/notes.spec.ts`.
- [x] Group editing through `ui.multi`, the batch threshold and the multi-row form:
      `a_group_edit_merges_the_masked_fields_over_ui_read_into_every_row_inside_one_transaction`,
      `a_mask_outside_ui_multi_or_a_field_outside_the_mask_answers_422_on_the_offending_path`,
      `a_group_edit_with_one_foreign_id_is_not_found_and_changes_nothing`,
      `a_group_edit_above_the_threshold_becomes_a_job_of_chunks` and
      `ui_multi_naming_the_key_a_secret_or_an_unknown_field_fails_freeze` in
      `crates/kernel/tests/kernel/group.rs`;
      `a_batch_above_the_threshold_becomes_a_job_of_chunks_and_one_at_the_threshold_runs_inline` and
      `a_batch_with_one_foreign_id_is_not_found_before_any_job_is_spawned` in
      `crates/kernel/tests/kernel/tasks.rs`;
      `note_delete_above_ten_ids_becomes_a_task_that_deletes_in_chunks_of_ten` in
      `products/demo/tests/demo/tasks.rs` and
      `note_edit_as_a_group_sets_kind_due_and_tags_of_the_selected_notes_and_becomes_a_task_above_ten`
      in `products/demo/tests/demo/note.rs`; the form in
      `frontend/libs/keel/renderer/src/multi-form.spec.ts` ("the selection bar offers edit when the
      form has ui.multi and the modal posts ids, fields and mask for the toggled fields alone", "a
      422 on mask lands on its row and a GroupResult reloads the table") and "two selected notes are
      edited as a group to kind todo with a due date" in `frontend/e2e/notes.spec.ts`; column order
      and the cell editor in `frontend/libs/keel/renderer/src/columns.spec.ts`.
- [x] The `upload` kind over the raw route and the brand logo as its first upload:
      `a_multipart_upload_on_its_raw_route_lands_the_file_in_the_parameters_and_the_audit_record`,
      `a_part_above_max_bytes_or_of_another_type_or_not_utf8_is_refused_on_the_field`,
      `a_control_file_field_without_x_keel_file_fails_freeze`,
      `the_call_route_refuses_an_upload_and_the_socket_takes_the_field_inline_under_max_bytes`,
      `a_cross_site_multipart_post_is_refused_403` and `an_upload_descriptor_carries_its_form_block`
      in `crates/kernel/tests/kernel/upload.rs`;
      `a_plain_svg_logo_is_served_for_its_host_and_an_unsafe_one_is_refused_naming_the_reason` and
      `settings_brand_set_no_longer_takes_a_logo_and_leaves_a_stored_one_in_place` in
      `crates/system/tests/system/settings_brand.rs`;
      `the_demo_logo_is_filled_with_the_platform_palette` in `products/demo/src/lib.rs`; the logo
      upload over `ctl` in `hack/prove-binary.sh target/debug/keel`; the file control and the
      multipart submit in `frontend/libs/keel/forms/src/field.component.spec.ts` and
      `frontend/libs/keel/forms/src/submit.spec.ts`, the upload in
      `frontend/libs/keel/client/src/client.spec.ts`, the scoped labels in
      `frontend/libs/keel/shell/src/labels.spec.ts`, and "the white-label screen says white-label by
      host name, hue and chroma say from the product when empty, an uploaded svg logo shows after a
      reload and a scripted one is refused on the field" in `frontend/e2e/system.spec.ts`. The SVG
      hardening of step 8c beyond the script check (DTD, namespaces, `animate`, event attributes,
      style urls) is not built and waits for the first product that takes a logo from a customer.
- [x] TLS from the first start and `trusted_proxies`:
      `bind_tls_serves_healthz_to_a_client_trusting_the_certificate_and_refuses_one_that_does_not`
      and `bind_tls_stops_within_its_grace_with_a_stream_open` in
      `crates/kernel/tests/kernel/tls.rs`;
      `the_runtime_listens_on_tls_alone_and_a_plain_request_gets_no_http_answer` in
      `crates/keel/tests/runtime.rs`; the fingerprint in the log and no key in it in
      `the_binary_enables_demo_serves_and_answers_ctl_and_http_with_the_envelope` in
      `bin/keel/tests/binary.rs`;
      `x_forwarded_for_is_honoured_from_a_trusted_proxy_alone_and_the_rightmost_untrusted_entry_wins`,
      `a_same_site_post_is_admitted_by_the_forwarded_host_behind_a_trusted_proxy_alone` and
      `a_malformed_forwarded_for_or_host_falls_back_to_the_peer_and_the_host_header` in
      `crates/kernel/tests/kernel/http.rs`,
      `x_forwarded_host_from_a_trusted_proxy_selects_the_brand_patch` in
      `crates/system/tests/system/settings_brand.rs`; every script of `hack/` speaking https with
      the generated certificate, the installer's `--cacert` wait and its report asserted by
      `hack/check-install.sh target/debug/keel`, and the browser suite pinning the certificate's
      SPKI hash in `hack/e2e.sh target/debug/keel`.
- [x] The OwnLate projects are recorded and `keel-dev i18n` holds the dictionaries:
      `extract_derives_the_keys_of_a_dump_by_the_key_table_and_keeps_its_explicit_ones`,
      `missing_reports_a_key_absent_from_ru_and_accepts_a_derived_key_the_shared_fallback_satisfies`,
      `pull_splits_a_release_by_prefix_into_the_dictionaries_and_leaves_a_synced_tree_unchanged`,
      `pull_fetches_the_translations_map_of_every_project_with_the_token_and_writes_the_dictionaries`
      and `the_pull_is_resolved_from_the_environment_and_masks_its_token` in `crates/dev/src/i18n/`;
      `cargo run -p keel-dev -- i18n missing --dump contracts/dump` green in the job `check` over
      the 883 keys of `en` and `ru`; `contracts/i18n.json` naming the projects `keel-core` and
      `keel-demo` with their directories and prefixes.
- [x] `@globalart/phoenix-ui` is pinned at `0.3.1`, batch 2, in `frontend/package.json`,
      `frontend/libs/keel/package.json` and `frontend/products/demo/package.json`, resolved by
      `pnpm install --frozen-lockfile` from Nexus; the first consumers are the export dropdown, the
      column order, the cell editor and the multi-row form above.
- [x] The frontend job is green: `pnpm install --frozen-lockfile`, `pnpm generate` followed by
      `git diff --exit-code`, `pnpm lint`, `pnpm format`, `pnpm test` (63 suites, 297 tests),
      `pnpm build`, `pnpm build-storybook`, `pnpm test-storybook` (6 story files, 27 stories),
      `node tools/stamp.mjs` and `pnpm publish --dry-run --no-git-checks` against Nexus; the
      custom-views share from `hack/descriptors.sh target/debug/keel` is `demo 1/9` and
      `composition 3/83`, `frontend/apps/shell/views.json` unchanged since M2.
- [x] `cargo fmt --all --check` is green.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [x] `cargo test --workspace` is green against Postgres, `env -u KEEL_TEST_DATABASE_URL cargo test
      -p keel-testkit` against the container, `cargo test -p keel-bin --no-default-features` without
      the demo, `cargo test -p keel-system --no-default-features --features <set>` for each of the
      fourteen sets of the matrix
      (`tasks_alone_boots_with_its_tables_holds_its_lock_and_answers_task_list_on_the_socket`,
      `cron_alone_boots_with_its_table_holds_its_lock_and_answers_cron_job_list_on_the_socket`,
      `notify_alone_boots_with_its_tables_its_poll_job_and_answers_its_lists_on_the_socket` and
      `webhooks_alone_boots_with_its_tables_and_answers_its_lists_on_the_socket` for the modules
      alone), and `hack/check-test-databases.sh` finds no database left behind.
- [x] `hack/check-deps.sh` is green: the demo depends on `keel-system` for `notify::api` and
      `task` through the facade alone, next to the M0, M1 and M2 rules.
- [x] `hack/check-docs.sh` is green: every M3 name it carries is found under `docs/design/` and
      under `crates/` or `frontend/libs/keel`, every Rust test this checklist cites is defined,
      every `*.spec.ts` it cites exists under `frontend/`, every command it cites exists, and the
      README's check lists, the "How we work" section and `ci.yml` agree, `i18n missing` and the
      fourteen-set matrix included.
- [x] The OwnLate projects `keel-core` and `keel-demo` exist in the translation service, seeded
      from `keel-dev i18n extract`, their ids set as the repository variables
      `KEEL_OWNLATE_PROJECT_CORE` and `KEEL_OWNLATE_PROJECT_DEMO` and the token as the secret
      `KEEL_OWNLATE_TOKEN`, after which the Build workflow's `keel-dev i18n pull` leaves the
      committed dictionaries unchanged — operator-run; until then the dictionaries are edited in
      the repository and `i18n missing` holds them complete.
- [x] `@globalart/keel` at the closing commit's version is published to Nexus by `publish-npm.yml`
      through `workflow_dispatch`, after which the `exports` of the published package, read from
      Nexus with pnpm's `view`, name the six entry points and the nested `testing/playwright` —
      operator-run; the dry run above is CI's rehearsal.
- [x] On `keel-demo`, `keel --version` under `/opt/keel/bin/current` names the closing commit's
      version, the first start generated the certificate under `/var/lib/keel/tls` and logged its
      fingerprint, and `/readyz` answers `ready` over https with that certificate, after the musl
      artifact of the closing commit's Build run was unpacked into `/opt/keel/versions/<version>`,
      `bin/current` switched and `keel.service` restarted; a paced `note.export` interrupted by
      `systemctl restart keel` finishes with every note written once — operator-run; to be ticked
      with the note "confirmed by hand on keel-demo". The gateway in front of the host speaks TLS
      to the listener through its backend policy.
- [ ] The owner has opened `https://<host>:<port>/` of `keel-demo` directly, accepted the
      certificate by its fingerprint, logged in, run `note.export` and watched the modal and the
      drawer, opened the notification centre, the dashboard and the about drawer, uploaded a logo
      for the machine's host name, and confirmed the notes item badge after
      `keel ctl cron.job.run id=notify.problems.poll` — operator-run; to be ticked with the note
      "confirmed by hand on keel-demo".
- [ ] Left undone in M3, planned as step 2b: `keel.account.delete` as a task walking the
      `AccountLifecycle` contributions with a checkpoint per module, `keel.account.suspend` and
      `keel.account.resume`; `notify` and `webhooks` expose `drop_account` in the shape of an
      `on_delete` handler, tested by `deleting_an_account_drops_its_messages_and_problems` in
      `crates/system/tests/system/notify.rs` and
      `deleting_an_account_drops_its_subscriptions_and_deliveries` in
      `crates/system/tests/system/webhooks.rs`, and the contribution wires them when the step
      lands, with the first milestone that needs an account deleted.
- [ ] Left undone in M3, planned as step 6b: the per-session cap of the event stream and the
      platform constant `SSE_MAX_CLIENTS`, the re-check of the credential at keep-alive,
      `meta.invalidate` on a permission change and the `events` group of the conformance suite;
      the replay window, the scope filter and `Last-Event-ID` of step 6a stand as built.

## M4 exit checklist

Every ticked item below is checked by CI on `main` — the tests of the workspace with the fuzzing
properties of the frame parser and the manifest verifier, the eighteen-set `keel-system` matrix
against Postgres and the mail catcher, the builds of `keel-bin` with each bundle alone,
`hack/prove-binary.sh`, `hack/check-install.sh`, `hack/check-release.sh`, `hack/conformance.sh`,
`hack/descriptors.sh`, `keel-dev i18n missing` and `hack/e2e.sh` against the built binary over
https, the job `rehearsal`, the job `frontend`, and `hack/check-docs.sh` over this checklist. What
needs root, the real service manager, bind or the published channel is proven by the operator on
virtual machines and on `keel-demo` with the scripts under `hack/` that `deploy/README.md` lists
under "Proving on a machine"; those items are left unticked and named, and each is ticked once it
holds, with the date-free note "confirmed by hand" and the machine. A "Done when" item that has a
CI half and a machine half is split into a ticked item and an operator-run one. The findings of
the security review left without a fix are listed last, unticked.

- [x] The installer installs from the signed channel, allots the uid range, writes the policy,
      installs the four units, creates no administrator and prints the setup link, and no unit
      but the privileged ones runs as root: `hack/check-install.sh target/debug/keel` (the
      signature check against a rehearsal pair and its refusals, the uid allotment against a stub
      `getent`, `Type=notify`, the users of the units, the path unit, every subcommand, flag and
      function the scripts and `deploy/README.md` call); the kernel half in
      `the_setup_link_creates_the_first_administrator_once_and_the_function_is_gone_after` and
      `the_runtime_sends_ready_and_watchdog_over_the_notify_socket` in `crates/keel/tests/setup.rs`;
      the link followed once in the browser by `frontend/e2e/setup.spec.ts` through `hack/e2e.sh
      target/debug/keel`. The script is bash, so the command of the "Done when" item is written
      `curl | bash -s -- --product dns`; a plain `sh` stops with one line naming bash.
- [ ] On a fresh Debian or Ubuntu virtual machine, `curl | bash -s -- --product dns` installs keel
      with `dns` enabled and `ps -eo user,comm` shows root only under `keel privileged` —
      operator-run: `hack/install-vm.sh prepare <archive> <dir>` on the development machine and
      `hack/install-vm.sh run <dir> --product dns` on the machine, or `hack/dns-vm.sh install
      <archive>`, which also prepares bind.
- [x] `keel product disable` leaves the tables of the bundle orphaned and `enable` brings them back
      without re-running an applied migration, on a running installation through the restart
      exit 75: `disable_leaves_the_tables_and_enable_reruns_no_applied_migration` and
      `enable_on_a_running_installation_requests_a_restart_after_the_dump` in
      `crates/keel/tests/products.rs`; `hack/prove-binary.sh target/debug/keel` switches `demo`
      off and on on its running site and asserts the dump, the tables, the ledger and the audit
      trail. The mechanism knows no product; `dns` joins it on the machine below.
- [ ] `keel product disable dns` and `enable dns` on the machine installed with `--product dns`
      leave the `dns_*` tables orphaned and bring them back — operator-run: `hack/product-vm.sh
      dns dns.zone.list`.
- [x] A zone is created from the UI, from the CLI and with an API token:
      `frontend/e2e/dns.spec.ts` ("alice creates a zone and its record through the list and the
      form", the zone created with a `kat_` token, "the zones of the browser, the CLI and the token
      land in one list") through `hack/e2e.sh target/debug/keel`, which creates the CLI zone with
      `ctl dns.zone.create --as alice` and waits for the three zones to be served by the stub
      agent; the module in
      `a_zone_created_by_a_user_renders_its_file_and_include_after_commit_and_lists_under_its_account`,
      `a_zone_over_the_account_limit_is_refused_before_the_transaction` and
      `deleting_the_account_removes_its_zones_and_their_files` in
      `products/dns/tests/dns/zones.rs`, the records in
      `a_record_of_every_type_validates_its_value_and_lands_in_the_rendered_zone`,
      `an_invalid_value_is_refused_on_the_field_before_the_transaction`,
      `every_change_bumps_the_serial_once_per_transaction` and
      `records_of_a_zone_over_the_limit_are_refused` in `products/dns/tests/dns/records.rs`, the
      templates in `a_zone_created_with_a_template_receives_its_expanded_records`,
      `a_template_of_another_account_is_not_offered_and_not_applied` and
      `the_platform_templates_are_present_after_the_first_start` in
      `products/dns/tests/dns/templates.rs`.
- [x] A broken template is rolled back without downtime: the agent renders, swaps, checks with the
      file in place, reloads and restores the previous file when the check or the reload fails,
      in `template_apply_renders_swaps_validates_reloads_and_reports_changed`,
      `a_failing_check_rolls_the_file_back_and_a_failing_reload_reloads_again` and
      `an_unchanged_render_answers_changed_false_and_touches_nothing` in
      `crates/host-agent/tests/agent.rs`, `a_template_override_under_etc_wins_over_the_compiled_text`
      in `crates/host-agent/tests/catalogue.rs`; the zone keeps its row, turns to `error` and
      raises its problem in
      `a_failing_check_leaves_the_row_but_marks_the_zone_with_the_error_and_the_problem` in
      `products/dns/tests/dns/zones.rs`.
- [ ] On the machine with bind, zones created over the control socket and with a token are
      answered by `dig`, and a broken `bind/zone.j2` under `/etc/keel/templates` turns the zone to
      `error` while the zone file stays byte-identical and `named` keeps its process, its serial
      and its answers throughout — operator-run: `hack/dns-vm.sh run --browser-zone <name>` after
      the administrator created `<name>` on the Zones screen.
- [x] The update path in CI: the root half in
      `an_update_request_installs_the_signed_version_switches_current_and_restarts`,
      `a_request_naming_an_unsigned_or_tampered_version_leaves_current_untouched`,
      `rollback_switches_to_the_previous_version_within_the_contract_and_refuses_across_it`,
      `the_request_carries_no_command_the_root_side_obeys` and
      `an_update_request_moves_down_only_to_the_previous_version_within_its_contract` in
      `crates/keel/tests/privileged.rs`; the manifest in
      `a_manifest_signed_with_the_release_key_verifies_and_a_changed_byte_does_not`,
      `a_manifest_signed_with_a_previous_key_still_verifies`,
      `an_archive_whose_sha256_differs_from_the_manifest_is_refused` and
      `a_version_below_min_upgrade_from_is_refused` in `crates/release`; the kernel half in
      `check_compares_the_running_version_with_the_channel_head_and_reads_min_upgrade_from`,
      `apply_drains_dumps_every_loaded_table_and_writes_the_request_with_the_principal`,
      `apply_refuses_a_version_below_min_upgrade_from_and_across_a_contract`,
      `rollback_writes_a_rollback_request` and
      `the_daily_check_raises_update_available_and_the_running_version_clears_it` in
      `crates/system/tests/system/update.rs`; the release workflow in
      `the_release_workflow_triggers_on_release_tags_alone` and
      `the_channel_file_names_a_contract_and_a_floor_the_workspace_version_admits` in
      `crates/release/tests/channel.rs`, and its dry run fed to the root half by
      `hack/check-release.sh target/debug/keel`. `keel --version` of the closing commit answers
      `keel 0.2.0`.
- [ ] The tag `keel-v0.2.0`, annotated, runs `release.yml` and publishes `0.2.0` to the stable
      channel: `https://dl.globalart.hu/keel/stable/manifest.json` names `0.2.0` and `keel-release
      verify` accepts it — operator-run, after the Nexus raw repository `keel-releases`, its role,
      the deploy account `keel-release`, the secrets `KEEL_RELEASE_KEY` and
      `KEEL_RELEASE_NEXUS_PASSWORD` and the route `dl.globalart.hu` exist.
- [ ] `keel-demo`, installed at `0.1.0` from the channel, is updated to `0.2.0` from the button of
      the screen "Updates" and rolled back once with the button, keeping its database — operator-run
      on `keel-demo`; `hack/update-vm.sh prepare <archive> <dir>`, `run <dir>` and `kernel <dir>`
      rehearse the same path against a rehearsal channel.
- [x] A backup restores on another installation with the second factor working:
      `a_backup_restored_into_a_fresh_database_and_data_dir_answers_the_same_boot_and_logs_in_with_the_same_totp`,
      `a_backup_without_the_key_restores_but_the_sealed_values_do_not_open`,
      `restore_refuses_an_archive_naming_a_bundle_the_binary_does_not_carry` and
      `a_backup_source_contribution_lands_in_the_archive_and_comes_back` in
      `crates/keel/tests/backup.rs`; `hack/prove-binary.sh target/debug/keel` takes a backup over
      the CLI and lists its manifest.
- [ ] The backup of one virtual machine, taken with `--with-secret-key`, restores on a second
      machine installed with the same products, and a principal with TOTP logs in there with a
      fresh code — operator-run: `hack/backup-vm.sh take <archive>` on the first machine and
      `hack/backup-vm.sh restore <archive>` on the second, the second factor checked by hand.
- [x] An operation outside `file_roots` is refused by the agent and lands in the audit trail:
      `a_file_operation_outside_file_roots_is_refused_and_audited`,
      `a_request_whose_product_has_no_section_is_refused` and
      `a_peer_that_is_not_the_configured_uid_is_refused` in `crates/host-agent/tests/agent.rs`;
      the kernel side joins the journal on the request id in
      `a_module_applies_a_template_through_the_executor_and_the_agent_audits_the_request_id` in
      `crates/host/tests/executor.rs`.
- [ ] On a machine installed by `install.sh`, the agent creates an account inside the range that
      runs a program, renders, checks, reloads and serves a zone, rolls a broken template back
      without a restart of `named`, and refuses an operation outside `file_roots`, found in
      `journalctl -u keel-privileged` — operator-run: `hack/host-vm.sh`, then `hack/host-vm.sh
      --cleanup`.
- [x] The rest of the catalogue and the kernel's client:
      `users_and_groups_are_created_inside_the_uid_range_under_the_product_prefix`,
      `files_are_written_through_a_temporary_and_listed_under_the_product_prefix`,
      `service_operations_reach_listed_units_through_systemctl_alone` and
      `no_setfsuid_and_no_credential_change_on_a_worker_thread` in
      `crates/host-agent/tests/agent.rs`; `exec_runs_only_allowlisted_binaries_with_their_fixed_arguments`,
      `pkg_install_calls_the_package_manager_and_reports_what_changed`,
      `cron_writes_a_drop_in_under_the_product_prefix_and_removes_it`,
      `a_product_agent_op_is_executed_under_its_policy_class_and_audited` and
      `two_templates_with_one_id_fail_the_freeze` in `crates/host-agent/tests/catalogue.rs`;
      `a_module_requiring_host_without_a_provider_fails_the_freeze`,
      `an_agent_error_surfaces_in_the_envelope_as_its_key_with_the_detail` and
      `without_host_loaded_the_kernel_never_connects_to_the_socket` in
      `crates/host/tests/executor.rs`.
- [x] `doctor`, `support-bundle` and the log ring:
      `doctor_is_green_on_a_fresh_site_and_names_a_missing_unit_a_bad_mode_and_an_orphaned_module`,
      `logs_tail_answers_the_newest_entries_in_order` and `the_support_bundle_carries_no_secret_value`
      in `crates/keel/tests/doctor.rs`.
- [ ] `keel doctor` is green against the real units, directories and database of an installation,
      names `secret.key` after a bad mode, and the support bundle carries no password or token —
      operator-run: `hack/doctor-vm.sh`.
- [x] The feature guard without a lease:
      `a_function_of_a_feature_not_in_the_set_answers_license_and_is_cut_from_the_projection_and_the_menu`
      and `the_m4_feature_set_names_every_feature_of_every_enabled_product` in
      `crates/kernel/tests/kernel/features.rs`; the `features` group of `hack/conformance.sh
      target/debug/keel` answers 402 for a probe function behind a disabled feature.
- [x] The two items M3 left undone. `keel.account.delete` as a task over `AccountLifecycle` with
      `suspend` and `resume`:
      `delete_runs_as_a_task_walking_on_delete_in_requires_order_and_removes_the_row_last`,
      `a_failing_on_delete_leaves_the_account_deleting_with_the_error_on_the_task` and
      `suspend_and_resume_switch_the_state_and_walk_the_contributions_after_commit` in
      `crates/kernel/tests/kernel/lifecycle.rs`, with `dns` the first `on_delete`; the modules of
      `keel-system` contribute none yet, which `docs/design/03-access.md` records. The per-session
      cap of the event stream and the re-check at keep-alive:
      `the_fifth_stream_of_a_session_closes_the_first_with_retry_zero` and
      `a_revoked_session_is_closed_at_the_next_keep_alive` in
      `crates/kernel/tests/kernel/events.rs`, and the `events` group of `hack/conformance.sh
      target/debug/keel`.
- [x] The dependency rehearsal: the job `rehearsal` runs `hack/rehearsal.sh <git-url> <revision>`
      on every push to `main`, which builds `bin/keel` with `keel-dns-zones`, `keel-dns-records`
      and `keel-dns-templates` from the pushed revision by git, runs their tests and
      `hack/prove-binary.sh` against that binary, and refuses three tampered commits in which a
      module crate reaches a workspace path.
- [x] The binary with each bundle alone compiles: `cargo build -p keel-bin --no-default-features
      --features demo` and `cargo build -p keel-bin --no-default-features --features dns`.
- [x] The security review of the privileged path: every finding it fixed is a named test in the
      crate it concerns — `the_program_run_under_another_uid_gets_a_clean_environment_and_the_root_directory`,
      `a_uid_whose_account_carries_a_group_outside_the_range_is_refused_before_the_exec` and
      `a_lookup_answering_another_account_than_the_uid_asked_is_refused` in
      `crates/host-agent/src/exec.rs`; `a_uid_range_reaching_the_id_that_means_unchanged_is_refused`
      and `a_service_named_like_an_option_never_reaches_systemctl` in
      `crates/host-agent/src/policy.rs`;
      `a_placement_never_lets_every_account_write_a_file_or_an_unsticky_directory` and
      `a_file_over_the_read_limit_is_refused_without_being_read` in
      `crates/host-agent/src/ops/files.rs`, with
      `a_file_the_agent_places_is_never_writable_by_every_account` end to end;
      `a_frame_field_over_the_field_limit_is_refused_before_it_reaches_the_journal`,
      `a_frame_nested_past_the_depth_limit_is_refused_as_a_frame_without_overflowing` and
      `a_frame_naming_a_field_twice_is_refused_so_no_reader_sees_another_product` in
      `crates/host-agent/src/protocol.rs`;
      `a_request_that_grows_after_its_check_is_refused_at_the_limit`,
      `a_request_swapped_for_a_link_after_its_check_is_read_from_the_checked_file` and
      `a_fifo_at_the_request_path_is_refused_without_blocking_and_removed` in
      `crates/keel/src/privileged/request.rs`;
      `an_entry_whose_pax_size_overrides_its_header_counts_against_the_limit` and
      `an_entry_writable_by_group_or_others_is_refused` in `crates/keel/src/privileged/update.rs`;
      `an_update_request_moves_down_only_to_the_previous_version_within_its_contract` in
      `crates/keel/tests/privileged.rs`. The frame parser and the manifest verifier are fuzzed by
      bounded properties in the ordinary test run, since the runner has no nightly toolchain:
      `any_bytes_parse_to_a_request_or_fail_as_a_frame`,
      `a_frame_is_accepted_exactly_when_its_fields_follow_the_rules` in
      `crates/host-agent/src/fuzz.rs`, `any_bytes_are_refused_by_the_verifier_without_a_panic` and
      `a_changed_bit_of_the_payload_or_the_signature_never_verifies` in
      `crates/release/tests/fuzz.rs`.
- [x] `hack/check-deps.sh` is green: `keel-release` stays free of workspace crates and of the async
      and http stack, a module crate depends only on `keel-sdk`, the `api` of what it requires and
      the module crates of its own bundle, next to the rules of M0 to M3; the rule for the spike
      left with it.
- [x] `hack/check-docs.sh` is green: every M4 name it carries is found under `docs/design/` and
      under `crates/`, every Rust test and spec this checklist cites is defined or exists, every
      command it cites exists, and the README's check lists, the "How we work" section and
      `ci.yml` agree, the job `rehearsal` and the eighteen-set matrix included.
- [ ] The OwnLate project `keel-dns` exists in the translation service with the dictionaries of
      `dns-zones`, `dns-records` and `dns-templates`, its id set as the repository variable
      `KEEL_OWNLATE_PROJECT_DNS` the Build workflow's `keel-dev i18n pull` reads, after which the pull leaves the committed
      dictionaries unchanged — operator-run.
- [ ] Left without a fix by the security review, for the milestone that needs each: a process the
      agent runs has no timeout, so a check or package manager that never exits holds the serial
      lock and every later host operation waits (timeouts per tool belong in the policy);
      `host.pkg.*` has no allowlist, so any package of the configured sources can be installed or
      removed; the owner of a placement may be any account of the host, since `bind` has to own
      zone files; the channel accepts `http://` and a head manifest carries no expiry, so a man in
      the middle can replay an older signed head or withhold one, though not forge one; the
      unpacker has no bound on the number of entries, a finding that needs the signing key; the
      arguments of a product operation are the product's types, which the SDK does not hold to
      `deny_unknown_fields`, the subject of the M7 review.
