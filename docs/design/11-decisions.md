# Decisions, alternatives and risks

This document is the decision record of the Keel design: the key decisions with the alternatives
that were considered and the amendments accepted during review, the gaps that the review found and
closed, the risks the design carries together with their countermeasures, and the questions that
remain open because their answers would change the design. The decisions themselves are specified
in the subject documents; this one records why they were taken and what was weighed against them.

## Key decisions

Each decision was weighed against its alternatives on three criteria: technical feasibility, fit
with the way panel ecosystems evolve over years, and the volume of work for a single engineer. The
outcome column uses four values:

- kept: the decision stands as proposed; the amendments refine the same decision;
- kept with amendments: the decision stands, but part of its original formulation was moved or
  removed;
- revised: part of the decision was found to be wrong and replaced;
- chosen by the owner: the alternatives were weighed and the product owner made the call.

| # | Decision | Alternatives considered | Outcome | Accepted amendments |
|---|----------|-------------------------|---------|---------------------|
| 1 | Kernel boundary drawn by the SDK; auth, tasks, license and the rest are system modules ([01-overview.md](01-overview.md), [05-modules.md](05-modules.md)) | a thick kernel of about twenty subsystems | kept | hidden `keel_sdk::system` for the hooks only system modules need; a cargo feature per system module and a CI matrix "each module alone, and without each in turn" |
| 2 | Explicit composition builder in `main.rs`, no `inventory` ([05-modules.md](05-modules.md)) | self-registration plus cargo features (revision 1) | kept | module order comes from the `requires` graph, not from the order of `.module()` calls; `ModuleDef` is a plain struct with static slices; the builder lists modules and product bundles and the enabled set is read at boot (decision 11) |
| 3 | Ownership through the account; a principal is a member of an account with a role ([03-access.md](03-access.md)) | a tree of users with `owner_id` | kept | 1:N at M0, but session and envelope carry the account explicitly so that `keel_membership` can be added additively later; a root system account |
| 4 | Schema + field attributes + a separate `ui` block; conditions in a shared AST ([02-functions.md](02-functions.md)) | everything in `x-ui` (revision 1) · a separate `ScreenDef` · a derive macro from day one | kept with amendments | `level`, `secret`, `check` and `options` moved out of `x-ui` into `x-keel`, since they are part of the protocol and executed by the kernel; `once` joined them in M1 for a value answered to the call that produced it and masked in every stored copy; no proc-macros of our own, a derive for `ui` is deferred until a need is shown |
| 5 | Opaque tokens with prefixes, `__Host-` cookie ([03-access.md](03-access.md)) | JWT, as the existing in-house core and sso services use | kept | one cookie `__Host-keel_session` per installation (decision 11); TLS only, from the first start; `auth.login` is a guest-level function |
| 6 | Fixed dispatcher pipeline: `before` outside the transaction, the transaction opened by the kernel in `Call`, outbox written inside the transaction, audit after commit ([02-functions.md](02-functions.md)) | the handler opens its own transaction | kept | `mode` is `read` or `write`; two kinds of hooks (`before` outside the transaction for plugins, `within` inside it for compiled-in modules); a nested `invoke` does not call plugin hooks; audit written in batches from a background worker |
| 7 | Kernel process never root; an agent with a typed operation catalogue; update separated ([06-host.md](06-host.md)) | an in-process root executor (revision 1) | kept | one privileged code path, the `privileged` subcommand, with the `update` and `host` policies; resources addressed by identifiers, not by paths; impersonation by re-exec |
| 8 | Postgres only; sqlmig 0.2 with a ledger per module; additive migrations; ULID ids ([04-storage.md](04-storage.md)) | an embedded file database by default with Postgres optional (an earlier draft) · DDL through sea-query · a third migrator · `INTEGER` ids · down migrations | chosen by the owner | see the review notes below |
| 9 | One monorepo until SDK 1.0, about fourteen crates, one Angular library ([01-overview.md](01-overview.md)) | a repository per product from day one · 30 crates and 8 npm packages | kept | crates are created lazily, seven at M0 plus the `keel-sdk-probe` module; semver of the facade from the first commit; a "dependency rehearsal" of a product's crates from M4 (decision 11) |
| 10 | Wire contract in `keel-types`, `keel-conformance`, CLI through the local socket, guarded raw routes, meta-version without principal ([10-contracts.md](10-contracts.md)) | internal types without fixtures · in-process CLI · `/call` only | revised | three parts kept; the metadata cache was wrong (see the review notes below) |
| 11 | One installation of Keel per host: the binary `keel` carries the kernel, the system modules and the module sets of every first-party product; a product is a bundle enabled on the installation; one process, one database, one privileged agent, one set of directories and units, one account tree, one session, one update, one release train ([01-overview.md](01-overview.md), [05-modules.md](05-modules.md), [09-operations.md](09-operations.md)) | one static binary per product with its own user, directories, database, units and agent, cooperating through `peers` even on one host (the earlier formulation of this design) · one binary per product sharing one database · a container per product | chosen by the owner | product ids stay for module grouping, brand, menu groups, license features, drop-in and OS user prefixes; products are installed independently, on a clean server only, never in a container; `keel product disable` leaves tables orphaned; product module crates may live in their own repositories and are pulled into the `keel` build, no extraction by tag; product ids carry no semver; peers only between installations on different hosts; amends decisions 2, 5 and 9 |
| 12 | The release channel is `https://dl.globalart.hu/keel/`: a raw repository of the in-house Nexus behind a host name compiled into the binary, anonymous to read, published by the tag workflow through the Nexus REST API ([09-operations.md](09-operations.md)) | GitHub Releases of the private repository (no anonymous download, blocked by egress proxies of some networks) · an OCI registry with artifacts (a registry client on every host for a `curl`) · an object store fronted directly (chosen as the fallback the host name allows later) | chosen by the owner | the host name is the contract with installations, the storage behind it is not; the signing key starts as a CI secret and moves offline before the product is sold; downloads may be gated by license-server tokens later on the same URL |

### Review notes

**Storage (decision 8).** The earlier draft proposed an embedded file database by default, to
keep the customer's server free of extra dependencies, with Postgres as an option. Review found
that a two-dialect design could not be kept honest cheaply: either CI runs every test against both
databases, or Postgres support is left unverified. The owner chose one dialect for the whole
product line: Postgres, installed by `install.sh` from the distribution's packages when no server
is present. This removes the second dialect, the extension the migrator would have needed and the
whole class of tests that run against two databases. The price (one more service on the
customer's server) is recorded under risks below; whether that service comes from the
distribution or is shipped privately is an open question.

**Contract and meta-version (decision 10).** Three of the four parts stood: the contract lives in
`keel-types` with golden fixtures, the CLI is a client of the local socket, and raw routes exist
only behind a mandatory guard and an audit event. The fourth part, a metadata version and ETag
that did not include the principal, was wrong: the descriptor body depends on the caller's
rights, so a cache key without the principal violates cache semantics; `su` in the same browser
would receive another principal's menu from the cache. The fix that went into the design:
`acl_gen` on `keel_principal`, incremented in the same transaction that changes the principal's
rights, role or account (for a reseller's subtree, by `path`); ETag = hash(build, modules,
products, lang, principal_id, acl_gen); descriptor responses are `private, no-store`; the SSE
event `meta.invalidate` hints the client to re-request. See [10-contracts.md](10-contracts.md).

**Descriptor attributes (decision 4).** The split of field attributes into two namespaces follows
from the guarantee each carries: `x-keel` is protocol semantics the kernel is obliged to execute in
guards and strip (`level`, `secret`, `once`, `check` presets, the `options` function), `x-ui` is
presentation that any client may ignore without consequence. Keeping `level` or `secret` under
`x-ui` would have made access control depend on an attribute the protocol allows to be dropped.

**Single installation (decision 11).** A binary per product gave every product its own user,
directories, database, agent, cookie and update, and made two products on one host neighbours
that had to be paired. Weighed against it: a customer who buys KeelPanel and KeelDNS for one
server would run two processes, two databases and two agents with two policy files, log in twice
and update twice, and the hosting product would reach DNS on the same machine over HTTP; on the
other side stood fault isolation and an independent release cadence per product. The owner chose
one installation per host: the composition lists every module and every bundle, the enabled
bundles are read from `keel_kv` at boot, only their modules are migrated and initialised, and a
product reaches another product on the same installation through a bridge (`dns-local`) rather
than a peer (`dns-peer`). Products are still installed independently — a clean host may carry
KeelPanel alone — and licensed independently on one lease. The price is that every product shares
one process, one release train and one account tree; all three are recorded under risks below.

## Gaps closed during review

A completeness pass compared the design against an inventory of the subsystems a panel ecosystem
needs and against the actual state of phoenix-ui. Everything found was folded into the subject
documents; the table lists what was closed so that the coverage is visible.

| Gap | Severity | What was done | Described in |
|-----|----------|---------------|--------------|
| Ownership was checked only in lists | blocker | mandatory `OwnedGuard` in the dispatcher for every function with a `key`; `.unowned()` must be declared explicitly; a standard test in `keel-testkit` | [02-functions.md](02-functions.md), [03-access.md](03-access.md) |
| Descriptor vocabulary was not closed | major | enums `kind`, `control`, `column.type`, `action.role`, `check` in `keel-types`, generated TS unions, icons checked at start against the phoenix icon set embedded from `contracts/icons.json` | [02-functions.md](02-functions.md) |
| Form dynamics had no contract | major | request and response shapes for `options`, `recompute`, group editing, `wizard`, `download` / `upload` / `stream` | [10-contracts.md](10-contracts.md) |
| i18n keys had no rules | major | keys derived from the registry by convention, one placeholder format, checks at start and in CI | [08-frontend.md](08-frontend.md) |
| No statistics and no `report` contract | major | `stats` module with `StatCollector`, a `ui` block for `report` (bands, diagrams), M5 | [05-modules.md](05-modules.md), [08-frontend.md](08-frontend.md) |
| Account limits | major | `LimitDef`, `keel_limit` with inheritance by `path`, `check_limit`, error code `limit_exceeded` | [03-access.md](03-access.md) |
| Account deletion without a lifecycle | major | `AccountLifecycle`; deletion as a task with checkpoints; `ON DELETE RESTRICT` by default | [03-access.md](03-access.md) |
| The kernel knew about services | major | `svc` is a module; `InstanceInfoSource` feeds `/api/v1/instance` | [05-modules.md](05-modules.md), [07-ecosystem.md](07-ecosystem.md) |
| `store` on patches broke against the pipeline | major | a callback inside the transaction for modules, an event after commit for plugins | [05-modules.md](05-modules.md), [10-contracts.md](10-contracts.md) |
| A plugin had nowhere to keep data and did not see kernel events | major | `plugin.kv.*`; rights `observe` / `hook` / `patch`; `settings`, `cron`, `problems`, `widgets` sections in the manifest | [05-modules.md](05-modules.md), [10-contracts.md](10-contracts.md) |
| No system account and no bootstrap | major | root account `kind: system`; `keel.setup.admin` behind a one-time key | [03-access.md](03-access.md), [09-operations.md](09-operations.md) |
| Export came from nowhere | major | generic `keel.export` through `ListSource`; above a row threshold it becomes a task | [02-functions.md](02-functions.md) |
| SSE without filtering and catch-up | major | filtering by `ScopePolicy` and principal; `Last-Event-ID` = outbox `seq`; the peer feed on a separate path | [08-frontend.md](08-frontend.md), [07-ecosystem.md](07-ecosystem.md) |
| Smaller items | minor | `get` and `options` in the `kind` enum; `HostExecutor` moved from the SDK to `keel_host::api`; audit in batches from a worker; `r.view` and `KeelViewContext`; webhooks as one module; `kok_` keys and CSRF reconciled; principal time zone; auth hygiene; request limits; object history in audit; `MemoryMax`, anti-rollback, uninstall, restore from the UI; contradictions between milestones removed | throughout |

## Risks

Each risk names the countermeasure that is part of the design. Milestones referred to here are
defined in [../plan.md](../plan.md).

### Metadata ceiling

The file manager, the configuration editor, the terminal and monitoring go into custom views
rather than metadata-driven screens; M5 built the first two for `panel`, two views over 66
functions, 3.0 %, and left the terminal out. If custom views exceed 20 % of functions, the economics of the
platform do not hold: the point of Keel is that most screens are rendered from descriptors.

Mitigation: a metric in CI from M2 (`keel-dev views share`: the views of `r.view`, the
replacements in the products' `views.json` and the shell's own views against the function
registry, alarm threshold 20 %); a registry of widgets and column types on both the Rust and the
TypeScript side; the rule "a custom screen is cheaper than a new attribute"; a revision of the `ui`
vocabulary once per milestone. See [08-frontend.md](08-frontend.md).

### Volume for one engineer

Reaching `dns` is estimated at roughly 9–12 months, `panel` at 15–18.

Mitigation: strictly sequential milestones; the host spike in M2 so that the riskiest hypothesis
is tested early; no `peers`, `plugins` or `cluster` before a product ships.

### Postgres as part of the delivery

One more service on the customer's server: the version comes from the distribution, it takes
memory, a server with different settings may already be installed, and an OS upgrade may carry a
Postgres major upgrade.

Mitigation: minimum version 15 and a check in `doctor`; Keel never manages a server it does not
own, only its own role and database; a logical dump before every update; a problem on the
dashboard when the database is unavailable. See [04-storage.md](04-storage.md) and
[09-operations.md](09-operations.md).

### Agent under root

The main attack surface: path policy, symlink and TOCTOU races, the exec allowlist, product-defined
`AgentOp` operations.

Mitigation: fuzzing of operation schemas; security review at M4 and M7; every operation audited
with actor and request id. See [06-host.md](06-host.md).

### Self-update under root

Compromise of the release signing key is compromise of every installation.

Mitigation: the key is kept offline; `previous_keys` in the binary; the root unit knows a single
operation, "install signed version X", and never reads commands from the request. See
[09-operations.md](09-operations.md).

### Two implementations of expr, check and validation

The Rust and TypeScript interpreters of conditions, the check presets and client-side validation
may drift apart.

Mitigation: shared fixtures from M0; a conformance test "a field is shown if and only if the
server accepts it". See [02-functions.md](02-functions.md).

### phoenix-ui additions on the critical path

Eleven components in another repository are on the critical path of forms already in M2.

Mitigation: batches tied to milestones; versions in `peerDependencies`; gaps closed in phoenix-ui,
not worked around at runtime. See [08-frontend.md](08-frontend.md).

### musl static binary

Any C dependency breaks the static build.

Mitigation: the rule "no C libraries" and an `ldd` check in CI. See
[09-operations.md](09-operations.md).

### Monorepo until 1.0

The temptation to bypass the SDK "just for a minute".

Mitigation: `keel-dev check` lints imports; bundles and module crates depend only on the facade;
conformance runs against the built binary; a "dependency rehearsal" from M4. See
[01-overview.md](01-overview.md).

### Naming collisions

The metadata vocabulary describes concepts every control panel has, so a name chosen
independently may already be in use elsewhere.

Mitigation: an own vocabulary for `kind`, `role`, `control`, `level` and the rest; an audit of
documentation and code for name collisions before the first release.

### One process for every product

A fault in one product's module stops every product on the host; enabling a product runs its
migrations in the shared database at the next start and restarts every product; a product
disabled while another product bridges to it leaves the bridge on its remote side.

Mitigation: modules of disabled products are never loaded; the module isolation rules, supervised
tasks and the systemd watchdog; the restart after `keel product enable` runs the same drain and
logical dump as an update; orphaned tables and ledgers on disable; `keel product disable`
refusing while a bridge points at the product unless `--switch-remote` is given; a CI soak run
per enabled set. See [05-modules.md](05-modules.md) and [07-ecosystem.md](07-ecosystem.md).

### Release train coupling

A fix for `dns` ships with `panel`'s pending changes, and a `panel` regression blocks a `dns`
release; a product's own repository still cannot release on its own.

Mitigation: additive migrations; one changelog with a section per product; the CI matrix over
enabled sets — each first-party bundle alone and all together; module crates pinned by the keel
repository even when they live elsewhere. See [01-overview.md](01-overview.md).

### One account tree for several products

Every `user` of the installation holds the default permissions of every enabled product, so a
customer who bought only KeelDNS on a host that also runs KeelPanel holds hosting keys unless
they are withdrawn; a product's account kinds meet accounts another product created; orphaned
tables of a disabled product keep `ON DELETE RESTRICT` references to accounts, so
`keel.account.delete`, which walks only loaded modules, fails on the orphaned ones.

Mitigation: permission keys are per module and can be withdrawn per product; per-level defaults
are reviewed per product, and a test checks that a principal without a product's keys sees none
of its screens; kinds are declared per bundle and scoping is by `path`, never by kind;
`keel.account.delete` names the orphaned module in its error, and `schema prune` or re-enabling
the product unblocks it. Per-account product entitlement and product-scoped administrators are
open questions. See [03-access.md](03-access.md).

### License gating of enabled products

A lease that stops listing a product decides the fate of its data, and before M5 the only gate on
`keel product enable` was convention.

Mitigation, as built in M5: never delete on a lease change — the product's modules degrade to
read-only, whatever the marks of their functions, and `license.product_unlisted` is raised with
the product as its subject; `keel.product.enable` refuses a product the installed lease does not
list; orphaned semantics on disable. Without a lease nothing degrades and every compiled-in
product may be enabled at its default edition, so the installer works before the operator has a
lease; the offline switch on a stopped service has no gate and is caught as unlisted at the next
start. See [05-modules.md](05-modules.md).

### One policy file for every product

A `file_roots` or `exec_allow` entry widened for one product widens what every module on the host
can reach, and the agent trusts the kernel's `product` claim.

Mitigation: per-product sections for the prefixes, enforced by the agent; the claim comes from
one process whose calling module is known to the loader; the shared keys are reviewed at the M4
and M7 security reviews. See [06-host.md](06-host.md).

### Binary size and build time

Every product's crates and every product's views compile into one musl LTO binary; the 40 MB cap
of M2 will not hold once `dns` and `panel` are in, and every change rebuilds everything. The M2
binary — the kernel, `auth`, `settings`, `audit`, the `demo` bundle and the embedded shell — is
19.8 MB (19,822,864 bytes) as a stripped `x86_64-unknown-linux-musl` release build with thin LTO
at the close of M2, which the Build workflow measures on every push against the cap.

Mitigation: lazily loaded frontend chunks per product, one named chunk per product that the
initial scripts reach only through a dynamic import, which `tools/product-chunks.mjs` checks at
every `pnpm build`; the cap for the release binary, set at the start of M5 at 42,000,000 bytes in
`hack/size-cap` and measured by `hack/check-size.sh` in the Build and release workflows, against
33,777,536 bytes measured then, leaving 8.2 MB for `panel`, its ACME client and the code editor;
`cargo` incremental builds in CI. The binary still embeds every chunk of the shell, since leaving
a product's chunk out of a composition without it would change `embed_frontend!` and the facade.
See [08-frontend.md](08-frontend.md) and [09-operations.md](09-operations.md).

### Kernel-cleanliness proof rests on CI

No shipped first-party binary lacks `keel-host` any more, so the guarantee that the kernel needs
no agent rests on `hack/check-deps.sh` and the CI composition without `keel-host`; if that job is
dropped the boundary erodes unnoticed. Third-party compositions (SSO in place of `auth`, no
`keel-host`) are likewise exercised only by the `keel-system` matrix.

Mitigation: both jobs are mandatory in CI and named in [05-modules.md](05-modules.md).

## Open questions

The answers change the design, which is why they are listed here rather than among the
assumptions. Everything else is decided and described in the subject documents. Where an option is
marked "proposed", it is the one the design currently assumes; it is not decided.

- The default listen port of the installation. The names are fixed (Keel; KeelDNS `dns`,
  KeelPanel `panel`, see [01-overview.md](01-overview.md)); the port is not. See
  [09-operations.md](09-operations.md).
- Account membership. 1:N at M0 with an additive `keel_membership` later (proposed), or N:M from
  the first day. Review favoured N:M from the first day. If billing with "a company of three
  employees" is planned within the first year, N:M immediately is the better choice; one tree
  serving a hosting and a billing product on the same host makes the question more pressing. See
  [03-access.md](03-access.md).
- Reseller depth. An arbitrary chain of accounts by `path` (proposed) or exactly one level. The
  answer affects the UI for granting rights and limits.
- Postgres on the host. Install from the distribution's packages (proposed: one dependency,
  upgraded with the OS) or ship a private build under `/opt/keel/pg` (the same version everywhere,
  but then Keel owns its updates and its data). See [04-storage.md](04-storage.md).
- License server. The first releases run on long offline leases issued by hand. When is the server
  written as a product on Keel? The release signing key starts as a CI secret and moves offline
  before the product is sold ([09-operations.md](09-operations.md), the release channel).
- Per-account product entitlement. Whether a customer account must be entitled to products
  individually (a KeelDNS-only customer on a host that also runs KeelPanel), or a shared
  installation is always every product for every account, restricted by permission keys only.
  Related: product-scoped administrators and account kinds on a shared tree. See
  [03-access.md](03-access.md).
- Level labels inside a product's own screens when several products are enabled: platform labels
  everywhere (proposed) or a bundle's labels inside its own screens. See
  [03-access.md](03-access.md).
- CLI parameters. `a.b=v`, repeated keys and `@file.json`, or JSON only.
- REST facade. `GET /api/v1/dns/zones/{id}` on top of the dispatcher for external integrators, or
  is `/call/{func}` plus OpenAPI enough. See [10-contracts.md](10-contracts.md).
- Frontend libraries of a product whose crates move to their own repository: kept in this
  repository under `frontend/products/<id>` or published to Nexus as `@globalart/keel-<id>`;
  decided when the first product moves (M7). See [08-frontend.md](08-frontend.md).

Open points that concern one subsystem only stay with its document;
[07-ecosystem.md](07-ecosystem.md), for example, leaves open pinning the certificate of a
neighbour in its link and lists what the review of M6 left without a fix. M6 settled the points
that document had left open: the catalogue of service kinds is the function `svc.kinds`, the
retention after `svc.<kind>.close` is the setting `svc.retention_days`, a protocol out of reach
raises a problem at the daily check, and `dns-peer` reports a failure after commit on the site row,
as a problem and in its task.
